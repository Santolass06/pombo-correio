//! Lado de quem recebe: servidor HTTP.

use std::{collections::HashMap, sync::Arc};

use anyhow::{Context, Result, bail};
use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post, put},
};
use futures_util::StreamExt;
use pombo_correio_core::{
    chunk::Manifest,
    crypto::{SEAL_OVERHEAD, TransferKey, open_chunk, open_manifest},
    identity::Identity,
};
use tokio::io::AsyncWriteExt;

use crate::protocolo::{Aceite, Oferta};

struct EmCurso {
    manifesto: Manifest,
    chave: TransferKey,
}

struct Estado {
    eu: Identity,
    destino: std::path::PathBuf,
    em_curso: tokio::sync::Mutex<HashMap<String, EmCurso>>,
}

/// Reduz o nome recebido ao seu último componente.
///
/// O nome vem de outra máquina e nunca pode ser usado como caminho: `..` ou uma
/// barra à frente escreveriam fora da pasta de destino. Ficar só com o nome do
/// ficheiro fecha isso.
fn nome_seguro(bruto: &str) -> String {
    std::path::Path::new(bruto)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty() && n != "." && n != "..")
        .unwrap_or_else(|| "recebido".into())
}

fn parcial(estado: &Estado, id: &str) -> std::path::PathBuf {
    estado.destino.join(format!("{id}.parcial"))
}

/// Quantos blocos completos já estão em disco.
async fn tenho(estado: &Estado, id: &str, chunk_size: u32) -> u32 {
    match tokio::fs::metadata(parcial(estado, id)).await {
        Ok(m) => (m.len() / chunk_size as u64) as u32,
        Err(_) => 0,
    }
}

async fn identidade(State(e): State<Arc<Estado>>) -> Json<pombo_correio_core::identity::PublicIdentity> {
    Json(e.eu.public())
}

async fn propor(
    State(estado): State<Arc<Estado>>,
    Json(oferta): Json<Oferta>,
) -> Result<Json<Aceite>, (StatusCode, String)> {
    let erro = |c: StatusCode, m: &str| (c, m.to_string());

    // Autenticar antes de abrir seja o que for: a assinatura prova que o
    // manifesto vem mesmo de quem diz vir.
    oferta
        .de
        .verify(&oferta.manifesto_selado, &oferta.assinatura)
        .map_err(|_| erro(StatusCode::FORBIDDEN, "assinatura inválida"))?;

    let chave = oferta
        .envelope
        .open(&estado.eu)
        .map_err(|_| erro(StatusCode::FORBIDDEN, "o envelope não é para mim"))?;

    let bruto = open_manifest(&chave, &oferta.manifesto_selado)
        .map_err(|_| erro(StatusCode::BAD_REQUEST, "manifesto não abre"))?;
    let manifesto: Manifest = serde_json::from_slice(&bruto)
        .map_err(|_| erro(StatusCode::BAD_REQUEST, "manifesto ilegível"))?;

    // Validar antes de alocar o que quer que seja com base nele.
    manifesto
        .validate()
        .map_err(|e| erro(StatusCode::BAD_REQUEST, &e.to_string()))?;

    let id = hex::encode(manifesto.root);
    let ja = tenho(&estado, &id, manifesto.chunk_size).await;

    println!(
        "\n  Oferta de {} — {} ({:.1} MB)",
        oferta.de.label,
        manifesto.name,
        manifesto.size as f64 / 1e6
    );
    println!("  Número de segurança: {}", oferta.de.safety_number());
    if ja > 0 {
        println!("  A retomar: já tenho {ja} de {} blocos", manifesto.expected_chunks());
    }

    estado
        .em_curso
        .lock()
        .await
        .insert(id.clone(), EmCurso { manifesto, chave });

    Ok(Json(Aceite { id, tenho: ja }))
}

#[derive(serde::Deserialize)]
struct De {
    de: u32,
}

async fn blocos(
    State(estado): State<Arc<Estado>>,
    Path(id): Path<String>,
    Query(De { de }): Query<De>,
    corpo: Body,
) -> Result<String, (StatusCode, String)> {
    receber_blocos(estado, id, de, corpo)
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))
}

async fn receber_blocos(estado: Arc<Estado>, id: String, de: u32, corpo: Body) -> Result<String> {
    let guarda = estado.em_curso.lock().await;
    let curso = guarda.get(&id).context("transferência desconhecida")?;
    let manifesto = curso.manifesto.clone();
    let chave = curso.chave.clone();
    drop(guarda);

    let ja = tenho(&estado, &id, manifesto.chunk_size).await;
    if de != ja {
        bail!("pediu para começar no bloco {de} mas tenho {ja}");
    }

    let caminho = parcial(&estado, &id);
    let mut ficheiro = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&caminho)
        .await?;

    let total = manifesto.expected_chunks();
    let mut indice = de;
    let mut fluxo = corpo.into_data_stream();
    let mut sobra: Vec<u8> = Vec::new();

    while indice < total {
        let esperado = manifesto.plain_len(indice).context("índice fora do manifesto")? + SEAL_OVERHEAD;

        // Encher até ter um bloco selado inteiro. O fluxo chega em pedaços de
        // tamanho arbitrário, que não coincidem com as fronteiras dos blocos.
        while sobra.len() < esperado {
            match fluxo.next().await {
                Some(pedaco) => sobra.extend_from_slice(&pedaco?),
                None => bail!("fluxo acabou no bloco {indice} de {total}"),
            }
        }

        let selado: Vec<u8> = sobra.drain(..esperado).collect();
        let claro = open_chunk(&chave, indice, &selado).context("bloco não abre")?;
        manifesto.verify_chunk(indice, &claro)?;

        ficheiro.write_all(&claro).await?;
        indice += 1;
    }

    ficheiro.flush().await?;
    drop(ficheiro);

    let final_ = estado.destino.join(nome_seguro(&manifesto.name));
    tokio::fs::rename(&caminho, &final_).await?;
    estado.em_curso.lock().await.remove(&id);

    println!("  Recebido: {}", final_.display());
    Ok("ok".into())
}

pub async fn servir(eu: Identity, destino: std::path::PathBuf, porta: u16) -> Result<()> {
    tokio::fs::create_dir_all(&destino).await?;

    let estado = Arc::new(Estado {
        eu,
        destino: destino.clone(),
        em_curso: Default::default(),
    });

    let app = Router::new()
        .route("/identidade", get(identidade))
        .route("/transferencia", post(propor))
        .route("/transferencia/{id}/blocos", put(blocos))
        .with_state(estado.clone());

    let escuta = tokio::net::TcpListener::bind(("0.0.0.0", porta)).await?;

    println!("Pombo-Correio — à espera");
    println!("  Este dispositivo: {}", estado.eu.label);
    println!("  Número de segurança: {}", estado.eu.public().safety_number());
    println!("  A escutar na porta {}", escuta.local_addr()?.port());
    println!("  A guardar em {}", destino.display());

    axum::serve(escuta, app).await?;
    Ok(())
}
