//! Avisa um dispositivo de que há um depósito à espera — Web Push (PWA/iOS)
//! ou FCM (Android nativo, `fcm.rs`), conforme a subscrição guardada.
//!
//! **Sem carga.** O relay não cifra nada (RFC 8291 fica de fora de
//! propósito) nem diz o que chegou — só acorda quem recebe, que decide
//! sozinho o que mostrar. Mais simples, e o relay continua a não saber nada
//! sobre o conteúdo dos depósitos.
//!
//! Melhor esforço: se o serviço de push estiver em baixo ou a rede falhar,
//! perde-se o aviso — quem recebe repara na próxima vez que abrir a app,
//! exatamente como antes do P4 existir. Nunca deve atrasar nem falhar a
//! subida que o desencadeou.

use axum::http::header;

use crate::{
    fcm::{self, Fcm},
    subscricoes::{Subscricao, Subscricoes},
    vapid::Vapid,
};

/// Tenta avisar `impressao`. Nunca propaga erro — quem chama não tem nada de
/// jeito a fazer com uma falha de entrega de push.
pub async fn notificar(
    cliente: &reqwest::Client,
    vapid: &Vapid,
    fcm: Option<&Fcm>,
    subscricoes: &Subscricoes,
    impressao: &str,
) {
    match subscricoes.obter(impressao).await {
        Some(Subscricao::WebPush { endpoint, .. }) => {
            notificar_web_push(cliente, vapid, subscricoes, impressao, &endpoint).await
        }
        Some(Subscricao::Fcm { token }) => notificar_fcm(cliente, fcm, subscricoes, impressao, &token).await,
        None => {}
    }
}

async fn notificar_fcm(cliente: &reqwest::Client, fcm: Option<&Fcm>, subscricoes: &Subscricoes, impressao: &str, token: &str) {
    // Sem `--firebase-conta-servico` configurado: melhor esforço, tal como um
    // serviço de push em baixo — a subscrição fica guardada na mesma.
    let Some(fcm) = fcm else { return };
    if matches!(fcm.enviar(cliente, token).await, fcm::Resultado::Morto) {
        let _ = subscricoes.remover(impressao).await;
    }
}

async fn notificar_web_push(cliente: &reqwest::Client, vapid: &Vapid, subscricoes: &Subscricoes, impressao: &str, endpoint: &str) {
    let Ok(endpoint) = reqwest::Url::parse(endpoint) else { return };
    let audiencia = endpoint.origin().ascii_serialization();

    let resposta = cliente
        .post(endpoint.clone())
        .header("TTL", "60")
        .header(header::CONTENT_LENGTH, "0")
        .header(
            header::AUTHORIZATION,
            format!("vapid t={}, k={}", vapid.assinar_jwt(&audiencia), vapid.chave_publica_base64url()),
        )
        .send()
        .await;

    // 404/410: o browser cancelou a subscrição (ou o serviço de push
    // esqueceu-a) — sem isto, cada depósito seguinte tentava outra vez a
    // mesma entrega já morta.
    if let Ok(r) = &resposta
        && matches!(r.status().as_u16(), 404 | 410)
    {
        let _ = subscricoes.remover(impressao).await;
    }
}

#[cfg(test)]
mod testes {
    use super::*;
    use crate::subscricoes::{Chaves, Subscricao};
    use axum::{Router, extract::State, http::HeaderMap, response::IntoResponse, routing::post};
    use std::sync::{Arc, Mutex};

    #[derive(Default, Clone)]
    struct Capturado(Arc<Mutex<Vec<(HeaderMap, String)>>>);

    async fn capturar(State(c): State<Capturado>, cabecalhos: HeaderMap, corpo: axum::body::Bytes) -> impl IntoResponse {
        c.0.lock().unwrap().push((cabecalhos, String::from_utf8_lossy(&corpo).to_string()));
        axum::http::StatusCode::CREATED
    }

    async fn servico_de_push_falso() -> (String, Capturado) {
        let capturado = Capturado::default();
        let app = Router::new().route("/entrega", post(capturar)).with_state(capturado.clone());
        let escuta = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endereco = escuta.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(escuta, app).await;
        });
        (format!("http://{endereco}"), capturado)
    }

    fn temporaria(nome: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("pombo-push-{}-{nome}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[tokio::test]
    async fn manda_um_pedido_vapid_bem_formado_e_sem_corpo() {
        let d = temporaria("bem-formado");
        let vapid = Vapid::carregar_ou_gerar(&d).unwrap();
        let subscricoes = Subscricoes::carregar(&d).unwrap();
        let (base, capturado) = servico_de_push_falso().await;
        let endpoint = format!("{base}/entrega");
        subscricoes
            .guardar("ab12", Subscricao::WebPush { endpoint, keys: Chaves { p256dh: "x".into(), auth: "y".into() } })
            .await
            .unwrap();

        notificar(&reqwest::Client::new(), &vapid, None, &subscricoes, "ab12").await;

        let capturas = capturado.0.lock().unwrap();
        assert_eq!(capturas.len(), 1);
        let (cabecalhos, corpo) = &capturas[0];
        assert_eq!(corpo, "");
        assert_eq!(cabecalhos.get("ttl").unwrap(), "60");
        let auth = cabecalhos.get("authorization").unwrap().to_str().unwrap();
        assert!(auth.starts_with("vapid t="));
        assert!(auth.contains(", k="));
        // O `aud` do JWT tem de bater com a origem do serviço de push falso,
        // não com o relay — é o que um serviço a sério verifica.
        let jwt = auth.split("t=").nth(1).unwrap().split(',').next().unwrap();
        let claims = jwt.split('.').nth(1).unwrap();
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        let claims: serde_json::Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(claims).unwrap()).unwrap();
        assert_eq!(claims["aud"], base);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[tokio::test]
    async fn sem_subscricao_nao_manda_nada() {
        let d = temporaria("sem-subscricao");
        let vapid = Vapid::carregar_ou_gerar(&d).unwrap();
        let subscricoes = Subscricoes::carregar(&d).unwrap();
        // Não rebenta, e não há o que verificar além de não ter atirado.
        notificar(&reqwest::Client::new(), &vapid, None, &subscricoes, "nao-existe").await;
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[tokio::test]
    async fn um_410_apaga_a_subscricao_morta() {
        let d = temporaria("410");
        let vapid = Vapid::carregar_ou_gerar(&d).unwrap();
        let subscricoes = Subscricoes::carregar(&d).unwrap();

        let app = Router::new().route("/entrega", post(|| async { axum::http::StatusCode::GONE }));
        let escuta = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endereco = escuta.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(escuta, app).await;
        });

        subscricoes
            .guardar(
                "ab12",
                Subscricao::WebPush {
                    endpoint: format!("http://{endereco}/entrega"),
                    keys: Chaves { p256dh: "x".into(), auth: "y".into() },
                },
            )
            .await
            .unwrap();

        notificar(&reqwest::Client::new(), &vapid, None, &subscricoes, "ab12").await;
        assert!(subscricoes.obter("ab12").await.is_none());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[tokio::test]
    async fn uma_subscricao_fcm_sem_credencial_configurada_nao_rebenta() {
        let d = temporaria("fcm-sem-credencial");
        let vapid = Vapid::carregar_ou_gerar(&d).unwrap();
        let subscricoes = Subscricoes::carregar(&d).unwrap();
        subscricoes.guardar("ab12", Subscricao::Fcm { token: "token-qualquer".into() }).await.unwrap();

        // `fcm: None` é o estado normal antes de o André configurar a
        // credencial — melhor esforço, a subscrição fica guardada na mesma.
        notificar(&reqwest::Client::new(), &vapid, None, &subscricoes, "ab12").await;
        assert!(subscricoes.obter("ab12").await.is_some());
        std::fs::remove_dir_all(&d).unwrap();
    }
}
