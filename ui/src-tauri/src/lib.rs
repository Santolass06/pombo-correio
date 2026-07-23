//! Ponte entre a interface e a camada de rede.
//!
//! **Os bytes dos ficheiros nunca passam por aqui.** O IPC do Tauri serializa
//! tudo para JSON e colapsa com cargas grandes; um ficheiro de 4 GB a atravessar
//! esta fronteira seria 4 GB de texto. O Rust é dono do ficheiro do princípio ao
//! fim, e para a WebView só vão acontecimentos: nomes, tamanhos, contagens.
//!
//! Esta regra é arquitetural, não uma otimização.

use std::sync::Arc;

use pombo_correio_core::identity::Identity;
use pombo_correio_rede::{Cancelar, Cancelaveis, Evento, Metricas, descoberta, enviar, receber};
use serde::Serialize;
use tauri::{Emitter, Manager};

/// Nome do acontecimento na WebView. Um só canal: a interface junta envios e
/// receções no mesmo painel.
const CANAL: &str = "pombo";

const PORTA: u16 = 53318;

struct Estado {
    eu: Identity,
    /// Sinais de cancelamento de tudo o que está a decorrer, por identificador
    /// de transferência — envios e receções. Partilhado com o servidor de
    /// receção, para que o mesmo botão cancele os dois casos.
    cancelaveis: Cancelaveis,
    /// Registo de métricas, partilhado com o servidor e com cada envio.
    metricas: Metricas,
}

/// Como a interface vê este dispositivo.
#[derive(Serialize)]
struct Eu {
    nome: String,
    numero_seguranca: String,
}

/// Como a interface vê um dispositivo encontrado na rede.
#[derive(Serialize)]
struct Vizinho {
    nome: String,
    endereco: String,
    impressao: Option<String>,
}

/// Cópia da identidade, para levar a uma tarefa em segundo plano.
///
/// A `Identity` guarda material secreto e não é clonável de propósito — copiá-la
/// à toa é como se espalham chaves por sítios onde não se lembra delas. Aqui é
/// deliberado e localizado.
fn copiar(eu: &Identity) -> Identity {
    let (s, e) = eu.secret_bytes();
    Identity::from_secret_bytes(s, e, eu.label.clone())
}

#[tauri::command]
fn identidade(estado: tauri::State<'_, Estado>) -> Eu {
    Eu {
        nome: estado.eu.label.clone(),
        numero_seguranca: estado.eu.public().safety_number(),
    }
}

/// Procura na rede local. Demora alguns segundos, por isso sai da thread da
/// interface — senão a janela congelava enquanto procura.
#[tauri::command]
async fn dispositivos(estado: tauri::State<'_, Estado>) -> Result<Vec<Vizinho>, String> {
    // Sem a própria impressão digital não se filtra o próprio dispositivo, que
    // também se anuncia na rede.
    let fp = descoberta::impressao_curta(&estado.eu.public());
    tokio::task::spawn_blocking(move || {
        descoberta::procurar(std::time::Duration::from_secs(3), Some(&fp))
    })
        .await
        .map_err(|e| e.to_string())?
        .map(|v| {
            v.into_iter()
                .map(|d| Vizinho {
                    nome: d.nome,
                    endereco: d.endereco.to_string(),
                    impressao: d.impressao,
                })
                .collect()
        })
        .map_err(|e| e.to_string())
}

/// Envia um ficheiro. Devolve assim que o envio arranca; o resto conta-se por
/// acontecimentos, para a interface não ficar presa à espera.
#[tauri::command]
async fn enviar_ficheiro(
    app: tauri::AppHandle,
    ficheiro: std::path::PathBuf,
    destino: String,
) -> Result<(), String> {
    let fp = descoberta::impressao_curta(&app.state::<Estado>().eu.public());
    let endereco = tokio::task::spawn_blocking(move || descoberta::resolver(&destino, PORTA, Some(&fp)))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;

    let eu = copiar(&app.state::<Estado>().eu);
    let metricas = app.state::<Estado>().metricas.clone();
    let cancelar: Cancelar = Default::default();

    // Este ouvinte, além de reencaminhar, gere o registo de cancelamento: só
    // aqui se conhece ao mesmo tempo o sinal e o identificador que a interface
    // vai usar para o accionar.
    let ouvinte = {
        let app = app.clone();
        let cancelar = cancelar.clone();
        Arc::new(move |e: Evento| {
            let estado = app.state::<Estado>();
            match &e {
                Evento::Oferta { id, .. } => {
                    estado.cancelaveis.lock().unwrap().insert(id.clone(), cancelar.clone());
                }
                Evento::Enviado { id }
                | Evento::Cancelado { id }
                | Evento::Falhou { id, .. } => {
                    estado.cancelaveis.lock().unwrap().remove(id);
                }
                _ => {}
            }
            let _ = app.emit(CANAL, ParaInterface::from(e));
        })
    };

    tauri::async_runtime::spawn(async move {
        // A app corre servidor na PORTA, por isso quem recebe pode avisá-la de
        // um cancelamento por esse endereço.
        if let Err(e) = enviar::enviar(&eu, &ficheiro, endereco, ouvinte.clone(), cancelar, &metricas, PORTA).await {
            // Só chega aqui um erro anterior ao envio propriamente dito — ligar
            // ou resolver o destino. A partir do aceite, o próprio `enviar`
            // fecha a transferência por evento.
            ouvinte(Evento::Falhou { id: String::new(), erro: e.to_string() });
        }
    });

    Ok(())
}

/// Manda parar um envio a decorrer. Silencioso se já terminou — a interface
/// pode carregar em cancelar mesmo no instante em que a transferência acaba.
#[tauri::command]
fn cancelar(estado: tauri::State<'_, Estado>, id: String) {
    if let Some(sinal) = estado.cancelaveis.lock().unwrap().get(&id) {
        sinal.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Encaminha os acontecimentos da rede para a WebView.
fn ouvinte_para(app: tauri::AppHandle) -> pombo_correio_rede::Ouvinte {
    Arc::new(move |e: Evento| {
        // Perder um acontecimento não pode derrubar uma transferência: se a
        // janela já fechou, não há para onde o enviar, e isso é normal.
        let _ = app.emit(CANAL, ParaInterface::from(e));
    })
}

/// Os acontecimentos, na forma que a interface consome.
///
/// Tipo próprio em vez de serializar o [`Evento`] diretamente: a chave `tipo`
/// dá à interface um `switch` simples, e a fronteira fica documentada num sítio
/// só em vez de espalhada pelo TypeScript.
#[derive(Serialize, Clone)]
#[serde(tag = "tipo")]
enum ParaInterface {
    #[serde(rename = "a_escutar")]
    AEscutar { porta: u16, anunciado: bool },

    #[serde(rename = "oferta")]
    Oferta {
        id: String,
        de: String,
        numero_seguranca: String,
        ficheiro: String,
        tamanho: u64,
        novo: bool,
        nome_anterior: Option<String>,
        retoma: u32,
        total: u32,
    },

    #[serde(rename = "destino")]
    Destino {
        nome: String,
        numero_seguranca: String,
    },

    #[serde(rename = "progresso")]
    Progresso {
        id: String,
        feitos: u32,
        total: u32,
        bytes: u64,
        bytes_por_seg: f64,
    },

    #[serde(rename = "recebido")]
    Recebido { id: String, caminho: String },

    #[serde(rename = "enviado")]
    Enviado { id: String },

    #[serde(rename = "cancelado")]
    Cancelado { id: String },

    #[serde(rename = "falhou")]
    Falhou { id: String, erro: String },
}

impl From<Evento> for ParaInterface {
    fn from(e: Evento) -> Self {
        match e {
            Evento::AEscutar { porta, anunciado } => Self::AEscutar { porta, anunciado },
            Evento::Oferta {
                id,
                de,
                ficheiro,
                tamanho,
                novo,
                nome_anterior,
                retoma,
                total,
            } => Self::Oferta {
                id,
                numero_seguranca: de.safety_number(),
                de: de.label,
                ficheiro,
                tamanho,
                novo,
                nome_anterior,
                retoma,
                total,
            },
            Evento::Destino { quem } => Self::Destino {
                numero_seguranca: quem.safety_number(),
                nome: quem.label,
            },
            Evento::Progresso {
                id,
                feitos,
                total,
                bytes,
                bytes_por_seg,
            } => Self::Progresso {
                id,
                feitos,
                total,
                bytes,
                bytes_por_seg,
            },
            Evento::Recebido { id, caminho } => Self::Recebido {
                id,
                caminho: caminho.display().to_string(),
            },
            Evento::Enviado { id } => Self::Enviado { id },
            Evento::Cancelado { id } => Self::Cancelado { id },
            Evento::Falhou { id, erro } => Self::Falhou { id, erro },
        }
    }
}

fn nome_do_dispositivo() -> String {
    #[cfg(windows)]
    let bruto = std::env::var("COMPUTERNAME").ok();
    // No Android não há `/etc/hostname` útil (dava sempre "localhost"); o modelo
    // do aparelho (ro.product.model) é o que a pessoa reconhece na lista.
    #[cfg(target_os = "android")]
    let bruto = std::process::Command::new("getprop")
        .arg("ro.product.model")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok());
    #[cfg(all(not(windows), not(target_os = "android")))]
    let bruto = std::fs::read_to_string("/etc/hostname").ok();

    bruto
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "dispositivo".into())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // No Android, o foreground service que mantém a app a receber em segundo
    // plano é um serviço Kotlin nativo (RecetorService), arrancado pela
    // MainActivity — não um plugin. A tentativa com o plugin de segundo plano
    // ficou registada no PLANO.md: a arquitetura headless dele exige símbolos
    // JNI que não temos, e rebentava o processo. O serviço nativo é simples e
    // fiável. O servidor de receção continua a correr no processo (no `setup`).
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let dados = app.path().app_data_dir()?;
            let eu = Identity::load_or_create(&dados.join("identidade"), nome_do_dispositivo())?;

            // O servidor arranca com a aplicação: ter a janela aberta é o que
            // significa estar disponível para receber. Pedir que se ligue algo à
            // parte seria a primeira coisa que alguém esqueceria.
            let ouvinte = ouvinte_para(app.handle().clone());
            let recetor = copiar(&eu);
            let destino = dados.join("recebidos");
            let conhecidos = dados.join("conhecidos.json");
            // O mesmo registo serve as receções e os envios, para o botão de
            // cancelar funcionar nos dois lados.
            let cancelaveis: Cancelaveis = Default::default();
            let cancelaveis_recetor = cancelaveis.clone();
            let metricas = Metricas::no_diretorio(&dados);
            let metricas_recetor = metricas.clone();

            tauri::async_runtime::spawn(async move {
                if let Err(e) = receber::servir(
                    recetor,
                    destino,
                    conhecidos,
                    PORTA,
                    ouvinte.clone(),
                    cancelaveis_recetor,
                    metricas_recetor,
                )
                .await
                {
                    ouvinte(Evento::Falhou {
                        id: String::new(),
                        erro: format!("não consegui ficar à escuta: {e}"),
                    });
                }
            });

            app.manage(Estado {
                eu,
                cancelaveis,
                metricas,
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            identidade,
            dispositivos,
            enviar_ficheiro,
            cancelar
        ])
        .run(tauri::generate_context!())
        .expect("a aplicação não arrancou");
}
