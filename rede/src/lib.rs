//! Transferência pela rede: descoberta, envio, receção.
//!
//! Separada do `cli` porque a interface gráfica precisa exatamente da mesma
//! lógica. O que muda entre as duas é apenas o que se faz com os
//! acontecimentos — o CLI imprime-os, a interface desenha-os.
//!
//! Por isso esta camada **não escreve no terminal**. Comunica por [`Evento`],
//! entregue a um fecho que quem chama fornece.

pub mod conhecidos;
pub mod descoberta;
pub mod enviar;
pub mod metricas;
pub mod protocolo;
pub mod receber;

pub use metricas::Metricas;

use std::sync::Arc;

use pombo_correio_core::identity::PublicIdentity;

/// Tudo o que acontece durante uma transferência.
///
/// Um só tipo para os dois lados: a interface junta-os no mesmo painel, e
/// separá-los obrigaria a duplicar o encaminhamento sem nada em troca.
#[derive(Debug, Clone)]
pub enum Evento {
    /// O servidor está de pé. `anunciado` é falso se o mDNS falhou — não
    /// impede receber, mas obriga a escrever o endereço à mão.
    AEscutar { porta: u16, anunciado: bool },

    /// Chegou uma proposta de transferência.
    Oferta {
        id: String,
        de: PublicIdentity,
        ficheiro: String,
        tamanho: u64,
        /// Primeira vez que se vê este dispositivo — é quando o número de
        /// segurança deve ser mostrado, e só então.
        novo: bool,
        /// Nome anterior, se o dispositivo é conhecido mas mudou de nome.
        nome_anterior: Option<String>,
        /// Blocos já em disco de uma tentativa anterior.
        retoma: u32,
        total: u32,
    },

    /// Para quem envia: com quem se está a falar.
    Destino { quem: PublicIdentity },

    /// Blocos concluídos até agora, bytes movidos nesta sessão, e a velocidade
    /// em bytes por segundo.
    ///
    /// A velocidade é medida **aqui**, dentro do ciclo de transferência, onde o
    /// tempo é exato. A interface não a pode calcular a partir da chegada dos
    /// eventos: eles atravessam o IPC aos solavancos, e o tempo entre chegadas
    /// não é o tempo real da transferência.
    Progresso {
        id: String,
        feitos: u32,
        total: u32,
        bytes: u64,
        bytes_por_seg: f64,
    },

    /// Ficheiro completo e verificado.
    Recebido { id: String, caminho: std::path::PathBuf },

    /// Envio completo.
    Enviado { id: String },

    /// Cancelado por quem enviava. O que já chegou fica em disco, por isso
    /// retomar continua a valer — cancelar não é perder o trabalho feito.
    Cancelado { id: String },

    /// Correu mal. A transferência pode ser retomada.
    Falhou { id: String, erro: String },
}

/// Origem de um ficheiro a enviar.
///
/// Mantém o `enviar` agnóstico à plataforma: no desktop e no CLI a origem é um
/// caminho no disco; no Android é um URI `content://` do seletor, que não é um
/// caminho mas que o plugin resolve num `std::fs::File`. O `enviar` só precisa
/// de saber o nome e de abrir um leitor fresco — abre duas vezes, uma para o
/// manifesto e outra para o streaming — por isso `abrir` dá sempre um handle
/// novo, posicionado no início.
pub trait Fonte: Send + Sync {
    fn nome(&self) -> String;
    fn abrir(&self) -> std::io::Result<std::fs::File>;
}

/// A origem mais comum: um caminho no disco (desktop, CLI).
pub struct FonteCaminho(pub std::path::PathBuf);

impl Fonte for FonteCaminho {
    fn nome(&self) -> String {
        self.0
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "ficheiro".into())
    }
    fn abrir(&self) -> std::io::Result<std::fs::File> {
        std::fs::File::open(&self.0)
    }
}

/// Sinal de cancelamento partilhado entre quem envia e quem manda parar.
pub type Cancelar = std::sync::Arc<std::sync::atomic::AtomicBool>;

/// Um sinal que nunca dispara, para quem não precisa de cancelar — o CLI
/// interrompe-se com Ctrl+C, que mata o processo.
pub fn sem_cancelamento() -> Cancelar {
    Default::default()
}

/// Registo dos sinais de cancelamento, por identificador de transferência.
///
/// Partilhado entre quem serve (que insere um sinal quando uma transferência
/// começa) e quem manda cancelar (que o aciona pelo identificador). Serve tanto
/// envios como receções — a mesma máquina nunca é as duas pontas da mesma
/// transferência, por isso um identificador é só de uma.
pub type Cancelaveis =
    std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, Cancelar>>>;

/// Porta em que todos os dispositivos escutam. Fora dos intervalos reservados e
/// fácil de decorar.
pub const PORTA_PADRAO: u16 = 53318;

/// Avisa o outro lado de que uma transferência foi cancelada.
///
/// Numa **ligação nova**, de propósito: o cancelamento não pode viajar pela
/// ligação da transferência, que está precisamente a ser cortada — o aviso
/// morreria com ela. Cada dispositivo corre um servidor, por isso quem cancela
/// bate à porta do outro para lho dizer.
///
/// É best-effort: se o outro lado não tiver servidor (o CLI a enviar, por
/// exemplo) ou já tiver desligado, não há nada a fazer e não é erro.
pub(crate) async fn avisar_cancelamento(base: &str, id: &str) {
    let cliente = reqwest::Client::new();
    let url = format!("{base}/cancelar/{id}");
    // Tenta mais do que uma vez: o aviso tem de chegar antes de o outro lado
    // desistir do fluxo cortado, e um único POST pode falhar ou atrasar-se se o
    // servidor do outro lado estiver ocupado a receber. Best-effort — se nenhuma
    // tentativa passar, não é erro (o outro lado pode nem ter servidor).
    for _ in 0..3 {
        if cliente
            .post(&url)
            .timeout(std::time::Duration::from_secs(2))
            .send()
            .await
            .is_ok_and(|r| r.status().is_success())
        {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// Para onde vão os acontecimentos.
pub type Ouvinte = Arc<dyn Fn(Evento) + Send + Sync>;

/// Um ouvinte que deita tudo fora, para testes e para quem não quer saber.
pub fn silencio() -> Ouvinte {
    Arc::new(|_| {})
}
