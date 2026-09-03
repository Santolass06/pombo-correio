//! O relay em HTTP.
//!
//! Deliberadamente burro: aceita bytes, guarda bytes, devolve bytes. Não abre
//! nada, não verifica nada, não sabe o que leva. É o que faz de ser a única peça
//! exposta à internet — comprometê-la não expõe ficheiro nenhum.

use std::{collections::HashSet, net::IpAddr, sync::Arc};

use anyhow::Result;
use axum::{
    Json, Router,
    body::Body,
    extract::{ConnectInfo, Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tower_governor::{
    GovernorLayer, errors::GovernorError, governor::GovernorConfigBuilder,
    key_extractor::KeyExtractor,
};
use tower_http::cors::{Any, CorsLayer};

use crate::{
    autenticado::{self, Autorizados},
    deposito::{Deposito, VALIDADE},
    fcm::Fcm,
    push,
    subscricoes::{Subscricao, Subscricoes},
    vapid::Vapid,
};

struct Estado {
    deposito: Deposito,
    autorizados: Autorizados,
    /// Pasta com a página do link sem instalação. `None` desliga o caminho todo
    /// — quem não a instalar tem um relay que continua a servir a app na mesma.
    pagina: Option<std::path::PathBuf>,
    /// Pasta com a PWA construída (`ui/dist`). Servida na raiz — ver `app_estatico`.
    /// Só faz sentido num dos nós de casa (S8: "um único sítio serve a app").
    app: Option<std::path::PathBuf>,
    /// Depósitos com uma subida a decorrer. Duas subidas ao mesmo depósito
    /// escreveriam por cima uma da outra e o ficheiro sairia com blocos de
    /// ambas — indistinguível de corrupção, e só descoberto no fim, na
    /// verificação de quem recebe.
    a_subir: tokio::sync::Mutex<HashSet<String>>,
    /// A identidade do relay perante os serviços de push (P4). Gerada uma
    /// vez, vive em `--dados` — nunca no repositório.
    vapid: Vapid,
    /// A conta de serviço Firebase (Q5) — `None` até o André a configurar,
    /// e o relay continua a funcionar na mesma para todo o resto.
    fcm: Option<Fcm>,
    subscricoes: Subscricoes,
    /// Um só cliente HTTP, reutilizado — não um novo por push (o `reqwest`
    /// junta ligações num pool; recriá-lo a cada envio perdia isso).
    cliente_push: reqwest::Client,
}

type Erro = (StatusCode, String);

fn mau(c: StatusCode, e: impl std::fmt::Display) -> Erro {
    (c, e.to_string())
}

#[derive(Deserialize)]
struct Criar {
    para: String,
    tamanho: u64,
}

#[derive(Serialize)]
struct Criado {
    id: String,
}

/// Abre um depósito. O corpo é a oferta, e passa por aqui sem ser lida.
///
/// Assinado, e o remetente tem de estar na lista de casa: sem isso, qualquer um
/// na internet enchia o disco. Note-se que a assinatura sozinha não chegaria —
/// prova posse de uma chave, não pertença a esta casa.
async fn criar(
    State(e): State<Arc<Estado>>,
    Query(q): Query<Criar>,
    cabecalhos: HeaderMap,
    oferta: String,
) -> Result<Json<Criado>, Erro> {
    let prova = autenticado::verificar(
        &cabecalhos,
        "deposito",
        &format!("{}|{}", q.para, q.tamanho),
    )?;
    if !e.autorizados.deixa(&prova.impressao) {
        return Err(mau(StatusCode::FORBIDDEN, "este relay não é teu"));
    }

    let id = e
        .deposito
        .criar(&prova.impressao, &q.para, q.tamanho, oferta)
        .await
        .map_err(|x| mau(StatusCode::BAD_REQUEST, x))?;
    Ok(Json(Criado { id }))
}

async fn estado_do_deposito(
    State(e): State<Arc<Estado>>,
    Path(id): Path<String>,
) -> Result<Json<crate::deposito::Estado>, Erro> {
    e.deposito
        .estado(&id)
        .await
        .map(Json)
        .map_err(|x| mau(StatusCode::NOT_FOUND, x))
}

#[derive(Deserialize)]
struct De {
    #[serde(default)]
    de: u64,
}

#[derive(Serialize)]
struct Subiu {
    bytes: u64,
}

/// Recebe os bytes selados, em fluxo. `?de=N` retoma: ver `abrir_para_subir`.
async fn subir(
    State(e): State<Arc<Estado>>,
    Path(id): Path<String>,
    Query(De { de }): Query<De>,
    corpo: Body,
) -> Result<Json<Subiu>, Erro> {
    let ficha = e
        .deposito
        .ficha(&id)
        .await
        .map_err(|x| mau(StatusCode::NOT_FOUND, x))?;

    if !e.a_subir.lock().await.insert(id.clone()) {
        return Err(mau(StatusCode::CONFLICT, "já há uma subida a decorrer"));
    }
    // A partir daqui há sempre que largar a marca, aconteça o que acontecer.
    let resultado = escrever(&e, &id, de, ficha.tamanho, corpo).await;
    e.a_subir.lock().await.remove(&id);

    let bytes = resultado.map_err(|x| mau(StatusCode::BAD_REQUEST, x))?;

    // Só ao ficar completo — não a cada bloco, e não em `criar()`: a `caixa`
    // lista o depósito assim que é aberto, mas a app de quem recebe não sabe
    // retomar uma subida a meio (P3/P5, `ponytail`), por isso avisar cedo
    // mandava-o buscar um manifesto que ainda não fecha.
    if bytes >= ficha.tamanho {
        let e = e.clone();
        let para = ficha.para.clone();
        tokio::spawn(async move {
            push::notificar(&e.cliente_push, &e.vapid, e.fcm.as_ref(), &e.subscricoes, &para).await
        });
    }

    Ok(Json(Subiu { bytes }))
}

async fn escrever(e: &Estado, id: &str, de: u64, tamanho: u64, corpo: Body) -> Result<u64> {
    // Vem já posicionado em `de`.
    let mut f = e.deposito.abrir_para_subir(id, de).await?;
    let mut escritos = de;
    let mut fluxo = corpo.into_data_stream();
    while let Some(pedaco) = fluxo.next().await {
        let pedaco = pedaco?;
        // Cortar no limite declarado, e não depois de o disco encher: quem envia
        // pode mentir no tamanho, e o relay é a peça exposta.
        if escritos + pedaco.len() as u64 > tamanho {
            f.flush().await?;
            anyhow::bail!("passou o tamanho declarado");
        }
        f.write_all(&pedaco).await?;
        escritos += pedaco.len() as u64;
    }
    f.flush().await?;
    Ok(escritos)
}

/// Lê `Range: bytes=N-`, que é a única forma de que a retoma precisa.
///
/// Um pedido com várias gamas, ou com fim declarado, não aparece aqui — quem
/// desce é o nosso cliente e pede sempre do ponto onde ia até ao fim. Ignorar o
/// que não se percebe (e servir tudo) seria pior do que dizer que não se serve:
/// o cliente juntaria os bytes ao que já tinha e o ficheiro sairia maior.
fn desde_onde(cabecalhos: &HeaderMap) -> Result<u64, Erro> {
    let Some(v) = cabecalhos.get(header::RANGE) else {
        return Ok(0);
    };
    let v = v.to_str().map_err(|_| mau(StatusCode::BAD_REQUEST, "Range ilegível"))?;
    let n = v
        .strip_prefix("bytes=")
        .and_then(|r| r.strip_suffix('-'))
        .and_then(|n| n.parse::<u64>().ok())
        .ok_or_else(|| {
            mau(
                StatusCode::RANGE_NOT_SATISFIABLE,
                "só se serve `bytes=N-`",
            )
        })?;
    Ok(n)
}

/// Devolve os bytes selados a partir de onde quem recebe ficou.
///
/// Serve o que existe, mesmo que a subida ainda vá a meio: quem recebe conhece o
/// manifesto, dá pela falta e volta a pedir. É o comportamento certo e ainda por
/// cima deixa a descida começar antes de a subida acabar.
async fn descer(
    State(e): State<Arc<Estado>>,
    Path(id): Path<String>,
    cabecalhos: HeaderMap,
) -> Result<Response, Erro> {
    // Passa pela ficha de propósito: valida o identificador e recusa um depósito
    // que já não existe, em vez de devolver um ficheiro vazio.
    e.deposito
        .ficha(&id)
        .await
        .map_err(|x| mau(StatusCode::NOT_FOUND, x))?;

    let de = desde_onde(&cabecalhos)?;
    let tem = e.deposito.bytes(&id).await;
    if de > tem {
        return Err(mau(
            StatusCode::RANGE_NOT_SATISFIABLE,
            format!("pediu do byte {de} mas só tenho {tem}"),
        ));
    }

    let mut f = tokio::fs::File::open(e.deposito.blocos_em(&id))
        .await
        .map_err(|x| mau(StatusCode::NOT_FOUND, x))?;
    f.seek(std::io::SeekFrom::Start(de))
        .await
        .map_err(|x| mau(StatusCode::INTERNAL_SERVER_ERROR, x))?;

    let corpo = Body::from_stream(tokio_util::io::ReaderStream::new(f));
    let restante = tem - de;

    let mut r = Response::builder()
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CONTENT_LENGTH, restante)
        .header(header::ACCEPT_RANGES, "bytes");

    if de > 0 {
        r = r.status(StatusCode::PARTIAL_CONTENT).header(
            header::CONTENT_RANGE,
            format!("bytes {de}-{}/{tem}", tem.saturating_sub(1)),
        );
    }

    r.body(corpo)
        .map_err(|x| mau(StatusCode::INTERNAL_SERVER_ERROR, x))
}

async fn apagar(State(e): State<Arc<Estado>>, Path(id): Path<String>) -> Result<StatusCode, Erro> {
    e.deposito
        .apagar(&id)
        .await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(|x| mau(StatusCode::NOT_FOUND, x))
}

/// O que está à espera de quem pergunta.
///
/// **Não leva a impressão digital no caminho**, e é de propósito: ela sai das
/// chaves que vêm assinadas no pedido, por isso não há sequer forma de pedir a
/// caixa de outra pessoa. Um parâmetro convidaria a tentar.
async fn caixa(State(e): State<Arc<Estado>>, cabecalhos: HeaderMap) -> Result<Json<Vec<String>>, Erro> {
    let prova = autenticado::verificar(&cabecalhos, "caixa", "")?;
    Ok(Json(e.deposito.caixa_de(&prova.impressao).await.unwrap_or_default()))
}

async fn saude() -> impl IntoResponse {
    "pombo-relay"
}

#[derive(Serialize)]
struct ChaveVapid {
    chave: String,
}

/// A chave pública VAPID — sem segredo nenhum aqui dentro, por isso sem
/// autenticação: é o que `pushManager.subscribe()` precisa antes de sequer
/// haver uma identidade a assinar coisa nenhuma.
async fn vapid_publica(State(e): State<Arc<Estado>>) -> Json<ChaveVapid> {
    Json(ChaveVapid { chave: e.vapid.chave_publica_base64url() })
}

/// Regista (ou substitui) a subscrição de push de quem pede.
///
/// Assinado como tudo o resto — a `extra` liga a assinatura ao
/// endpoint/token concreto, não só à ação, senão uma assinatura apanhada a
/// subscrever serviria para registar qualquer outro em nome da mesma pessoa.
async fn subscrever(
    State(e): State<Arc<Estado>>,
    cabecalhos: HeaderMap,
    Json(sub): Json<Subscricao>,
) -> Result<StatusCode, Erro> {
    let prova = autenticado::verificar(&cabecalhos, "subscricao", sub.extra())?;
    if !e.autorizados.deixa(&prova.impressao) {
        return Err(mau(StatusCode::FORBIDDEN, "este relay não é teu"));
    }
    e.subscricoes
        .guardar(&prova.impressao, sub)
        .await
        .map_err(|x| mau(StatusCode::INTERNAL_SERVER_ERROR, x))?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct Convite {
    id: String,
}

/// Autoriza quem pede, se souber o `id` de um **link** criado por alguém já
/// autorizado (S10 T3, Review3 F-14) — abri-lo já ensina a identidade de
/// quem o criou (P5); se essa pessoa já é de casa, quem abre merece a
/// mesma confiança, sem o André copiar impressões à mão.
///
/// Assinado com `extra=id`, não só a ação — sem isso, uma assinatura apanhada
/// a convidar-se para um `id` serviria para se convidar por outro qualquer.
/// A prova é sempre da **própria** impressão de quem assina: não há como
/// pedir a autorização de outra pessoa por este caminho.
///
/// **Só serve para links** (`ficha.para == CAIXA_LIGACAO`), não para
/// qualquer depósito: sem esta condição, quem recebesse um depósito normal
/// (o `id` aprende-se por `/caixa`, sem precisar de assinatura nenhuma)
/// autorizava-se sozinho só por ter sido o destinatário de alguém de casa —
/// a reciprocidade que o desenho original (PLANO.md) decidiu **não**
/// construir, por falta de um mecanismo de prova pensado para esse caso.
///
/// **Um convite serve uma vez.** Achado a rever isto, não hipotético: um
/// link é feito para ser reencaminhado (é a frase que a própria interface
/// mostra — "quem tiver este endereço abre o ficheiro"), por isso o
/// segredo de 128 bits sozinho não limita quantas identidades novas se
/// autorizam com o mesmo `id`; sem consumir o convite, um único link
/// WhatsApp-partilhado autorizava um número sem limite de dispositivos no
/// relay público. `consumir_convite` apaga o `de` da ficha a seguir a
/// autorizar — a próxima tentativa cai no mesmo 403 de "sem criador
/// registado" que já existia para links anteriores ao T3.
async fn convidar(
    State(e): State<Arc<Estado>>,
    cabecalhos: HeaderMap,
    Json(corpo): Json<Convite>,
) -> Result<StatusCode, Erro> {
    let prova = autenticado::verificar(&cabecalhos, "convite", &corpo.id)?;

    // Já autorizado: nada a fazer, mas não é erro — o cliente chama isto
    // sempre que abre um link, sem saber de antemão se já está autorizado.
    if e.autorizados.deixa(&prova.impressao) {
        return Ok(StatusCode::NO_CONTENT);
    }

    let ficha = e.deposito.ficha(&corpo.id).await.map_err(|x| mau(StatusCode::NOT_FOUND, x))?;
    if ficha.para != pombo_correio_core::protocolo::CAIXA_LIGACAO
        || ficha.de.is_empty()
        || !e.autorizados.deixa(&ficha.de)
    {
        return Err(mau(StatusCode::FORBIDDEN, "este link não serve de convite"));
    }

    e.autorizados
        .autorizar(&prova.impressao)
        .map_err(|x| mau(StatusCode::INTERNAL_SERVER_ERROR, x))?;
    // Melhor esforço: se isto falhar, o pior caso é o link continuar a
    // valer como convite — não vale a pena desfazer a autorização já
    // concedida por causa disso.
    let _ = e.deposito.consumir_convite(&corpo.id).await;
    Ok(StatusCode::NO_CONTENT)
}

/// Recusa tudo o que não seja um nome de ficheiro nosso.
///
/// Vem do URL e vai virar caminho em disco. Em vez de tentar limpar o que chega,
/// aceita-se apenas o alfabeto que os nossos ficheiros usam — a mesma escolha do
/// identificador de depósito, e pela mesma razão: é curto de ler e não tem
/// cantos. Sem isto, `../../etc/passwd` saía pela porta da frente.
fn caminho_de_pagina(pedido: &str) -> Option<std::path::PathBuf> {
    let mut fora = std::path::PathBuf::new();
    for parte in pedido.split('/') {
        let aceitavel = !parte.is_empty()
            && parte != ".."
            && parte
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'));
        if !aceitavel {
            return None;
        }
        fora.push(parte);
    }
    (!fora.as_os_str().is_empty()).then_some(fora)
}

/// O tipo de conteúdo pelo sufixo.
///
/// O `.wasm` é o que obriga a existir esta função: servido como
/// `octet-stream`, o `instantiateStreaming` recusa-o e a página não arranca.
fn tipo_de(ficheiro: &std::path::Path) -> &'static str {
    match ficheiro.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("wasm") => "application/wasm",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        Some("webmanifest") => "application/manifest+json",
        _ => "application/octet-stream",
    }
}

async fn ficheiro_da_pagina(estado: &Estado, relativo: &std::path::Path) -> Result<Response, Erro> {
    let raiz = estado
        .pagina
        .as_ref()
        .ok_or_else(|| mau(StatusCode::NOT_FOUND, "este relay não serve a página"))?;

    let caminho = raiz.join(relativo);
    let bytes = tokio::fs::read(&caminho)
        .await
        .map_err(|_| mau(StatusCode::NOT_FOUND, "não existe"))?;

    Ok(([(header::CONTENT_TYPE, tipo_de(&caminho))], bytes).into_response())
}

/// A página do link. **O identificador não é usado aqui para nada** — quem o lê
/// é o JavaScript, a partir da barra de endereço, junto com a chave que vem
/// depois do `#` e que nunca chega até cá.
async fn pagina_do_link(State(e): State<Arc<Estado>>) -> Result<Response, Erro> {
    ficheiro_da_pagina(&e, std::path::Path::new("index.html")).await
}

async fn recurso_da_pagina(
    State(e): State<Arc<Estado>>,
    Path(caminho): Path<String>,
) -> Result<Response, Erro> {
    let relativo =
        caminho_de_pagina(&caminho).ok_or_else(|| mau(StatusCode::NOT_FOUND, "não existe"))?;
    ficheiro_da_pagina(&e, &relativo).await
}

/// A PWA (`ui/dist`), servida na raiz.
///
/// É `fallback`, não uma rota própria — o axum só a chama depois de tentar
/// todas as rotas exatas acima, por isso `/saude`, `/deposito`, `/caixa` etc.
/// nunca colidem com um ficheiro do build que se chamasse na mesma. Sem
/// rotas do lado do cliente hoje, mas cai para o `index.html` de qualquer
/// forma: é o comportamento certo para uma SPA, e um caminho que não é
/// ficheiro nenhum (`/`, por exemplo) tem de chegar a alguma página.
async fn app_estatico(State(e): State<Arc<Estado>>, uri: axum::http::Uri) -> Result<Response, Erro> {
    let raiz = e
        .app
        .as_ref()
        .ok_or_else(|| mau(StatusCode::NOT_FOUND, "este relay não serve a app"))?;

    let pedido = uri.path().trim_start_matches('/');
    let relativo = if pedido.is_empty() {
        std::path::PathBuf::from("index.html")
    } else {
        // Mesma sanitização da página do link — vem do URL e vira caminho em
        // disco, pela mesma razão de sempre.
        caminho_de_pagina(pedido).unwrap_or_else(|| std::path::PathBuf::from("index.html"))
    };

    match tokio::fs::read(raiz.join(&relativo)).await {
        Ok(bytes) => Ok(([(header::CONTENT_TYPE, tipo_de(&relativo))], bytes).into_response()),
        Err(_) => {
            let bytes = tokio::fs::read(raiz.join("index.html"))
                .await
                .map_err(|_| mau(StatusCode::NOT_FOUND, "não existe"))?;
            Ok(([(header::CONTENT_TYPE, "text/html; charset=utf-8")], bytes).into_response())
        }
    }
}

/// A PWA fica servida por um só nó (S8: `localStorage` é por origem), mas os
/// dois nós de casa continuam ativo-ativo (R4) — a `WasmBackend` fala com os
/// dois por `fetch()`, de outra origem. Sem CORS, o Safari bloqueia
/// silenciosamente essas respostas: a HA degrada-se para um nó só sem erro
/// nenhum que o diga. Largo de propósito: quem autoriza um pedido é a
/// assinatura (`autenticado::verificar`), não o CORS — abrir aqui não abre
/// nada que ela já não guardasse.
fn cors() -> CorsLayer {
    CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any)
}

/// O IP a confiar para o limitador de débito — o do `nginx`, quando é mesmo
/// o `nginx` que está a ligar; o do socket, sempre que não é (Review3 F-11).
///
/// O `PeerIpKeyExtractor` de origem lê só o IP do TCP — bom enquanto o
/// cliente liga direto, mas desde o Q4 o `nginx` do Pi termina TLS em
/// `pombo-correio.duckdns.org:443` e reencaminha para `127.0.0.1:53319`: o
/// IP do TCP passou a ser sempre o do `nginx`, e todo o tráfego público
/// caía no mesmo balde de rajada. O `SmartIpKeyExtractor` de fábrica
/// resolve isso, mas confia sempre em `X-Real-IP`/`X-Forwarded-For` — e a
/// 53319 continua **também** encaminhada a direito pelo router, sem
/// `nginx` nenhum pelo meio; confiar sempre no cabeçalho deixava forjá-lo
/// ali e fugir ao limite de graça, o próprio aviso da documentação do
/// `tower_governor` para este extrator. Por isso: só se lê `X-Real-IP`
/// (o `nginx` do Pi já o define, `proxy_set_header X-Real-IP $remote_addr`
/// — nada a mudar aí) quando quem liga é mesmo o `127.0.0.1` do `nginx`
/// local; a porta direta continua a usar o IP do socket, como sempre usou.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct IpConfiavel;

impl KeyExtractor for IpConfiavel {
    type Key = IpAddr;

    fn extract<T>(&self, req: &axum::http::Request<T>) -> Result<Self::Key, GovernorError> {
        let peer = req
            .extensions()
            .get::<ConnectInfo<std::net::SocketAddr>>()
            .map(|c| c.0.ip())
            .ok_or(GovernorError::UnableToExtractKey)?;

        if peer.is_loopback()
            && let Some(ip) = req
                .headers()
                .get("x-real-ip")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.trim().parse::<IpAddr>().ok())
        {
            return Ok(ip);
        }
        Ok(peer)
    }
}

/// Limite de débito por IP — só nas rotas públicas sem autenticação nenhuma
/// (`/saude`, `/vapid`, `GET /d/{id}`): as outras já exigem uma assinatura
/// Ed25519 válida, que custa uma identidade gerada, não um pedido HTTP
/// (Review0 F-02 / Review2 F-09).
///
/// `ponytail:` 5 pedidos de rajada, 1/s a reencher — números de partida,
/// nunca medidos contra tráfego a sério. Afinar se o Q4 mostrar tráfego
/// público a sério.
fn rotas_publicas() -> Router<Arc<Estado>> {
    let limite = Arc::new(
        GovernorConfigBuilder::default()
            .key_extractor(IpConfiavel)
            .per_second(1)
            .burst_size(5)
            .finish()
            .expect("configuração do limitador de débito é estática e válida"),
    );

    Router::new()
        .route("/saude", get(saude))
        .route("/vapid", get(vapid_publica))
        .route("/d/{id}", get(pagina_do_link))
        .layer(GovernorLayer::new(limite))
}

/// As rotas, num sítio só — senão o servidor de testes e o a sério divergem, e a
/// diferença aparece em produção.
fn rotas(estado: Arc<Estado>) -> Router {
    Router::new()
        .merge(rotas_publicas())
        .route("/deposito", post(criar))
        // Os três métodos do mesmo caminho vão juntos: registá-lo duas vezes faz
        // o axum entrar em pânico ao arrancar.
        .route(
            "/deposito/{id}",
            get(estado_do_deposito).put(subir).delete(apagar),
        )
        .route("/deposito/{id}/blocos", get(descer))
        .route("/caixa", get(caixa))
        .route("/subscricao", post(subscrever))
        .route("/convite", post(convidar))
        // O link sem instalação: o que a página precisa de carregar (o `/d/{id}`
        // em si já vai em `rotas_publicas`, com o limitador).
        .route("/pagina/{*caminho}", get(recurso_da_pagina))
        .fallback(app_estatico)
        .layer(cors())
        .with_state(estado)
}

/// Monta o estado partilhado. Num sítio só, senão o servidor de testes e o a
/// sério divergem — a mesma razão de `rotas`.
fn montar(
    raiz: std::path::PathBuf,
    autorizados: Autorizados,
    pagina: Option<std::path::PathBuf>,
    app: Option<std::path::PathBuf>,
    firebase_conta_servico: Option<std::path::PathBuf>,
) -> Result<Arc<Estado>> {
    // Uma credencial mal formada não pode impedir o relay de arrancar por
    // causa de um extra — falha alto e claro no arranque (`?`), não em
    // silêncio a meio do primeiro push.
    let fcm = firebase_conta_servico.map(|p| Fcm::carregar(&p)).transpose()?;
    Ok(Arc::new(Estado {
        vapid: Vapid::carregar_ou_gerar(&raiz)?,
        fcm,
        subscricoes: Subscricoes::carregar(&raiz)?,
        cliente_push: reqwest::Client::new(),
        deposito: Deposito::novo(raiz)?,
        autorizados,
        pagina,
        app,
        a_subir: Default::default(),
    }))
}

#[allow(clippy::too_many_arguments)]
pub async fn servir(
    raiz: std::path::PathBuf,
    porta: u16,
    autorizados: Autorizados,
    pagina: Option<std::path::PathBuf>,
    app: Option<std::path::PathBuf>,
    firebase_conta_servico: Option<std::path::PathBuf>,
) -> Result<()> {
    let estado = montar(raiz, autorizados, pagina, app, firebase_conta_servico)?;

    // A limpeza corre à parte e não bloqueia nada. De hora a hora chega: a
    // validade é de dias, e o que importa é que aconteça sem ninguém se lembrar.
    let limpeza = estado.clone();
    tokio::spawn(async move {
        loop {
            if let Ok(n) = limpeza.deposito.limpar(VALIDADE).await
                && n > 0
            {
                eprintln!("limpeza: {n} depósitos fora de validade");
            }
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
        }
    });

    let escuta = tokio::net::TcpListener::bind(("0.0.0.0", porta)).await?;
    eprintln!("pombo-relay à escuta na porta {}", escuta.local_addr()?.port());
    // `with_connect_info`: o limitador de débito (Q3) precisa do IP real do
    // TCP, não só do que os cabeçalhos dizem.
    axum::serve(
        escuta,
        rotas(estado).into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;
    Ok(())
}

/// Igual ao `servir`, mas devolve a porta escolhida pelo sistema. Só para
/// testes, que não podem fixar portas sem colidirem entre si.
pub async fn servir_em_porta_livre(
    raiz: std::path::PathBuf,
    autorizados: Autorizados,
) -> Result<(std::net::SocketAddr, tokio::task::JoinHandle<()>)> {
    servir_em_porta_livre_com_pagina(raiz, autorizados, None).await
}

/// Como o anterior, mas com a página do link instalada.
pub async fn servir_em_porta_livre_com_pagina(
    raiz: std::path::PathBuf,
    autorizados: Autorizados,
    pagina: Option<std::path::PathBuf>,
) -> Result<(std::net::SocketAddr, tokio::task::JoinHandle<()>)> {
    servir_em_porta_livre_com_pagina_e_app(raiz, autorizados, pagina, None).await
}

/// Como o anterior, mas também com a PWA instalada.
pub async fn servir_em_porta_livre_com_pagina_e_app(
    raiz: std::path::PathBuf,
    autorizados: Autorizados,
    pagina: Option<std::path::PathBuf>,
    app: Option<std::path::PathBuf>,
) -> Result<(std::net::SocketAddr, tokio::task::JoinHandle<()>)> {
    let estado = montar(raiz, autorizados, pagina, app, None)?;

    let escuta = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let endereco = escuta.local_addr()?;
    let tarefa = tokio::spawn(async move {
        let _ = axum::serve(
            escuta,
            rotas(estado).into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await;
    });
    Ok((endereco, tarefa))
}

#[cfg(test)]
mod testes {
    use super::*;

    fn cabecalho(v: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::RANGE, v.parse().unwrap());
        h
    }

    #[test]
    fn ip_confiavel_só_le_o_cabecalho_de_quem_e_mesmo_o_nginx_local() {
        use std::net::{Ipv4Addr, SocketAddr};

        let pedido = |peer: SocketAddr, cabecalho: Option<&str>| {
            let mut b = axum::http::Request::builder().extension(ConnectInfo(peer));
            if let Some(v) = cabecalho {
                b = b.header("x-real-ip", v);
            }
            b.body(()).unwrap()
        };

        // O nginx local (127.0.0.1) diz quem é: confia-se no cabeçalho.
        let via_nginx =
            pedido(SocketAddr::from((Ipv4Addr::LOCALHOST, 12345)), Some("203.0.113.7"));
        assert_eq!(
            IpConfiavel.extract(&via_nginx).unwrap(),
            IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7))
        );

        // Alguém a bater direto na 53319 (não é o nginx) finge o cabeçalho —
        // ignorado, usa-se o IP real do socket. É o furo que o
        // `SmartIpKeyExtractor` de fábrica não fecha (Review3 F-11).
        let direto_a_fingir =
            pedido(SocketAddr::from((Ipv4Addr::new(198, 51, 100, 9), 4444)), Some("1.2.3.4"));
        assert_eq!(
            IpConfiavel.extract(&direto_a_fingir).unwrap(),
            IpAddr::V4(Ipv4Addr::new(198, 51, 100, 9))
        );

        // O nginx local sem o cabeçalho (não devia acontecer, mas se
        // acontecer): cai para o IP do socket — degrada para o
        // comportamento antigo, não abre uma falha nova.
        let nginx_sem_cabecalho = pedido(SocketAddr::from((Ipv4Addr::LOCALHOST, 12345)), None);
        assert_eq!(
            IpConfiavel.extract(&nginx_sem_cabecalho).unwrap(),
            IpAddr::V4(Ipv4Addr::LOCALHOST)
        );
    }

    #[test]
    fn o_caminho_da_pagina_nao_sai_da_pasta() {
        // Vem do URL e vira caminho em disco. Sem isto, o relay — que é a peça
        // exposta à internet — servia o disco todo.
        assert!(caminho_de_pagina("../../etc/passwd").is_none());
        assert!(caminho_de_pagina("wasm/../../segredo").is_none());
        assert!(caminho_de_pagina("/etc/passwd").is_none());
        assert!(caminho_de_pagina("").is_none());
        // O `%2e%2e` já chega descodificado do axum, por isso é o mesmo caso.
        assert!(caminho_de_pagina("..").is_none());

        assert_eq!(
            caminho_de_pagina("wasm/pombo_correio_pagina_bg.wasm"),
            Some(std::path::PathBuf::from("wasm/pombo_correio_pagina_bg.wasm"))
        );
        assert_eq!(
            caminho_de_pagina("pagina.js"),
            Some(std::path::PathBuf::from("pagina.js"))
        );
    }

    #[test]
    fn o_wasm_sai_com_o_tipo_que_o_browser_exige() {
        // Servido como octet-stream, o `instantiateStreaming` recusa-o e a
        // página não arranca — falha que só aparece no browser.
        use std::path::Path;
        assert_eq!(tipo_de(Path::new("x.wasm")), "application/wasm");
        assert_eq!(tipo_de(Path::new("x.js")), "text/javascript; charset=utf-8");
        assert_eq!(tipo_de(Path::new("index.html")), "text/html; charset=utf-8");
    }

    #[test]
    fn range_aberto_e_o_unico_que_se_aceita() {
        assert_eq!(desde_onde(&HeaderMap::new()).unwrap(), 0);
        assert_eq!(desde_onde(&cabecalho("bytes=1048576-")).unwrap(), 1048576);
        // Servir tudo a quem pediu uma gama fechada faria o cliente juntar bytes
        // ao que já tinha, e o ficheiro sairia maior. Mais vale recusar.
        assert!(desde_onde(&cabecalho("bytes=0-99")).is_err());
        assert!(desde_onde(&cabecalho("bytes=-500")).is_err());
        assert!(desde_onde(&cabecalho("coisas=1-")).is_err());
    }
}
