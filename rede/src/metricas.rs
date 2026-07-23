//! Registo de métricas para diagnóstico.
//!
//! Existe porque validar entre dispositivos é difícil: quem desenvolve não vê o
//! Pi nem a rede de casa. Cada acontecimento relevante vai para um ficheiro
//! JSONL — uma linha JSON por acontecimento — que se pode ler, filtrar com
//! ferramentas de linha de comandos, ou colar para análise.
//!
//! A peça mais útil é o **tempo por fase** de cada transferência: ler da rede,
//! decifrar, verificar, escrever no disco. É isso que responde à pergunta "onde
//! se perde a velocidade?", que de outra forma só se responde por adivinhação.

use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

/// Escreve acontecimentos, ou não faz nada se estiver desligado.
///
/// Clonável de propósito: o servidor de receção e cada envio partilham o mesmo
/// ficheiro, e o `Mutex` partilhado serializa as escritas para que as linhas não
/// se entrelacem.
#[derive(Clone)]
pub struct Metricas(Option<Arc<Mutex<std::fs::File>>>);

impl Metricas {
    /// Abre `<dir>/metricas/eventos.jsonl` para acrescentar.
    ///
    /// Não é fatal falhar: sem métricas ainda se transfere. Um erro aqui vira
    /// registo desligado, não um erro que sobe.
    pub fn no_diretorio(dir: &Path) -> Self {
        let pasta = dir.join("metricas");
        let ficheiro = std::fs::create_dir_all(&pasta).ok().and_then(|_| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(pasta.join("eventos.jsonl"))
                .ok()
        });
        Self(ficheiro.map(|f| Arc::new(Mutex::new(f))))
    }

    /// Um registo que deita tudo fora, para testes e para quem não quer medir.
    pub fn desligada() -> Self {
        Self(None)
    }

    /// Acrescenta um acontecimento. `dados` tem de ser um objeto JSON; as suas
    /// chaves juntam-se às de sempre (`ts`, `tipo`).
    pub fn registar(&self, tipo: &str, dados: Value) {
        let Some(bloqueio) = &self.0 else { return };

        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();

        let mut obj = json!({ "ts": ms, "tipo": tipo });
        if let (Some(base), Value::Object(extra)) = (obj.as_object_mut(), dados) {
            base.extend(extra);
        }

        // Um erro a escrever não pode derrubar uma transferência: o registo é
        // observação, não parte do trabalho.
        if let Ok(mut f) = bloqueio.lock() {
            let _ = writeln!(f, "{obj}");
            let _ = f.flush();
        }
    }
}

/// Acumula o tempo gasto em cada fase de uma transferência.
///
/// Os quatro campos somam (aproximadamente) o tempo total: se um domina, é o
/// gargalo. `rede` a dominar aponta para a ligação ou para o outro lado; `disco`
/// a dominar num Pi aponta para o cartão SD.
#[derive(Default, Clone, Copy)]
pub struct Fases {
    pub rede: Duration,
    pub decifra: Duration,
    pub verifica: Duration,
    pub disco: Duration,
}

impl Fases {
    /// As fases como objeto JSON, em milissegundos.
    pub fn json(&self) -> Value {
        json!({
            "rede_ms": self.rede.as_millis(),
            "decifra_ms": self.decifra.as_millis(),
            "verifica_ms": self.verifica.as_millis(),
            "disco_ms": self.disco.as_millis(),
        })
    }
}

/// Mede quanto tempo um bloco demora e soma-o à fase indicada.
///
/// `let claro = cronometrar(&mut fases.decifra, || open_chunk(...))?;`
pub fn cronometrar<T>(fase: &mut Duration, f: impl FnOnce() -> T) -> T {
    let inicio = std::time::Instant::now();
    let r = f();
    *fase += inicio.elapsed();
    r
}
