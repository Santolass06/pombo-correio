//! O relay: um depósito de **bytes opacos**.
//!
//! Não sabe o nome do ficheiro (vai no manifesto selado), nem o tamanho em
//! claro, nem distingue um vídeo de um PDF, e **não tem chave para abrir nada**.
//! Quem verifica continua a ser quem recebe, bloco a bloco, contra o manifesto.
//! Comprometer o relay não expõe ficheiro nenhum — é a razão de ser deste
//! desenho, porque é ele que fica exposto à internet.
//!
//! Fala **bytes** nos dois sentidos (ver PLANO.md, «Desenho R1–R3»). Os blocos
//! são assunto de quem envia e de quem recebe.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use futures_util::StreamExt;
use pombo_correio_core::{
    identity::PublicIdentity,
    relay::{CABECALHO_ASSINATURA, CABECALHO_INSTANTE, CABECALHO_QUEM, verificar_pedido},
};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

/// Relógio em segundos Unix. Injetável para os testes não dormirem a sério.
pub type Relogio = Arc<dyn Fn() -> u64 + Send + Sync>;

pub fn relogio_do_sistema() -> Relogio {
    Arc::new(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    })
}

#[derive(Clone)]
pub struct Config {
    pub dir: PathBuf,
    /// Tamanho selado máximo de um depósito.
    pub max_deposito: u64,
    /// Soma máxima dos tamanhos declarados de todos os depósitos.
    pub quota: u64,
    /// Vida de um depósito, em segundos.
    pub ttl_s: u64,
    pub max_dispositivos: usize,
    /// Se definido, `POST /registo` exige este valor em `x-pombo-token`.
    pub token_de_registo: Option<String>,
    pub relogio: Relogio,
}

impl Config {
    pub fn por_omissao(dir: PathBuf) -> Self {
        Self {
            dir,
            max_deposito: 8 << 30,
            quota: 50 << 30,
            ttl_s: 7 * 24 * 3600,
            max_dispositivos: 64,
            token_de_registo: None,
            relogio: relogio_do_sistema(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
struct Meta {
    id: String,
    /// Impressões digitais em hexadecimal.
    de: String,
    para: String,
    /// Tamanho selado declarado.
    tamanho: u64,
    /// A `Oferta`, opaca para o relay.
    oferta: serde_json::Value,
    criado: u64,
}

struct Deposito {
    meta: Meta,
    /// Serializa os `PUT` do mesmo depósito.
    escrita: Arc<tokio::sync::Mutex<()>>,
}

struct Interior {
    dispositivos: HashMap<String, PublicIdentity>,
    depositos: HashMap<String, Deposito>,
}

pub struct Estado {
    cfg: Config,
    interior: Mutex<Interior>,
}

type Erro = (StatusCode, String);

fn erro(c: StatusCode, m: &str) -> Erro {
    (c, m.to_string())
}

/// Os identificadores são nomes de ficheiro: só hexadecimal de 32 dígitos, para
/// que nada vindo da rede consiga sair da pasta do relay.
fn id_valido(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

impl Estado {
    /// Abre (ou cria) o relay numa pasta, recuperando o que lá estiver.
    pub fn abrir(cfg: Config) -> anyhow::Result<Arc<Self>> {
        std::fs::create_dir_all(&cfg.dir)?;
        let dispositivos: HashMap<String, PublicIdentity> =
            std::fs::read(cfg.dir.join("dispositivos.json"))
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default();

        let mut depositos = HashMap::new();
        for entrada in std::fs::read_dir(&cfg.dir)? {
            let caminho = entrada?.path();
            if caminho.extension().is_some_and(|e| e == "meta") {
                if let Some(meta) = std::fs::read(&caminho)
                    .ok()
                    .and_then(|b| serde_json::from_slice::<Meta>(&b).ok())
                    .filter(|m| id_valido(&m.id))
                {
                    depositos.insert(
                        meta.id.clone(),
                        Deposito { meta, escrita: Default::default() },
                    );
                }
            }
        }
        Ok(Arc::new(Self { cfg, interior: Mutex::new(Interior { dispositivos, depositos }) }))
    }

    fn agora(&self) -> u64 {
        (self.cfg.relogio)()
    }

    fn dados(&self, id: &str) -> PathBuf {
        self.cfg.dir.join(format!("{id}.dados"))
    }

    fn meta_path(&self, id: &str) -> PathBuf {
        self.cfg.dir.join(format!("{id}.meta"))
    }

    /// Apaga os depósitos expirados. Chamado no início de cada pedido: dispensa
    /// uma tarefa de fundo e torna a expiração observável por testes.
    pub fn varrer(&self) {
        let agora = self.agora();
        let mut i = self.interior.lock().unwrap();
        let expirados: Vec<String> = i
            .depositos
            .values()
            .filter(|d| d.meta.criado.saturating_add(self.cfg.ttl_s) <= agora)
            .map(|d| d.meta.id.clone())
            .collect();
        for id in expirados {
            i.depositos.remove(&id);
            let _ = std::fs::remove_file(self.dados(&id));
            let _ = std::fs::remove_file(self.meta_path(&id));
        }
    }

    /// Autentica um pedido assinado por um dispositivo **registado**.
    fn autenticar(
        &self,
        h: &HeaderMap,
        metodo: &Method,
        uri: &Uri,
        corpo: &[u8],
    ) -> Result<String, Erro> {
        let (quem, p) = self.cabecalhos(h)?;
        let dev = self
            .interior
            .lock()
            .unwrap()
            .dispositivos
            .get(&quem)
            .cloned()
            .ok_or_else(|| erro(StatusCode::UNAUTHORIZED, "dispositivo não registado"))?;
        self.verificar(&dev, h, metodo, uri, corpo, p)?;
        Ok(quem)
    }

    fn cabecalhos(&self, h: &HeaderMap) -> Result<(String, u64), Erro> {
        let ler = |n: &str| h.get(n).and_then(|v| v.to_str().ok()).map(str::to_string);
        let quem = ler(CABECALHO_QUEM).ok_or_else(|| erro(StatusCode::UNAUTHORIZED, "falta quem"))?;
        let inst = ler(CABECALHO_INSTANTE)
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| erro(StatusCode::UNAUTHORIZED, "falta o instante"))?;
        Ok((quem, inst))
    }

    fn verificar(
        &self,
        dev: &PublicIdentity,
        h: &HeaderMap,
        metodo: &Method,
        uri: &Uri,
        corpo: &[u8],
        instante: u64,
    ) -> Result<(), Erro> {
        let sig = h
            .get(CABECALHO_ASSINATURA)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| erro(StatusCode::UNAUTHORIZED, "falta a assinatura"))?;
        let caminho = uri.path_and_query().map(|p| p.as_str()).unwrap_or(uri.path());
        if verificar_pedido(dev, metodo.as_str(), caminho, instante, sig, corpo, self.agora()) {
            Ok(())
        } else {
            Err(erro(StatusCode::UNAUTHORIZED, "assinatura inválida ou fora de prazo"))
        }
    }
}

pub fn router(estado: Arc<Estado>) -> Router {
    Router::new()
        .route("/registo", post(registar))
        .route("/deposito", post(criar))
        .route("/deposito/{id}", get(estado_do_deposito).delete(apagar))
        .route("/deposito/{id}/blocos", put(subir).get(descer))
        .route("/caixa", get(caixa))
        .with_state(estado)
}

/// Põe o relay a servir num `listener` já aberto.
pub async fn servir(listener: tokio::net::TcpListener, cfg: Config) -> anyhow::Result<()> {
    let estado = Estado::abrir(cfg)?;
    axum::serve(listener, router(estado)).await?;
    Ok(())
}

// --- Registo -------------------------------------------------------------

async fn registar(
    State(e): State<Arc<Estado>>,
    h: HeaderMap,
    metodo: Method,
    uri: Uri,
    corpo: Bytes,
) -> Result<StatusCode, Erro> {
    e.varrer();
    if let Some(token) = &e.cfg.token_de_registo {
        if h.get("x-pombo-token").and_then(|v| v.to_str().ok()) != Some(token.as_str()) {
            return Err(erro(StatusCode::FORBIDDEN, "token de registo inválido"));
        }
    }
    let p: PublicIdentity =
        serde_json::from_slice(&corpo).map_err(|_| erro(StatusCode::BAD_REQUEST, "identidade ilegível"))?;
    let fp = hex::encode(p.fingerprint());

    // Auto-assinado: prova que quem se regista tem a chave de assinatura que diz
    // ter, e que a impressão digital cabe ao que anuncia.
    let (quem, inst) = e.cabecalhos(&h)?;
    if quem != fp {
        return Err(erro(StatusCode::BAD_REQUEST, "a impressão digital não bate"));
    }
    e.verificar(&p, &h, &metodo, &uri, &corpo, inst)?;

    let mut i = e.interior.lock().unwrap();
    if !i.dispositivos.contains_key(&fp) && i.dispositivos.len() >= e.cfg.max_dispositivos {
        return Err(erro(StatusCode::INSUFFICIENT_STORAGE, "relay cheio de dispositivos"));
    }
    i.dispositivos.insert(fp, p);
    let json = serde_json::to_vec(&i.dispositivos).unwrap();
    drop(i);
    std::fs::write(e.cfg.dir.join("dispositivos.json"), json)
        .map_err(|_| erro(StatusCode::INTERNAL_SERVER_ERROR, "não gravei o registo"))?;
    Ok(StatusCode::OK)
}

// --- Depósito ------------------------------------------------------------

#[derive(Deserialize)]
struct Pedido {
    /// Impressão digital do destinatário.
    para: String,
    /// Tamanho selado total.
    tamanho: u64,
    oferta: serde_json::Value,
}

async fn criar(
    State(e): State<Arc<Estado>>,
    h: HeaderMap,
    metodo: Method,
    uri: Uri,
    corpo: Bytes,
) -> Result<Json<serde_json::Value>, Erro> {
    e.varrer();
    let de = e.autenticar(&h, &metodo, &uri, &corpo)?;
    let p: Pedido =
        serde_json::from_slice(&corpo).map_err(|_| erro(StatusCode::BAD_REQUEST, "pedido ilegível"))?;

    if p.tamanho == 0 || p.tamanho > e.cfg.max_deposito {
        return Err(erro(StatusCode::PAYLOAD_TOO_LARGE, "depósito acima do tamanho máximo"));
    }
    let mut id_bytes = [0u8; 16];
    getrandom::fill(&mut id_bytes).map_err(|_| erro(StatusCode::INTERNAL_SERVER_ERROR, "sem aleatoriedade"))?;
    let id = hex::encode(id_bytes);

    let meta = Meta { id: id.clone(), de, para: p.para, tamanho: p.tamanho, oferta: p.oferta, criado: e.agora() };

    let mut i = e.interior.lock().unwrap();
    let usado: u64 = i.depositos.values().map(|d| d.meta.tamanho).sum();
    if usado.saturating_add(meta.tamanho) > e.cfg.quota {
        return Err(erro(StatusCode::INSUFFICIENT_STORAGE, "o relay não tem espaço"));
    }
    std::fs::write(e.meta_path(&id), serde_json::to_vec(&meta).unwrap())
        .map_err(|_| erro(StatusCode::INTERNAL_SERVER_ERROR, "não gravei o depósito"))?;
    i.depositos.insert(id.clone(), Deposito { meta, escrita: Default::default() });
    Ok(Json(serde_json::json!({ "id": id })))
}

/// Bytes que o relay tem, lidos do disco: a verdade é o ficheiro.
fn tenho(e: &Estado, id: &str) -> u64 {
    std::fs::metadata(e.dados(id)).map(|m| m.len()).unwrap_or(0)
}

fn obter(e: &Estado, id: &str) -> Result<(Meta, Arc<tokio::sync::Mutex<()>>), Erro> {
    if !id_valido(id) {
        return Err(erro(StatusCode::NOT_FOUND, "depósito desconhecido"));
    }
    e.interior
        .lock()
        .unwrap()
        .depositos
        .get(id)
        .map(|d| (d.meta.clone(), d.escrita.clone()))
        .ok_or_else(|| erro(StatusCode::NOT_FOUND, "depósito desconhecido"))
}

#[derive(Deserialize)]
struct De {
    de: u64,
}

async fn subir(
    State(e): State<Arc<Estado>>,
    Path(id): Path<String>,
    Query(De { de }): Query<De>,
    h: HeaderMap,
    metodo: Method,
    uri: Uri,
    corpo: Body,
) -> Result<Json<serde_json::Value>, Erro> {
    e.varrer();
    // O corpo em fluxo não entra na assinatura (ver PLANO.md).
    let quem = e.autenticar(&h, &metodo, &uri, b"")?;
    let (meta, escrita) = obter(&e, &id)?;
    if quem != meta.de {
        return Err(erro(StatusCode::FORBIDDEN, "o depósito não é teu"));
    }
    let _guarda = escrita.lock().await;

    let ja = tenho(&e, &id);
    if de > ja {
        return Err(erro(StatusCode::CONFLICT, "há um buraco: pediste para começar depois do que tenho"));
    }

    let mut f = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(e.dados(&id))
        .await
        .map_err(|_| erro(StatusCode::INTERNAL_SERVER_ERROR, "não abri o depósito"))?;
    // Cortar a cauda possivelmente rasgada de uma subida anterior.
    f.set_len(de).await.map_err(|_| erro(StatusCode::INTERNAL_SERVER_ERROR, "truncar falhou"))?;
    f.seek(std::io::SeekFrom::Start(de)).await.map_err(|_| erro(StatusCode::INTERNAL_SERVER_ERROR, "seek falhou"))?;

    let mut pos = de;
    let mut fluxo = corpo.into_data_stream();
    while let Some(pedaco) = fluxo.next().await {
        let pedaco = pedaco.map_err(|_| erro(StatusCode::BAD_REQUEST, "a ligação caiu"))?;
        if pos + pedaco.len() as u64 > meta.tamanho {
            f.set_len(meta.tamanho.min(pos)).await.ok();
            return Err(erro(StatusCode::PAYLOAD_TOO_LARGE, "mais bytes do que o declarado"));
        }
        f.write_all(&pedaco).await.map_err(|_| erro(StatusCode::INTERNAL_SERVER_ERROR, "escrita falhou"))?;
        pos += pedaco.len() as u64;
    }
    f.flush().await.ok();
    Ok(Json(serde_json::json!({ "tenho": pos, "completo": pos == meta.tamanho })))
}

fn descricao(e: &Estado, m: &Meta) -> serde_json::Value {
    let t = tenho(e, &m.id);
    serde_json::json!({
        "id": m.id, "de": m.de, "para": m.para, "tamanho": m.tamanho,
        "tenho": t, "completo": t == m.tamanho, "oferta": m.oferta,
    })
}

async fn estado_do_deposito(
    State(e): State<Arc<Estado>>,
    Path(id): Path<String>,
    h: HeaderMap,
    metodo: Method,
    uri: Uri,
) -> Result<Json<serde_json::Value>, Erro> {
    e.varrer();
    let quem = e.autenticar(&h, &metodo, &uri, b"")?;
    let (meta, _) = obter(&e, &id)?;
    if quem != meta.de && quem != meta.para {
        return Err(erro(StatusCode::FORBIDDEN, "o depósito não é teu"));
    }
    Ok(Json(descricao(&e, &meta)))
}

async fn caixa(
    State(e): State<Arc<Estado>>,
    h: HeaderMap,
    metodo: Method,
    uri: Uri,
) -> Result<Json<Vec<serde_json::Value>>, Erro> {
    e.varrer();
    let quem = e.autenticar(&h, &metodo, &uri, b"")?;
    let metas: Vec<Meta> = e
        .interior
        .lock()
        .unwrap()
        .depositos
        .values()
        .filter(|d| d.meta.para == quem)
        .map(|d| d.meta.clone())
        .collect();
    let mut itens: Vec<_> = metas
        .iter()
        .map(|m| descricao(&e, m))
        .filter(|d| d["completo"] == true)
        .collect();
    itens.sort_by_key(|d| d["id"].as_str().map(str::to_string));
    Ok(Json(itens))
}

async fn descer(
    State(e): State<Arc<Estado>>,
    Path(id): Path<String>,
    h: HeaderMap,
    metodo: Method,
    uri: Uri,
) -> Result<Response, Erro> {
    e.varrer();
    let quem = e.autenticar(&h, &metodo, &uri, b"")?;
    let (meta, _) = obter(&e, &id)?;
    if quem != meta.para {
        return Err(erro(StatusCode::FORBIDDEN, "o depósito não é teu"));
    }
    if tenho(&e, &id) != meta.tamanho {
        return Err(erro(StatusCode::CONFLICT, "o depósito ainda não está completo"));
    }

    // Só se suporta `bytes=N-`: é tudo o que a retoma precisa.
    let inicio = match h.get(header::RANGE).and_then(|v| v.to_str().ok()) {
        None => 0,
        Some(r) => r
            .strip_prefix("bytes=")
            .and_then(|r| r.strip_suffix('-'))
            .and_then(|n| n.parse::<u64>().ok())
            .filter(|n| *n < meta.tamanho)
            .ok_or_else(|| erro(StatusCode::RANGE_NOT_SATISFIABLE, "intervalo inválido"))?,
    };

    let mut f = tokio::fs::File::open(e.dados(&id))
        .await
        .map_err(|_| erro(StatusCode::INTERNAL_SERVER_ERROR, "não abri o depósito"))?;
    f.seek(std::io::SeekFrom::Start(inicio))
        .await
        .map_err(|_| erro(StatusCode::INTERNAL_SERVER_ERROR, "seek falhou"))?;

    let corpo = Body::from_stream(tokio_util::io::ReaderStream::with_capacity(f, 256 * 1024));
    let mut r = Response::new(corpo);
    r.headers_mut().insert(header::CONTENT_LENGTH, HeaderValue::from(meta.tamanho - inicio));
    if inicio > 0 {
        *r.status_mut() = StatusCode::PARTIAL_CONTENT;
        r.headers_mut().insert(
            header::CONTENT_RANGE,
            HeaderValue::from_str(&format!("bytes {inicio}-{}/{}", meta.tamanho - 1, meta.tamanho)).unwrap(),
        );
    }
    Ok(r.into_response())
}

async fn apagar(
    State(e): State<Arc<Estado>>,
    Path(id): Path<String>,
    h: HeaderMap,
    metodo: Method,
    uri: Uri,
) -> Result<StatusCode, Erro> {
    e.varrer();
    let quem = e.autenticar(&h, &metodo, &uri, b"")?;
    let (meta, escrita) = obter(&e, &id)?;
    if quem != meta.de && quem != meta.para {
        return Err(erro(StatusCode::FORBIDDEN, "o depósito não é teu"));
    }
    let _guarda = escrita.lock().await;
    e.interior.lock().unwrap().depositos.remove(&id);
    let _ = std::fs::remove_file(e.dados(&id));
    let _ = std::fs::remove_file(e.meta_path(&id));
    Ok(StatusCode::NO_CONTENT)
}
