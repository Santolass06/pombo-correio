//! Relay do Pombo-Correio: guarda ficheiros cifrados até quem os espera os vir
//! buscar.
//!
//! Existe para o caso em que os dois dispositivos não se alcançam — redes
//! diferentes, ou o destinatário simplesmente desligado. **Só vê texto
//! cifrado**, e é essa a razão de ser desta forma: é a única peça exposta à
//! internet, e comprometê-la não expõe ficheiro nenhum.

pub mod autenticado;
pub mod deposito;
pub mod fcm;
pub mod push;
pub mod servidor;
pub mod subscricoes;
pub mod vapid;
