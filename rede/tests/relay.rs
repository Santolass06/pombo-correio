//! A escada: direto → relay. Em loopback, com um relay a sério (HTTP real).
//!
//! Provado aqui: a decisão de desviar, o depósito, a caixa, a descida com
//! retoma por `Range`, a verificação bloco a bloco **contra um relay que
//! adultera**, e o apagar depois de verificado. **Não** provado: internet, TLS,
//! relay noutra máquina, nem a interface.

use std::sync::{Arc, Mutex};

use pombo_correio_core::{chunk::CHUNK_SIZE, identity::Identity};
use pombo_correio_rede::{
    Evento, FonteCaminho, Metricas, Ouvinte,
    enviar::DestinoInacessivel,
    receber,
    relay::{Relay, enviar_com_alternativa, receber_da_caixa},
    sem_cancelamento,
};
use pombo_correio_relay::{Config, Estado, router};

struct Temp(std::path::PathBuf);
impl Temp {
    fn novo(n: &str) -> Self {
        let d = std::path::PathBuf::from("/var/tmp").join(format!("pombo-rr-{n}-{}", std::process::id()));
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

type Registo = Arc<Mutex<Vec<Evento>>>;

fn ouvinte() -> (Ouvinte, Registo) {
    let r: Registo = Default::default();
    let r2 = r.clone();
    (Arc::new(move |e| r2.lock().unwrap().push(e)), r)
}

async fn relay_de_pe(dir: &Temp) -> Relay {
    let estado = Estado::abrir(Config::por_omissao(dir.0.join("relay"))).unwrap();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ender = l.local_addr().unwrap().to_string();
    tokio::spawn(async move { axum::serve(l, router(estado)).await.unwrap() });
    Relay::novo(&ender)
}

/// Um endereço onde ninguém escuta: ligar-se dá «recusado» já.
fn porta_morta() -> std::net::SocketAddr {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap()
}

fn ficheiro(dir: &Temp, nome: &str, bytes: usize) -> (std::path::PathBuf, Vec<u8>) {
    let d: Vec<u8> = (0..bytes).map(|i| (i * 7 % 253) as u8).collect();
    let p = dir.0.join(nome);
    std::fs::write(&p, &d).unwrap();
    (p, d)
}

fn impressao(i: &Identity) -> String {
    pombo_correio_rede::descoberta::impressao_curta(&i.public())
}

/// O caminho feliz inteiro, com um ficheiro de 3 blocos (o último curto): o
/// destino está morto → deixa no relay (e **não** diz `Enviado`) → o destinatário
/// vai à caixa → chega byte-exato → o depósito desaparece.
#[tokio::test(flavor = "multi_thread")]
async fn destino_morto_vai_pelo_relay_e_chega_byte_exato() {
    let dir = Temp::novo("feliz");
    let relay = relay_de_pe(&dir).await;
    let (a, b) = (Identity::generate("A"), Identity::generate("B"));
    relay.registar(&a, None).await.unwrap();
    relay.registar(&b, None).await.unwrap();
    let (caminho, original) = ficheiro(&dir, "foto.bin", CHUNK_SIZE * 2 + 12_345);

    let (ouv_a, ev_a) = ouvinte();
    enviar_com_alternativa(
        &a, &FonteCaminho(caminho), porta_morta(), &impressao(&b), Some(&relay),
        ouv_a, sem_cancelamento(), &Metricas::desligada(), 0,
    )
    .await
    .unwrap();
    {
        let ev = ev_a.lock().unwrap();
        assert!(ev.iter().any(|e| matches!(e, Evento::NoRelay { .. })), "tinha de dizer que ficou no relay");
        assert!(!ev.iter().any(|e| matches!(e, Evento::Enviado { .. })), "ainda não chegou: não pode dizer Enviado");
    }

    let caixa = relay.caixa(&b).await.unwrap();
    assert_eq!(caixa.len(), 1);
    assert!(relay.caixa(&a).await.unwrap().is_empty(), "o remetente não tem nada à espera");

    let (ouv_b, ev_b) = ouvinte();
    let recebidos = dir.0.join("recebidos");
    let fim = receber_da_caixa(
        &b, &relay, &caixa[0], &recebidos, dir.0.join("conh.json"), ouv_b, sem_cancelamento(), &Metricas::desligada(),
    )
    .await
    .unwrap()
    .expect("não foi cancelado");
    assert_eq!(std::fs::read(&fim).unwrap(), original, "byte-exato");
    assert!(ev_b.lock().unwrap().iter().any(|e| matches!(e, Evento::Recebido { .. })));

    assert!(relay.caixa(&b).await.unwrap().is_empty(), "o depósito tem de ter sido apagado");
    assert!(
        std::fs::read_dir(dir.0.join("relay")).unwrap().filter_map(|e| e.ok()).all(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            !n.ends_with(".dados")
        }),
        "e o ficheiro também"
    );
}

/// Retoma pela descida: um parcial com o 1.º bloco já verificado → só se pede o
/// resto, por `Range` em bytes, e o resultado é idêntico. Falha se a conta
/// `k * (chunk + SEAL_OVERHEAD)` estiver errada.
#[tokio::test(flavor = "multi_thread")]
async fn descida_retoma_a_partir_do_bloco_seguinte() {
    let dir = Temp::novo("retoma");
    let relay = relay_de_pe(&dir).await;
    let (a, b) = (Identity::generate("A"), Identity::generate("B"));
    relay.registar(&a, None).await.unwrap();
    relay.registar(&b, None).await.unwrap();
    let (caminho, original) = ficheiro(&dir, "v.bin", CHUNK_SIZE * 2 + 999);
    enviar_com_alternativa(
        &a, &FonteCaminho(caminho), porta_morta(), &impressao(&b), Some(&relay),
        pombo_correio_rede::silencio(), sem_cancelamento(), &Metricas::desligada(), 0,
    )
    .await
    .unwrap();
    let item = relay.caixa(&b).await.unwrap().remove(0);

    // Parcial de uma tentativa anterior: o primeiro bloco + meio bloco de lixo
    // rasgado (tem de ser cortado, não aproveitado).
    let recebidos = dir.0.join("recebidos");
    std::fs::create_dir_all(&recebidos).unwrap();
    let raiz = {
        let m = pombo_correio_core::chunk::Manifest::from_bytes("v.bin", &original);
        hex::encode(m.root)
    };
    let mut parcial = original[..CHUNK_SIZE].to_vec();
    parcial.extend_from_slice(&[0xAA; 5000]);
    std::fs::write(recebidos.join(format!("{raiz}.parcial")), parcial).unwrap();

    let (ouv, ev) = ouvinte();
    let fim = receber_da_caixa(&b, &relay, &item, &recebidos, dir.0.join("c.json"), ouv, sem_cancelamento(), &Metricas::desligada())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(std::fs::read(fim).unwrap(), original);
    let retoma = ev.lock().unwrap().iter().find_map(|e| match e {
        Evento::Oferta { retoma, .. } => Some(*retoma),
        _ => None,
    });
    assert_eq!(retoma, Some(1), "devia ter retomado no bloco 1");
}

/// O relay não é de confiança: um byte adulterado no depósito faz a receção
/// falhar, **não grava o ficheiro final** e **não apaga** o depósito.
#[tokio::test(flavor = "multi_thread")]
async fn relay_que_adultera_e_apanhado() {
    let dir = Temp::novo("adultera");
    let relay = relay_de_pe(&dir).await;
    let (a, b) = (Identity::generate("A"), Identity::generate("B"));
    relay.registar(&a, None).await.unwrap();
    relay.registar(&b, None).await.unwrap();
    let (caminho, _) = ficheiro(&dir, "x.bin", CHUNK_SIZE + 4096);
    enviar_com_alternativa(
        &a, &FonteCaminho(caminho), porta_morta(), &impressao(&b), Some(&relay),
        pombo_correio_rede::silencio(), sem_cancelamento(), &Metricas::desligada(), 0,
    )
    .await
    .unwrap();

    let dados = std::fs::read_dir(dir.0.join("relay")).unwrap().filter_map(|e| e.ok()).map(|e| e.path())
        .find(|p| p.extension().is_some_and(|e| e == "dados")).unwrap();
    let mut bytes = std::fs::read(&dados).unwrap();
    let meio = bytes.len() / 2;
    bytes[meio] ^= 0x01;
    std::fs::write(&dados, bytes).unwrap();

    let item = relay.caixa(&b).await.unwrap().remove(0);
    let recebidos = dir.0.join("recebidos");
    let r = receber_da_caixa(&b, &relay, &item, &recebidos, dir.0.join("c.json"), pombo_correio_rede::silencio(), sem_cancelamento(), &Metricas::desligada()).await;
    assert!(r.is_err(), "tinha de recusar bytes adulterados");
    assert!(!recebidos.join("x.bin").exists(), "nada de ficheiro final");
    assert_eq!(relay.caixa(&b).await.unwrap().len(), 1, "o depósito não se apaga se nada foi aceite");
}

/// Sem relay configurado, o comportamento é o de sempre: erro tipado, sem
/// efeitos. É a garantia de que o relay é puramente aditivo.
#[tokio::test(flavor = "multi_thread")]
async fn sem_relay_o_erro_e_o_de_sempre() {
    let dir = Temp::novo("sem");
    let a = Identity::generate("A");
    let (caminho, _) = ficheiro(&dir, "f.bin", 100);
    let e = enviar_com_alternativa(
        &a, &FonteCaminho(caminho), porta_morta(), "0000", None,
        pombo_correio_rede::silencio(), sem_cancelamento(), &Metricas::desligada(), 0,
    )
    .await
    .unwrap_err();
    assert!(e.downcast_ref::<DestinoInacessivel>().is_some());
    assert_eq!(e.to_string(), "não consegui falar com o destino");
}

/// Destino vivo → vai direto e **nunca toca no relay** (que aqui nem existe).
/// Falha se o desvio disparar sem o destino estar inacessível.
#[tokio::test(flavor = "multi_thread")]
async fn destino_vivo_nao_usa_o_relay() {
    let dir = Temp::novo("direto");
    let (a, b) = (Identity::generate("A"), Identity::generate("B"));
    let porta = porta_morta().port();
    let recebidos = dir.0.join("recebidos");
    let (r2, c2) = (recebidos.clone(), dir.0.join("c.json"));
    tokio::spawn(async move {
        let _ = receber::servir(b, r2, c2, porta, pombo_correio_rede::silencio(), Default::default(), Metricas::desligada()).await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let relay_inexistente = Relay::novo(&porta_morta().to_string());
    let (caminho, original) = ficheiro(&dir, "d.bin", CHUNK_SIZE + 77);
    let (ouv, ev) = ouvinte();
    enviar_com_alternativa(
        &a, &FonteCaminho(caminho), format!("127.0.0.1:{porta}").parse().unwrap(), "x", Some(&relay_inexistente),
        ouv, sem_cancelamento(), &Metricas::desligada(), 0,
    )
    .await
    .unwrap();
    assert!(ev.lock().unwrap().iter().any(|e| matches!(e, Evento::Enviado { .. })));
    assert!(!ev.lock().unwrap().iter().any(|e| matches!(e, Evento::NoRelay { .. })));
    assert_eq!(std::fs::read(recebidos.join("d.bin")).unwrap(), original);
}
