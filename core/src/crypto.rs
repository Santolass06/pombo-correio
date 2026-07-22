//! Cifra de transferências.
//!
//! Cada transferência tem uma chave aleatória própria. Os blocos são selados
//! um a um com essa chave, e a chave é embrulhada para a chave pública X25519
//! de cada destinatário.
//!
//! É esta separação que permite ao relay guardar ficheiros sem os poder ler:
//! ele vê blocos selados e um envelope que não sabe abrir.

use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use serde::{Deserialize, Serialize};
use x25519_dalek::{EphemeralSecret, PublicKey as XPublicKey};

use crate::{Error, Result, identity::{Identity, PublicIdentity}};

/// Bytes do CSPRNG do sistema.
///
/// Falhar aqui significa que o sistema operativo não consegue dar aleatoriedade
/// — não há nada de sensato a fazer a seguir, e continuar com material fraco
/// seria pior do que parar.
fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).expect("o CSPRNG do sistema falhou");
    b
}

/// Chave simétrica de uma transferência. Vive só o tempo da transferência.
#[derive(Clone)]
pub struct TransferKey([u8; 32]);

impl TransferKey {
    pub fn generate() -> Self {
        Self(random_bytes())
    }

    fn cipher(&self) -> XChaCha20Poly1305 {
        XChaCha20Poly1305::new((&self.0).into())
    }
}

/// A chave de uma transferência, embrulhada para um destinatário.
///
/// Acompanha o manifesto. Um envelope por destinatário; os blocos selados são
/// partilhados por todos, porque a chave que os abre é a mesma.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    /// Chave pública efémera de quem enviou.
    pub ephemeral: [u8; 32],
    pub nonce: [u8; 24],
    /// Chave de transferência cifrada (32 bytes + 16 de autenticação).
    pub sealed: Vec<u8>,
}

/// Deriva a chave que embrulha o envelope, a partir do segredo Diffie-Hellman.
///
/// As duas chaves públicas entram na derivação para ligar a chave resultante a
/// este par concreto — sem isso, o mesmo segredo poderia ser reaproveitado
/// noutro contexto.
fn wrap_key(shared: &[u8; 32], ephemeral: &[u8; 32], recipient: &[u8; 32]) -> [u8; 32] {
    let mut h = blake3::Hasher::new_derive_key("pombo-envelope-v1");
    h.update(shared);
    h.update(ephemeral);
    h.update(recipient);
    *h.finalize().as_bytes()
}

impl Envelope {
    /// Embrulha uma chave de transferência para um destinatário.
    ///
    /// Usa um par efémero em vez da chave estática de quem envia, à maneira do
    /// `age`. Isto dá sigilo futuro: o segredo efémero é destruído aqui, por
    /// isso quem venha a roubar a chave estática do destinatário mais tarde não
    /// consegue reabrir o que já passou pelo relay.
    pub fn seal(key: &TransferKey, recipient: &PublicIdentity) -> Self {
        let secret = EphemeralSecret::random_from_rng(&mut rand::rng());
        let ephemeral = XPublicKey::from(&secret).to_bytes();
        let shared = secret.diffie_hellman(&recipient.encrypting_key()).to_bytes();

        let wrapping = wrap_key(&shared, &ephemeral, &recipient.encrypting);
        let cipher = XChaCha20Poly1305::new((&wrapping).into());
        let nonce: [u8; 24] = random_bytes();

        let sealed = cipher
            .encrypt(&XNonce::from(nonce), key.0.as_slice())
            .expect("selar 32 bytes não pode falhar");

        Self { ephemeral, nonce, sealed }
    }

    /// Abre o envelope com a chave privada do destinatário.
    pub fn open(&self, me: &Identity) -> Result<TransferKey> {
        let shared = me.dh_with(&XPublicKey::from(self.ephemeral));
        let wrapping = wrap_key(&shared, &self.ephemeral, &me.public().encrypting);

        let plain = XChaCha20Poly1305::new((&wrapping).into())
            .decrypt(&XNonce::from(self.nonce), self.sealed.as_slice())
            .map_err(|_| Error::Decrypt)?;

        let bytes: [u8; 32] = plain.as_slice().try_into().map_err(|_| Error::Decrypt)?;
        Ok(TransferKey(bytes))
    }
}

/// Quanto é que selar acrescenta: 24 de nonce + 16 de autenticação.
pub const SEAL_OVERHEAD: usize = 24 + 16;

/// O nonce vai à frente do texto cifrado. Com os 192 bits do XChaCha20, gerá-lo
/// ao acaso é seguro, o que dispensa gerir contadores e o risco de os repetir —
/// risco que a retoma tornaria real, porque o contador teria de sobreviver
/// entre sessões.
fn seal_with(key: &TransferKey, aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
    let nonce: [u8; 24] = random_bytes();
    let ct = key
        .cipher()
        .encrypt(&XNonce::from(nonce), Payload { msg: plaintext, aad })
        .expect("selar não pode falhar");

    let mut out = Vec::with_capacity(24 + ct.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    out
}

fn open_with(key: &TransferKey, aad: &[u8], sealed: &[u8]) -> Result<Vec<u8>> {
    if sealed.len() < SEAL_OVERHEAD {
        return Err(Error::Decrypt);
    }
    let (nonce, ct) = sealed.split_at(24);
    let nonce: [u8; 24] = nonce.try_into().expect("fatia de 24 já verificada");

    key.cipher()
        .decrypt(&XNonce::from(nonce), Payload { msg: ct, aad })
        .map_err(|_| Error::Decrypt)
}

/// Sela um bloco, ligando-o ao seu índice.
///
/// O índice entra como dados associados: fica autenticado sem ser cifrado, e é
/// o que impede trocar de posição blocos já selados sem os conseguir abrir.
pub fn seal_chunk(key: &TransferKey, index: u32, plaintext: &[u8]) -> Vec<u8> {
    seal_with(key, &index.to_le_bytes(), plaintext)
}

/// Abre um bloco selado, verificando que corresponde ao índice indicado.
pub fn open_chunk(key: &TransferKey, index: u32, sealed: &[u8]) -> Result<Vec<u8>> {
    open_with(key, &index.to_le_bytes(), sealed)
}

/// Dados associados do manifesto. Distintos de qualquer índice de bloco, para
/// que um manifesto selado nunca possa ser aceite no lugar de um bloco.
const AAD_MANIFEST: &[u8] = b"pombo-manifest-v1";

/// Sela o manifesto.
///
/// O manifesto traz o nome do ficheiro. Sem o selar, o nome viajaria em claro
/// mesmo com os blocos cifrados — e o nome costuma dizer mais sobre o conteúdo
/// do que os próprios bytes.
pub fn seal_manifest(key: &TransferKey, plaintext: &[u8]) -> Vec<u8> {
    seal_with(key, AAD_MANIFEST, plaintext)
}

pub fn open_manifest(key: &TransferKey, sealed: &[u8]) -> Result<Vec<u8>> {
    open_with(key, AAD_MANIFEST, sealed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bloco_selado_reabre_igual() {
        let k = TransferKey::generate();
        let dados = b"uma foto qualquer";
        assert_eq!(open_chunk(&k, 7, &seal_chunk(&k, 7, dados)).unwrap(), dados);
    }

    #[test]
    fn chave_errada_nao_abre() {
        let selado = seal_chunk(&TransferKey::generate(), 0, b"x");
        assert!(open_chunk(&TransferKey::generate(), 0, &selado).is_err());
    }

    #[test]
    fn indice_errado_nao_abre() {
        // O que impede trocar blocos de posição já selados.
        let k = TransferKey::generate();
        assert!(open_chunk(&k, 1, &seal_chunk(&k, 0, b"x")).is_err());
    }

    #[test]
    fn bloco_adulterado_nao_abre() {
        let k = TransferKey::generate();
        let mut selado = seal_chunk(&k, 0, b"conteudo importante");
        let ultimo = selado.len() - 1;
        selado[ultimo] ^= 1;
        assert!(open_chunk(&k, 0, &selado).is_err());
    }

    #[test]
    fn selado_truncado_nao_rebenta() {
        let k = TransferKey::generate();
        assert!(open_chunk(&k, 0, b"curto").is_err());
    }

    #[test]
    fn selar_duas_vezes_da_resultados_diferentes() {
        // Nonce aleatório por bloco: dois selos do mesmo conteúdo não podem
        // coincidir, senão dava para inferir repetições dentro do ficheiro.
        let k = TransferKey::generate();
        assert_ne!(seal_chunk(&k, 0, b"igual"), seal_chunk(&k, 0, b"igual"));
    }

    #[test]
    fn envelope_entrega_a_chave_ao_destinatario() {
        let destino = Identity::generate("telemóvel");
        let chave = TransferKey::generate();

        let recuperada = Envelope::seal(&chave, &destino.public()).open(&destino).unwrap();

        // A prova é funcional: um bloco selado com a original abre com a recuperada.
        let selado = seal_chunk(&chave, 0, b"prova");
        assert_eq!(open_chunk(&recuperada, 0, &selado).unwrap(), b"prova");
    }

    #[test]
    fn envelope_de_outro_nao_abre() {
        let destino = Identity::generate("destino");
        let intruso = Identity::generate("intruso");
        let env = Envelope::seal(&TransferKey::generate(), &destino.public());
        assert!(env.open(&intruso).is_err());
    }

    #[test]
    fn envelope_adulterado_nao_abre() {
        let destino = Identity::generate("destino");
        let mut env = Envelope::seal(&TransferKey::generate(), &destino.public());
        env.ephemeral[0] ^= 1;
        assert!(env.open(&destino).is_err());
    }

    #[test]
    fn cada_envelope_usa_um_efemero_novo() {
        // Se o efémero se repetisse, perdia-se o sigilo futuro.
        let d = Identity::generate("d").public();
        let a = Envelope::seal(&TransferKey::generate(), &d);
        let b = Envelope::seal(&TransferKey::generate(), &d);
        assert_ne!(a.ephemeral, b.ephemeral);
    }

    #[test]
    fn manifesto_selado_reabre_igual() {
        let k = TransferKey::generate();
        let m = b"{\"name\":\"ferias.mp4\"}";
        assert_eq!(open_manifest(&k, &seal_manifest(&k, m)).unwrap(), m);
    }

    #[test]
    fn manifesto_nao_passa_por_bloco_nem_o_contrario() {
        // Domínios separados: nenhum dos dois pode ser aceite no lugar do outro.
        let k = TransferKey::generate();
        assert!(open_chunk(&k, 0, &seal_manifest(&k, b"x")).is_err());
        assert!(open_manifest(&k, &seal_chunk(&k, 0, b"x")).is_err());
    }

    #[test]
    fn overhead_anunciado_corresponde_ao_real() {
        let k = TransferKey::generate();
        let dados = vec![0u8; 1000];
        assert_eq!(seal_chunk(&k, 0, &dados).len(), dados.len() + SEAL_OVERHEAD);
    }
}
