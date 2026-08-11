//! Um ficheiro a atravessar o relay por link, sem destinatário nenhum.
//!
//! O teste faz o que o browser faz, com **o mesmo código** que corre lá dentro
//! (`pombo-correio-pagina`): lê a chave do fragmento, vai buscar a oferta, abre o
//! manifesto e desenrola o fluxo bloco a bloco. O que não prova é o JavaScript à
//! volta nem o browser — isso só se vê num browser a sério.

use std::{path::PathBuf, sync::Arc};

use pombo_correio_core::identity::Identity;
use pombo_correio_pagina::Descarga;
use pombo_correio_rede::{FonteCaminho, Metricas, relay, sem_cancelamento};

struct Temp(PathBuf);

impl Temp {
    fn nova(nome: &str) -> Self {
        let d = std::env::temp_dir().join(format!(
            "pombo-lk-{}-{}-{nome}",
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

async fn relay_de_pe(nome: &str) -> (String, Temp) {
    let dir = Temp::nova(nome);
    let (endereco, _) = pombo_correio_relay::servidor::servir_em_porta_livre(
        dir.0.clone(),
        pombo_correio_relay::autenticado::Autorizados::abertos(),
    )
    .await
    .unwrap();
    (format!("http://{endereco}"), dir)
}

fn conteudo(n: usize) -> Vec<u8> {
    (0..n).map(|i| ((i * 13 + i / 331) % 256) as u8).collect()
}

/// Parte o link como o browser o parte: o caminho vai ao servidor, o fragmento
/// fica no dispositivo.
fn partir(link: &str) -> (String, String, String) {
    let (endereco, chave) = link.split_once('#').expect("um link leva sempre chave");
    let id = endereco.rsplit('/').next().unwrap().to_string();
    let base = endereco.strip_suffix(&format!("/d/{id}")).unwrap().to_string();
    (base, id, chave.to_string())
}

/// O que a página faz depois de ter a chave: busca, abre e junta.
async fn como_o_browser(base: &str, id: &str, chave: &str) -> Vec<u8> {
    let cliente = reqwest::Client::new();

    let estado: serde_json::Value = cliente
        .get(format!("{base}/deposito/{id}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(estado["completo"], true, "o depósito devia estar completo");

    let mut descarga = Descarga::nova(chave, estado["oferta"].as_str().unwrap()).unwrap();

    let selados = cliente
        .get(format!("{base}/deposito/{id}/blocos"))
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();

    // O mesmo enquadramento do JavaScript: pergunta de quantos bytes é o bloco
    // seguinte, corta essa fatia, entrega.
    let mut saida = Vec::new();
    let mut resto = &selados[..];
    loop {
        let preciso = descarga.proximo_tamanho() as usize;
        if preciso == 0 {
            break;
        }
        assert!(resto.len() >= preciso, "o fluxo acabou a meio de um bloco");
        saida.extend(descarga.bloco(&resto[..preciso]).unwrap());
        resto = &resto[preciso..];
    }
    assert!(resto.is_empty(), "sobraram bytes no fim");
    saida
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn o_link_entrega_o_ficheiro_byte_a_byte() {
    let (base_relay, deposito) = relay_de_pe("link").await;
    let casa = Temp::nova("link-casa");

    let ana = Identity::generate("Portátil da Ana");
    // Mais do que um bloco, para o último sair mais curto.
    let bytes = conteudo(1024 * 1024 + 7777);
    let origem = casa.0.join("ferias.jpg");
    std::fs::write(&origem, &bytes).unwrap();

    let link = relay::depositar_ligacao(
        &ana,
        &FonteCaminho(origem),
        &base_relay,
        Arc::new(|_| {}),
        sem_cancelamento(),
        &Metricas::desligada(),
    )
    .await
    .unwrap()
    .expect("devia ter ficado no relay");

    let (base, id, chave) = partir(&link);
    assert_eq!(base, base_relay);
    assert_eq!(chave.len(), 64, "a chave vai em hexadecimal, 32 bytes");

    assert_eq!(como_o_browser(&base, &id, &chave).await, bytes);

    // E o nome do ficheiro nunca esteve em claro no relay — o link não muda isso.
    let no_relay: Vec<u8> = std::fs::read_dir(&deposito.0)
        .unwrap()
        .flat_map(|e| std::fs::read(e.unwrap().path()).unwrap_or_default())
        .collect();
    assert!(
        !String::from_utf8_lossy(&no_relay).contains("ferias.jpg"),
        "o relay ficou a saber o nome do ficheiro"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn o_link_nao_aparece_na_caixa_de_ninguem() {
    // O depósito por link não é para pessoa nenhuma. Se aparecesse numa caixa, a
    // app tentava abrir um envelope que não é dela e falhava a cada arranque.
    let (base, _r) = relay_de_pe("caixa-link").await;
    let casa = Temp::nova("caixa-link-casa");

    let ana = Identity::generate("Ana");
    let origem = casa.0.join("publico.bin");
    std::fs::write(&origem, conteudo(4000)).unwrap();

    relay::depositar_ligacao(
        &ana,
        &FonteCaminho(origem),
        &base,
        Arc::new(|_| {}),
        sem_cancelamento(),
        &Metricas::desligada(),
    )
    .await
    .unwrap()
    .unwrap();

    // Nem para quem enviou, que é quem mais perto está de ser o dono.
    let quantos = relay::buscar(
        &ana,
        &base,
        &casa.0.join("recebidos"),
        &casa.0.join("conhecidos.json"),
        Arc::new(|_| {}),
        Default::default(),
        &Metricas::desligada(),
    )
    .await
    .unwrap();

    assert_eq!(quantos, 0, "um link não é correio de ninguém");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn o_mesmo_link_serve_duas_vezes() {
    // Ao contrário da caixa, a página não apaga o depósito: um link pode ser
    // aberto por mais do que uma pessoa, ou outra vez por quem perdeu o ficheiro.
    let (base_relay, _r) = relay_de_pe("duas-vezes").await;
    let casa = Temp::nova("duas-vezes-casa");

    let ana = Identity::generate("Ana");
    let bytes = conteudo(9000);
    let origem = casa.0.join("cartaz.pdf");
    std::fs::write(&origem, &bytes).unwrap();

    let link = relay::depositar_ligacao(
        &ana,
        &FonteCaminho(origem),
        &base_relay,
        Arc::new(|_| {}),
        sem_cancelamento(),
        &Metricas::desligada(),
    )
    .await
    .unwrap()
    .unwrap();

    let (base, id, chave) = partir(&link);
    assert_eq!(como_o_browser(&base, &id, &chave).await, bytes);
    assert_eq!(como_o_browser(&base, &id, &chave).await, bytes);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sem_a_chave_do_fragmento_nao_se_abre_nada()  {
    // O que o relay tem, sozinho, não chega — é o ponto todo de a chave viajar
    // depois do `#`. Este teste é o que falha se alguém alguma vez a puser no
    // caminho do URL "para simplificar".
    let (base_relay, _r) = relay_de_pe("sem-chave").await;
    let casa = Temp::nova("sem-chave-casa");

    let ana = Identity::generate("Ana");
    let origem = casa.0.join("segredo.bin");
    std::fs::write(&origem, conteudo(3000)).unwrap();

    let link = relay::depositar_ligacao(
        &ana,
        &FonteCaminho(origem),
        &base_relay,
        Arc::new(|_| {}),
        sem_cancelamento(),
        &Metricas::desligada(),
    )
    .await
    .unwrap()
    .unwrap();

    let (base, id, _chave) = partir(&link);
    let estado: serde_json::Value = reqwest::get(format!("{base}/deposito/{id}"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let oferta = estado["oferta"].as_str().unwrap();

    // Tudo o que o servidor tem está aqui, e com outra chave não abre.
    assert!(Descarga::nova(&hex::encode([0u8; 32]), oferta).is_err());
}
