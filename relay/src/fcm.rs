//! Acorda um Android nativo pelo Firebase Cloud Messaging — o equivalente do
//! `vapid.rs`/Web Push, mas para quem não tem `PushManager` nenhum.
//!
//! Sem SDK: a API HTTP v1 da Google é dois pedidos simples. Um JWT RS256
//! assinado com a chave privada da conta de serviço troca-se por um *access
//! token* OAuth2 (`POST /token`), que autentica o envio propriamente dito
//! (`POST .../messages:send`). Sem cache do token — um JWT novo por envio,
//! tal como o VAPID já faz; um depósito não é frequente o suficiente para a
//! troca extra pesar, e cache seria estado a mais para o mesmo resultado.
//!
//! **Sem carga aqui também** (mesma escolha do Web Push): mensagem de dados
//! vazia, só acorda a app. O que chegou continua a perguntar-se ao relay.

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const AMBITO: &str = "https://www.googleapis.com/auth/firebase.messaging";
const URL_TOKEN: &str = "https://oauth2.googleapis.com/token";

/// Só os três campos do JSON da conta de serviço que interessam — o resto
/// (`private_key_id`, `client_id`, ...) fica ignorado por omissão do serde.
#[derive(Deserialize)]
struct ContaServico {
    project_id: String,
    client_email: String,
    private_key: String,
}

pub struct Fcm {
    conta: ContaServico,
    /// Já convertida uma vez no arranque — falhar aqui é falhar cedo (chave
    /// mal formada), não a meio do primeiro envio.
    chave: jsonwebtoken::EncodingKey,
}

#[derive(Serialize)]
struct ClaimsToken<'a> {
    iss: &'a str,
    scope: &'a str,
    aud: &'a str,
    iat: u64,
    exp: u64,
}

#[derive(Deserialize)]
struct RespostaToken {
    access_token: String,
}

fn agora() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// O que aconteceu ao tentar entregar — o suficiente para `push.rs` decidir
/// se a subscrição continua viva.
pub enum Resultado {
    Entregue,
    /// A Google diz que o token já não existe (app desinstalada, ou trocado)
    /// — mesmo tratamento do 404/410 do Web Push: apaga-se a subscrição.
    Morto,
    /// Rede em baixo, credencial recusada, etc. — melhor esforço, nunca
    /// propaga: quem chama não tem nada de jeito a fazer com isto.
    Falhou,
}

impl Fcm {
    /// Lê a conta de serviço de `caminho` (o `.json` gerado no Firebase
    /// Console — ver PLANO.md, "Antes do S9"). `None` no `--firebase-conta-
    /// -servico` do relay significa sem FCM configurado, não um erro.
    pub fn carregar(caminho: &Path) -> Result<Self> {
        let bytes = std::fs::read(caminho).context("a ler a conta de serviço Firebase")?;
        let conta: ContaServico = serde_json::from_slice(&bytes).context("conta de serviço Firebase mal formada")?;
        let chave = jsonwebtoken::EncodingKey::from_rsa_pem(conta.private_key.as_bytes())
            .context("chave privada da conta de serviço Firebase mal formada")?;
        Ok(Self { conta, chave })
    }

    async fn access_token(&self, cliente: &reqwest::Client) -> Result<String> {
        let iat = agora();
        let claims = ClaimsToken {
            iss: &self.conta.client_email,
            scope: AMBITO,
            aud: URL_TOKEN,
            iat,
            exp: iat + 3600,
        };
        let cabecalho = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
        let jwt = jsonwebtoken::encode(&cabecalho, &claims, &self.chave)?;

        let resposta = cliente
            .post(URL_TOKEN)
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                ("assertion", &jwt),
            ])
            .send()
            .await
            .context("não consegui falar com o OAuth2 da Google")?
            .error_for_status()
            .context("a Google recusou a troca do JWT por um access token")?
            .json::<RespostaToken>()
            .await
            .context("resposta do OAuth2 da Google incompreensível")?;
        Ok(resposta.access_token)
    }

    /// Tenta acordar `token` (o registo FCM do dispositivo). Nunca propaga
    /// erro — só diz o suficiente para `push.rs` decidir se a subscrição
    /// continua válida.
    pub async fn enviar(&self, cliente: &reqwest::Client, token: &str) -> Resultado {
        let Ok(acesso) = self.access_token(cliente).await else { return Resultado::Falhou };

        let url = format!("https://fcm.googleapis.com/v1/projects/{}/messages:send", self.conta.project_id);
        // Sem `priority: "high"` a Google entrega como lhe convier — pode
        // adiar minutos a um processo em segundo plano, o oposto do "acorda
        // agora" que a mensagem de dados existe para fazer (achado a testar
        // a sério no tablet, S10 T2: sem isto o `onMessageReceived` nunca
        // chegou a correr com a app em segundo plano).
        let corpo = serde_json::json!({
            "message": { "token": token, "data": {}, "android": { "priority": "high" } }
        });

        match cliente.post(url).bearer_auth(acesso).json(&corpo).send().await {
            Ok(r) if r.status() == reqwest::StatusCode::NOT_FOUND => Resultado::Morto,
            Ok(r) if r.status().is_success() => Resultado::Entregue,
            Ok(r) => {
                // BAD_REQUEST com "UNREGISTERED" é a outra forma que a Google
                // tem de dizer "este token já não existe" — o 404 sozinho
                // não cobre esse caso.
                let corpo = r.text().await.unwrap_or_default();
                if corpo.contains("UNREGISTERED") { Resultado::Morto } else { Resultado::Falhou }
            }
            Err(_) => Resultado::Falhou,
        }
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    // Par RSA só para teste, gerado com `openssl genrsa`/`openssl rsa
    // -pubout` — nunca usado fora daqui. Auto-verificação: assina um JWT com
    // a chave privada e confirma-o com a pública, tal como o `vapid::testes`
    // já faz para o ES256. É o que apanharia um erro de codificação (PKCS#1
    // vs PKCS#8, claims trocadas) que a Google recusaria em silêncio.
    const CHAVE_PRIVADA_TESTE: &str = include_str!("fcm_chave_teste_privada.pem");
    const CHAVE_PUBLICA_TESTE: &str = include_str!("fcm_chave_teste_publica.pem");

    fn fcm_de_teste() -> Fcm {
        let conta = ContaServico {
            project_id: "pombo-teste".into(),
            client_email: "relay@pombo-teste.iam.gserviceaccount.com".into(),
            private_key: CHAVE_PRIVADA_TESTE.into(),
        };
        let chave = jsonwebtoken::EncodingKey::from_rsa_pem(conta.private_key.as_bytes()).unwrap();
        Fcm { conta, chave }
    }

    #[tokio::test]
    async fn o_jwt_do_access_token_assina_e_confirma_com_a_chave_publica() {
        let fcm = fcm_de_teste();
        let iat = agora();
        let claims = ClaimsToken {
            iss: &fcm.conta.client_email,
            scope: AMBITO,
            aud: URL_TOKEN,
            iat,
            exp: iat + 3600,
        };
        let cabecalho = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
        let jwt = jsonwebtoken::encode(&cabecalho, &claims, &fcm.chave).unwrap();

        let decodificacao = jsonwebtoken::DecodingKey::from_rsa_pem(CHAVE_PUBLICA_TESTE.as_bytes()).unwrap();
        let mut validacao = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::RS256);
        validacao.set_audience(&[URL_TOKEN]);
        validacao.validate_exp = false; // testes não correm sempre dentro da mesma hora

        let dados = jsonwebtoken::decode::<serde_json::Value>(&jwt, &decodificacao, &validacao).unwrap();
        assert_eq!(dados.claims["iss"], "relay@pombo-teste.iam.gserviceaccount.com");
        assert_eq!(dados.claims["scope"], AMBITO);
    }

    #[test]
    fn conta_de_servico_ignora_campos_que_nao_interessam() {
        let json = serde_json::json!({
            "type": "service_account",
            "project_id": "pombo-teste",
            "private_key_id": "abc123",
            "private_key": CHAVE_PRIVADA_TESTE,
            "client_email": "relay@pombo-teste.iam.gserviceaccount.com",
            "client_id": "999",
        });
        let conta: ContaServico = serde_json::from_value(json).unwrap();
        assert_eq!(conta.project_id, "pombo-teste");
        assert_eq!(conta.client_email, "relay@pombo-teste.iam.gserviceaccount.com");
    }
}
