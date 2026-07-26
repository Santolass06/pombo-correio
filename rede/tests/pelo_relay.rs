//! Um ficheiro a atravessar o relay, sem os dois lados se falarem.
//!
//! Loopback: prova o protocolo, a retoma e que o relay não vê nada. **Não**
//! prova comportamento pela internet nem entre as duas máquinas de casa.

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use pombo_correio_core::identity::Identity;
use pombo_correio_rede::{Cancelaveis, Evento, Metricas, Ouvinte, relay};

struct Temp(PathBuf);

impl Temp {
    fn nova(nome: &str) -> Self {
        let d = std::env::temp_dir().join(format!(
            "pombo-vr-{}-{}-{nome}",
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

#[derive(Default)]
struct Registo(Mutex<Vec<String>>);

impl Registo {
    fn ouvinte(self: &Arc<Self>) -> Ouvinte {
        let eu = self.clone();
        Arc::new(move |e: Evento| {
            let nome = match e {
                Evento::AEscutar { .. } => "a_escutar",
                Evento::Oferta { .. } => "oferta",
                Evento::Destino { .. } => "destino",
                Evento::Progresso { .. } => "progresso",
                Evento::Recebido { .. } => "recebido",
                Evento::Enviado { .. } => "enviado",
                Evento::Depositado { .. } => "depositado",
                Evento::Cancelado { .. } => "cancelado",
                Evento::Falhou { .. } => "falhou",
            };
            eu.0.lock().unwrap().push(nome.into());
        })
    }
    fn viu(&self, tipo: &str) -> bool {
        self.0.lock().unwrap().iter().any(|t| t == tipo)
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
    (0..n).map(|i| ((i * 7 + i / 251) % 256) as u8).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn o_ficheiro_atravessa_o_relay_byte_a_byte() {
    let (base, _r) = relay_de_pe("ponta").await;
    let casa = Temp::nova("casa");

    let ana = Identity::generate("Portátil da Ana");
    let bento = Identity::generate("Telemóvel do Bento");

    // Um ficheiro maior do que um bloco, para o último sair mais curto — é onde
    // as contas de tamanho selado costumam partir.
    let bytes = conteudo(1024 * 1024 + 4321);
    let origem = casa.0.join("ferias.bin");
    std::fs::write(&origem, &bytes).unwrap();

    let envio = Arc::new(Registo::default());
    relay::depositar(
        &ana,
        &pombo_correio_rede::FonteCaminho(origem.clone()),
        &bento.public(),
        &base,
        envio.ouvinte(),
        Default::default(),
        &Metricas::desligada(),
    )
    .await
    .unwrap();

    // Ficou no relay — não "enviado". O Bento ainda nem sabe que existe.
    assert!(envio.viu("depositado"), "devia ter depositado");
    assert!(!envio.viu("enviado"), "não chegou a ninguém ainda");

    let recebidos = casa.0.join("recebidos");
    let rec = Arc::new(Registo::default());
    let quantos = relay::buscar(
        &bento,
        &base,
        &recebidos,
        &casa.0.join("conhecidos.json"),
        rec.ouvinte(),
        Cancelaveis::default(),
        &Metricas::desligada(),
    )
    .await
    .unwrap();

    assert_eq!(quantos, 1);
    assert!(rec.viu("recebido"));

    // O que interessa: os bytes são os mesmos. O relay é o caminho, não o dono.
    let chegou = std::fs::read(recebidos.join("ferias.bin")).unwrap();
    assert_eq!(chegou, bytes);

    // E o nome do ficheiro nunca esteve em claro no relay: o manifesto vai
    // selado, e é lá dentro que ele está.
    let no_relay: Vec<u8> = std::fs::read_dir(&_r.0)
        .unwrap()
        .flat_map(|e| std::fs::read(e.unwrap().path()).unwrap_or_default())
        .collect();
    assert!(
        !String::from_utf8_lossy(&no_relay).contains("ferias.bin"),
        "o relay ficou a saber o nome do ficheiro"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_caixa_de_outro_nao_se_abre() {
    // O envelope é para o Bento. A Carla vê o depósito na sua caixa? Não — a
    // caixa é por impressão digital. E mesmo que fosse lá parar, não abria.
    let (base, _r) = relay_de_pe("caixa").await;
    let casa = Temp::nova("caixa-casa");

    let ana = Identity::generate("Ana");
    let bento = Identity::generate("Bento");
    let carla = Identity::generate("Carla");

    let origem = casa.0.join("segredo.bin");
    std::fs::write(&origem, conteudo(5000)).unwrap();

    relay::depositar(
        &ana,
        &pombo_correio_rede::FonteCaminho(origem),
        &bento.public(),
        &base,
        Arc::new(|_| {}),
        Default::default(),
        &Metricas::desligada(),
    )
    .await
    .unwrap();

    let quantos = relay::buscar(
        &carla,
        &base,
        &casa.0.join("recebidos-carla"),
        &casa.0.join("conhecidos-carla.json"),
        Arc::new(|_| {}),
        Cancelaveis::default(),
        &Metricas::desligada(),
    )
    .await
    .unwrap();

    assert_eq!(quantos, 0, "a Carla não tem nada à espera dela");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn buscar_duas_vezes_nao_traz_o_mesmo_outra_vez() {
    // O depósito é apagado depois de o ficheiro estar em disco com o nome final.
    // Apagar antes trocaria uma falha recuperável (voltar a descarregar) por
    // perda; não apagar deixaria o relay a servir o mesmo para sempre.
    let (base, _r) = relay_de_pe("duas").await;
    let casa = Temp::nova("duas-casa");

    let ana = Identity::generate("Ana");
    let bento = Identity::generate("Bento");
    let origem = casa.0.join("uma-vez.bin");
    std::fs::write(&origem, conteudo(3000)).unwrap();

    relay::depositar(
        &ana,
        &pombo_correio_rede::FonteCaminho(origem),
        &bento.public(),
        &base,
        Arc::new(|_| {}),
        Default::default(),
        &Metricas::desligada(),
    )
    .await
    .unwrap();

    let recebidos = casa.0.join("recebidos");
    let conh = casa.0.join("conhecidos.json");
    let metricas = Metricas::desligada();
    let buscar = async || {
        relay::buscar(
            &bento,
            &base,
            &recebidos,
            &conh,
            Arc::new(|_| {}),
            Cancelaveis::default(),
            &metricas,
        )
        .await
        .unwrap()
    };

    assert_eq!(buscar().await, 1);
    assert_eq!(buscar().await, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn uma_oferta_adulterada_no_relay_e_recusada() {
    // O relay é a peça exposta. Se lhe mexerem no manifesto selado, a assinatura
    // deixa de bater — e é verificada ANTES de se abrir o que quer que seja.
    let (base, relay_dir) = relay_de_pe("adulterada").await;
    let casa = Temp::nova("adulterada-casa");

    let ana = Identity::generate("Ana");
    let bento = Identity::generate("Bento");
    let origem = casa.0.join("bom.bin");
    std::fs::write(&origem, conteudo(3000)).unwrap();

    relay::depositar(
        &ana,
        &pombo_correio_rede::FonteCaminho(origem),
        &bento.public(),
        &base,
        Arc::new(|_| {}),
        Default::default(),
        &Metricas::desligada(),
    )
    .await
    .unwrap();

    // Mexer num byte do manifesto selado, do lado do relay.
    for e in std::fs::read_dir(&relay_dir.0).unwrap() {
        let caminho = e.unwrap().path();
        if caminho.extension().is_some_and(|x| x == "json") {
            let texto = std::fs::read_to_string(&caminho).unwrap();
            let mexido = texto.replacen("manifesto_selado\\\":[", "manifesto_selado\\\":[9,", 1);
            assert_ne!(texto, mexido, "o teste tem de conseguir mexer mesmo");
            std::fs::write(&caminho, mexido).unwrap();
        }
    }

    let quantos = relay::buscar(
        &bento,
        &base,
        &casa.0.join("recebidos"),
        &casa.0.join("conhecidos.json"),
        Arc::new(|_| {}),
        Cancelaveis::default(),
        &Metricas::desligada(),
    )
    .await
    .unwrap();

    assert_eq!(quantos, 0, "não podia aceitar uma oferta adulterada");
}
