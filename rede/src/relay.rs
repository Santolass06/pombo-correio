//! Cliente do relay: a escada a seguir ao envio direto.
//!
//! O relay guarda **bytes opacos** (ver `relay/` e PLANO.md, «Desenho R1–R3»).
//! Aqui vive tudo o que é nosso: quem envia sela os blocos e deposita; quem
//! recebe vai à caixa, descarrega com `Range`, e **verifica cada bloco contra o
//! manifesto** — exatamente como na LAN. A garantia byte-exata não muda de dono.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::atomic::Ordering::Relaxed,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use pombo_correio_core::{
    chunk::Manifest,
    crypto::{Envelope, SEAL_OVERHEAD, TransferKey, open_chunk, open_manifest, seal_chunk, seal_manifest},
    identity::{Identity, PublicIdentity},
    relay::{CABECALHO_ASSINATURA, CABECALHO_INSTANTE, CABECALHO_QUEM, assinar_pedido},
};
use serde::Deserialize;
use tokio::io::AsyncReadExt;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

use crate::{Cancelar, Evento, Metricas, Ouvinte, conhecidos::Conhecidos, enviar::DestinoInacessivel, protocolo::Oferta};

/// Um relay, por endereço. Não guarda identidade: cada pedido é assinado por
/// quem o faz.
#[derive(Clone)]
pub struct Relay {
    base: String,
    http: reqwest::Client,
}

/// Um depósito à espera na caixa de quem recebe.
#[derive(Debug, Clone, Deserialize)]
pub struct ItemCaixa {
    pub id: String,
    pub tamanho: u64,
    pub oferta: Oferta,
}

fn agora() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Relay {
    /// `endereco` é `host:porta` ou um URL completo.
    pub fn novo(endereco: &str) -> Self {
        let base = if endereco.contains("://") {
            endereco.trim_end_matches('/').to_string()
        } else {
            format!("http://{endereco}")
        };
        Self { base, http: reqwest::Client::new() }
    }

    fn pedido(&self, eu: &Identity, metodo: &str, caminho: &str, corpo: &[u8]) -> reqwest::RequestBuilder {
        let (quem, inst, sig) = assinar_pedido(eu, metodo, caminho, agora(), corpo);
        self.http
            .request(metodo.parse().expect("método válido"), format!("{}{caminho}", self.base))
            .header(CABECALHO_QUEM, quem)
            .header(CABECALHO_INSTANTE, inst)
            .header(CABECALHO_ASSINATURA, sig)
    }

    /// Regista este dispositivo no relay (idempotente). `token` só é preciso se
    /// o relay o exigir.
    pub async fn registar(&self, eu: &Identity, token: Option<&str>) -> Result<()> {
        let corpo = serde_json::to_vec(&eu.public())?;
        let mut p = self.pedido(eu, "POST", "/registo", &corpo).body(corpo);
        if let Some(t) = token {
            p = p.header("x-pombo-token", t);
        }
        let r = p.send().await.context("não consegui falar com o relay")?;
        if !r.status().is_success() {
            bail!("o relay recusou o registo: {}", r.text().await.unwrap_or_default());
        }
        Ok(())
    }

    pub async fn dispositivos(&self, eu: &Identity) -> Result<Vec<PublicIdentity>> {
        let r = self.pedido(eu, "GET", "/dispositivos", b"").send().await?;
        if !r.status().is_success() {
            bail!("o relay recusou: {}", r.text().await.unwrap_or_default());
        }
        Ok(r.json().await?)
    }

    pub async fn caixa(&self, eu: &Identity) -> Result<Vec<ItemCaixa>> {
        let r = self.pedido(eu, "GET", "/caixa", b"").send().await?;
        if !r.status().is_success() {
            bail!("o relay recusou: {}", r.text().await.unwrap_or_default());
        }
        Ok(r.json().await?)
    }

    async fn apagar(&self, eu: &Identity, id: &str) {
        let _ = self.pedido(eu, "DELETE", &format!("/deposito/{id}"), b"").send().await;
    }
}

/// Tamanho do que vai para o relay: cada bloco leva `SEAL_OVERHEAD` a mais.
fn tamanho_selado(m: &Manifest) -> u64 {
    m.size + m.expected_chunks() as u64 * SEAL_OVERHEAD as u64
}

fn tipar<S: futures_util::Stream<Item = std::io::Result<bytes::Bytes>>>(s: S) -> S {
    s
}

/// Envia pelo direto e, **só se o destino não for alcançável**, deixa no relay.
///
/// Não se desvia uma transferência que já ia a meio: `DestinoInacessivel` só
/// sai antes de qualquer oferta, por isso nada fica a meio nem duplicado. Sem
/// relay configurado, é exatamente o `enviar_fonte` de sempre.
#[allow(clippy::too_many_arguments)]
pub async fn enviar_com_alternativa(
    eu: &Identity,
    fonte: &dyn crate::Fonte,
    destino: std::net::SocketAddr,
    impressao_do_destino: &str,
    relay: Option<&Relay>,
    ouvinte: Ouvinte,
    cancelar: Cancelar,
    metricas: &Metricas,
    porta_resposta: u16,
) -> Result<()> {
    match crate::enviar::enviar_fonte(eu, fonte, destino, ouvinte.clone(), cancelar.clone(), metricas, porta_resposta).await {
        Err(e) if relay.is_some() && e.downcast_ref::<DestinoInacessivel>().is_some() => {
            metricas.registar("relay_desvio", serde_json::json!({ "destino": destino.to_string() }));
            enviar_por_relay(eu, relay.unwrap(), fonte, impressao_do_destino, ouvinte, cancelar, metricas).await
        }
        outro => outro,
    }
}

/// Deposita um ficheiro, selado, para um dispositivo que o relay conhece.
pub async fn enviar_por_relay(
    eu: &Identity,
    relay: &Relay,
    fonte: &dyn crate::Fonte,
    impressao_do_destino: &str,
    ouvinte: Ouvinte,
    cancelar: Cancelar,
    metricas: &Metricas,
) -> Result<()> {
    let nome = fonte.nome();
    let manifesto = Manifest::from_reader(&nome, fonte.abrir().context("não consegui abrir o ficheiro a enviar")?)?;
    if manifesto.size == 0 {
        bail!("um ficheiro vazio não vai pelo relay");
    }

    let destino = relay
        .dispositivos(eu)
        .await?
        .into_iter()
        .find(|p| hex::encode(p.fingerprint()).starts_with(impressao_do_destino))
        .context("o relay não conhece esse dispositivo — ele ainda não se registou lá")?;
    ouvinte(Evento::Destino { quem: destino.clone() });

    let chave = TransferKey::generate();
    let manifesto_selado = seal_manifest(&chave, &serde_json::to_vec(&manifesto)?);
    let oferta = Oferta {
        de: eu.public(),
        envelope: Envelope::seal(&chave, &destino),
        assinatura: eu.sign(&manifesto_selado),
        manifesto_selado,
        porta_resposta: 0,
    };

    let total = manifesto.expected_chunks();
    let corpo = serde_json::to_vec(&serde_json::json!({
        "para": hex::encode(destino.fingerprint()),
        "tamanho": tamanho_selado(&manifesto),
        "oferta": oferta,
    }))?;
    let r = relay.pedido(eu, "POST", "/deposito", &corpo).body(corpo).send().await.context("não consegui falar com o relay")?;
    if !r.status().is_success() {
        bail!("o relay recusou o depósito: {}", r.text().await.unwrap_or_default());
    }
    let id = r.json::<serde_json::Value>().await?["id"].as_str().context("resposta do relay sem id")?.to_string();

    let m = manifesto.clone();
    let mut origem = tokio::fs::File::from_std(fonte.abrir()?);
    let (ouv, id_f, canc) = (ouvinte.clone(), id.clone(), cancelar.clone());
    let fluxo = tipar(async_stream::try_stream! {
        let mut buf = vec![0u8; m.chunk_size as usize];
        let (mut movidos, mut ultimo) = (0u64, std::time::Instant::now());
        for indice in 0..total {
            if canc.load(Relaxed) { break; }
            let esperado = m.plain_len(indice).expect("índice dentro do total");
            let mut cheio = 0;
            while cheio < esperado {
                match origem.read(&mut buf[cheio..esperado]).await? {
                    0 => break,
                    n => cheio += n,
                }
            }
            if cheio != esperado {
                Err(std::io::Error::other(format!("o ficheiro mudou de tamanho a meio (bloco {indice})")))?;
            }
            yield bytes::Bytes::from(seal_chunk(&chave, indice, &buf[..esperado]));
            movidos += esperado as u64;
            if indice + 1 >= total || ultimo.elapsed().as_millis() >= 100 {
                let dt = ultimo.elapsed().as_secs_f64().max(1e-9);
                ultimo = std::time::Instant::now();
                ouv(Evento::Progresso { id: id_f.clone(), feitos: indice + 1, total, bytes: movidos, bytes_por_seg: esperado as f64 / dt });
            }
        }
    });

    ouvinte(Evento::Oferta {
        id: id.clone(),
        de: eu.public(),
        ficheiro: nome.clone(),
        tamanho: manifesto.size,
        novo: false,
        nome_anterior: None,
        retoma: 0,
        total,
    });
    let resposta = relay
        .pedido(eu, "PUT", &format!("/deposito/{id}/blocos?de=0"), b"")
        .body(reqwest::Body::wrap_stream(fluxo))
        .send()
        .await;

    if cancelar.load(Relaxed) {
        relay.apagar(eu, &id).await;
        ouvinte(Evento::Cancelado { id });
        return Ok(());
    }
    match resposta {
        Ok(r) if r.status().is_success() => {
            metricas.registar("relay_deposito_fim", serde_json::json!({ "id": id, "bytes": manifesto.size }));
            ouvinte(Evento::NoRelay { id, ficheiro: nome });
        }
        Ok(r) => {
            relay.apagar(eu, &id).await;
            ouvinte(Evento::Falhou { id, erro: format!("o relay recusou: {}", r.text().await.unwrap_or_default()) });
        }
        Err(_) => {
            // Nada de retoma entre sessões: a chave da transferência não é
            // guardada, por isso recomeçar é um depósito novo.
            relay.apagar(eu, &id).await;
            ouvinte(Evento::Falhou { id, erro: "a ligação ao relay caiu — podes tentar enviar de novo".into() });
        }
    }
    Ok(())
}

/// Descarrega, verifica e grava um depósito da caixa; depois apaga-o do relay.
///
/// Devolve o caminho final, ou `None` se foi cancelado. O parcial é o mesmo
/// `<raiz>.parcial` da receção direta, por isso uma receção começada por um
/// caminho retoma-se pelo outro.
#[allow(clippy::too_many_arguments)]
pub async fn receber_da_caixa(
    eu: &Identity,
    relay: &Relay,
    item: &ItemCaixa,
    destino: &Path,
    ficheiro_conhecidos: PathBuf,
    ouvinte: Ouvinte,
    cancelar: Cancelar,
    metricas: &Metricas,
) -> Result<Option<PathBuf>> {
    let oferta = &item.oferta;
    oferta.de.verify(&oferta.manifesto_selado, &oferta.assinatura).map_err(|_| anyhow::anyhow!("assinatura inválida"))?;
    let chave = oferta.envelope.open(eu).map_err(|_| anyhow::anyhow!("o envelope não é para mim"))?;
    let manifesto: Manifest = serde_json::from_slice(
        &open_manifest(&chave, &oferta.manifesto_selado).map_err(|_| anyhow::anyhow!("manifesto não abre"))?,
    )?;
    manifesto.validate().map_err(|e| anyhow::anyhow!(e.to_string()))?;
    if item.tamanho != tamanho_selado(&manifesto) {
        bail!("o tamanho do depósito não bate com o manifesto");
    }

    std::fs::create_dir_all(destino)?;
    let id = hex::encode(manifesto.root);
    let parcial = destino.join(format!("{id}.parcial"));
    let total = manifesto.expected_chunks();
    let cs = manifesto.chunk_size as u64;
    let ja = tokio::fs::metadata(&parcial).await.map(|m| (m.len() / cs) as u32).unwrap_or(0).min(total);

    let mut conhecidos = Conhecidos::carregar(ficheiro_conhecidos);
    let anterior = conhecidos.nome_anterior(&oferta.de).map(str::to_string);
    let novo = anterior.is_none();
    let _ = conhecidos.registar(&oferta.de);
    ouvinte(Evento::Oferta {
        id: id.clone(),
        de: oferta.de.clone(),
        ficheiro: manifesto.name.clone(),
        tamanho: manifesto.size,
        novo,
        nome_anterior: anterior.filter(|a| *a != oferta.de.label),
        retoma: ja,
        total,
    });

    // Corta qualquer cauda rasgada: só contam blocos inteiros já verificados.
    let mut f = tokio::fs::OpenOptions::new().create(true).write(true).truncate(false).open(&parcial).await?;
    f.set_len(ja as u64 * cs).await?;
    f.seek(std::io::SeekFrom::End(0)).await?;

    let mut indice = ja;
    if indice < total {
        // O relay não sabe o que é um bloco: o bloco k começa no byte
        // k * (chunk_size + SEAL_OVERHEAD).
        let desde = indice as u64 * (cs + SEAL_OVERHEAD as u64);
        let mut p = relay.pedido(eu, "GET", &format!("/deposito/{}/blocos", item.id), b"");
        if desde > 0 {
            p = p.header("range", format!("bytes={desde}-"));
        }
        let r = p.send().await.context("não consegui falar com o relay")?;
        if !r.status().is_success() {
            bail!("o relay recusou a descida: {}", r.text().await.unwrap_or_default());
        }
        let mut fluxo = r.bytes_stream();
        let mut sobra: Vec<u8> = Vec::new();
        let (mut movidos, mut ultimo) = (0u64, std::time::Instant::now());
        let comeco = std::time::Instant::now();

        while indice < total {
            if cancelar.load(Relaxed) {
                ouvinte(Evento::Cancelado { id });
                return Ok(None);
            }
            let esperado = manifesto.plain_len(indice).context("índice fora do manifesto")? + SEAL_OVERHEAD;
            while sobra.len() < esperado {
                match fluxo.next().await {
                    Some(Ok(pedaco)) => sobra.extend_from_slice(&pedaco),
                    _ => {
                        let erro = "a ligação ao relay caiu — fica retomável".to_string();
                        ouvinte(Evento::Falhou { id: id.clone(), erro: erro.clone() });
                        bail!(erro);
                    }
                }
            }
            let selado: Vec<u8> = sobra.drain(..esperado).collect();
            let claro = open_chunk(&chave, indice, &selado).context("bloco não abre")?;
            manifesto.verify_chunk(indice, &claro)?;
            f.write_all(&claro).await?;
            indice += 1;
            movidos += claro.len() as u64;
            if indice >= total || ultimo.elapsed().as_millis() >= 100 {
                let dt = ultimo.elapsed().as_secs_f64().max(1e-9);
                ultimo = std::time::Instant::now();
                ouvinte(Evento::Progresso { id: id.clone(), feitos: indice, total, bytes: movidos, bytes_por_seg: claro.len() as f64 / dt });
            }
        }
        metricas.registar(
            "relay_rececao_fim",
            serde_json::json!({ "id": id, "bytes": movidos, "segundos": comeco.elapsed().as_secs_f64() }),
        );
    }
    f.flush().await?;
    drop(f);

    let final_ = destino.join(crate::receber::nome_seguro(&manifesto.name));
    tokio::fs::rename(&parcial, &final_).await?;
    ouvinte(Evento::Recebido { id, caminho: final_.clone() });
    // Só agora, com tudo verificado e gravado. Se falhar, o TTL apanha-o.
    relay.apagar(eu, &item.id).await;
    Ok(Some(final_))
}

/// Vai à caixa de tempos a tempos e recebe o que lá estiver, até `parar`.
///
/// Um relay em baixo é normal (casa sem corrente, sem rede): erros de ligação
/// não se reportam, tenta-se na volta seguinte. Um depósito que falha três
/// vezes por razão que não é a ligação (assinatura, manifesto) deixa de ser
/// tentado, para não inundar a interface.
#[allow(clippy::too_many_arguments)]
pub async fn vigiar_caixa(
    eu: &Identity,
    relay: &Relay,
    destino: &Path,
    ficheiro_conhecidos: PathBuf,
    ouvinte: Ouvinte,
    metricas: &Metricas,
    intervalo: Duration,
    parar: Cancelar,
) {
    let mut falhas: HashMap<String, u32> = HashMap::new();
    while !parar.load(Relaxed) {
        if let Ok(itens) = relay.caixa(eu).await {
            for item in itens {
                if falhas.get(&item.id).copied().unwrap_or(0) >= 3 {
                    continue;
                }
                let r = receber_da_caixa(
                    eu, relay, &item, destino, ficheiro_conhecidos.clone(), ouvinte.clone(), parar.clone(), metricas,
                )
                .await;
                if r.is_err() {
                    *falhas.entry(item.id.clone()).or_default() += 1;
                }
            }
        }
        tokio::time::sleep(intervalo).await;
    }
}
