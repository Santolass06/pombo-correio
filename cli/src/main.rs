mod conhecidos;
mod descoberta;
mod enviar;
mod progresso;
mod protocolo;
mod receber;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use pombo_correio_core::identity::Identity;

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

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let dados = pasta_dados(cli.dados)?;
    let eu = Identity::load_or_create(&dados.join("identidade"), nome_do_dispositivo())?;

    match cli.comando {
        Comando::Receber { porta } => {
            receber::servir(
                eu,
                dados.join("recebidos"),
                dados.join("conhecidos.json"),
                porta,
            )
            .await
        }

        Comando::Enviar { ficheiro, destino } => {
            let endereco = descoberta::resolver(&destino, PORTA)?;
            enviar::enviar(&eu, &ficheiro, endereco).await
        }

        Comando::Dispositivos => {
            println!("À procura na rede local...");
            let achados = descoberta::procurar(std::time::Duration::from_secs(3))?;

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
