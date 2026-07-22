use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("chave inválida: {0}")]
    InvalidKey(String),

    #[error("assinatura inválida")]
    BadSignature,

    #[error("falha ao decifrar — chave errada ou dados corrompidos")]
    Decrypt,

    #[error("bloco {index} não corresponde ao hash esperado")]
    ChunkMismatch { index: u32 },

    #[error("a raiz de Merkle não corresponde ao manifesto")]
    RootMismatch,

    #[error("manifesto inválido: {0}")]
    BadManifest(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
