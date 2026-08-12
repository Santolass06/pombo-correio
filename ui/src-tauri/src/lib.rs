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
use pombo_correio_rede::{Cancelar, Cancelaveis, Evento, Metricas, descoberta, enviar, receber, relay};
use serde::{Deserialize, Serialize};
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
///
/// Também volta para cá tal e qual quando se envia: é o que permite aproveitar
/// o endereço já descoberto em vez de o procurar de novo.
#[derive(Serialize, Deserialize)]
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

/// Para onde enviar, partindo do que a interface já tinha na lista.
///
/// O endereço vem como dica e confirma-se num instante; só se não servir é que
/// se procura. Sem isto pagava-se a janela inteira da descoberta antes de cada
/// envio — e no Android antes mesmo de o seletor de ficheiros abrir.
async fn endereco_de(app: &tauri::AppHandle, destino: &Vizinho) -> Result<std::net::SocketAddr, String> {
    let fp = descoberta::impressao_curta(&app.state::<Estado>().eu.public());
    descoberta::resolver_com_dica(
        destino.endereco.parse().ok(),
        &destino.nome,
        destino.impressao.as_deref(),
        PORTA,
        Some(&fp),
    )
    .await
    .map_err(|e| e.to_string())
}

/// Por onde o envio vai seguir. É a escada, decidida antes de se mexer um byte.
enum Caminho {
    /// Direto: o destino respondeu e está ali.
    Direto(std::net::SocketAddr),
    /// Pelo relay de casa, porque o destino não se alcança agora.
    Relay {
        base: String,
        para: pombo_correio_core::identity::PublicIdentity,
    },
}

/// Desce a escada: direto primeiro, relay depois.
///
/// A ordem não é estética. O caminho direto não sai de casa e anda à velocidade
/// da LAN; o relay gasta o upload de casa **duas vezes** (entra e sai) e só
/// entrega quando a outra pessoa abrir a app.
async fn caminho_para(app: &tauri::AppHandle, destino: &Vizinho) -> Result<Caminho, String> {
    let directo = endereco_de(app, destino).await;
    if let Ok(e) = directo {
        return Ok(Caminho::Direto(e));
    }
    let porque_nao = directo.unwrap_err();

    let dados = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let Some(base) = relay::primeiro_de_pe(relay::configurados(&dados)).await else {
        return Err(porque_nao);
    };

    // Sem a chave pública não há envelope, e sem envelope o ficheiro teria de ir
    // em claro pelo relay — que é exatamente o que não pode acontecer.
    let impressao = destino.impressao.as_deref().unwrap_or_default();
    let Some(para) = pombo_correio_rede::conhecidos::chaves_para(&dados, impressao) else {
        return Err(format!(
            "{porque_nao} — e nunca troquei ficheiros com este dispositivo, por isso também não o posso deixar no relay"
        ));
    };

    Ok(Caminho::Relay { base, para })
}

/// Põe ficheiros a caminho do destino e devolve logo; o resto conta-se por
/// acontecimentos, para a interface não ficar presa à espera.
///
/// **Um de cada vez, de propósito:** em paralelo dividiriam a mesma rede e
/// acabariam todos mais tarde. Cada um leva o seu sinal de cancelamento, para o
/// botão de um não parar os outros.
fn despachar(
    app: &tauri::AppHandle,
    fontes: Vec<Box<dyn pombo_correio_rede::Fonte>>,
    caminho: Caminho,
) {
    let eu = copiar(&app.state::<Estado>().eu);
    let metricas = app.state::<Estado>().metricas.clone();
    // Onde guardar as chaves de quem receber, para lhe poder deixar ficheiros no
    // relay mais tarde. Sem pasta de dados o envio faz-se na mesma — só não se
    // aprende nada com ele.
    let dados = app.path().app_data_dir().ok();
    let app = app.clone();

    tauri::async_runtime::spawn(async move {
        for fonte in fontes {
            let cancelar: Cancelar = Default::default();
            let ouvinte = ouvinte_de_envio(app.clone(), cancelar.clone());

            let r = match &caminho {
                // A app corre servidor na PORTA, por isso quem recebe pode
                // avisá-la de um cancelamento por esse endereço.
                Caminho::Direto(endereco) => {
                    enviar::enviar_fonte(&eu, fonte.as_ref(), *endereco, ouvinte.clone(), cancelar, &metricas, PORTA, dados.as_deref())
                        .await
                }
                Caminho::Relay { base, para } => {
                    pombo_correio_rede::relay::depositar(&eu, fonte.as_ref(), para, base, ouvinte.clone(), cancelar, &metricas)
                        .await
                }
            };

            if let Err(e) = r {
                // Só chega aqui um erro anterior ao envio propriamente dito —
                // ligar ou abrir a origem. A partir do aceite, o próprio
                // `enviar` fecha a transferência por evento.
                ouvinte(Evento::Falhou { id: String::new(), erro: e.to_string() });
            }
        }
    });
}

/// Envia um ficheiro escolhido pelo diálogo do desktop.
#[tauri::command]
async fn enviar_ficheiro(
    app: tauri::AppHandle,
    ficheiro: std::path::PathBuf,
    destino: Vizinho,
) -> Result<(), String> {
    let caminho = caminho_para(&app, &destino).await?;
    despachar(&app, vec![Box::new(pombo_correio_rede::FonteCaminho(ficheiro))], caminho);
    Ok(())
}

/// Deixa um ficheiro no relay e devolve um link, para quem não tem a app.
///
/// Não passa pelo `caminho_para`: aqui não há destino a alcançar, e por isso
/// também não há degrau direto — um link só existe se houver servidor de casa
/// que o sirva.
///
/// `ficheiro` vem do diálogo do desktop; no Android é `None` e o seletor abre-se
/// deste lado, porque lá não há caminhos, há `content://`.
#[tauri::command]
async fn criar_ligacao(
    app: tauri::AppHandle,
    ficheiro: Option<std::path::PathBuf>,
) -> Result<Option<String>, String> {
    let dados = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let Some(base) = relay::primeiro_de_pe(relay::configurados(&dados)).await else {
        return Err(
            "nenhum servidor de casa responde, e um link precisa de um para o servir".into(),
        );
    };

    let fonte: Box<dyn pombo_correio_rede::Fonte> = match ficheiro {
        Some(caminho) => Box::new(pombo_correio_rede::FonteCaminho(caminho)),
        None => match escolher_no_android(&app).await? {
            Some(f) => f,
            None => return Ok(None), // cancelou o seletor
        },
    };

    let eu = copiar(&app.state::<Estado>().eu);
    let metricas = app.state::<Estado>().metricas.clone();
    let cancelar: Cancelar = Default::default();
    let ouvinte = ouvinte_de_envio(app.clone(), cancelar.clone());

    pombo_correio_rede::relay::depositar_ligacao(
        &eu,
        fonte.as_ref(),
        &base,
        ouvinte,
        cancelar,
        &metricas,
    )
    .await
    .map_err(|e| e.to_string())
}

/// O seletor do Android, que é bloqueante e devolve um `content://`.
async fn escolher_no_android(
    app: &tauri::AppHandle,
) -> Result<Option<Box<dyn pombo_correio_rede::Fonte>>, String> {
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Err("sem ficheiro escolhido".into())
    }
    #[cfg(target_os = "android")]
    {
        use tauri_plugin_android_fs::AndroidFsExt;

        let app_sel = app.clone();
        let escolha = tokio::task::spawn_blocking(move || {
            let afs = app_sel.android_fs();
            let uri = afs.picker().pick_file(None, &[], false).map_err(|e| e.to_string())?;
            Ok::<_, String>(uri.map(|uri| {
                let nome = afs.get_name(&uri).unwrap_or_else(|_| "ficheiro".into());
                (uri, nome)
            }))
        })
        .await
        .map_err(|e| e.to_string())??;

        Ok(escolha.map(|(uri, nome)| {
            Box::new(FonteAndroid { app: app.clone(), uri, nome }) as Box<dyn pombo_correio_rede::Fonte>
        }))
    }
}

/// Vai ver ao relay se há coisa à espera.
///
/// Chamado ao abrir a app e de vez em quando pela interface. Sem push (que entra
/// em R6), é assim que se dá pelo que chegou enquanto o aparelho estava
/// desligado.
#[tauri::command]
async fn buscar_no_relay(app: tauri::AppHandle) -> Result<usize, String> {
    let dados = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let relays = relay::configurados(&dados);
    if relays.is_empty() {
        return Ok(0);
    }

    let eu = copiar(&app.state::<Estado>().eu);
    let metricas = app.state::<Estado>().metricas.clone();
    let cancelaveis = app.state::<Estado>().cancelaveis.clone();
    let ouvinte = ouvinte_para(app.clone());

    let mut chegaram = 0;
    for base in relays {
        // Um relay em baixo não pode impedir o outro de entregar. É o ponto todo
        // de haver dois.
        if let Ok(n) = pombo_correio_rede::relay::buscar(
            &eu,
            &base,
            &dados.join("recebidos"),
            &dados.join("conhecidos.json"),
            ouvinte.clone(),
            cancelaveis.clone(),
            &metricas,
        )
        .await
        {
            chegaram += n;
        }
    }
    Ok(chegaram)
}

/// Ouvinte de um envio: reencaminha para a WebView e gere o registo de
/// cancelamento — só aqui se conhece o sinal e o identificador ao mesmo tempo.
fn ouvinte_de_envio(app: tauri::AppHandle, cancelar: Cancelar) -> pombo_correio_rede::Ouvinte {
    Arc::new(move |e: Evento| {
        let estado = app.state::<Estado>();
        match &e {
            // As chaves de quem recebe são guardadas pelo `enviar_fonte`, em
            // `vistos.json` e só se o ficheiro chegar inteiro. Aqui já se
            // escreveram no `conhecidos.json`, o que era cedo (basta ter falado
            // com o endereço) e no sítio errado — é esse ficheiro que decide
            // mostrar o número de segurança a quem nos envia pela primeira vez.
            Evento::Oferta { id, .. } => {
                estado.cancelaveis.lock().unwrap().insert(id.clone(), cancelar.clone());
            }
            Evento::Enviado { id }
            | Evento::Depositado { id, .. }
            | Evento::Cancelado { id }
            | Evento::Falhou { id, .. } => {
                estado.cancelaveis.lock().unwrap().remove(id);
            }
            _ => {}
        }
        let _ = app.emit(CANAL, ParaInterface::from(e));
    })
}

/// Origem de um ficheiro escolhido no Android — um URI `content://` que o
/// plugin resolve num `std::fs::File`. Reabre a cada chamada (o `enviar` lê a
/// origem duas vezes: manifesto e streaming).
#[cfg(target_os = "android")]
struct FonteAndroid {
    app: tauri::AppHandle,
    uri: tauri_plugin_android_fs::FsUri,
    nome: String,
}

#[cfg(target_os = "android")]
impl pombo_correio_rede::Fonte for FonteAndroid {
    fn nome(&self) -> String {
        self.nome.clone()
    }
    fn abrir(&self) -> std::io::Result<std::fs::File> {
        use tauri_plugin_android_fs::AndroidFsExt;
        self.app
            .android_fs()
            .open_file_readable(&self.uri)
            .map_err(std::io::Error::other)
    }
}

/// Envio a partir do Android: abre o seletor de ficheiros do sistema (que
/// devolve um `content://`), e envia. Comando separado do `enviar_ficheiro`
/// porque o Android não trabalha com caminhos.
#[tauri::command]
async fn enviar_android(app: tauri::AppHandle, destino: Vizinho) -> Result<(), String> {
    #[cfg(not(target_os = "android"))]
    {
        let _ = (app, destino);
        Err("só no Android".into())
    }
    #[cfg(target_os = "android")]
    {
        use tauri_plugin_android_fs::AndroidFsExt;

        // Antes do seletor, de propósito: se o destino já lá não estiver, mais
        // vale dizê-lo do que deixar escolher um ficheiro para nada. Custa
        // milissegundos enquanto a dica servir.
        let caminho = caminho_para(&app, &destino).await?;

        // O seletor é bloqueante (abre uma atividade e espera), por isso fora da
        // thread da interface.
        let app_sel = app.clone();
        let escolha = tokio::task::spawn_blocking(move || {
            let afs = app_sel.android_fs();
            let uri = afs
                .picker()
                .pick_file(None, &[], false)
                .map_err(|e| e.to_string())?;
            match uri {
                Some(uri) => {
                    let nome = afs.get_name(&uri).unwrap_or_else(|_| "ficheiro".into());
                    Ok::<_, String>(Some((uri, nome)))
                }
                None => Ok(None), // o utilizador cancelou o seletor
            }
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e)?;

        let Some((uri, nome)) = escolha else {
            return Ok(()); // cancelado, sem erro
        };

        despachar(&app, vec![Box::new(FonteAndroid { app: app.clone(), uri, nome })], caminho);
        Ok(())
    }
}

/// Um ficheiro que outra app partilhou para cá, à espera de destino.
#[derive(Serialize, Deserialize, Clone)]
struct Partilhado {
    uri: String,
    nome: String,
}

/// Lê e **apaga** a lista de URIs que a `MainActivity` deixou em disco.
///
/// Apagar não é limpeza, é correção: a permissão de leitura de um `content://`
/// partilhado morre com a atividade que o recebeu, por isso entregar o mesmo URI
/// uma segunda vez seria entregar uma coisa que já não abre.
///
/// Fora do Android só o teste lhe pega — não há folha de partilha em mais lado
/// nenhum — mas fica compilada em toda a parte para a regra "consome uma vez"
/// ter teste onde é cómodo corrê-lo.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn consumir_pendentes(caminho: &std::path::Path) -> Vec<String> {
    let Ok(texto) = std::fs::read_to_string(caminho) else {
        return Vec::new();
    };
    let _ = std::fs::remove_file(caminho);
    texto
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// O que outra app partilhou para cá desde a última vez que se perguntou.
///
/// A interface pergunta ao montar e sempre que a janela volta a ficar visível —
/// é por aí que entra uma partilha com a app já aberta.
#[tauri::command]
fn partilha_pendente(app: tauri::AppHandle) -> Vec<Partilhado> {
    #[cfg(not(target_os = "android"))]
    {
        // Só o Android tem folha de partilha; no desktop não há nada à espera.
        let _ = app;
        Vec::new()
    }
    #[cfg(target_os = "android")]
    {
        use tauri_plugin_android_fs::{AndroidFsExt, FsUri};

        let Ok(dados) = app.path().app_data_dir() else {
            return Vec::new();
        };
        let afs = app.android_fs();
        consumir_pendentes(&dados.join("partilha-pendente"))
            .into_iter()
            .map(|uri| {
                let nome = afs
                    .get_name(&FsUri::from_uri(uri.clone()))
                    .unwrap_or_else(|_| "ficheiro".into());
                Partilhado { uri, nome }
            })
            .collect()
    }
}

/// Envia o que outra app partilhou para cá.
///
/// Os URIs vêm da interface, e não do disco: o ficheiro já foi consumido em
/// `partilha_pendente`, de propósito.
#[tauri::command]
async fn enviar_partilhados(
    app: tauri::AppHandle,
    destino: Vizinho,
    ficheiros: Vec<Partilhado>,
) -> Result<(), String> {
    #[cfg(not(target_os = "android"))]
    {
        let _ = (app, destino, ficheiros);
        Err("só no Android".into())
    }
    #[cfg(target_os = "android")]
    {
        if ficheiros.is_empty() {
            return Ok(());
        }
        let caminho = caminho_para(&app, &destino).await?;
        let fontes = ficheiros
            .into_iter()
            .map(|f| {
                Box::new(FonteAndroid {
                    app: app.clone(),
                    uri: tauri_plugin_android_fs::FsUri::from_uri(f.uri),
                    nome: f.nome,
                }) as Box<dyn pombo_correio_rede::Fonte>
            })
            .collect();
        despachar(&app, fontes, caminho);
        Ok(())
    }
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

    #[serde(rename = "depositado")]
    Depositado { id: String, para: Option<String> },

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
            Evento::Depositado { id, para } => Self::Depositado { id, para },
            Evento::Cancelado { id } => Self::Cancelado { id },
            Evento::Falhou { id, erro } => Self::Falhou { id, erro },
        }
    }
}

fn nome_do_dispositivo(dados: &std::path::Path) -> String {
    #[cfg(windows)]
    let bruto = std::env::var("COMPUTERNAME").ok();

    // No Android o nome que a pessoa reconhece é o das Definições
    // (`Settings.Global.device_name`, ex. "OnePlus Pad 2"), não o modelo. Só é
    // acessível por código Android, por isso a MainActivity (Kotlin) lê-o e
    // escreve-o neste ficheiro antes de o Rust arrancar. Cai para o modelo se
    // ainda não estiver.
    #[cfg(target_os = "android")]
    let bruto = std::fs::read_to_string(dados.join("nome-dispositivo")).ok().or_else(|| {
        std::process::Command::new("getprop")
            .arg("ro.product.model")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
    });

    #[cfg(all(not(windows), not(target_os = "android")))]
    let bruto = std::fs::read_to_string("/etc/hostname").ok();

    #[cfg(all(not(windows), not(target_os = "android")))]
    let _ = dados;

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
        .plugin(tauri_plugin_android_fs::init())
        .setup(|app| {
            let dados = app.path().app_data_dir()?;
            let eu = Identity::load_or_create(&dados.join("identidade"), nome_do_dispositivo(&dados))?;

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
            enviar_android,
            partilha_pendente,
            enviar_partilhados,
            buscar_no_relay,
            criar_ligacao,
            cancelar
        ])
        .run(tauri::generate_context!())
        .expect("a aplicação não arrancou");
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn partilha_pendente_consome_uma_vez() {
        let dir = std::env::temp_dir().join(format!("pombo-partilha-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("partilha-pendente");
        std::fs::write(&f, "content://a\n\ncontent://b\n").unwrap();

        assert_eq!(consumir_pendentes(&f), ["content://a", "content://b"]);
        // A segunda leitura tem de vir vazia. Um URI de partilha não vale fora
        // da sessão que o recebeu; ressuscitá-lo daria um erro de permissão que
        // ninguém saberia ler.
        assert!(consumir_pendentes(&f).is_empty());

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
