//! A dica de endereço que a lista já traz, e quando não se pode confiar nela.
//!
//! O que importa provar:
//!
//! 1. Uma dica boa **evita a busca na rede**. É a razão de tudo isto existir:
//!    procurar de novo custava a janela inteira do mDNS (3 s) antes de cada
//!    envio, e no Android antes de o seletor de ficheiros sequer abrir.
//! 2. Uma dica que aponta para **outro dispositivo é recusada**. Ao guardar
//!    endereços passa a haver o risco de um IP ter mudado de dono desde que a
//!    lista foi feita — e entregar o ficheiro a quem não devia é muito pior do
//!    que demorar.
//!
//! Prova-se em loopback: chega para o que está aqui em causa, que é a decisão
//! de usar ou deitar fora a dica. O caminho de recurso — a busca por mDNS
//! quando a dica não serve — não é exercitado aqui, porque depende de multicast
//! na rede; esse só se dá por validado entre máquinas reais.

use std::sync::Arc;
use std::time::{Duration, Instant};

use pombo_correio_core::identity::Identity;
use pombo_correio_rede::{Metricas, descoberta, receber};

/// Diretório temporário que se limpa sozinho.
struct Temp(std::path::PathBuf);
impl Temp {
    fn novo(n: &str) -> Self {
        let d = std::env::temp_dir().join(format!("pombo-dica-{n}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Self(d)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Põe um recetor de pé numa porta livre e devolve o endereço e a impressão
/// digital com que ele se apresenta.
async fn recetor_de_pe(dir: &Temp, nome: &str) -> (std::net::SocketAddr, String) {
    let eu = Identity::generate(nome);
    let impressao = descoberta::impressao_curta(&eu.public());

    let escuta = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let porta = escuta.local_addr().unwrap().port();
    drop(escuta);

    let destino = dir.0.join(format!("recebidos-{nome}"));
    let conhecidos = dir.0.join(format!("conhecidos-{nome}.json"));
    tokio::spawn(async move {
        let _ = receber::servir(
            eu,
            destino,
            conhecidos,
            porta,
            Arc::new(|_| {}),
            Default::default(),
            Metricas::desligada(),
        )
        .await;
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    (format!("127.0.0.1:{porta}").parse().unwrap(), impressao)
}

/// Uma dica correta é aceite, e depressa.
///
/// O limite de tempo é a asserção que interessa: a janela da busca é de 3
/// segundos, portanto qualquer coisa perto disso significa que se foi procurar
/// à rede em vez de aproveitar o que já se sabia. É este teste que falha se
/// alguém repuser a chamada direta ao `resolver`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dica_certa_evita_a_busca() {
    let dir = Temp::novo("certa");
    let (endereco, impressao) = recetor_de_pe(&dir, "recetor").await;

    let relogio = Instant::now();
    let obtido = descoberta::resolver_com_dica(
        Some(endereco),
        "um nome que não existe na rede",
        Some(&impressao),
        53318,
        None,
    )
    .await
    .expect("a dica era boa e mesmo assim não resolveu");
    let demorou = relogio.elapsed();

    assert_eq!(obtido, endereco, "não devolveu o endereço da dica");
    assert!(
        demorou < Duration::from_secs(1),
        "demorou {demorou:?} — sinal de que foi procurar à rede em vez de usar a dica"
    );
}

/// Uma dica que aponta para outro dispositivo não é usada.
///
/// O nome dado não existe na rede, por isso o recurso à busca falha — e é
/// exatamente isso que se quer ver: falhar, em vez de entregar o ficheiro a
/// quem estava naquele endereço.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dica_para_o_dispositivo_errado_e_recusada() {
    let dir = Temp::novo("errada");
    let (endereco, _) = recetor_de_pe(&dir, "outro").await;

    // A impressão de alguém que não é quem está naquele endereço.
    let esperada = descoberta::impressao_curta(&Identity::generate("quem-eu-queria").public());

    let resultado = descoberta::resolver_com_dica(
        Some(endereco),
        "um nome que não existe na rede",
        Some(&esperada),
        53318,
        None,
    )
    .await;

    assert!(
        resultado.is_err(),
        "aceitou um endereço onde está outro dispositivo: {resultado:?}"
    );
}

/// Uma dica para onde não está ninguém é abandonada depressa.
///
/// Sem limite de tempo à ligação, um IP calado prende o envio durante os
/// minutos que o sistema leva a desistir — muito pior do que a busca que se
/// está a evitar.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dica_morta_nao_fica_presa() {
    // Porta livre: liga-se e é recusada, ou fica calada. Em qualquer dos casos
    // não pode demorar mais do que o limite curto.
    let escuta = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let morto: std::net::SocketAddr = escuta.local_addr().unwrap();
    drop(escuta);

    let relogio = Instant::now();
    let _ = descoberta::resolver_com_dica(
        Some(morto),
        "um nome que não existe na rede",
        Some("0123456789abcdef"),
        53318,
        None,
    )
    .await;

    // Generoso: o que se quer excluir são os minutos do timeout do sistema.
    assert!(
        relogio.elapsed() < Duration::from_secs(10),
        "ficou presa {:?} numa dica morta",
        relogio.elapsed()
    );
}
