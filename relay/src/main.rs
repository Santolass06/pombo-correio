//! `pombo-relay`: o depósito de bytes opacos dos servidores de casa.

use pombo_correio_relay::{Config, servir};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let mut porta: u16 = 53319;
    let mut cfg = Config::por_omissao("relay-dados".into());
    while let Some(a) = args.next() {
        let mut valor = |n: &str| args.next().unwrap_or_else(|| panic!("{n} precisa de um valor"));
        match a.as_str() {
            "--porta" => porta = valor("--porta").parse()?,
            "--dir" => cfg.dir = valor("--dir").into(),
            "--ttl-horas" => cfg.ttl_s = valor("--ttl-horas").parse::<u64>()? * 3600,
            "--max-gb" => cfg.max_deposito = valor("--max-gb").parse::<u64>()? << 30,
            "--quota-gb" => cfg.quota = valor("--quota-gb").parse::<u64>()? << 30,
            "--token-de-registo" => cfg.token_de_registo = Some(valor("--token-de-registo")),
            outro => anyhow::bail!(
                "argumento desconhecido: {outro}\nuso: pombo-relay [--porta N] [--dir D] [--ttl-horas H] [--max-gb G] [--quota-gb G] [--token-de-registo T]"
            ),
        }
    }
    tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(async {
        let l = tokio::net::TcpListener::bind(("0.0.0.0", porta)).await?;
        eprintln!("relay à escuta em {}", l.local_addr()?);
        servir(l, cfg).await
    })
}
