//! A identidade do relay perante os serviços de push (VAPID, RFC 8292).
//!
//! Sem isto, um serviço de push (Apple, Google...) recusa a entrega — é a
//! forma de provar, sem conta nenhuma, que quem manda o push é sempre o
//! mesmo relay. **Não autentica o utilizador**: só diz "isto vem de casa do
//! André outra vez", ao serviço de push, não ao browser.
//!
//! A chave nunca é escrita no repositório: vive dentro de `--dados`, que já
//! é sempre um caminho fora do `git` (é onde os depósitos também ficam).

use std::path::Path;

use anyhow::{Context, Result};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use p256::ecdsa::{Signature, SigningKey, VerifyingKey, signature::Signer};
use serde::Serialize;

/// Identifica o software a quem opera o serviço de push, se algo correr mal —
/// é o que o RFC 8292 pede no `sub`. Um URL, não o email do André: não há
/// razão para o dar a um serviço de terceiros que nunca pediu.
const SUB: &str = "https://github.com/Santolass06/pombo-correio";

/// Quanto tempo um JWT VAPID vale. O RFC recomenda não passar de 24h; um
/// token novo por pedido evita guardar estado nenhum de expiração.
const VALIDADE_JWT: u64 = 12 * 3600;

pub struct Vapid {
    chave: SigningKey,
}

fn agora() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Vapid {
    /// Lê a chave de `<dados>/vapid_chave`; gera-a e guarda-a se for a
    /// primeira vez. `0600` — a mesma cautela da `identidade` do CLI/app.
    pub fn carregar_ou_gerar(dados: &Path) -> Result<Self> {
        let caminho = dados.join("vapid_chave");
        let bytes = match std::fs::read(&caminho) {
            Ok(b) => b,
            Err(_) => {
                let mut escalar = [0u8; 32];
                getrandom::fill(&mut escalar).context("o CSPRNG do sistema falhou")?;
                std::fs::create_dir_all(dados)?;
                std::fs::write(&caminho, escalar)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&caminho, std::fs::Permissions::from_mode(0o600))?;
                }
                escalar.to_vec()
            }
        };
        let chave =
            SigningKey::from_slice(&bytes).context("vapid_chave em disco não é uma chave P-256 válida")?;
        Ok(Self { chave })
    }

    /// O ponto público, sem compressão (`0x04 || X || Y`), em base64url — o
    /// formato que `pushManager.subscribe({applicationServerKey})` espera.
    pub fn chave_publica_base64url(&self) -> String {
        let ponto = VerifyingKey::from(&self.chave).to_encoded_point(false);
        URL_SAFE_NO_PAD.encode(ponto.as_bytes())
    }

    /// Assina um JWT ES256 (RFC 8292) para o serviço de push em `audiencia`
    /// (o esquema+anfitrião do `endpoint` da subscrição — **não** a origem
    /// do relay). Cada envio gera um novo: mais simples do que guardar e
    /// invalidar um cache.
    pub fn assinar_jwt(&self, audiencia: &str) -> String {
        #[derive(Serialize)]
        struct Claims<'a> {
            aud: &'a str,
            exp: u64,
            sub: &'a str,
        }

        let cabecalho = URL_SAFE_NO_PAD.encode(br#"{"typ":"JWT","alg":"ES256"}"#);
        let claims = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&Claims { aud: audiencia, exp: agora() + VALIDADE_JWT, sub: SUB })
                .expect("Claims serializa sempre"),
        );
        let corpo = format!("{cabecalho}.{claims}");

        // ES256 quer a assinatura em bruto (r‖s, 64 bytes) — não o DER que o
        // `ecdsa` também sabe produzir. `Signature::to_bytes()` já devolve a
        // forma fixa; usar a errada dá um JWT bem formado que qualquer
        // serviço de push recusa em silêncio (401), sem dizer porquê.
        let assinatura: Signature = self.chave.sign(corpo.as_bytes());
        format!("{corpo}.{}", URL_SAFE_NO_PAD.encode(assinatura.to_bytes()))
    }
}

#[cfg(test)]
mod testes {
    use super::*;
    use p256::ecdsa::signature::Verifier;

    fn temporaria(nome: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("pombo-vapid-{}-{nome}", std::process::id()))
    }

    #[test]
    fn gera_na_primeira_vez_e_relê_depois_igual() {
        let d = temporaria("relê");
        let a = Vapid::carregar_ou_gerar(&d).unwrap();
        let b = Vapid::carregar_ou_gerar(&d).unwrap();
        assert_eq!(a.chave_publica_base64url(), b.chave_publica_base64url());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_chave_publica_e_um_ponto_p256_sem_compressao() {
        let d = temporaria("ponto");
        let v = Vapid::carregar_ou_gerar(&d).unwrap();
        let bytes = URL_SAFE_NO_PAD.decode(v.chave_publica_base64url()).unwrap();
        // 0x04 (sem compressão) + 32 bytes de X + 32 de Y.
        assert_eq!(bytes.len(), 65);
        assert_eq!(bytes[0], 0x04);
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// O teste que importa: um JWT que a própria chave pública consiga
    /// verificar é o mesmo que um serviço de push real vai tentar verificar.
    /// Se `to_bytes()` desse DER em vez de r‖s, isto falhava aqui — não só
    /// contra a Apple.
    #[test]
    fn o_jwt_verifica_com_a_propria_chave_publica() {
        let d = temporaria("jwt");
        let v = Vapid::carregar_ou_gerar(&d).unwrap();
        let jwt = v.assinar_jwt("https://push.exemplo.org");

        let partes: Vec<&str> = jwt.split('.').collect();
        assert_eq!(partes.len(), 3);

        let cabecalho: serde_json::Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(partes[0]).unwrap()).unwrap();
        assert_eq!(cabecalho["alg"], "ES256");

        let claims: serde_json::Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(partes[1]).unwrap()).unwrap();
        assert_eq!(claims["aud"], "https://push.exemplo.org");
        assert_eq!(claims["sub"], SUB);

        let assinatura = URL_SAFE_NO_PAD.decode(partes[2]).unwrap();
        let assinatura = Signature::from_slice(&assinatura).unwrap();
        let publica = VerifyingKey::from(&v.chave);
        let corpo = format!("{}.{}", partes[0], partes[1]);
        publica.verify(corpo.as_bytes(), &assinatura).unwrap();
        std::fs::remove_dir_all(&d).unwrap();
    }
}
