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
pub mod protocolo;
pub mod receber;

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

/// Para onde vão os acontecimentos.
pub type Ouvinte = Arc<dyn Fn(Evento) + Send + Sync>;

/// Um ouvinte que deita tudo fora, para testes e para quem não quer saber.
pub fn silencio() -> Ouvinte {
    Arc::new(|_| {})
}
