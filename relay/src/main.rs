//! `pombo-relay` — o servidor de casa.

use anyhow::Result;
use clap::Parser;
use pombo_correio_relay::autenticado::Autorizados;

#[derive(Parser)]
#[command(name = "pombo-relay", about = "Relay do Pombo-Correio")]
struct Cli {
    /// Onde ficam os depósitos.
    #[arg(long, default_value = "/var/lib/pombo-relay")]
    dados: std::path::PathBuf,

    #[arg(long, default_value_t = 53319)]
    porta: u16,

    /// Ficheiro com as impressões digitais que podem depositar, uma por linha.
    ///
    /// **Obrigatório assim que o relay esteja acessível de fora.** Sem ele
    /// aceita-se qualquer identidade válida, o que só serve dentro de casa:
    /// assinar prova posse de uma chave, e gerar um par novo custa
    /// microssegundos.
    #[arg(long)]
    autorizados: Option<std::path::PathBuf>,

    /// Pasta com a página do link sem instalação (o `pagina/www` construído).
    ///
    /// Sem ela, `/d/{id}` responde 404 e o resto do relay funciona na mesma —
    /// os links é que deixam de abrir.
    #[arg(long)]
    pagina: Option<std::path::PathBuf>,

    /// Pasta com a PWA construída (`ui/dist`, `npm run build`), servida na raiz.
    ///
    /// Só faz sentido num único nó de casa — ver "Antes do S8" no PLANO.md: uma
    /// PWA instalada a partir de dois sítios diferentes seria duas apps com
    /// identidades diferentes (`localStorage` é por origem).
    #[arg(long)]
    app: Option<std::path::PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let autorizados = match &cli.autorizados {
        Some(f) => Autorizados::do_ficheiro(f)?,
        None => {
            eprintln!("aviso: sem --autorizados, qualquer um pode depositar aqui");
            Autorizados::abertos()
        }
    };

    pombo_correio_relay::servidor::servir(cli.dados, cli.porta, autorizados, cli.pagina, cli.app).await
}
