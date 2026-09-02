//! O degrau de baixo da escada: passar pelo relay de casa.
//!
//! Serve quando o outro dispositivo não se alcança — redes diferentes, ou
//! simplesmente desligado. **O relay só vê texto cifrado**: a chave da
//! transferência vai embrulhada para a chave pública de quem recebe, e o
//! manifesto (que traz o nome do ficheiro) vai selado.
//!
//! Consequência que interessa dizer alto: **só se envia pelo relay para quem já
//! se conheceu**. O envelope precisa da chave pública do destinatário, e fora da
//! rede local não há a quem a pedir — pedi-la ao relay seria deixá-lo escolher
//! para quem os ficheiros abrem.

use anyhow::{Context, Result, bail};
use pombo_correio_core::{
    chunk::Manifest,
    crypto::{Envelope, SEAL_OVERHEAD, TransferKey, open_manifest, seal_manifest},
    identity::{Identity, PublicIdentity},
    protocolo::mensagem_assinada,
};
use tokio::io::AsyncSeekExt;

use crate::{Cancelar, Evento, Metricas, Ouvinte, conhecidos::Conhecidos, protocolo::Oferta};

/// Os relays de casa, um URL por linha em `<dados>/relays`.
///
/// Ficheiro em vez de ecrã de definições, para já: é um endereço que se escreve
/// uma vez, quando os servidores de casa forem instalados, e que ninguém da
/// família alguma vez vai mexer. **Sem ficheiro não há relay**, e a app comporta-
/// -se exatamente como antes — o degrau de baixo simplesmente não existe.
pub fn configurados(dados: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(dados.join("relays"))
        .map(|t| {
            t.lines()
                .map(|l| l.split('#').next().unwrap_or("").trim().trim_end_matches('/').to_string())
                .filter(|l| !l.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Qual dos nós de casa está de pé. É para isto que são dois.
///
/// Pela ordem do ficheiro, e não em paralelo: com dois nós a diferença é uma
/// ligação falhada de um segundo, e o código sequencial lê-se de uma vez.
/// `ponytail: sequencial; happy eyeballs se alguma vez forem muitos nós.`
///
/// O tempo de ligação é curto de propósito — um servidor desligado não recusa a
/// ligação, fica calado, e o sistema só desistiria ao fim de minutos.
pub async fn primeiro_de_pe(relays: Vec<String>) -> Option<String> {
    let cliente = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(2))
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .ok()?;

    for base in relays {
        let vivo = cliente
            .get(format!("{base}/saude"))
            .send()
            .await
            .is_ok_and(|r| r.status().is_success());
        if vivo {
            return Some(base);
        }
    }
    None
}

/// Onde o relay guarda um bloco selado.
///
/// Todos os blocos selados têm o mesmo tamanho menos o último, por isso a
/// conversão de blocos para bytes é uma multiplicação. É esta conta que permite
/// ao relay não saber o que é um bloco.
fn byte_do_bloco(m: &Manifest, bloco: u32) -> u64 {
    bloco as u64 * (m.chunk_size as u64 + SEAL_OVERHEAD as u64)
}

fn tamanho_selado(m: &Manifest) -> u64 {
    let total = m.expected_chunks();
    (0..total)
        .map(|i| m.plain_len(i).unwrap_or(0) as u64 + SEAL_OVERHEAD as u64)
        .sum()
}

fn impressao_curta(p: &PublicIdentity) -> String {
    hex::encode(p.fingerprint())[..16].to_string()
}

/// Assina um pedido ao relay.
///
/// A impressão digital sai das chaves, e a ação e os argumentos entram na
/// mensagem: uma assinatura feita para listar a caixa não serve para depositar.
fn assinado(eu: &Identity, acao: &str, extra: &str) -> reqwest::header::HeaderMap {
    let p = eu.public();
    let instante = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let msg = mensagem_assinada(acao, &impressao_curta(&p), extra, instante);

    let mut h = reqwest::header::HeaderMap::new();
    let ident = format!("{}{}", hex::encode(p.verifying), hex::encode(p.encrypting));
    // Só entram dígitos hexadecimais e decimais: nada aqui pode falhar a virar
    // cabeçalho, e um `expect` diria mais do que engolir o erro.
    h.insert("x-pombo-identidade", ident.parse().expect("hexadecimal"));
    h.insert("x-pombo-instante", instante.to_string().parse().expect("dígitos"));
    h.insert(
        "x-pombo-assinatura",
        hex::encode(eu.sign(msg.as_bytes())).parse().expect("hexadecimal"),
    );
    h
}

/// Regista o token FCM deste dispositivo no relay, para acordar por push
/// quando o processo estiver mesmo morto (Q5 — o resto do R6).
///
/// Melhor esforço, como o resto do que fala com o relay: um relay em baixo
/// não impede a app de continuar a funcionar, só fica sem saber para onde
/// mandar o próximo aviso.
pub async fn inscrever_fcm(eu: &Identity, base: &str, token: &str) -> Result<()> {
    let cliente = reqwest::Client::new();
    let resposta = cliente
        .post(format!("{base}/subscricao"))
        .headers(assinado(eu, "subscricao", token))
        .json(&serde_json::json!({ "tipo": "fcm", "token": token }))
        .send()
        .await
        .context("não consegui falar com o relay")?;
    if !resposta.status().is_success() {
        bail!("o relay recusou a subscrição FCM: {}", resposta.text().await.unwrap_or_default());
    }
    Ok(())
}

#[derive(serde::Deserialize)]
struct Criado {
    id: String,
}

#[derive(serde::Deserialize)]
struct EstadoDeposito {
    oferta: String,
    bytes: u64,
    #[allow(dead_code)]
    tamanho: u64,
    completo: bool,
}

/// Quantas vezes se tenta a subida antes de desistir.
///
/// Pela internet as ligações caem, e é justamente para isso que existe a retoma:
/// desistir à primeira tornava-a decorativa. Três chegam — se as três falham, o
/// problema não é uma falha passageira.
const TENTATIVAS: u32 = 3;

/// A palavra que ocupa o lugar da impressão digital num depósito por link.
///
/// A caixa compara por igualdade exata contra impressões de 16 dígitos
/// hexadecimais, e esta palavra não é uma — por isso um depósito por link nunca
/// aparece na caixa de ninguém, sem o relay precisar de saber que existem links.
const CAIXA_LIGACAO: &str = "ligacao";

/// Deixa um ficheiro no relay, à espera de quem o vai buscar.
#[allow(clippy::too_many_arguments)]
pub async fn depositar(
    eu: &Identity,
    fonte: &dyn crate::Fonte,
    para: &PublicIdentity,
    base: &str,
    ouvinte: Ouvinte,
    cancelar: Cancelar,
    metricas: &Metricas,
) -> Result<()> {
    depositar_em(
        eu,
        fonte,
        para,
        &impressao_curta(para),
        TransferKey::generate(),
        base,
        ouvinte,
        cancelar,
        metricas,
    )
    .await
    .map(|_| ())
}

/// Deixa um ficheiro no relay para **quem tiver o link**, e devolve-o.
///
/// O envelope é selado para uma identidade gerada e deitada fora aqui mesmo: fica
/// válido e ninguém no mundo o abre, nem quem enviou. Quem abre o ficheiro é
/// quem tiver a chave, e ela vai no fragmento do URL — a parte que o browser
/// nunca envia ao servidor.
///
/// Devolve `None` se não chegou a ficar lá (falhou ou foi cancelado); nesse caso
/// o ouvinte já disse porquê.
pub async fn depositar_ligacao(
    eu: &Identity,
    fonte: &dyn crate::Fonte,
    base: &str,
    ouvinte: Ouvinte,
    cancelar: Cancelar,
    metricas: &Metricas,
) -> Result<Option<String>> {
    let chave = TransferKey::generate();
    let hex_chave = hex::encode(chave.to_bytes());
    let ninguem = Identity::generate("ligação").public();

    let id_relay = depositar_em(
        eu,
        fonte,
        &ninguem,
        CAIXA_LIGACAO,
        chave,
        base,
        ouvinte,
        cancelar,
        metricas,
    )
    .await?;

    Ok(id_relay.map(|id| format!("{base}/d/{id}#{hex_chave}")))
}

/// O corpo partilhado pelos dois: a única diferença é para quem se sela e em que
/// caixa fica.
#[allow(clippy::too_many_arguments)]
async fn depositar_em(
    eu: &Identity,
    fonte: &dyn crate::Fonte,
    para: &PublicIdentity,
    caixa: &str,
    chave: TransferKey,
    base: &str,
    ouvinte: Ouvinte,
    cancelar: Cancelar,
    metricas: &Metricas,
) -> Result<Option<String>> {
    let nome = fonte.nome();
    let manifesto = Manifest::from_reader(&nome, fonte.abrir().context("não consegui abrir o ficheiro a enviar")?)?;
    let total = manifesto.expected_chunks();
    let selado = tamanho_selado(&manifesto);

    let manifesto_selado = seal_manifest(&chave, &serde_json::to_vec(&manifesto)?);
    let oferta = Oferta {
        de: eu.public(),
        envelope: Envelope::seal(&chave, para),
        assinatura: eu.sign(&manifesto_selado),
        manifesto_selado,
        // Não há ligação de volta: quem recebe pode nem estar ligado. Cancelar
        // aqui é apagar o depósito, não avisar ninguém.
        porta_resposta: 0,
    };

    let cliente = reqwest::Client::new();
    let resposta = cliente
        .post(format!("{base}/deposito?para={caixa}&tamanho={selado}"))
        .headers(assinado(eu, "deposito", &format!("{caixa}|{selado}")))
        .body(serde_json::to_string(&oferta)?)
        .send()
        .await
        .context("não consegui falar com o relay")?;

    if !resposta.status().is_success() {
        bail!("o relay recusou: {}", resposta.text().await.unwrap_or_default());
    }
    let id_relay = resposta.json::<Criado>().await?.id;

    // O identificador que a interface vê é o de sempre — a raiz de Merkle — para
    // um envio pelo relay e a receção correspondente serem a mesma linha.
    let id = hex::encode(manifesto.root);
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
    metricas.registar(
        "deposito_inicio",
        serde_json::json!({ "id": id, "ficheiro": nome, "tamanho": manifesto.size, "relay": base }),
    );

    let relaxed = std::sync::atomic::Ordering::Relaxed;
    let mut ultimo_erro = String::new();

    for tentativa in 0..TENTATIVAS {
        if cancelar.load(relaxed) {
            break;
        }

        // De onde retomar. Arredonda-se **para baixo**, até à fronteira do
        // bloco: o que o relay tenha a mais é meio bloco selado, que ninguém
        // consegue abrir, e o depósito trunca-o ao aceitar o `?de=`.
        let ja = if tentativa == 0 {
            0
        } else {
            let e: EstadoDeposito = cliente
                .get(format!("{base}/deposito/{id_relay}"))
                .send()
                .await?
                .json()
                .await?;
            (e.bytes / (manifesto.chunk_size as u64 + SEAL_OVERHEAD as u64)) as u32
        };

        let mut origem = tokio::fs::File::from_std(fonte.abrir()?);
        origem
            .seek(std::io::SeekFrom::Start(ja as u64 * manifesto.chunk_size as u64))
            .await?;

        let fluxo = crate::enviar::fluxo_selado(
            origem,
            manifesto.clone(),
            chave.clone(),
            ja,
            id.clone(),
            ouvinte.clone(),
            cancelar.clone(),
        );

        let r = cliente
            .put(format!(
                "{base}/deposito/{id_relay}?de={}",
                byte_do_bloco(&manifesto, ja)
            ))
            .body(reqwest::Body::wrap_stream(fluxo))
            .send()
            .await;

        match r {
            Ok(r) if r.status().is_success() && !cancelar.load(relaxed) => {
                metricas.registar(
                    "deposito_fim",
                    serde_json::json!({ "id": id, "bytes": manifesto.size, "relay": base }),
                );
                ouvinte(Evento::Depositado {
                    id,
                    // Num link não há a quem chegar — o destinatário é quem vier
                    // a ter o endereço, e ainda ninguém o tem.
                    para: (caixa != CAIXA_LIGACAO).then(|| para.label.clone()),
                });
                return Ok(Some(id_relay));
            }
            Ok(r) if !r.status().is_success() => {
                ultimo_erro = r.text().await.unwrap_or_else(|_| "o relay recusou".into());
            }
            Ok(_) => break, // cancelado a meio
            Err(e) => ultimo_erro = e.to_string(),
        }
    }

    // Cancelar um depósito é apagá-lo: não há ninguém do outro lado a quem
    // avisar, e deixá-lo lá seria entregar o ficheiro depois de a pessoa ter
    // dito que não.
    if cancelar.load(relaxed) {
        let _ = cliente
            .delete(format!("{base}/deposito/{id_relay}"))
            .send()
            .await;
        metricas.registar("deposito_cancelado", serde_json::json!({ "id": id }));
        ouvinte(Evento::Cancelado { id });
        return Ok(None);
    }

    metricas.registar(
        "deposito_falhou",
        serde_json::json!({ "id": id, "erro": ultimo_erro }),
    );
    ouvinte(Evento::Falhou {
        id,
        erro: format!("não consegui deixar o ficheiro no relay: {ultimo_erro}"),
    });
    Ok(None)
}

/// Vai buscar ao relay tudo o que estiver à espera. Devolve quantos chegaram.
///
/// Um depósito que ainda esteja a subir fica para a próxima: sem estar completo,
/// os últimos blocos não estariam lá e a verificação parava a meio.
#[allow(clippy::too_many_arguments)]
pub async fn buscar(
    eu: &Identity,
    base: &str,
    destino: &std::path::Path,
    conhecidos: &std::path::Path,
    ouvinte: Ouvinte,
    cancelaveis: crate::Cancelaveis,
    metricas: &Metricas,
) -> Result<usize> {
    tokio::fs::create_dir_all(destino).await?;
    let cliente = reqwest::Client::new();

    let ids: Vec<String> = cliente
        .get(format!("{base}/caixa"))
        .headers(assinado(eu, "caixa", ""))
        .send()
        .await
        .context("não consegui falar com o relay")?
        .json()
        .await
        .context("o relay respondeu coisa que não percebi")?;

    let mut chegaram = 0;
    for id_relay in ids {
        match trazer(eu, base, &cliente, &id_relay, destino, conhecidos, &ouvinte, &cancelaveis, metricas).await {
            Ok(true) => chegaram += 1,
            Ok(false) => {}
            Err(e) => {
                // Um depósito estragado não pode impedir os outros de chegar.
                metricas.registar(
                    "relay_deposito_falhou",
                    serde_json::json!({ "id": id_relay, "erro": e.to_string() }),
                );
            }
        }
    }
    Ok(chegaram)
}

#[allow(clippy::too_many_arguments)]
async fn trazer(
    eu: &Identity,
    base: &str,
    cliente: &reqwest::Client,
    id_relay: &str,
    destino: &std::path::Path,
    conhecidos: &std::path::Path,
    ouvinte: &Ouvinte,
    cancelaveis: &crate::Cancelaveis,
    metricas: &Metricas,
) -> Result<bool> {
    let e: EstadoDeposito = cliente
        .get(format!("{base}/deposito/{id_relay}"))
        .send()
        .await?
        .json()
        .await?;

    if !e.completo {
        return Ok(false);
    }

    let oferta: Oferta = serde_json::from_str(&e.oferta).context("oferta ilegível")?;

    // Autenticar antes de abrir seja o que for. Mesma ordem do caminho direto, e
    // pela mesma razão: sem isto estaríamos a decifrar material de qualquer um —
    // e aqui esse "qualquer um" chega pela internet.
    oferta
        .de
        .verify(&oferta.manifesto_selado, &oferta.assinatura)
        .map_err(|_| anyhow::anyhow!("assinatura inválida"))?;

    let chave = oferta
        .envelope
        .open(eu)
        .map_err(|_| anyhow::anyhow!("o envelope não é para mim"))?;

    let bruto = open_manifest(&chave, &oferta.manifesto_selado).map_err(|_| anyhow::anyhow!("manifesto não abre"))?;
    let manifesto: Manifest = serde_json::from_slice(&bruto)?;
    manifesto.validate()?;

    // O mesmo identificador do caminho direto — a raiz de Merkle. É o que faz um
    // ficheiro começado na LAN e acabado pelo relay reencontrar o seu parcial em
    // vez de recomeçar do zero.
    let id = hex::encode(manifesto.root);
    let parcial = destino.join(format!("{id}.parcial"));
    let ja = tokio::fs::metadata(&parcial)
        .await
        .map(|m| (m.len() / manifesto.chunk_size as u64) as u32)
        .unwrap_or(0);
    let total = manifesto.expected_chunks();

    let mut registo = Conhecidos::carregar(conhecidos.to_path_buf());
    let anterior = registo.nome_anterior(&oferta.de).map(str::to_string);
    let novo = anterior.is_none();
    let _ = registo.registar(&oferta.de);

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

    let cancelar: Cancelar = Default::default();
    cancelaveis.lock().unwrap().insert(id.clone(), cancelar.clone());

    let resposta = cliente
        .get(format!("{base}/deposito/{id_relay}/blocos"))
        .header("Range", format!("bytes={}-", byte_do_bloco(&manifesto, ja)))
        .send()
        .await?;

    let escoado = crate::receber::escoar(
        resposta.bytes_stream(),
        &manifesto,
        &chave,
        &parcial,
        ja,
        &id,
        ouvinte,
        &cancelar,
        metricas,
    )
    .await;

    cancelaveis.lock().unwrap().remove(&id);
    let (feitos, fim) = escoado?;

    if feitos < total {
        // O parcial fica em disco e o depósito também: a próxima tentativa
        // continua de onde ia. Só um cancelamento explícito apaga o depósito.
        if fim == crate::receber::Fim::Cancelado {
            let _ = cliente.delete(format!("{base}/deposito/{id_relay}")).send().await;
            ouvinte(Evento::Cancelado { id });
        } else {
            ouvinte(Evento::Falhou {
                id,
                erro: "a ligação ao relay caiu — continua da próxima vez".into(),
            });
        }
        return Ok(false);
    }

    let final_ = destino.join(crate::receber::nome_seguro(&manifesto.name));
    tokio::fs::rename(&parcial, &final_).await?;

    // Só se apaga depois de o ficheiro estar em disco com o nome final. Apagar
    // antes trocaria uma falha recuperável (voltar a descarregar) por perda.
    let _ = cliente.delete(format!("{base}/deposito/{id_relay}")).send().await;

    ouvinte(Evento::Recebido { id, caminho: final_ });
    Ok(true)
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn sem_ficheiro_de_relays_nao_ha_relay() {
        // A app tem de continuar a funcionar exatamente como antes em casa de
        // quem nunca instalar servidor nenhum.
        let d = std::env::temp_dir().join(format!("pombo-rl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        assert!(configurados(&d).is_empty());

        std::fs::write(
            d.join("relays"),
            "https://casa.exemplo/  # o A10\n\n# o Pi ainda não\nhttps://pi.exemplo\n",
        )
        .unwrap();
        assert_eq!(
            configurados(&d),
            ["https://casa.exemplo", "https://pi.exemplo"]
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_conta_de_blocos_para_bytes_bate_com_o_que_o_relay_guarda() {
        // É esta conta que deixa o relay não saber o que é um bloco. Se
        // divergisse, uma descida retomada começava a meio de um bloco e nada
        // abriria — falha que só aparece depois de uma queda de rede.
        let m = Manifest {
            name: "x".into(),
            size: 3 * 1024 * 1024 + 7,
            chunk_size: 1024 * 1024,
            root: [0; 32],
            chunks: vec![pombo_correio_core::chunk::ChunkHash([0u8; 32]); 4],
        };

        assert_eq!(byte_do_bloco(&m, 0), 0);
        assert_eq!(byte_do_bloco(&m, 2), 2 * (1024 * 1024 + SEAL_OVERHEAD as u64));

        // O total é a soma dos blocos, com o último mais curto.
        assert_eq!(
            tamanho_selado(&m),
            3 * (1024 * 1024 + SEAL_OVERHEAD as u64) + 7 + SEAL_OVERHEAD as u64
        );
    }
}
