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
        /// Endereço do destino, por exemplo `192.168.1.50:53318`.
        destino: String,
    },
    /// Mostra a identidade deste dispositivo.
    Identidade,
}

fn pasta_dados(escolhida: Option<std::path::PathBuf>) -> Result<std::path::PathBuf> {
    if let Some(d) = escolhida {
        return Ok(d);
    }
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
fn nome_do_dispositivo() -> String {
    std::fs::read_to_string("/etc/hostname")
        .map(|s| s.trim().to_string())
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "dispositivo".into())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let dados = pasta_dados(cli.dados)?;
    let eu = Identity::load_or_create(&dados.join("identidade"), nome_do_dispositivo())?;

    match cli.comando {
        Comando::Receber { porta } => receber::servir(eu, dados.join("recebidos"), porta).await,
        Comando::Enviar { ficheiro, destino } => enviar::enviar(&eu, &ficheiro, &destino).await,
        Comando::Identidade => {
            println!("{}", eu.label);
            println!("Número de segurança: {}", eu.public().safety_number());
            Ok(())
        }
    }
}
