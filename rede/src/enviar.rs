//! Lado de quem envia.

use anyhow::{Context, Result, bail};
use pombo_correio_core::{
    chunk::Manifest,
    crypto::{Envelope, TransferKey, seal_chunk, seal_manifest},
    identity::{Identity, PublicIdentity},
};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use std::sync::atomic::Ordering::Relaxed;

use crate::{
    Cancelar, Evento, Ouvinte,
    protocolo::{Aceite, Oferta},
};

/// Traduz um erro de rede numa frase que alguém sem conhecimentos técnicos
/// entenda. O erro cru do `reqwest` — "error sending request for url ..." — não
/// diz nada a quem só quer saber se o ficheiro chegou.
fn amigavel(e: &reqwest::Error) -> String {
    if e.is_connect() {
        "não consegui ligar-me ao destino — está aberto e na mesma rede?".into()
    } else if e.is_timeout() {
        "o destino demorou demasiado a responder".into()
    } else {
        // A meio de um upload, a ligação a cair pode ser rede ou o outro lado a
        // cancelar — não há como distinguir por HTTP. A mensagem é neutra e
        // convida a retomar, que é seguro: o parcial ficou do outro lado.
        "a ligação ao destino terminou — podes tentar enviar de novo".into()
    }
}

/// Não se conseguiu falar com o destino (antes de qualquer oferta). É o único
/// erro que justifica tentar o relay: nada chegou a ser proposto, por isso nada
/// fica a meio nem duplicado.
#[derive(Debug)]
pub struct DestinoInacessivel(pub String);

impl std::fmt::Display for DestinoInacessivel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "não consegui falar com o destino")
    }
}
impl std::error::Error for DestinoInacessivel {}

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

/// Envia um ficheiro identificado por um caminho no disco (desktop, CLI).
///
/// Fino invólucro sobre [`enviar_fonte`]: só embrulha o caminho numa
/// [`crate::FonteCaminho`]. Existe para os chamadores de sempre não mudarem.
pub async fn enviar(
    eu: &Identity,
    ficheiro: &std::path::Path,
    destino: std::net::SocketAddr,
    ouvinte: Ouvinte,
    cancelar: Cancelar,
    metricas: &crate::Metricas,
    porta_resposta: u16,
) -> Result<()> {
    let fonte = crate::FonteCaminho(ficheiro.to_owned());
    enviar_fonte(eu, &fonte, destino, ouvinte, cancelar, metricas, porta_resposta).await
}

/// Envia o que quer que uma [`crate::Fonte`] descreva — no Android, um URI
/// `content://`; no desktop, um caminho.
pub async fn enviar_fonte(
    eu: &Identity,
    fonte: &dyn crate::Fonte,
    destino: std::net::SocketAddr,
    ouvinte: Ouvinte,
    cancelar: Cancelar,
    metricas: &crate::Metricas,
    porta_resposta: u16,
) -> Result<()> {
    // O `Display` de um endereço IPv6 já traz os parênteses retos que o URL
    // exige, por isso não é preciso tratá-lo à parte.
    let base = format!("http://{destino}");

    let nome = fonte.nome();

    let aberto = fonte.abrir().context("não consegui abrir o ficheiro a enviar")?;
    let manifesto = Manifest::from_reader(&nome, aberto)?;

    let cliente = reqwest::Client::new();

    // Quem recebe publica a sua chave pública; é para ela que a chave da
    // transferência vai embrulhada.
    let deles: PublicIdentity = cliente
        .get(format!("{base}/identidade"))
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| DestinoInacessivel(e.to_string()))?
        .json()
        .await
        .map_err(|e| DestinoInacessivel(e.to_string()))?;

    ouvinte(Evento::Destino { quem: deles.clone() });

    let chave = TransferKey::generate();
    let manifesto_selado = seal_manifest(&chave, &serde_json::to_vec(&manifesto)?);

    let oferta = Oferta {
        de: eu.public(),
        envelope: Envelope::seal(&chave, &deles),
        assinatura: eu.sign(&manifesto_selado),
        manifesto_selado,
        porta_resposta,
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
    ouvinte(Evento::Oferta {
        id: aceite.id.clone(),
        de: eu.public(),
        ficheiro: nome.clone(),
        tamanho: manifesto.size,
        novo: false,
        nome_anterior: None,
        retoma: aceite.tenho,
        total,
    });

    if aceite.tenho >= total {
        ouvinte(Evento::Enviado { id: aceite.id.clone() });
        return Ok(());
    }

    metricas.registar(
        "envio_inicio",
        serde_json::json!({ "id": aceite.id, "ficheiro": nome, "tamanho": manifesto.size, "de_bloco": aceite.tenho, "total_blocos": total, "destino": destino.to_string() }),
    );
    let comeco = std::time::Instant::now();

    // Um pedido só, com os blocos a sair em fluxo. Um pedido por bloco daria
    // um ida-e-volta por MiB e limitaria o débito à latência da ligação.
    // Abre-se a fonte de novo (handle fresco) e envolve-se no I/O assíncrono.
    let mut origem = tokio::fs::File::from_std(fonte.abrir()?);
    origem
        .seek(std::io::SeekFrom::Start(
            aceite.tenho as u64 * manifesto.chunk_size as u64,
        ))
        .await?;

    // Vigia à parte: espera que o cancelamento seja acionado e avisa quem recebe
    // por uma ligação nova, no instante em que acontece — em paralelo com o
    // envio, sem depender do timing do fluxo. Enquanto quem recebe ainda processa
    // os blocos em curso, o aviso chega e ele para pela verificação no topo do
    // ciclo. Abortada assim que o envio termina.
    let vigia = {
        let cancelar = cancelar.clone();
        let base = base.clone();
        let id = aceite.id.clone();
        tokio::spawn(async move {
            loop {
                if cancelar.load(Relaxed) {
                    crate::avisar_cancelamento(&base, &id).await;
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
    };

    let m = manifesto.clone();
    let ouvinte_fluxo = ouvinte.clone();
    let id_fluxo = aceite.id.clone();
    let cancelar_fluxo = cancelar.clone();
    let fluxo = tipar(async_stream::try_stream! {
        let mut buf = vec![0u8; m.chunk_size as usize];
        let mut movidos: u64 = 0;
        // Um evento por bloco inundaria o IPC do Tauri a mais de cem por segundo,
        // e os eventos acumulavam-se ao ponto de o débito mostrado ficar preso
        // muito abaixo do real. Uma cadência de 100 ms chega ao olho e mantém o
        // canal livre; o último bloco desenha-se sempre, para a barra fechar.
        let mut ultimo_evento = std::time::Instant::now();
        let mut bytes_no_ultimo_evento: u64 = 0;

        for indice in aceite.tenho..total {
            // Parar antes de ler o bloco seguinte: o que já saiu fica em disco do
            // outro lado, por isso cancelar não deita fora o trabalho feito.
            if cancelar_fluxo.load(Relaxed) {
                // Para de enviar. O aviso a quem recebe é dado por uma tarefa
                // vigia à parte (ver abaixo), para não depender do timing deste
                // fluxo.
                break;
            }

            let esperado = m.plain_len(indice).expect("índice dentro do total");
            let lido = ler_bloco(&mut origem, &mut buf[..esperado]).await?;
            if lido != esperado {
                Err(std::io::Error::other(format!(
                    "o ficheiro mudou de tamanho a meio: esperava {esperado} bytes no bloco {indice}, li {lido}"
                )))?;
            }
            yield bytes::Bytes::from(seal_chunk(&chave, indice, &buf[..esperado]));

            // Depois do `yield`: o fluxo é preguiçoso e só chega aqui quando o
            // bloco anterior saiu mesmo pela ligação. Contar antes mostraria o
            // ritmo a que se lê do disco, não o que está a ir para a rede.
            movidos += esperado as u64;
            let feitos = indice + 1;
            if feitos >= total || ultimo_evento.elapsed().as_millis() >= 100 {
                let dt = ultimo_evento.elapsed().as_secs_f64();
                let bytes_por_seg = if dt > 0.0 { (movidos - bytes_no_ultimo_evento) as f64 / dt } else { 0.0 };
                ultimo_evento = std::time::Instant::now();
                bytes_no_ultimo_evento = movidos;
                ouvinte_fluxo(Evento::Progresso { id: id_fluxo.clone(), feitos, total, bytes: movidos, bytes_por_seg });
            }
        }
    });

    let envio = cliente
        .put(format!(
            "{base}/transferencia/{}/blocos?de={}",
            aceite.id, aceite.tenho
        ))
        .body(reqwest::Body::wrap_stream(fluxo))
        .send()
        .await;

    // O envio acabou (de que maneira for): a vigia já não é precisa. Se cancelou,
    // ela já disparou o aviso; se não, estava só à espera.
    vigia.abort();

    // O cancelamento vem primeiro: interromper o fluxo deixa o servidor a
    // receber menos do que esperava, o que ele reporta como erro — mas isso é a
    // consequência do cancelamento, não uma falha a mostrar como tal.
    if cancelar.load(Relaxed) {
        metricas.registar("envio_cancelado", serde_json::json!({ "id": aceite.id }));
        // O aviso a quem recebe foi dado pela vigia, no instante do cancelamento.
        ouvinte(Evento::Cancelado { id: aceite.id });
        return Ok(());
    }

    let s = comeco.elapsed().as_secs_f64();
    let enviados = manifesto.size - aceite.tenho as u64 * manifesto.chunk_size as u64;

    // A partir daqui os erros já não sobem: há um cartão de transferência que
    // precisa de ser fechado, e deixar o erro propagar deixava-o preso para
    // sempre. Reporta-se pelo evento e devolve-se Ok.
    match envio {
        Err(e) => {
            metricas.registar("envio_falhou", serde_json::json!({ "id": aceite.id, "erro": e.to_string() }));
            ouvinte(Evento::Falhou { id: aceite.id, erro: amigavel(&e) })
        }
        // 409: foi o outro lado a cancelar. É um cancelamento, não uma falha —
        // os dois lados mostram o mesmo estado.
        Ok(r) if r.status() == reqwest::StatusCode::CONFLICT => {
            metricas.registar("envio_cancelado_pelo_destino", serde_json::json!({ "id": aceite.id }));
            ouvinte(Evento::Cancelado { id: aceite.id })
        }
        Ok(r) if !r.status().is_success() => ouvinte(Evento::Falhou {
            id: aceite.id,
            erro: r.text().await.unwrap_or_else(|_| "o destino recusou".into()),
        }),
        Ok(_) => {
            metricas.registar(
                "envio_fim",
                serde_json::json!({ "id": aceite.id, "bytes": enviados, "segundos": s, "mb_por_s": enviados as f64 / 1e6 / s.max(1e-9) }),
            );
            ouvinte(Evento::Enviado { id: aceite.id })
        }
    }
    Ok(())
}
