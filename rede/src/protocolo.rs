//! Tipos que atravessam a rede.
//!
//! O modelo é sempre o mesmo, seja o outro lado um dispositivo na LAN, pelo
//! Tailscale, ou o relay de casa: quem recebe expõe um servidor, quem envia
//! empurra. Manter uma só direção evita ter duas implementações de servidor.

use pombo_correio_core::{crypto::Envelope, identity::PublicIdentity};
use serde::{Deserialize, Serialize};

/// Proposta de transferência: o envelope com a chave, e o manifesto selado.
///
/// O manifesto vai selado porque traz o nome do ficheiro. O envelope vai em
/// claro — é a chave embrulhada e só o destinatário a consegue abrir.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Oferta {
    pub de: PublicIdentity,
    pub envelope: Envelope,
    pub manifesto_selado: Vec<u8>,
    /// Assinatura de quem envia sobre o manifesto selado.
    #[serde(with = "sig")]
    pub assinatura: [u8; 64],
    /// Porta do servidor de quem envia, para quem recebe o poder avisar de um
    /// cancelamento. Zero se não tiver servidor (o CLI a enviar), e então não se
    /// avisa — nada mau, o CLI interrompe-se com Ctrl+C.
    #[serde(default)]
    pub porta_resposta: u16,
}

/// Resposta a uma oferta.
#[derive(Debug, Serialize, Deserialize)]
pub struct Aceite {
    /// Identifica a transferência. É a raiz de Merkle em hexadecimal, por isso
    /// duas tentativas do mesmo ficheiro reencontram o mesmo trabalho a meio.
    pub id: String,
    /// Blocos que quem recebe já tem verificados. Zero numa transferência nova.
    pub tenho: u32,
}

/// O `serde` não trata arrays de 64 por omissão; passa por Vec na fronteira.
mod sig {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &[u8; 64], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex::encode(v))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 64], D::Error> {
        let s = String::deserialize(d)?;
        let v = hex::decode(&s).map_err(serde::de::Error::custom)?;
        v.try_into()
            .map_err(|_| serde::de::Error::custom("assinatura não tem 64 bytes"))
    }
}
