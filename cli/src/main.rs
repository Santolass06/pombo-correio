mod progresso;

use std::{collections::HashMap, sync::Arc};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use pombo_correio_core::identity::Identity;
use pombo_correio_rede::{
    Evento, Metricas, Ouvinte, descoberta, receber,
    relay::{Relay, enviar_com_alternativa, vigiar_caixa},
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
        /// Relay de casa (`host:porta`): regista-se lá e vai à caixa de 20 em
        /// 20 segundos. Sem isto, só se recebe diretamente.
        #[arg(long)]
        relay: Option<String>,
        /// Token de registo, se o relay o exigir.
        #[arg(long)]
        token: Option<String>,
    },
    /// Envia um ficheiro para outro dispositivo.
    Enviar {
        ficheiro: std::path::PathBuf,
        /// Nome do dispositivo na rede local, ou endereço como
        /// `192.168.1.50:53318` para fora dela.
        destino: String,
        /// Relay de casa (`host:porta`). Só é usado se o destino não for
        /// alcançável; exige que o destino se tenha registado no relay.
        #[arg(long)]
        relay: Option<String>,
        /// Impressão digital (prefixo de pelo menos 8 hex) do destino no relay,
        /// para quando `destino` é um endereço e não um nome da rede local.
        #[arg(long)]
        impressao: Option<String>,
        /// Token de registo, se o relay o exigir.
        #[arg(long)]
        token: Option<String>,
    },
    /// Lista os dispositivos à espera na rede local.
    Dispositivos,
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

        Evento::Cancelado { id } => {
            barras.lock().unwrap().remove(&id);
            println!("\n  Cancelado. O que já passou fica guardado — retomar continua a valer.");
        }

        Evento::NoRelay { ficheiro, .. } => {
            println!("  {ficheiro}: deixado no relay — chega quando o destino abrir a app")
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
        Comando::Receber { porta, relay, token } => {
            println!("Pombo-Correio — à espera");
            println!("  Este dispositivo: {}", eu.label);
            println!("  Número de segurança: {}", eu.public().safety_number());
            println!("  A guardar em {}", dados.join("recebidos").display());

            if let Some(endereco) = relay {
                let eu_caixa = Identity::from_secret_bytes(eu.secret_bytes().0, eu.secret_bytes().1, eu.label.clone());
                let (destino, conhecidos) = (dados.join("recebidos"), dados.join("conhecidos.json"));
                let (ouv, metricas) = (ouvinte.clone(), Metricas::no_diretorio(&dados));
                println!("  Relay: {endereco}");
                tokio::spawn(async move {
                    vigiar_caixa(
                        &eu_caixa, &Relay::novo(&endereco), &destino, conhecidos, ouv, &metricas,
                        std::time::Duration::from_secs(20), Default::default(), token,
                    )
                    .await;
                });
            }

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

        Comando::Enviar { ficheiro, destino, relay, impressao, token } => {
            let fp = descoberta::impressao_curta(&eu.public());
            let relay = relay.map(|r| Relay::novo(&r));
            if let Some(r) = &relay {
                // Para poder depositar, este dispositivo tem de estar registado.
                // Falhar aqui não impede o envio direto.
                if let Err(e) = r.registar(&eu, token.as_deref()).await {
                    println!("  (relay indisponível: {e})");
                }
            }
            let endereco = descoberta::resolver(&destino, PORTA, Some(&fp))?;
            println!("A preparar {}...", ficheiro.display());
            let metricas = Metricas::no_diretorio(&dados);
            let fonte = pombo_correio_rede::FonteCaminho(ficheiro);
            // Porta 0: o CLI a enviar não corre servidor, por isso quem recebe
            // não o pode avisar de um cancelamento — mas o Ctrl+C basta.
            enviar_com_alternativa(
                &eu, &fonte, endereco, impressao.as_deref().unwrap_or(""), relay.as_ref(),
                ouvinte, sem_cancelamento(), &metricas, 0,
            )
            .await
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
            Ok(())
        }
    }
}
