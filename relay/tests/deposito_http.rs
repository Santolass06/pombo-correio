//! O relay pela rede, com um servidor a sério e um cliente HTTP.
//!
//! Loopback: prova o protocolo e a retoma, **não** prova comportamento pela
//! internet nem entre as duas máquinas de casa. Isso fica para quando os nós
//! estiverem de pé.

use std::path::PathBuf;

use pombo_correio_core::identity::Identity;
use pombo_correio_relay::autenticado::{self, Autorizados};

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
    let (endereco, _) =
        pombo_correio_relay::servidor::servir_em_porta_livre(dir.0.clone(), Autorizados::abertos())
            .await
            .unwrap();
    (format!("http://{endereco}"), dir)
}

fn agora() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Assina um pedido como o cliente a sério fará.
fn assinar(eu: &Identity, acao: &str, extra: &str) -> reqwest::header::HeaderMap {
    let p = eu.public();
    let impressao = hex::encode(p.fingerprint())[..16].to_string();
    let t = agora();
    let sig = eu.sign(autenticado::mensagem(acao, &impressao, extra, t).as_bytes());

    let mut h = reqwest::header::HeaderMap::new();
    h.insert(
        "x-pombo-identidade",
        format!("{}{}", hex::encode(p.verifying), hex::encode(p.encrypting))
            .parse()
            .unwrap(),
    );
    h.insert("x-pombo-instante", t.to_string().parse().unwrap());
    h.insert("x-pombo-assinatura", hex::encode(sig).parse().unwrap());
    h
}

fn impressao_de(eu: &Identity) -> String {
    hex::encode(eu.public().fingerprint())[..16].to_string()
}

async fn criar(base: &str, para: &str, tamanho: u64, oferta: &str) -> String {
    criar_como(&Identity::generate("emissor"), base, para, tamanho, oferta)
        .await
        .expect("devia ter criado")
}

async fn criar_como(
    eu: &Identity,
    base: &str,
    para: &str,
    tamanho: u64,
    oferta: &str,
) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct R {
        id: String,
    }
    let r = reqwest::Client::new()
        .post(format!("{base}/deposito?para={para}&tamanho={tamanho}"))
        .headers(assinar(eu, "deposito", &format!("{para}|{tamanho}")))
        .body(oferta.to_string())
        .send()
        .await
        .unwrap();
    if !r.status().is_success() {
        return None;
    }
    Some(r.json::<R>().await.unwrap().id)
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
    let eu = Identity::generate("eu");
    let ele = Identity::generate("ele");

    let meu = criar(&base, &impressao_de(&eu), 10, "{}").await;
    let _dele = criar(&base, &impressao_de(&ele), 10, "{}").await;

    let caixa: Vec<String> = reqwest::Client::new()
        .get(format!("{base}/caixa"))
        .headers(assinar(&eu, "caixa", ""))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(caixa, vec![meu]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_caixa_sem_assinatura_nao_se_ve() {
    // Antes do R2 bastava saber a impressão digital de alguém — que é pública,
    // anda anunciada na rede local — para saber quanta coisa esperava por ela.
    let (base, _d) = relay("caixa-nua").await;
    let r = reqwest::get(format!("{base}/caixa")).await.unwrap();
    assert_eq!(r.status(), reqwest::StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn depositar_sem_assinatura_nao_da()  {
    let (base, _d) = relay("dep-nu").await;
    let r = reqwest::Client::new()
        .post(format!("{base}/deposito?para=abc123&tamanho=10"))
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), reqwest::StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn de_fora_de_casa_nao_se_deposita() {
    // Assinar prova posse de uma chave, não pertença a esta casa: sem a lista,
    // qualquer um na internet enchia o disco.
    let dir = Temp::nova("lista");
    let lista = dir.0.join("autorizados.txt");
    let de_casa = Identity::generate("de casa");
    std::fs::write(&lista, format!("{}\n", impressao_de(&de_casa))).unwrap();

    let (endereco, _) = pombo_correio_relay::servidor::servir_em_porta_livre(
        dir.0.clone(),
        Autorizados::do_ficheiro(&lista).unwrap(),
    )
    .await
    .unwrap();
    let base = format!("http://{endereco}");

    assert!(criar_como(&de_casa, &base, "abc123", 10, "{}").await.is_some());
    let estranho = Identity::generate("estranho");
    assert!(criar_como(&estranho, &base, "abc123", 10, "{}").await.is_none());
}

/// A página do link: o relay serve ficheiros de uma pasta, e essa pasta é a
/// única coisa que ele serve de disco. Vale a pena testá-lo com um servidor a
/// sério, porque o que aqui parte é o caminho vindo do URL.
mod pagina {
    use super::*;

    async fn relay_com_pagina(nome: &str) -> (String, Temp, Temp) {
        let dados = Temp::nova(nome);
        let www = Temp::nova(&format!("{nome}-www"));
        std::fs::write(www.0.join("index.html"), "<!doctype html><p>olá</p>").unwrap();
        std::fs::create_dir_all(www.0.join("wasm")).unwrap();
        std::fs::write(www.0.join("wasm/modulo_bg.wasm"), b"\0asm-a-fingir").unwrap();
        // O ficheiro que ninguém pode conseguir alcançar a partir do URL.
        std::fs::write(dados.0.join("segredo.txt"), "isto não sai daqui").unwrap();

        let (endereco, _) = pombo_correio_relay::servidor::servir_em_porta_livre_com_pagina(
            dados.0.clone(),
            Autorizados::abertos(),
            Some(www.0.clone()),
        )
        .await
        .unwrap();
        (format!("http://{endereco}"), dados, www)
    }

    #[tokio::test]
    async fn o_identificador_do_link_devolve_sempre_a_pagina() {
        // Qualquer `/d/{id}` dá a mesma página: quem lê o identificador é o
        // JavaScript, da barra de endereço, junto com a chave que vem depois do
        // `#` e que nunca chega ao servidor.
        let (base, _d, _w) = relay_com_pagina("serve").await;

        let r = reqwest::get(format!("{base}/d/{}", "ab".repeat(16))).await.unwrap();
        assert!(r.status().is_success());
        assert_eq!(r.headers()["content-type"], "text/html; charset=utf-8");
        assert!(r.text().await.unwrap().contains("olá"));
    }

    #[tokio::test]
    async fn o_wasm_sai_com_o_tipo_certo() {
        // Com o tipo errado o `instantiateStreaming` recusa-o e a página não
        // arranca — e isso só se vê num browser, longe daqui.
        let (base, _d, _w) = relay_com_pagina("tipo").await;

        let r = reqwest::get(format!("{base}/pagina/wasm/modulo_bg.wasm")).await.unwrap();
        assert!(r.status().is_success());
        assert_eq!(r.headers()["content-type"], "application/wasm");
        assert_eq!(r.bytes().await.unwrap().as_ref(), b"\0asm-a-fingir");
    }

    #[tokio::test]
    async fn nao_se_sai_da_pasta_da_pagina() {
        // O relay é a peça exposta à internet. Sem esta guarda, servia o disco.
        let (base, _d, _w) = relay_com_pagina("fuga").await;

        for tentativa in [
            "../segredo.txt",
            "../../etc/passwd",
            "wasm/../../segredo.txt",
            "%2e%2e/segredo.txt",
        ] {
            let r = reqwest::get(format!("{base}/pagina/{tentativa}")).await.unwrap();
            assert!(
                !r.status().is_success(),
                "{tentativa} não podia ter sido servido"
            );
        }
    }

    #[tokio::test]
    async fn sem_pagina_instalada_o_resto_do_relay_continua_a_servir() {
        // Quem não instalar a página tem um relay igual ao de antes, só sem links.
        let (base, _d) = relay("sem-pagina").await;

        assert!(!reqwest::get(format!("{base}/d/{}", "ab".repeat(16)))
            .await
            .unwrap()
            .status()
            .is_success());
        assert!(reqwest::get(format!("{base}/saude")).await.unwrap().status().is_success());
    }
}

/// A PWA (`ui/dist`), servida na raiz por `fallback` — a rota que o P1 do S8
/// acrescentou. Testa a mesma coisa que a `mod pagina` acima, e mais uma: que
/// as rotas exatas do relay continuam a ganhar a um ficheiro do build com o
/// mesmo nome, e que uma rota sem ficheiro cai para o `index.html` (SPA).
mod app {
    use super::*;

    async fn relay_com_app(nome: &str) -> (String, Temp, Temp) {
        let dados = Temp::nova(nome);
        let dist = Temp::nova(&format!("{nome}-dist"));
        std::fs::write(dist.0.join("index.html"), "<!doctype html><p>pombo</p>").unwrap();
        std::fs::create_dir_all(dist.0.join("assets")).unwrap();
        std::fs::write(dist.0.join("assets/App.abc123.js"), "console.log(1)").unwrap();
        std::fs::write(dist.0.join("manifest.json"), "{}").unwrap();

        let (endereco, _) = pombo_correio_relay::servidor::servir_em_porta_livre_com_pagina_e_app(
            dados.0.clone(),
            Autorizados::abertos(),
            None,
            Some(dist.0.clone()),
        )
        .await
        .unwrap();
        (format!("http://{endereco}"), dados, dist)
    }

    #[tokio::test]
    async fn a_raiz_serve_o_index() {
        let (base, _d, _dist) = relay_com_app("raiz").await;

        let r = reqwest::get(&base).await.unwrap();
        assert!(r.status().is_success());
        assert_eq!(r.headers()["content-type"], "text/html; charset=utf-8");
        assert!(r.text().await.unwrap().contains("pombo"));
    }

    #[tokio::test]
    async fn um_ficheiro_do_build_sai_com_o_tipo_certo() {
        let (base, _d, _dist) = relay_com_app("tipo").await;

        let r = reqwest::get(format!("{base}/assets/App.abc123.js")).await.unwrap();
        assert!(r.status().is_success());
        assert_eq!(r.headers()["content-type"], "text/javascript; charset=utf-8");
    }

    #[tokio::test]
    async fn caminho_sem_ficheiro_cai_para_o_index() {
        // Uma SPA sem rotas do lado do cliente hoje — mas uma recarga numa
        // rota futura, ou um erro de digitação, tem de dar a app, não um 404.
        let (base, _d, _dist) = relay_com_app("spa").await;

        let r = reqwest::get(format!("{base}/isto/nao/existe")).await.unwrap();
        assert!(r.status().is_success());
        assert!(r.text().await.unwrap().contains("pombo"));
    }

    #[tokio::test]
    async fn as_rotas_do_relay_ganham_a_um_ficheiro_do_build_com_o_mesmo_nome() {
        // O `fallback` só dispara quando nada acima bate certo — é o que evita
        // a app e o relay pisarem-se um ao outro.
        let (base, _d, _dist) = relay_com_app("colisao").await;

        assert!(reqwest::get(format!("{base}/saude")).await.unwrap().status().is_success());
        assert_eq!(reqwest::get(format!("{base}/saude")).await.unwrap().text().await.unwrap(), "pombo-relay");
    }

    #[tokio::test]
    async fn sem_app_instalada_o_resto_do_relay_continua_a_servir() {
        let (base, _d) = relay("sem-app").await;

        let r = reqwest::get(&base).await.unwrap();
        assert!(!r.status().is_success());
        assert!(reqwest::get(format!("{base}/saude")).await.unwrap().status().is_success());
    }
}
