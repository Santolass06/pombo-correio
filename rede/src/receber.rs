//! Lado de quem recebe: servidor HTTP.

use std::{collections::HashMap, sync::Arc};

use anyhow::{Context, Result, bail};
use axum::{
    Json, Router,
    body::Body,
    extract::{ConnectInfo, Path, Query, State},
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

use crate::{
    Cancelaveis, Evento, Metricas, Ouvinte,
    conhecidos::Conhecidos,
    metricas::{Fases, cronometrar},
    protocolo::{Aceite, Oferta},
};

struct EmCurso {
    manifesto: Manifest,
    chave: TransferKey,
    /// Porta do servidor de quem envia, anunciada na oferta. Zero = sem
    /// servidor, não avisar de cancelamentos.
    porta_resposta: u16,
    /// Sinal de cancelamento desta transferência. Criado **aqui**, quando a
    /// oferta é aceite — não quando o PUT começa. Senão há uma janela entre os
    /// dois em que um aviso /cancelar chega e não encontra token, e o
    /// cancelamento perde-se (era o bug do emissor cancelar cedo).
    cancelar: crate::Cancelar,
}

struct Estado {
    eu: Identity,
    destino: std::path::PathBuf,
    em_curso: tokio::sync::Mutex<HashMap<String, EmCurso>>,
    conhecidos: tokio::sync::Mutex<Conhecidos>,
    cancelaveis: Cancelaveis,
    metricas: Metricas,
    ouvinte: Ouvinte,
}

/// Reduz o nome recebido ao seu último componente.
///
/// O nome vem de outra máquina e nunca pode ser usado como caminho: `..` ou uma
/// barra à frente escreveriam fora da pasta de destino. Ficar só com o nome do
/// ficheiro fecha isso, nos dois sistemas — o `Path` do Windows trata `/` e `\`
/// como separadores, o do Unix só `/`, e nenhum dos dois deixa passar um `..`
/// inteiro.
pub(crate) fn nome_seguro(bruto: &str) -> String {
    let base = std::path::Path::new(bruto)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty() && n != "." && n != "..")
        .unwrap_or_else(|| "recebido".into());

    // Só no Windows. Em Unix estes caracteres são legítimos e trocá-los seria
    // estragar nomes perfeitamente válidos.
    #[cfg(windows)]
    let base = aceitavel_no_windows(&base);

    base
}

/// Torna um nome aceitável para o sistema de ficheiros do Windows.
///
/// Um nome vindo de Linux ou Android traz com frequência `:` — carimbos
/// temporais como `2026-06-22 23:04:58.mkv` são o caso comum — e o Windows
/// recusa-o ao gravar. Sem isto, a transferência corria até ao fim, verificava
/// todos os blocos, e só falhava no `rename` final.
///
/// Compilado em todas as plataformas para poder ser testado em todas; só é
/// aplicado no Windows. Sem isto, esta lógica só teria cobertura de testes na
/// máquina onde é mais incómodo corrê-los.
#[cfg_attr(not(windows), allow(dead_code))]
fn aceitavel_no_windows(nome: &str) -> String {
    const PROIBIDOS: &[char] = &['<', '>', ':', '"', '/', '\\', '|', '?', '*'];

    let limpo: String = nome
        .chars()
        .map(|c| {
            // Os de controlo também são recusados, e passam despercebidos.
            if PROIBIDOS.contains(&c) || (c as u32) < 0x20 {
                '_'
            } else {
                c
            }
        })
        .collect();

    // Pontos e espaços no fim são silenciosamente removidos pelo Windows, o que
    // faria o ficheiro aparecer com outro nome do que aquele que se confirmou.
    let limpo = limpo.trim_end_matches(['.', ' ']);

    // Nomes de dispositivo herdados do MS-DOS. Continuam reservados, com ou sem
    // extensão: `CON.txt` é tão inutilizável como `CON`.
    const RESERVADOS: &[&str] = &[
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];

    let raiz = limpo.split('.').next().unwrap_or("");
    if RESERVADOS.iter().any(|r| raiz.eq_ignore_ascii_case(r)) {
        return format!("_{limpo}");
    }

    if limpo.is_empty() {
        return "recebido".into();
    }
    limpo.to_string()
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

    // O número de segurança só interessa quando há algo a verificar; assinalar
    // um dispositivo já conhecido a cada ficheiro ensinaria a ignorá-lo, que é
    // a pior coisa que se pode fazer a uma verificação.
    let mut registo = estado.conhecidos.lock().await;
    let anterior = registo.nome_anterior(&oferta.de).map(str::to_string);
    let novo_dispositivo = anterior.is_none();
    let _ = registo.registar(&oferta.de);
    drop(registo);

    (estado.ouvinte)(Evento::Oferta {
        id: id.clone(),
        de: oferta.de.clone(),
        ficheiro: manifesto.name.clone(),
        tamanho: manifesto.size,
        novo: novo_dispositivo,
        nome_anterior: anterior.filter(|a| *a != oferta.de.label),
        retoma: ja,
        total: manifesto.expected_chunks(),
    });

    let cancelar: crate::Cancelar = Default::default();
    estado.cancelaveis.lock().unwrap().insert(id.clone(), cancelar.clone());
    estado.em_curso.lock().await.insert(
        id.clone(),
        EmCurso { manifesto, chave, porta_resposta: oferta.porta_resposta, cancelar },
    );

    Ok(Json(Aceite { id, tenho: ja }))
}

#[derive(serde::Deserialize)]
struct De {
    de: u32,
}

/// Marca o erro "cancelado por quem recebe" para o distinguir de uma falha
/// verdadeira. Vai como estado HTTP próprio, para que quem envia saiba que foi
/// um cancelamento intencional e não um problema.
#[derive(Debug)]
struct CanceladoPeloRecetor;
impl std::fmt::Display for CanceladoPeloRecetor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "cancelado por quem recebe")
    }
}
impl std::error::Error for CanceladoPeloRecetor {}

/// Um dispositivo avisa que cancelou uma transferência. Aciona o sinal local
/// correspondente — seja de um envio ou de uma receção — e o resto trata-se
/// sozinho no ciclo respetivo.
async fn cancelar_endpoint(
    State(estado): State<Arc<Estado>>,
    Path(id): Path<String>,
) -> StatusCode {
    if let Some(sinal) = estado.cancelaveis.lock().unwrap().get(&id) {
        sinal.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    StatusCode::OK
}

async fn blocos(
    State(estado): State<Arc<Estado>>,
    Path(id): Path<String>,
    Query(De { de }): Query<De>,
    ConnectInfo(quem_envia): ConnectInfo<std::net::SocketAddr>,
    corpo: Body,
) -> Result<String, (StatusCode, String)> {
    receber_blocos(estado, id, de, quem_envia.ip(), corpo).await.map_err(|e| {
        // 409 quando foi o próprio recetor a cancelar; qualquer outra coisa é
        // um pedido malformado ou uma falha.
        if e.downcast_ref::<CanceladoPeloRecetor>().is_some() {
            (StatusCode::CONFLICT, "cancelado".into())
        } else {
            (StatusCode::BAD_REQUEST, e.to_string())
        }
    })
}

async fn receber_blocos(
    estado: Arc<Estado>,
    id: String,
    de: u32,
    ip_emissor: std::net::IpAddr,
    corpo: Body,
) -> Result<String> {
    let guarda = estado.em_curso.lock().await;
    let curso = guarda.get(&id).context("transferência desconhecida")?;
    let manifesto = curso.manifesto.clone();
    let chave = curso.chave.clone();
    let porta_resposta = curso.porta_resposta;
    let cancelar = curso.cancelar.clone();
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
    let mut movidos: u64 = 0;
    // Mesma cadência do lado que envia, e pela mesma razão: um evento por bloco
    // inundaria o IPC e o débito mostrado ficaria para trás do real.
    let mut ultimo_evento = std::time::Instant::now();
    let mut bytes_no_ultimo_evento: u64 = 0;

    // Diagnóstico: tempo por fase, e uma amostra por segundo com a repartição
    // acumulada. É isto que diz onde a velocidade se perde.
    let comeco = std::time::Instant::now();
    let mut fases = Fases::default();
    let mut ultima_amostra = std::time::Instant::now();
    estado.metricas.registar(
        "rececao_inicio",
        serde_json::json!({ "id": id, "ficheiro": manifesto.name, "tamanho": manifesto.size, "de_bloco": de, "total_blocos": total }),
    );

    let cancelado = std::sync::atomic::Ordering::Relaxed;
    'blocos: while indice < total {
        // Cancelado localmente (a interface carregou em cancelar) ou por aviso
        // de quem envia (o endpoint /cancelar acionou o mesmo sinal).
        if cancelar.load(cancelado) {
            break 'blocos;
        }

        let esperado = manifesto.plain_len(indice).context("índice fora do manifesto")? + SEAL_OVERHEAD;

        // Encher até ter um bloco selado inteiro. O fluxo chega em pedaços de
        // tamanho arbitrário, que não coincidem com as fronteiras dos blocos.
        // O tempo aqui é "rede", mas inclui esperar que o outro lado envie — se
        // domina, o gargalo pode ser a ligação ou o emissor, não este disco.
        let t_rede = std::time::Instant::now();
        while sobra.len() < esperado {
            match fluxo.next().await {
                Some(pedaco) => sobra.extend_from_slice(&pedaco?),
                None => {
                    // O fluxo acabou antes de tempo. Pode ser um cancelamento de
                    // quem envia cujo aviso ainda vem a caminho — dá-se uma folga
                    // a ver se o sinal chega. 2 s cobre um aviso atrasado por
                    // carga; num cancelamento real chega em milissegundos.
                    for _ in 0..20 {
                        if cancelar.load(cancelado) {
                            break;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }
                    if cancelar.load(cancelado) {
                        break 'blocos;
                    }
                    // Não foi cancelamento: a ligação caiu. O parcial fica em
                    // disco, mas tem de se emitir um evento terminal — senão a
                    // interface fica presa em "a passar" para sempre.
                    estado.cancelaveis.lock().unwrap().remove(&id);
                    estado.em_curso.lock().await.remove(&id);
                    (estado.ouvinte)(Evento::Falhou {
                        id: id.clone(),
                        erro: "a ligação a quem enviava caiu — podes pedir de novo".into(),
                    });
                    bail!("fluxo acabou no bloco {indice} de {total}");
                }
            }
        }
        fases.rede += t_rede.elapsed();

        let selado: Vec<u8> = sobra.drain(..esperado).collect();
        let claro = cronometrar(&mut fases.decifra, || open_chunk(&chave, indice, &selado))
            .context("bloco não abre")?;
        cronometrar(&mut fases.verifica, || manifesto.verify_chunk(indice, &claro))?;

        let t_disco = std::time::Instant::now();
        ficheiro.write_all(&claro).await?;
        fases.disco += t_disco.elapsed();

        indice += 1;
        movidos += claro.len() as u64;

        if indice >= total || ultimo_evento.elapsed().as_millis() >= 100 {
            let dt = ultimo_evento.elapsed().as_secs_f64();
            let bytes_por_seg = if dt > 0.0 { (movidos - bytes_no_ultimo_evento) as f64 / dt } else { 0.0 };
            ultimo_evento = std::time::Instant::now();
            bytes_no_ultimo_evento = movidos;
            (estado.ouvinte)(Evento::Progresso {
                id: id.clone(),
                feitos: indice,
                total,
                bytes: movidos,
                bytes_por_seg,
            });
        }

        if ultima_amostra.elapsed().as_secs_f64() >= 1.0 {
            ultima_amostra = std::time::Instant::now();
            let s = comeco.elapsed().as_secs_f64();
            let mut amostra = fases.json();
            amostra["blocos"] = indice.into();
            amostra["bytes"] = movidos.into();
            amostra["mb_por_s"] = (movidos as f64 / 1e6 / s.max(1e-9)).into();
            estado.metricas.registar("rececao_amostra", amostra);
        }
    }

    ficheiro.flush().await?;
    drop(ficheiro);
    estado.cancelaveis.lock().unwrap().remove(&id);
    estado.em_curso.lock().await.remove(&id);

    // Saiu do ciclo sem chegar ao fim: foi cancelamento. O parcial fica em
    // disco (retomar continua a valer). Avisa-se quem envia por uma ligação
    // nova, e devolve-se 409 — que ele reconhece — para o caso de a resposta
    // ainda lhe chegar apesar de estar a meio do upload.
    if indice < total {
        estado.metricas.registar("rececao_cancelada", serde_json::json!({ "id": id, "blocos": indice }));
        // Avisa quem envia, se ele tiver servidor (porta anunciada na oferta).
        if porta_resposta != 0 {
            crate::avisar_cancelamento(&format!("http://{ip_emissor}:{porta_resposta}"), &id).await;
        }
        (estado.ouvinte)(Evento::Cancelado { id });
        return Err(CanceladoPeloRecetor.into());
    }

    let final_ = estado.destino.join(nome_seguro(&manifesto.name));
    tokio::fs::rename(&caminho, &final_).await?;

    let s = comeco.elapsed().as_secs_f64();
    let mut resumo = fases.json();
    resumo["id"] = id.clone().into();
    resumo["bytes"] = movidos.into();
    resumo["segundos"] = s.into();
    resumo["mb_por_s"] = (movidos as f64 / 1e6 / s.max(1e-9)).into();
    estado.metricas.registar("rececao_fim", resumo);

    (estado.ouvinte)(Evento::Recebido { id, caminho: final_ });
    Ok("ok".into())
}

pub async fn servir(
    eu: Identity,
    destino: std::path::PathBuf,
    conhecidos: std::path::PathBuf,
    porta: u16,
    ouvinte: Ouvinte,
    cancelaveis: Cancelaveis,
    metricas: Metricas,
) -> Result<()> {
    tokio::fs::create_dir_all(&destino).await?;

    let estado = Arc::new(Estado {
        eu,
        destino: destino.clone(),
        em_curso: Default::default(),
        conhecidos: Conhecidos::carregar(conhecidos).into(),
        cancelaveis,
        metricas,
        ouvinte: ouvinte.clone(),
    });

    let app = Router::new()
        .route("/identidade", get(identidade))
        .route("/transferencia", post(propor))
        .route("/transferencia/{id}/blocos", put(blocos))
        .route("/cancelar/{id}", post(cancelar_endpoint))
        .with_state(estado.clone());

    let escuta = tokio::net::TcpListener::bind(("0.0.0.0", porta)).await?;
    let porta_real = escuta.local_addr()?.port();

    // Falhar a anunciar não é motivo para não receber: numa rede que bloqueie
    // multicast, escrever o endereço à mão continua a funcionar.
    let anuncio = crate::descoberta::anunciar(&estado.eu.public(), porta_real).ok();

    ouvinte(Evento::AEscutar {
        porta: porta_real,
        anunciado: anuncio.is_some(),
    });
    let _anuncio = anuncio;

    // `into_make_service_with_connect_info` para o `blocos` saber o endereço de
    // quem envia, e assim o poder avisar num cancelamento.
    axum::serve(
        escuta,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nome_fica_reduzido_ao_ultimo_componente() {
        assert_eq!(nome_seguro("ferias.mp4"), "ferias.mp4");
        assert_eq!(nome_seguro("com espaços.mkv"), "com espaços.mkv");
    }

    #[test]
    fn travessia_de_caminho_e_neutralizada() {
        // O nome vem de outra máquina; sem isto escrevia fora da pasta.
        assert_eq!(nome_seguro("/etc/passwd"), "passwd");
        assert_eq!(nome_seguro("../../../etc/passwd"), "passwd");
        assert_eq!(nome_seguro(".."), "recebido");
        assert_eq!(nome_seguro("/"), "recebido");
        assert_eq!(nome_seguro(""), "recebido");
    }

    #[test]
    fn carimbo_temporal_de_linux_passa_a_ser_gravavel_no_windows() {
        // O caso real: nomes assim saem de gravadores de ecrã em Linux e
        // Android, e o Windows recusa-os ao gravar.
        assert_eq!(
            aceitavel_no_windows("2026-06-22 23:04:58.mkv"),
            "2026-06-22 23_04_58.mkv"
        );
    }

    #[test]
    fn caracteres_proibidos_no_windows_sao_trocados() {
        assert_eq!(aceitavel_no_windows(r#"a<b>c:d"e|f?g*h"#), "a_b_c_d_e_f_g_h");
        assert_eq!(aceitavel_no_windows("com\u{1}controlo"), "com_controlo");
    }

    #[test]
    fn ponto_ou_espaco_no_fim_e_removido() {
        // O Windows removia-os em silêncio, e o ficheiro ficava com um nome
        // diferente daquele que se confirmou na oferta.
        assert_eq!(aceitavel_no_windows("nota."), "nota");
        assert_eq!(aceitavel_no_windows("nota "), "nota");
    }

    #[test]
    fn nomes_de_dispositivo_do_ms_dos_sao_desviados() {
        // Continuam reservados, com ou sem extensão.
        assert_eq!(aceitavel_no_windows("CON"), "_CON");
        assert_eq!(aceitavel_no_windows("con.txt"), "_con.txt");
        assert_eq!(aceitavel_no_windows("COM1.log"), "_COM1.log");
        assert_eq!(aceitavel_no_windows("console.txt"), "console.txt");
    }

    #[test]
    fn nome_que_desaparece_todo_tem_recurso() {
        assert_eq!(aceitavel_no_windows("..."), "recebido");
    }
}
