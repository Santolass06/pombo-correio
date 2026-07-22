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
use pombo_correio_rede::{Evento, descoberta, enviar, receber};
use serde::Serialize;
use tauri::{Emitter, Manager};

/// Nome do acontecimento na WebView. Um só canal: a interface junta envios e
/// receções no mesmo painel.
const CANAL: &str = "pombo";

const PORTA: u16 = 53318;

struct Estado {
    eu: Identity,
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
async fn dispositivos() -> Result<Vec<Vizinho>, String> {
    tokio::task::spawn_blocking(|| descoberta::procurar(std::time::Duration::from_secs(3)))
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
    let endereco = tokio::task::spawn_blocking(move || descoberta::resolver(&destino, PORTA))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;

    let eu = copiar(&app.state::<Estado>().eu);
    let ouvinte = ouvinte_para(app.clone());

    tauri::async_runtime::spawn(async move {
        if let Err(e) = enviar::enviar(&eu, &ficheiro, endereco, ouvinte.clone()).await {
            ouvinte(Evento::Falhou {
                id: String::new(),
                erro: e.to_string(),
            });
        }
    });

    Ok(())
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
    },

    #[serde(rename = "recebido")]
    Recebido { id: String, caminho: String },

    #[serde(rename = "enviado")]
    Enviado { id: String },

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
            } => Self::Progresso {
                id,
                feitos,
                total,
                bytes,
            },
            Evento::Recebido { id, caminho } => Self::Recebido {
                id,
                caminho: caminho.display().to_string(),
            },
            Evento::Enviado { id } => Self::Enviado { id },
            Evento::Falhou { id, erro } => Self::Falhou { id, erro },
        }
    }
}

fn nome_do_dispositivo() -> String {
    #[cfg(windows)]
    let bruto = std::env::var("COMPUTERNAME").ok();
    #[cfg(not(windows))]
    let bruto = std::fs::read_to_string("/etc/hostname").ok();

    bruto
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "dispositivo".into())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
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

            tauri::async_runtime::spawn(async move {
                if let Err(e) =
                    receber::servir(recetor, destino, conhecidos, PORTA, ouvinte.clone()).await
                {
                    ouvinte(Evento::Falhou {
                        id: String::new(),
                        erro: format!("não consegui ficar à escuta: {e}"),
                    });
                }
            });

            app.manage(Estado { eu });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            identidade,
            dispositivos,
            enviar_ficheiro
        ])
        .run(tauri::generate_context!())
        .expect("a aplicação não arrancou");
}
