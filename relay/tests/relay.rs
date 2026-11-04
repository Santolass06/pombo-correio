//! O relay, de ponta a ponta em loopback: HTTP a sério, relógio injetado.
//!
//! Provado aqui: a lógica do depósito, da retoma em bytes, da autenticação e da
//! expiração. **Não** provado: TLS, a internet, nem o relay entre máquinas.

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering::SeqCst},
};

use pombo_correio_core::{
    identity::{Identity, PublicIdentity},
    relay::{CABECALHO_ASSINATURA, CABECALHO_INSTANTE, CABECALHO_QUEM, assinar_pedido},
};
use pombo_correio_relay::{Config, Estado, router};

struct Temp(std::path::PathBuf);
impl Temp {
    fn novo(n: &str) -> Self {
        let d = std::path::PathBuf::from("/var/tmp").join(format!("pombo-relay-{n}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        Self(d)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const T0: u64 = 1_000_000;

struct Relay {
    base: String,
    relogio: Arc<AtomicU64>,
    http: reqwest::Client,
}

async fn relay(dir: &Temp, ajusta: impl FnOnce(&mut Config)) -> Relay {
    let relogio = Arc::new(AtomicU64::new(T0));
    let r = relogio.clone();
    let mut cfg = Config::por_omissao(dir.0.clone());
    cfg.relogio = Arc::new(move || r.load(SeqCst));
    ajusta(&mut cfg);
    let estado = Estado::abrir(cfg).unwrap();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, router(estado)).await.unwrap() });
    Relay { base, relogio, http: reqwest::Client::new() }
}

impl Relay {
    fn pedido(&self, eu: &Identity, metodo: &str, caminho: &str, corpo: &[u8]) -> reqwest::RequestBuilder {
        self.pedido_em(eu, metodo, caminho, corpo, self.relogio.load(SeqCst))
    }

    fn pedido_em(&self, eu: &Identity, metodo: &str, caminho: &str, corpo: &[u8], instante: u64) -> reqwest::RequestBuilder {
        let (quem, inst, sig) = assinar_pedido(eu, metodo, caminho, instante, corpo);
        self.http
            .request(metodo.parse().unwrap(), format!("{}{caminho}", self.base))
            .header(CABECALHO_QUEM, quem)
            .header(CABECALHO_INSTANTE, inst)
            .header(CABECALHO_ASSINATURA, sig)
    }

    async fn registar(&self, eu: &Identity) {
        let corpo = serde_json::to_vec(&eu.public()).unwrap();
        let r = self.pedido(eu, "POST", "/registo", &corpo).body(corpo).send().await.unwrap();
        assert_eq!(r.status(), 200);
    }

    /// Cria um depósito de `dados.len()` bytes para `para`; devolve o id.
    async fn criar(&self, de: &Identity, para: &PublicIdentity, tamanho: usize) -> String {
        let corpo = serde_json::to_vec(&serde_json::json!({
            "para": hex::encode(para.fingerprint()), "tamanho": tamanho, "oferta": {"opaca": true}
        }))
        .unwrap();
        let r = self.pedido(de, "POST", "/deposito", &corpo).body(corpo).send().await.unwrap();
        assert_eq!(r.status(), 200);
        r.json::<serde_json::Value>().await.unwrap()["id"].as_str().unwrap().to_string()
    }

    async fn subir(&self, de: &Identity, id: &str, desde: usize, bytes: &[u8]) -> reqwest::Response {
        let c = format!("/deposito/{id}/blocos?de={desde}");
        self.pedido(de, "PUT", &c, b"").body(bytes.to_vec()).send().await.unwrap()
    }

    async fn estado(&self, eu: &Identity, id: &str) -> reqwest::Response {
        self.pedido(eu, "GET", &format!("/deposito/{id}"), b"").send().await.unwrap()
    }
}

fn dados(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i * 31 % 251) as u8).collect()
}

/// Uma subida rasgada a meio retoma: o relay trunca à fronteira pedida e o
/// resultado é byte-idêntico. Falha se o `PUT` acrescentar em vez de truncar.
#[tokio::test(flavor = "multi_thread")]
async fn subida_rasgada_retoma_byte_identica() {
    let dir = Temp::novo("retoma");
    let r = relay(&dir, |_| {}).await;
    let (a, b) = (Identity::generate("a"), Identity::generate("b"));
    r.registar(&a).await;
    r.registar(&b).await;
    let d = dados(1000);
    let id = r.criar(&a, &b.public(), d.len()).await;

    // Primeira tentativa: 400 bytes chegam e a ligação "morre".
    assert_eq!(r.subir(&a, &id, 0, &d[..400]).await.status(), 200);
    let est: serde_json::Value = r.estado(&a, &id).await.json().await.unwrap();
    assert_eq!(est["tenho"], 400);
    assert_eq!(est["completo"], false);

    // O cliente arredonda para trás (aqui, 300) e recomeça: a cauda 300..400 é
    // reescrita, não duplicada.
    let rr = r.subir(&a, &id, 300, &d[300..]).await;
    assert_eq!(rr.status(), 200);
    let est: serde_json::Value = r.estado(&a, &id).await.json().await.unwrap();
    assert_eq!(est["tenho"], 1000);
    assert_eq!(est["completo"], true);

    let baixado = r.pedido(&b, "GET", &format!("/deposito/{id}/blocos"), b"").send().await.unwrap();
    assert_eq!(baixado.bytes().await.unwrap().as_ref(), d.as_slice());
}

/// Recomeçar depois do que o relay tem deixaria um buraco: recusado.
#[tokio::test(flavor = "multi_thread")]
async fn subir_com_buraco_e_recusado() {
    let dir = Temp::novo("buraco");
    let r = relay(&dir, |_| {}).await;
    let (a, b) = (Identity::generate("a"), Identity::generate("b"));
    r.registar(&a).await;
    let id = r.criar(&a, &b.public(), 100).await;
    assert_eq!(r.subir(&a, &id, 0, &dados(10)).await.status(), 200);
    assert_eq!(r.subir(&a, &id, 50, &dados(10)).await.status(), 409);
}

/// A descida com `Range` a meio devolve exatamente o resto.
#[tokio::test(flavor = "multi_thread")]
async fn descida_com_range_a_meio() {
    let dir = Temp::novo("range");
    let r = relay(&dir, |_| {}).await;
    let (a, b) = (Identity::generate("a"), Identity::generate("b"));
    r.registar(&a).await;
    r.registar(&b).await;
    let d = dados(5000);
    let id = r.criar(&a, &b.public(), d.len()).await;
    r.subir(&a, &id, 0, &d).await;

    let c = format!("/deposito/{id}/blocos");
    let resp = r.pedido(&b, "GET", &c, b"").header("range", "bytes=1234-").send().await.unwrap();
    assert_eq!(resp.status(), 206);
    assert_eq!(resp.bytes().await.unwrap().as_ref(), &d[1234..]);

    let fora = r.pedido(&b, "GET", &c, b"").header("range", "bytes=5000-").send().await.unwrap();
    assert_eq!(fora.status(), 416);
}

/// O id é do relay e imprevisível: dois depósitos do mesmo conteúdo para a mesma
/// pessoa nunca partilham id (nem têm a raiz de Merkle como id).
#[tokio::test(flavor = "multi_thread")]
async fn ids_distintos_para_o_mesmo_conteudo() {
    let dir = Temp::novo("ids");
    let r = relay(&dir, |_| {}).await;
    let (a, b) = (Identity::generate("a"), Identity::generate("b"));
    r.registar(&a).await;
    let i1 = r.criar(&a, &b.public(), 10).await;
    let i2 = r.criar(&a, &b.public(), 10).await;
    assert_ne!(i1, i2);
    assert_eq!(i1.len(), 32);
}

/// Expirado = desaparece de todo o lado, incluindo o disco. Relógio injetado.
#[tokio::test(flavor = "multi_thread")]
async fn ttl_apaga_o_deposito() {
    let dir = Temp::novo("ttl");
    let r = relay(&dir, |c| c.ttl_s = 100).await;
    let (a, b) = (Identity::generate("a"), Identity::generate("b"));
    r.registar(&a).await;
    r.registar(&b).await;
    let d = dados(50);
    let id = r.criar(&a, &b.public(), d.len()).await;
    r.subir(&a, &id, 0, &d).await;

    r.relogio.store(T0 + 99, SeqCst);
    assert_eq!(r.estado(&b, &id).await.status(), 200);
    r.relogio.store(T0 + 100, SeqCst);
    assert_eq!(r.estado(&b, &id).await.status(), 404);
    let caixa: Vec<serde_json::Value> = r.pedido(&b, "GET", "/caixa", b"").send().await.unwrap().json().await.unwrap();
    assert!(caixa.is_empty());
    assert!(!dir.0.join(format!("{id}.dados")).exists(), "o ficheiro tinha de ir embora");
}

/// Autenticação: chave errada, instante velho, e isolamento entre dispositivos.
#[tokio::test(flavor = "multi_thread")]
async fn autenticacao_e_isolamento() {
    let dir = Temp::novo("auth");
    let r = relay(&dir, |_| {}).await;
    let (a, b, c) = (Identity::generate("a"), Identity::generate("b"), Identity::generate("c"));
    for x in [&a, &b, &c] {
        r.registar(x).await;
    }
    let d = dados(40);
    let id = r.criar(&a, &b.public(), d.len()).await;
    r.subir(&a, &id, 0, &d).await;

    // Instante fora da janela → 401.
    let velho = r.pedido_em(&b, "GET", "/caixa", b"", T0 - 500).send().await.unwrap();
    assert_eq!(velho.status(), 401);

    // Assinatura de C com a impressão digital de B → 401.
    let (_, inst, sig) = assinar_pedido(&c, "GET", "/caixa", T0, b"");
    let falso = r
        .http
        .get(format!("{}/caixa", r.base))
        .header(CABECALHO_QUEM, hex::encode(b.public().fingerprint()))
        .header(CABECALHO_INSTANTE, inst)
        .header(CABECALHO_ASSINATURA, sig)
        .send()
        .await
        .unwrap();
    assert_eq!(falso.status(), 401);

    // Dispositivo não registado → 401.
    let intruso = Identity::generate("x");
    assert_eq!(r.pedido(&intruso, "GET", "/caixa", b"").send().await.unwrap().status(), 401);

    // C não vê nada de A→B: nem na caixa, nem o estado, nem os bytes.
    let caixa_c: Vec<serde_json::Value> = r.pedido(&c, "GET", "/caixa", b"").send().await.unwrap().json().await.unwrap();
    assert!(caixa_c.is_empty());
    assert_eq!(r.estado(&c, &id).await.status(), 403);
    assert_eq!(r.pedido(&c, "GET", &format!("/deposito/{id}/blocos"), b"").send().await.unwrap().status(), 403);
    // O remetente não descarrega o que depositou (só o destinatário).
    assert_eq!(r.pedido(&a, "GET", &format!("/deposito/{id}/blocos"), b"").send().await.unwrap().status(), 403);
    // C não apaga nem escreve no depósito alheio.
    assert_eq!(r.pedido(&c, "DELETE", &format!("/deposito/{id}"), b"").send().await.unwrap().status(), 403);
    assert_eq!(r.subir(&c, &id, 0, b"x").await.status(), 403);

    // B vê-o na caixa.
    let caixa_b: Vec<serde_json::Value> = r.pedido(&b, "GET", "/caixa", b"").send().await.unwrap().json().await.unwrap();
    assert_eq!(caixa_b.len(), 1);
    assert_eq!(caixa_b[0]["id"], id.as_str());
}

/// Depósito incompleto: invisível na caixa e não descarregável.
#[tokio::test(flavor = "multi_thread")]
async fn incompleto_nao_aparece_nem_desce() {
    let dir = Temp::novo("incompleto");
    let r = relay(&dir, |_| {}).await;
    let (a, b) = (Identity::generate("a"), Identity::generate("b"));
    r.registar(&a).await;
    r.registar(&b).await;
    let id = r.criar(&a, &b.public(), 100).await;
    r.subir(&a, &id, 0, &dados(60)).await;
    let caixa: Vec<serde_json::Value> = r.pedido(&b, "GET", "/caixa", b"").send().await.unwrap().json().await.unwrap();
    assert!(caixa.is_empty());
    assert_eq!(r.pedido(&b, "GET", &format!("/deposito/{id}/blocos"), b"").send().await.unwrap().status(), 409);
}

/// Limites: acima do máximo, acima da quota, e mais bytes do que o declarado.
#[tokio::test(flavor = "multi_thread")]
async fn limites_de_tamanho_e_quota() {
    let dir = Temp::novo("limites");
    let r = relay(&dir, |c| {
        c.max_deposito = 100;
        c.quota = 150;
    })
    .await;
    let (a, b) = (Identity::generate("a"), Identity::generate("b"));
    r.registar(&a).await;
    let pedido = |t: usize| serde_json::to_vec(&serde_json::json!({"para": hex::encode(b.public().fingerprint()), "tamanho": t, "oferta": {}})).unwrap();

    let grande = pedido(101);
    let s = r.pedido(&a, "POST", "/deposito", &grande).body(grande).send().await.unwrap().status();
    assert_eq!(s, 413);

    let id = r.criar(&a, &b.public(), 100).await;
    // Quota: 100 + 100 > 150.
    let seg = pedido(100);
    let s = r.pedido(&a, "POST", "/deposito", &seg).body(seg).send().await.unwrap().status();
    assert_eq!(s, 507);

    // Mais bytes do que o declarado → cortado.
    assert_eq!(r.subir(&a, &id, 0, &dados(101)).await.status(), 413);
}

/// Registo: token exigido quando configurado; impressão digital tem de bater.
#[tokio::test(flavor = "multi_thread")]
async fn registo_com_token() {
    let dir = Temp::novo("token");
    let r = relay(&dir, |c| c.token_de_registo = Some("segredo".into())).await;
    let a = Identity::generate("a");
    let corpo = serde_json::to_vec(&a.public()).unwrap();
    let sem = r.pedido(&a, "POST", "/registo", &corpo).body(corpo.clone()).send().await.unwrap();
    assert_eq!(sem.status(), 403);
    let com = r.pedido(&a, "POST", "/registo", &corpo).header("x-pombo-token", "segredo").body(corpo).send().await.unwrap();
    assert_eq!(com.status(), 200);
}

/// Reiniciar o relay não perde depósitos nem registos.
#[tokio::test(flavor = "multi_thread")]
async fn sobrevive_a_reinicio() {
    let dir = Temp::novo("reinicio");
    let (a, b) = (Identity::generate("a"), Identity::generate("b"));
    let id;
    {
        let r = relay(&dir, |_| {}).await;
        r.registar(&a).await;
        r.registar(&b).await;
        id = r.criar(&a, &b.public(), 20).await;
        r.subir(&a, &id, 0, &dados(20)).await;
    }
    let r2 = relay(&dir, |_| {}).await;
    let est: serde_json::Value = r2.estado(&b, &id).await.json().await.unwrap();
    assert_eq!(est["completo"], true);
}
