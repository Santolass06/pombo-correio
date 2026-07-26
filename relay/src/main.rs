//! `pombo-relay` — o servidor de casa.

use anyhow::Result;
use clap::Parser;

#[derive(Parser)]
#[command(name = "pombo-relay", about = "Relay do Pombo-Correio")]
struct Cli {
    /// Onde ficam os depósitos.
    #[arg(long, default_value = "/var/lib/pombo-relay")]
    dados: std::path::PathBuf,

    #[arg(long, default_value_t = 53319)]
    porta: u16,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    pombo_correio_relay::servidor::servir(cli.dados, cli.porta).await
}
