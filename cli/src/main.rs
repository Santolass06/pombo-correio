mod progresso;

use std::{collections::HashMap, sync::Arc};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use pombo_correio_core::identity::Identity;
use pombo_correio_rede::{
    Evento, Metricas, Ouvinte, descoberta, enviar, receber, relay,
    sem_cancelamento,
};

use progresso::Progresso;

/// Porta por omissão. Fora dos intervalos reservados e fácil de decorar.
const PORTA: u16 = 53318;

#[derive(Parser)]
#[command(name = "pombo", about = "Envia ficheiros entre os dispositivos de casa")]
struct Cli {
    /// Pasta com a identidade e os ficheiros recebidos.
    #[arg(long, global = true)]
    dados: Option<std::path::PathBuf>,

    #[command(subcommand)]
    comando: Comando,
}

#[derive(Subcommand)]
enum Comando {
    /// Fica à espera de ficheiros.
    Receber {
        #[arg(long, default_value_t = PORTA)]
        porta: u16,
    },
    /// Envia um ficheiro para outro dispositivo.
    Enviar {
        ficheiro: std::path::PathBuf,
        /// Nome do dispositivo na rede local, ou endereço como
        /// `192.168.1.50:53318` para fora dela.
        destino: String,
    },
    /// Lista os dispositivos à espera na rede local.
    Dispositivos,
    /// Deixa um ficheiro no relay e devolve um link, para quem não tem a app.
    ///
    /// Quem abrir o link decifra o ficheiro no próprio browser: a chave vai
    /// depois do `#`, que nunca chega ao servidor. Dura 7 dias.
    Ligacao { ficheiro: std::path::PathBuf },
    /// Vai buscar ao relay de casa o que estiver à espera deste dispositivo.
    ///
    /// Os relays lêem-se de `<dados>/relays`, um URL por linha. É por aqui que
    /// se valida o caminho do relay entre máquinas reais.
    Caixa,
    /// Mostra a identidade deste dispositivo.
    Identidade,
}

/// Onde ficam a identidade e os ficheiros recebidos.
///
/// Cada sistema tem a sua convenção e nenhuma delas é adivinhável a partir das
/// outras: o Windows não define `HOME`, e o Unix não tem `APPDATA`.
fn pasta_dados(escolhida: Option<std::path::PathBuf>) -> Result<std::path::PathBuf> {
    if let Some(d) = escolhida {
        return Ok(d);
    }

    #[cfg(windows)]
    let base = std::env::var_os("APPDATA")
        .map(std::path::PathBuf::from)
        .context("APPDATA não está definido")?;

    #[cfg(not(windows))]
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share"))
        })
        .context("nem XDG_DATA_HOME nem HOME estão definidos")?;

    Ok(base.join("pombo"))
}

/// Nome por omissão deste dispositivo, para não obrigar a configurar nada antes
/// da primeira utilização.
///
/// Não vale a pena uma dependência para isto: são duas fontes, uma por sistema.
/// Sem a variante do Windows, todas as máquinas Windows apareceriam com o mesmo
/// nome na lista de dispositivos — que é precisamente o que torna a lista
/// inútil.
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

/// Traduz os acontecimentos da rede em linhas de terminal.
///
/// É só isto que distingue o CLI da interface gráfica — a lógica de
/// transferência é a mesma nos dois.
fn ouvinte_terminal() -> Ouvinte {
    // Uma barra por transferência: várias podem correr ao mesmo tempo.
    let barras: Arc<std::sync::Mutex<HashMap<String, Progresso>>> = Default::default();

    Arc::new(move |e| match e {
        Evento::AEscutar { porta, anunciado } => {
            println!("  A escutar na porta {porta}");
            if !anunciado {
                println!("  Sem anúncio na rede local — usa o endereço directamente");
            }
        }

        Evento::Destino { quem } => {
            println!("Destino: {} — {}", quem.label, quem.safety_number());
        }

        Evento::Oferta {
            id,
            de,
            ficheiro,
            tamanho,
            novo,
            nome_anterior,
            retoma,
            total,
        } => {
            println!("\n  {ficheiro} ({:.1} MB)", tamanho as f64 / 1e6);
            if novo {
                println!("  DISPOSITIVO NOVO — confirma este número com quem enviou:");
                println!("    {}", de.safety_number());
            } else if let Some(antes) = nome_anterior {
                println!("  (este dispositivo chamava-se \"{antes}\")");
            }
            if retoma > 0 {
                println!("  A retomar: {retoma} de {total} blocos já feitos");
            }
            barras.lock().unwrap().insert(id, Progresso::novo(total));
        }

        Evento::Progresso {
            id,
            feitos,
            total,
            bytes,
            // O CLI corre síncrono, sem IPC pelo meio, por isso mede a sua
            // própria velocidade de forma fiável a partir dos bytes.
            bytes_por_seg: _,
        } => {
            let mut b = barras.lock().unwrap();
            b.entry(id)
                .or_insert_with(|| Progresso::novo(total))
                .passo(feitos, bytes);
        }

        Evento::Recebido { id, caminho } => {
            if let Some(mut p) = barras.lock().unwrap().remove(&id) {
                p.fim();
            }
            println!("  Recebido: {}", caminho.display());
        }

        Evento::Enviado { id } => {
            if let Some(mut p) = barras.lock().unwrap().remove(&id) {
                p.fim();
            }
            println!("Enviado.");
        }

        Evento::Depositado { id, para } => {
            if let Some(mut p) = barras.lock().unwrap().remove(&id) {
                p.fim();
            }
            // Não é "enviado": está num servidor de casa e o destino ainda nem
            // sabe dele.
            match para {
                Some(quem) => println!("Guardado no relay — chega a {quem} quando ele abrir a app."),
                None => println!("Guardado no relay. O link fica aqui em baixo."),
            }
        }

        Evento::Cancelado { id } => {
            barras.lock().unwrap().remove(&id);
            println!("\n  Cancelado. O que já passou fica guardado — retomar continua a valer.");
        }

        Evento::Falhou { erro, .. } => println!("  Falhou: {erro}"),
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let dados = pasta_dados(cli.dados)?;
    let eu = Identity::load_or_create(&dados.join("identidade"), nome_do_dispositivo())?;
    let ouvinte = ouvinte_terminal();

    match cli.comando {
        Comando::Receber { porta } => {
            println!("Pombo-Correio — à espera");
            println!("  Este dispositivo: {}", eu.label);
            println!("  Número de segurança: {}", eu.public().safety_number());
            println!("  A guardar em {}", dados.join("recebidos").display());

            receber::servir(
                eu,
                dados.join("recebidos"),
                dados.join("conhecidos.json"),
                porta,
                ouvinte,
                Default::default(),
                Metricas::no_diretorio(&dados),
            )
            .await
        }

        Comando::Enviar { ficheiro, destino } => {
            let fp = descoberta::impressao_curta(&eu.public());
            let metricas = Metricas::no_diretorio(&dados);
            println!("A preparar {}...", ficheiro.display());

            match descoberta::resolver(&destino, PORTA, Some(&fp)) {
                // Porta 0: o CLI a enviar não corre servidor, por isso quem
                // recebe não o pode avisar de um cancelamento — mas o Ctrl+C
                // basta.
                Ok(endereco) => {
                    enviar::enviar(&eu, &ficheiro, endereco, ouvinte, sem_cancelamento(), &metricas, 0, Some(&dados)).await
                }
                Err(porque_nao) => {
                    // Desce a escada, como a app. Sem chave pública de quem
                    // recebe não há envelope — e ela só existe se já tiver havido
                    // contacto direto, num sentido ou no outro.
                    // Qual dos nós, e não o primeiro da lista: o primeiro pode
                    // estar em baixo, e é precisamente para isso que são dois.
                    let de_pe = relay::primeiro_de_pe(relay::configurados(&dados)).await;
                    let alvo = pombo_correio_rede::conhecidos::chaves_para(&dados, &destino);

                    match (de_pe.as_ref(), alvo) {
                        (Some(base), Some(para)) => {
                            println!("O destino não responde — a deixar no relay.");
                            relay::depositar(&eu, &pombo_correio_rede::FonteCaminho(ficheiro), &para, base, ouvinte, sem_cancelamento(), &metricas).await
                        }
                        (Some(_), None) => Err(porque_nao.context(
                            "e pelo relay também não: dá o destino pela impressão digital de alguém com quem já tenhas trocado ficheiros",
                        )),
                        (None, _) => Err(porque_nao),
                    }
                }
            }
        }

        Comando::Ligacao { ficheiro } => {
            let Some(base) = relay::primeiro_de_pe(relay::configurados(&dados)).await else {
                println!(
                    "Nenhum relay de casa responde.\n\
                     Um link precisa de um servidor que o sirva — vê {}.",
                    dados.join("relays").display()
                );
                return Ok(());
            };

            println!("A preparar {}...", ficheiro.display());
            let link = relay::depositar_ligacao(
                &eu,
                &pombo_correio_rede::FonteCaminho(ficheiro),
                &base,
                ouvinte,
                sem_cancelamento(),
                &Metricas::no_diretorio(&dados),
            )
            .await?;

            if let Some(link) = link {
                println!("\n{link}\n");
                println!("Quem tiver este endereço abre o ficheiro, sem instalar nada.");
                println!("Dura 7 dias. O endereço tem de ser alcançável de onde a pessoa estiver.");
            }
            Ok(())
        }

        Comando::Caixa => {
            let relays = relay::configurados(&dados);
            if relays.is_empty() {
                println!(
                    "Sem relay configurado.\n\
                     Escreve os URLs em {}, um por linha.",
                    dados.join("relays").display()
                );
                return Ok(());
            }

            let metricas = Metricas::no_diretorio(&dados);
            let mut chegaram = 0;
            for base in &relays {
                // Um relay em baixo não pode impedir o outro de entregar. É o
                // ponto todo de haver dois.
                match relay::buscar(&eu, base, &dados.join("recebidos"), &dados.join("conhecidos.json"), ouvinte.clone(), Default::default(), &metricas).await {
                    Ok(n) => chegaram += n,
                    Err(e) => println!("  {base}: {e}"),
                }
            }
            println!("{chegaram} ficheiro(s) trazidos do relay.");
            Ok(())
        }

        Comando::Dispositivos => {
            println!("À procura na rede local...");
            let fp = descoberta::impressao_curta(&eu.public());
            let achados = descoberta::procurar(std::time::Duration::from_secs(3), Some(&fp))?;

            if achados.is_empty() {
                println!(
                    "Nenhum dispositivo à espera.\n\
                     Do outro lado tem de estar `pombo receber` a correr, na mesma rede."
                );
                return Ok(());
            }

            for d in &achados {
                println!("  {}  —  {}", d.nome, d.endereco);
            }
            Ok(())
        }

        Comando::Identidade => {
            println!("{}", eu.label);
            println!("Número de segurança: {}", eu.public().safety_number());
            // A impressão curta é o que vai para a lista de autorizados do
            // relay. Sem ela aqui, a lista só se conseguia escrever a adivinhar.
            println!("Impressão: {}", descoberta::impressao_curta(&eu.public()));
            Ok(())
        }
    }
}
