//! O relay em HTTP.
//!
//! Deliberadamente burro: aceita bytes, guarda bytes, devolve bytes. Não abre
//! nada, não verifica nada, não sabe o que leva. É o que faz de ser a única peça
//! exposta à internet — comprometê-la não expõe ficheiro nenhum.

use std::{collections::HashSet, sync::Arc};

use anyhow::Result;
use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

use crate::deposito::{Deposito, VALIDADE};

struct Estado {
    deposito: Deposito,
    /// Depósitos com uma subida a decorrer. Duas subidas ao mesmo depósito
    /// escreveriam por cima uma da outra e o ficheiro sairia com blocos de
    /// ambas — indistinguível de corrupção, e só descoberto no fim, na
    /// verificação de quem recebe.
    a_subir: tokio::sync::Mutex<HashSet<String>>,
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
async fn criar(
    State(e): State<Arc<Estado>>,
    Query(q): Query<Criar>,
    oferta: String,
) -> Result<Json<Criado>, Erro> {
    let id = e
        .deposito
        .criar(&q.para, q.tamanho, oferta)
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

/// O que está à espera de alguém.
///
/// **Sem autenticação nesta peça** — entra em R2, com assinatura Ed25519. Por
/// agora só diz identificadores, que já são imprevisíveis; mas quem souber uma
/// impressão digital fica a saber quanta coisa espera por essa pessoa, e isso
/// não fica assim.
async fn caixa(State(e): State<Arc<Estado>>, Path(impressao): Path<String>) -> Json<Vec<String>> {
    Json(e.deposito.caixa_de(&impressao).await.unwrap_or_default())
}

async fn saude() -> impl IntoResponse {
    "pombo-relay"
}

/// As rotas, num sítio só — senão o servidor de testes e o a sério divergem, e a
/// diferença aparece em produção.
fn rotas(estado: Arc<Estado>) -> Router {
    Router::new()
        .route("/saude", get(saude))
        .route("/deposito", post(criar))
        // Os três métodos do mesmo caminho vão juntos: registá-lo duas vezes faz
        // o axum entrar em pânico ao arrancar.
        .route(
            "/deposito/{id}",
            get(estado_do_deposito).put(subir).delete(apagar),
        )
        .route("/deposito/{id}/blocos", get(descer))
        .route("/caixa/{impressao}", get(caixa))
        .with_state(estado)
}

pub async fn servir(raiz: std::path::PathBuf, porta: u16) -> Result<()> {
    let estado = Arc::new(Estado {
        deposito: Deposito::novo(raiz)?,
        a_subir: Default::default(),
    });

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
    axum::serve(escuta, rotas(estado)).await?;
    Ok(())
}

/// Igual ao `servir`, mas devolve a porta escolhida pelo sistema. Só para
/// testes, que não podem fixar portas sem colidirem entre si.
pub async fn servir_em_porta_livre(
    raiz: std::path::PathBuf,
) -> Result<(std::net::SocketAddr, tokio::task::JoinHandle<()>)> {
    let estado = Arc::new(Estado {
        deposito: Deposito::novo(raiz)?,
        a_subir: Default::default(),
    });

    let escuta = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let endereco = escuta.local_addr()?;
    let tarefa = tokio::spawn(async move {
        let _ = axum::serve(escuta, rotas(estado)).await;
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
