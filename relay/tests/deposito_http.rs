//! O relay pela rede, com um servidor a sério e um cliente HTTP.
//!
//! Loopback: prova o protocolo e a retoma, **não** prova comportamento pela
//! internet nem entre as duas máquinas de casa. Isso fica para quando os nós
//! estiverem de pé.

use std::path::PathBuf;

struct Temp(PathBuf);

impl Temp {
    fn nova(nome: &str) -> Self {
        let d = std::env::temp_dir().join(format!(
            "pombo-relay-{}-{}-{nome}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&d).unwrap();
        Self(d)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn relay(nome: &str) -> (String, Temp) {
    let dir = Temp::nova(nome);
    let (endereco, _) = pombo_correio_relay::servidor::servir_em_porta_livre(dir.0.clone())
        .await
        .unwrap();
    (format!("http://{endereco}"), dir)
}

async fn criar(base: &str, para: &str, tamanho: u64, oferta: &str) -> String {
    #[derive(serde::Deserialize)]
    struct R {
        id: String,
    }
    reqwest::Client::new()
        .post(format!("{base}/deposito?para={para}&tamanho={tamanho}"))
        .body(oferta.to_string())
        .send()
        .await
        .unwrap()
        .json::<R>()
        .await
        .unwrap()
        .id
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn o_que_sobe_desce_igual() {
    let (base, _d) = relay("igual").await;
    let bytes: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();

    let id = criar(&base, "abc123", bytes.len() as u64, r#"{"oferta":"selada"}"#).await;

    let r = reqwest::Client::new()
        .put(format!("{base}/deposito/{id}"))
        .body(bytes.clone())
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "subida falhou: {}", r.status());

    let de_volta = reqwest::get(format!("{base}/deposito/{id}/blocos"))
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();

    // O relay é o caminho, não o dono: se mexesse um byte, a verificação de quem
    // recebe rejeitava o ficheiro inteiro e ninguém saberia porquê.
    assert_eq!(de_volta.as_ref(), bytes.as_slice());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_oferta_volta_tal_e_qual() {
    // O relay guarda-a sem a ler. É isso que deixa mudar o formato de fio sem
    // lhe tocar — e é também porque não a pode ler.
    let (base, _d) = relay("oferta").await;
    let oferta = r#"{"de":{"x":1},"envelope":"AAAA","manifesto_selado":[1,2,3]}"#;
    let id = criar(&base, "abc123", 10, oferta).await;

    let e: serde_json::Value = reqwest::get(format!("{base}/deposito/{id}"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(e["oferta"], oferta);
    assert_eq!(e["bytes"], 0);
    assert_eq!(e["completo"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subida_cortada_retoma_do_bloco_e_nao_do_byte() {
    // O caso real: a ligação cai a meio e o relay fica com meio bloco. Quem
    // envia só sabe recomeçar na fronteira de um bloco, por isso pede para
    // retomar *atrás* do que lá está — e o que estava a mais tem de sumir.
    let (base, _d) = relay("retoma").await;
    let cliente = reqwest::Client::new();
    let total = 1000u64;
    let id = criar(&base, "abc123", total, "{}").await;

    cliente
        .put(format!("{base}/deposito/{id}"))
        .body(vec![1u8; 600])
        .send()
        .await
        .unwrap();

    let e: serde_json::Value = reqwest::get(format!("{base}/deposito/{id}"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(e["bytes"], 600);

    // Retomar na fronteira do bloco anterior: 400.
    cliente
        .put(format!("{base}/deposito/{id}?de=400"))
        .body(vec![2u8; 600])
        .send()
        .await
        .unwrap();

    let tudo = reqwest::get(format!("{base}/deposito/{id}/blocos"))
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();

    assert_eq!(tudo.len(), 1000);
    assert_eq!(&tudo[..400], &[1u8; 400]);
    assert_eq!(&tudo[400..], &[2u8; 600]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn descida_cortada_retoma_por_range() {
    let (base, _d) = relay("range").await;
    let bytes: Vec<u8> = (0..5000u32).map(|i| (i % 253) as u8).collect();
    let id = criar(&base, "abc123", bytes.len() as u64, "{}").await;

    reqwest::Client::new()
        .put(format!("{base}/deposito/{id}"))
        .body(bytes.clone())
        .send()
        .await
        .unwrap();

    let r = reqwest::Client::new()
        .get(format!("{base}/deposito/{id}/blocos"))
        .header("Range", "bytes=2000-")
        .send()
        .await
        .unwrap();

    assert_eq!(r.status(), reqwest::StatusCode::PARTIAL_CONTENT);
    let cauda = r.bytes().await.unwrap();
    assert_eq!(cauda.as_ref(), &bytes[2000..]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mentir_no_tamanho_nao_enche_o_disco() {
    // O relay é a peça exposta à internet. Declarar 100 e enviar 5000 tem de
    // parar nos 100, e não depois de o disco acabar.
    let (base, _d) = relay("mentira").await;
    let id = criar(&base, "abc123", 100, "{}").await;

    let r = reqwest::Client::new()
        .put(format!("{base}/deposito/{id}"))
        .body(vec![7u8; 5000])
        .send()
        .await
        .unwrap();
    assert!(!r.status().is_success());

    let e: serde_json::Value = reqwest::get(format!("{base}/deposito/{id}"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(e["bytes"].as_u64().unwrap() <= 100);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn identificador_inventado_nao_serve_nada() {
    let (base, _d) = relay("inventado").await;

    for id in ["..%2F..%2Fetc%2Fpasswd", "abc", &"f".repeat(32)] {
        let r = reqwest::get(format!("{base}/deposito/{id}")).await.unwrap();
        assert!(!r.status().is_success(), "{id} devia falhar");
        let r = reqwest::get(format!("{base}/deposito/{id}/blocos")).await.unwrap();
        assert!(!r.status().is_success(), "{id}/blocos devia falhar");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn apagado_deixa_de_existir_e_leva_os_bytes() {
    let (base, dir) = relay("apagar").await;
    let id = criar(&base, "abc123", 10, "{}").await;
    reqwest::Client::new()
        .put(format!("{base}/deposito/{id}"))
        .body(vec![3u8; 10])
        .send()
        .await
        .unwrap();

    let r = reqwest::Client::new()
        .delete(format!("{base}/deposito/{id}"))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success());

    assert!(!dir.0.join(format!("{id}.blocos")).exists());
    let r = reqwest::get(format!("{base}/deposito/{id}")).await.unwrap();
    assert!(!r.status().is_success());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_caixa_de_um_nao_e_a_do_outro() {
    let (base, _d) = relay("caixas").await;
    let meu = criar(&base, "impressao-a", 10, "{}").await;
    let _dele = criar(&base, "impressao-b", 10, "{}").await;

    let caixa: Vec<String> = reqwest::get(format!("{base}/caixa/impressao-a"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(caixa, vec![meu]);
}
