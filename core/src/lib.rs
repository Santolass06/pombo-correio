//! Núcleo do Pombo-Correio — protocolo, identidade, cifra e endereçamento por conteúdo.
//!
//! Este crate não sabe nada sobre interface gráfica nem sobre o relay. É
//! partilhado por todos os alvos: cliente desktop, Android, o relay, e — via
//! `wasm32` — a PWA de iOS. Manter esta camada livre de I/O de rede é o que
//! permite que o iPhone corra exatamente a mesma lógica que o Linux.

pub mod chunk;
pub mod error;
pub mod identity;

pub use error::{Error, Result};
