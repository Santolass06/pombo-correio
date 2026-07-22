//! Lado de quem envia.

use anyhow::{Context, Result, bail};
use pombo_correio_core::{
    chunk::Manifest,
    crypto::{Envelope, TransferKey, seal_chunk, seal_manifest},
    identity::{Identity, PublicIdentity},
};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use crate::protocolo::{Aceite, Oferta};

/// Fixa o tipo do fluxo produzido pela macro.
///
/// `try_stream!` não permite anotar os tipos e o `wrap_stream` é genérico
/// demais para os inferir. Uma identidade tipada resolve-o sem custo.
fn tipar<S: futures_util::Stream<Item = std::io::Result<bytes::Bytes>>>(s: S) -> S {
    s
}

/// Lê um bloco inteiro, insistindo até o encher.
///
/// O mesmo cuidado do manifesto: `read` pode devolver menos do que foi pedido
/// sem estar no fim. Sem isto, os blocos sairiam com tamanhos que não batem
/// certo com o manifesto e nada abriria do outro lado.
async fn ler_bloco<R: AsyncReadExt + Unpin>(r: &mut R, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut cheio = 0;
    while cheio < buf.len() {
        match r.read(&mut buf[cheio..]).await? {
            0 => break,
            n => cheio += n,
        }
    }
    Ok(cheio)
}

pub async fn enviar(eu: &Identity, ficheiro: &std::path::Path, destino: &str) -> Result<()> {
    let base = if destino.starts_with("http") {
        destino.trim_end_matches('/').to_string()
    } else {
        format!("http://{destino}")
    };

    let nome = ficheiro
        .file_name()
        .context("caminho sem nome de ficheiro")?
        .to_string_lossy()
        .into_owned();

    println!("A preparar {nome}...");
    let aberto = std::fs::File::open(ficheiro)
        .with_context(|| format!("não consegui abrir {}", ficheiro.display()))?;
    let manifesto = Manifest::from_reader(&nome, aberto)?;

    let cliente = reqwest::Client::new();

    // Quem recebe publica a sua chave pública; é para ela que a chave da
    // transferência vai embrulhada.
    let deles: PublicIdentity = cliente
        .get(format!("{base}/identidade"))
        .send()
        .await
        .context("não consegui falar com o destino")?
        .json()
        .await?;

    println!("Destino: {} — {}", deles.label, deles.safety_number());

    let chave = TransferKey::generate();
    let manifesto_selado = seal_manifest(&chave, &serde_json::to_vec(&manifesto)?);

    let oferta = Oferta {
        de: eu.public(),
        envelope: Envelope::seal(&chave, &deles),
        assinatura: eu.sign(&manifesto_selado),
        manifesto_selado,
    };

    let resposta = cliente
        .post(format!("{base}/transferencia"))
        .json(&oferta)
        .send()
        .await?;

    if !resposta.status().is_success() {
        bail!("recusado: {}", resposta.text().await.unwrap_or_default());
    }
    let aceite: Aceite = resposta.json().await?;

    let total = manifesto.expected_chunks();
    if aceite.tenho >= total {
        println!("Já lá está inteiro.");
        return Ok(());
    }
    if aceite.tenho > 0 {
        println!("A retomar do bloco {} de {total}", aceite.tenho);
    }

    // Um pedido só, com os blocos a sair em fluxo. Um pedido por bloco daria
    // um ida-e-volta por MiB e limitaria o débito à latência da ligação.
    let mut origem = tokio::fs::File::open(ficheiro).await?;
    origem
        .seek(std::io::SeekFrom::Start(
            aceite.tenho as u64 * manifesto.chunk_size as u64,
        ))
        .await?;

    let m = manifesto.clone();
    let fluxo = tipar(async_stream::try_stream! {
        let mut buf = vec![0u8; m.chunk_size as usize];
        for indice in aceite.tenho..total {
            let esperado = m.plain_len(indice).expect("índice dentro do total");
            let lido = ler_bloco(&mut origem, &mut buf[..esperado]).await?;
            if lido != esperado {
                Err(std::io::Error::other(format!(
                    "o ficheiro mudou de tamanho a meio: esperava {esperado} bytes no bloco {indice}, li {lido}"
                )))?;
            }
            yield bytes::Bytes::from(seal_chunk(&chave, indice, &buf[..esperado]));
        }
    });

    let resposta = cliente
        .put(format!(
            "{base}/transferencia/{}/blocos?de={}",
            aceite.id, aceite.tenho
        ))
        .body(reqwest::Body::wrap_stream(fluxo))
        .send()
        .await?;

    if !resposta.status().is_success() {
        bail!("falhou: {}", resposta.text().await.unwrap_or_default());
    }

    println!("Enviado.");
    Ok(())
}
