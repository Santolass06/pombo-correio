//! Identidade de dispositivo.
//!
//! Cada dispositivo detém dois pares de chaves:
//!
//! - **Ed25519** para *assinar* — prova de quem enviou o quê.
//! - **X25519** para *cifrar* — envelope das chaves de transferência.
//!
//! São separados de propósito: usar a mesma chave para assinatura e acordo de
//! chaves é possível mas frágil, e a conversão Ed25519→X25519 abre um flanco
//! desnecessário quando gerar dois pares custa microssegundos.
//!
//! A impressão digital pública é apresentada ao utilizador em grupos numéricos
//! ao estilo do Signal, e não em hexadecimal — o objetivo é que dois familiares
//! consigam compará-la em voz alta ao telefone.

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use x25519_dalek::{PublicKey as XPublicKey, StaticSecret};

use crate::{Error, Result};

/// A parte pública de uma identidade — o que se troca ao emparelhar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicIdentity {
    /// Chave de verificação Ed25519.
    pub verifying: [u8; 32],
    /// Chave pública X25519.
    pub encrypting: [u8; 32],
    /// Nome legível escolhido pelo utilizador ("Telemóvel do avô").
    pub label: String,
}

impl PublicIdentity {
    /// Impressão digital estável, derivada de **ambas** as chaves.
    ///
    /// Incluir as duas é deliberado: se só cobrisse a de assinatura, um atacante
    /// poderia trocar a chave de cifra mantendo a impressão digital intacta.
    pub fn fingerprint(&self) -> [u8; 32] {
        let mut h = blake3::Hasher::new();
        h.update(b"pombo-id-v1");
        h.update(&self.verifying);
        h.update(&self.encrypting);
        *h.finalize().as_bytes()
    }

    /// Impressão digital em seis grupos de cinco dígitos, para leitura em voz alta.
    pub fn safety_number(&self) -> String {
        let fp = self.fingerprint();
        fp.chunks(4)
            .take(6)
            .map(|w| {
                let n = u32::from_be_bytes([w[0], w[1], w[2], w[3]]);
                format!("{:05}", n % 100_000)
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn verifying_key(&self) -> Result<VerifyingKey> {
        VerifyingKey::from_bytes(&self.verifying)
            .map_err(|e| Error::InvalidKey(format!("Ed25519: {e}")))
    }

    pub fn encrypting_key(&self) -> XPublicKey {
        XPublicKey::from(self.encrypting)
    }

    /// Verifica uma assinatura produzida por este dispositivo.
    pub fn verify(&self, msg: &[u8], sig: &[u8; 64]) -> Result<()> {
        let vk = self.verifying_key()?;
        vk.verify(msg, &Signature::from_bytes(sig))
            .map_err(|_| Error::BadSignature)
    }
}

/// Identidade completa, com as partes secretas. Nunca sai do dispositivo.
pub struct Identity {
    signing: SigningKey,
    secret: StaticSecret,
    pub label: String,
}

impl Identity {
    /// Gera uma identidade nova a partir do CSPRNG do sistema.
    pub fn generate(label: impl Into<String>) -> Self {
        let mut rng = rand::rng();
        Self {
            signing: SigningKey::generate(&mut rng),
            secret: StaticSecret::random_from_rng(&mut rng),
            label: label.into(),
        }
    }

    /// Reconstrói a partir do material guardado em disco.
    pub fn from_secret_bytes(signing: [u8; 32], secret: [u8; 32], label: impl Into<String>) -> Self {
        Self {
            signing: SigningKey::from_bytes(&signing),
            secret: StaticSecret::from(secret),
            label: label.into(),
        }
    }

    /// Material secreto, para persistir. Trata-o como tal.
    pub fn secret_bytes(&self) -> ([u8; 32], [u8; 32]) {
        (self.signing.to_bytes(), self.secret.to_bytes())
    }

    pub fn public(&self) -> PublicIdentity {
        PublicIdentity {
            verifying: self.signing.verifying_key().to_bytes(),
            encrypting: XPublicKey::from(&self.secret).to_bytes(),
            label: self.label.clone(),
        }
    }

    pub fn sign(&self, msg: &[u8]) -> [u8; 64] {
        self.signing.sign(msg).to_bytes()
    }

    /// Segredo partilhado Diffie-Hellman com outro dispositivo.
    ///
    /// O resultado bruto nunca deve ser usado como chave — passa sempre por
    /// [`Identity::shared_key`], que aplica derivação.
    fn dh(&self, peer: &XPublicKey) -> [u8; 32] {
        self.secret.diffie_hellman(peer).to_bytes()
    }

    /// Chave simétrica derivada, ligada a um contexto específico.
    ///
    /// O `context` separa domínios: a chave que embrulha uma transferência não
    /// pode ser reutilizável noutro ponto do protocolo.
    pub fn shared_key(&self, peer: &PublicIdentity, context: &str) -> [u8; 32] {
        let shared = self.dh(&peer.encrypting_key());

        // Ordenar as duas chaves públicas torna a derivação simétrica: ambos os
        // lados chegam ao mesmo valor sem negociar quem é "o primeiro".
        let (mine, theirs) = (self.public().encrypting, peer.encrypting);
        let (a, b) = if mine <= theirs { (mine, theirs) } else { (theirs, mine) };

        let mut h = blake3::Hasher::new_derive_key("pombo-dh-v1");
        h.update(&shared);
        h.update(context.as_bytes());
        h.update(&a);
        h.update(&b);
        *h.finalize().as_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assinatura_valida_e_deteta_adulteracao() {
        let id = Identity::generate("portátil");
        let pubk = id.public();
        let sig = id.sign("olá".as_bytes());

        assert!(pubk.verify("olá".as_bytes(), &sig).is_ok());
        assert!(
            pubk.verify("ola".as_bytes(), &sig).is_err(),
            "aceitou mensagem alterada"
        );
    }

    #[test]
    fn outra_identidade_nao_valida_a_assinatura() {
        let a = Identity::generate("a");
        let b = Identity::generate("b");
        assert!(b.public().verify(b"x", &a.sign(b"x")).is_err());
    }

    #[test]
    fn dh_converge_dos_dois_lados() {
        let a = Identity::generate("a");
        let b = Identity::generate("b");

        assert_eq!(
            a.shared_key(&b.public(), "transfer"),
            b.shared_key(&a.public(), "transfer"),
            "os dois lados têm de derivar a mesma chave"
        );
    }

    #[test]
    fn contextos_diferentes_dao_chaves_diferentes() {
        let a = Identity::generate("a");
        let b = Identity::generate("b");
        assert_ne!(
            a.shared_key(&b.public(), "transfer"),
            a.shared_key(&b.public(), "pairing"),
        );
    }

    #[test]
    fn persistencia_preserva_a_identidade() {
        let id = Identity::generate("pi");
        let (s, e) = id.secret_bytes();
        let restored = Identity::from_secret_bytes(s, e, "pi");
        assert_eq!(id.public(), restored.public());
    }

    #[test]
    fn safety_number_tem_forma_legivel() {
        let n = Identity::generate("x").public().safety_number();
        let groups: Vec<_> = n.split(' ').collect();
        assert_eq!(groups.len(), 6);
        assert!(groups.iter().all(|g| g.len() == 5 && g.chars().all(|c| c.is_ascii_digit())));
    }

    #[test]
    fn fingerprint_cobre_a_chave_de_cifra() {
        // Trocar só a chave de cifra tem de mudar a impressão digital, senão o
        // emparelhamento verificado em voz alta não protege contra substituição.
        let mut p = Identity::generate("x").public();
        let before = p.fingerprint();
        p.encrypting = Identity::generate("y").public().encrypting;
        assert_ne!(before, p.fingerprint());
    }
}
