//! Parte do protocolo do relay que cliente e relay têm de acordar byte a byte:
//! a mensagem que se assina em cada pedido.
//!
//! Vive no `core/` (sem I/O) para que o relay, o cliente e — um dia — a PWA em
//! WASM assinem e verifiquem exatamente a mesma coisa.

use crate::identity::{Identity, PublicIdentity};

/// Quanto o relógio de quem pede pode diferir do relay, em segundos. Pedidos
/// fora desta janela são recusados, o que limita a repetição de um pedido
/// capturado.
pub const JANELA_S: u64 = 120;

pub const CABECALHO_QUEM: &str = "x-pombo-quem";
pub const CABECALHO_INSTANTE: &str = "x-pombo-instante";
pub const CABECALHO_ASSINATURA: &str = "x-pombo-assinatura";

/// A mensagem assinada: método, caminho **com** query, instante e hash do corpo.
///
/// O caminho entra inteiro para que uma assinatura de `GET /caixa` não sirva
/// para `DELETE /deposito/x`, e o instante para que não sirva amanhã.
pub fn mensagem(metodo: &str, caminho: &str, instante: u64, corpo: &[u8]) -> Vec<u8> {
    format!(
        "pombo-relay-v1\n{metodo}\n{caminho}\n{instante}\n{}",
        blake3::hash(corpo).to_hex()
    )
    .into_bytes()
}

/// Os três cabeçalhos de um pedido assinado, por esta ordem:
/// (quem, instante, assinatura).
pub fn assinar_pedido(
    eu: &Identity,
    metodo: &str,
    caminho: &str,
    instante: u64,
    corpo: &[u8],
) -> (String, String, String) {
    let sig = eu.sign(&mensagem(metodo, caminho, instante, corpo));
    (
        hex::encode(eu.public().fingerprint()),
        instante.to_string(),
        hex::encode(sig),
    )
}

/// Verifica um pedido assinado por `quem`. O instante tem de estar na janela.
pub fn verificar_pedido(
    quem: &PublicIdentity,
    metodo: &str,
    caminho: &str,
    instante: u64,
    assinatura_hex: &str,
    corpo: &[u8],
    agora: u64,
) -> bool {
    if agora.abs_diff(instante) > JANELA_S {
        return false;
    }
    let Ok(bytes) = hex::decode(assinatura_hex) else { return false };
    let Ok(sig) = <[u8; 64]>::try_from(bytes) else { return false };
    quem.verify(&mensagem(metodo, caminho, instante, corpo), &sig).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assinatura_boa_passa_e_cada_campo_adulterado_falha() {
        let eu = Identity::generate("a");
        let (_, inst, sig) = assinar_pedido(&eu, "GET", "/caixa", 1000, b"");
        let p = eu.public();
        assert!(verificar_pedido(&p, "GET", "/caixa", 1000, &sig, b"", 1000));
        // Cada campo da mensagem tem de contar.
        assert!(!verificar_pedido(&p, "POST", "/caixa", 1000, &sig, b"", 1000));
        assert!(!verificar_pedido(&p, "GET", "/outro", 1000, &sig, b"", 1000));
        assert!(!verificar_pedido(&p, "GET", "/caixa", 1001, &sig, b"", 1001));
        assert!(!verificar_pedido(&p, "GET", "/caixa", 1000, &sig, b"x", 1000));
        assert_eq!(inst, "1000");
    }

    #[test]
    fn instante_fora_da_janela_e_recusado() {
        let eu = Identity::generate("a");
        let (_, _, sig) = assinar_pedido(&eu, "GET", "/caixa", 1000, b"");
        let p = eu.public();
        assert!(verificar_pedido(&p, "GET", "/caixa", 1000, &sig, b"", 1000 + JANELA_S));
        assert!(!verificar_pedido(&p, "GET", "/caixa", 1000, &sig, b"", 1000 + JANELA_S + 1));
        assert!(!verificar_pedido(&p, "GET", "/caixa", 1000, &sig, b"", 1000 - JANELA_S - 1));
    }

    #[test]
    fn outra_chave_e_recusada() {
        let a = Identity::generate("a");
        let b = Identity::generate("b");
        let (_, _, sig) = assinar_pedido(&a, "GET", "/caixa", 5, b"");
        assert!(!verificar_pedido(&b.public(), "GET", "/caixa", 5, &sig, b"", 5));
    }
}
