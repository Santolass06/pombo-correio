//! Progresso de uma transferência, numa linha que se reescreve a si própria.

use std::{
    io::{IsTerminal, Write},
    time::{Duration, Instant},
};

/// Com que frequência se reescreve a linha. Mais depressa do que isto só serve
/// para gastar ciclos: o olho não acompanha e o terminal começa a piscar.
const CADENCIA: Duration = Duration::from_millis(100);

pub struct Progresso {
    total: u32,
    inicio: Instant,
    ultimo_desenho: Instant,
    bytes: u64,
    /// Fora de um terminal não se desenha nada. O `\r` só faz sentido num ecrã;
    /// num ficheiro de registo ou num tubo encheria tudo de lixo, e o teste de
    /// integração lê esta saída linha a linha.
    visivel: bool,
}

impl Progresso {
    pub fn novo(total: u32) -> Self {
        Self {
            total,
            inicio: Instant::now(),
            ultimo_desenho: Instant::now() - CADENCIA,
            bytes: 0,
            visivel: std::io::stdout().is_terminal(),
        }
    }

    /// Regista o estado atual. Ambos os valores são absolutos: `feitos` conta
    /// desde o início do ficheiro, para que uma transferência retomada mostre a
    /// percentagem do todo e não a do que falta, e `bytes` conta os movidos
    /// nesta sessão, que é o que dá um débito honesto depois de uma retoma.
    pub fn passo(&mut self, feitos: u32, bytes: u64) {
        self.bytes = bytes;

        // O último bloco desenha-se sempre, senão a linha ficaria parada nos
        // 99% quando a transferência já acabou.
        let ultimo = feitos >= self.total;
        if !self.visivel || (self.ultimo_desenho.elapsed() < CADENCIA && !ultimo) {
            return;
        }
        self.ultimo_desenho = Instant::now();

        let pct = feitos as f64 / self.total.max(1) as f64 * 100.0;
        print!(
            "\r  {pct:5.1}%  {feitos}/{} blocos · {:.1} MB/s        ",
            self.total,
            self.debito()
        );
        let _ = std::io::stdout().flush();
    }

    fn debito(&self) -> f64 {
        let s = self.inicio.elapsed().as_secs_f64();
        if s > 0.0 { self.bytes as f64 / s / 1e6 } else { 0.0 }
    }

    /// Fecha a linha do `\r` e resume.
    pub fn fim(&mut self) {
        if !self.visivel {
            return;
        }
        println!(
            "\r  {:.1} MB em {:.1}s · {:.1} MB/s média        ",
            self.bytes as f64 / 1e6,
            self.inicio.elapsed().as_secs_f64(),
            self.debito()
        );
    }
}
