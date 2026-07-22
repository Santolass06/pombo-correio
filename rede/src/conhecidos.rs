//! Registo dos dispositivos já vistos.
//!
//! Serve uma pergunta só: *já recebi deste antes?* Se sim, não vale a pena
//! chatear ninguém com verificações. Se não, mostra-se o número de segurança
//! para ser confirmado com quem enviou.
//!
//! A chave é a impressão digital, nunca o nome. O nome é escolhido pelo dono do
//! aparelho e muda quando lhe apetecer; a impressão digital deriva das chaves e
//! só muda se o dispositivo for outro — que é precisamente o caso que interessa
//! detetar.

use std::{collections::HashMap, path::PathBuf};

use anyhow::{Context, Result};
use pombo_correio_core::identity::PublicIdentity;

pub struct Conhecidos {
    caminho: PathBuf,
    /// Impressão digital em hexadecimal → nome com que se apresentou.
    mapa: HashMap<String, String>,
}

impl Conhecidos {
    /// Lê o registo. Um ficheiro em falta é um registo vazio, não um erro: na
    /// primeira utilização é exatamente o que se espera.
    ///
    /// Um ficheiro ilegível também não impede receber — perde-se a memória de
    /// quem já cá passou, o que é um incómodo, não uma falha.
    pub fn carregar(caminho: PathBuf) -> Self {
        let mapa = std::fs::read(&caminho)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self { caminho, mapa }
    }

    /// Com que nome este dispositivo se apresentou da última vez, se já cá veio.
    pub fn nome_anterior(&self, quem: &PublicIdentity) -> Option<&str> {
        self.mapa.get(&hex::encode(quem.fingerprint())).map(String::as_str)
    }

    /// Grava o dispositivo como conhecido.
    pub fn registar(&mut self, quem: &PublicIdentity) -> Result<()> {
        self.mapa
            .insert(hex::encode(quem.fingerprint()), quem.label.clone());

        if let Some(dir) = self.caminho.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&self.caminho, serde_json::to_vec_pretty(&self.mapa)?)
            .with_context(|| format!("não consegui gravar {}", self.caminho.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pombo_correio_core::identity::Identity;

    fn temp(nome: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pombo-conh-{nome}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d.join("conhecidos.json")
    }

    #[test]
    fn desconhecido_ate_ser_registado() {
        let c = temp("novo");
        let quem = Identity::generate("Telemóvel do avô").public();

        let mut reg = Conhecidos::carregar(c.clone());
        assert_eq!(reg.nome_anterior(&quem), None);

        reg.registar(&quem).unwrap();
        assert_eq!(reg.nome_anterior(&quem), Some("Telemóvel do avô"));

        // E sobrevive a uma nova leitura do disco.
        assert_eq!(
            Conhecidos::carregar(c.clone()).nome_anterior(&quem),
            Some("Telemóvel do avô")
        );
        let _ = std::fs::remove_dir_all(c.parent().unwrap());
    }

    #[test]
    fn a_identidade_manda_e_nao_o_nome() {
        // Mudar o nome do aparelho não o torna um estranho...
        let c = temp("renome");
        let id = Identity::generate("PC");
        let mut reg = Conhecidos::carregar(c.clone());
        reg.registar(&id.public()).unwrap();

        let mut renomeado = id.public();
        renomeado.label = "PC da sala".into();
        assert!(reg.nome_anterior(&renomeado).is_some());

        // ...mas outro aparelho com o mesmo nome continua a ser um estranho.
        let impostor = Identity::generate("PC").public();
        assert_eq!(reg.nome_anterior(&impostor), None);
        let _ = std::fs::remove_dir_all(c.parent().unwrap());
    }

    #[test]
    fn ficheiro_corrompido_nao_impede_receber() {
        let c = temp("lixo");
        std::fs::create_dir_all(c.parent().unwrap()).unwrap();
        std::fs::write(&c, b"isto nao e json").unwrap();

        let reg = Conhecidos::carregar(c.clone());
        assert_eq!(reg.nome_anterior(&Identity::generate("x").public()), None);
        let _ = std::fs::remove_dir_all(c.parent().unwrap());
    }
}
