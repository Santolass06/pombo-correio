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

/// O que se guarda de cada dispositivo.
///
/// O formato antigo só tinha o nome. Pelo relay não chega: para embrulhar a
/// chave da transferência é preciso a **chave pública** de quem recebe, e fora
/// da rede local não há a quem a pedir — o relay não a pode saber, senão abria
/// os ficheiros. Daí guardá-la aqui.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
enum Registado {
    /// Formato atual.
    Completo(PublicIdentity),
    /// Formato antigo: só o nome. Lê-se para não perder a memória de quem já cá
    /// passou; sobe a `Completo` no primeiro reencontro.
    SoNome(String),
}

impl Registado {
    fn nome(&self) -> &str {
        match self {
            Registado::Completo(p) => &p.label,
            Registado::SoNome(n) => n,
        }
    }
}

/// O ficheiro dos que só receberam de nós.
///
/// Ver [`chaves_para`]: é um registo à parte de propósito, e o preço de o
/// juntar ao `conhecidos.json` seria um buraco de segurança.
pub const VISTOS: &str = "vistos.json";

/// Registo de dispositivos, guardado num ficheiro.
///
/// Serve dois ficheiros com o mesmo código, porque a diferença entre eles não
/// está na forma — está em **quem escreve**:
///
/// - `conhecidos.json`: escrito por quem **recebe**. Responde a *"já recebi
///   deste?"*, e é essa resposta que decide mostrar o número de segurança.
/// - `vistos.json`: escrito por quem **envia**, e só depois de um envio direto
///   ter acabado bem. Responde a *"sei as chaves dele?"*, e só serve para
///   embrulhar a chave num depósito no relay.
pub struct Conhecidos {
    caminho: PathBuf,
    /// Impressão digital em hexadecimal → o que se sabe dele.
    mapa: HashMap<String, Registado>,
}

/// A chave pública de quem tem esta impressão curta, venha de onde vier.
///
/// Procura primeiro em quem já nos enviou e só depois em quem só recebeu de
/// nós. A ordem não muda o resultado — a impressão digital deriva das chaves,
/// por isso as duas respostas ou coincidem ou não existem.
///
/// É o que permite deixar um ficheiro no relay para alguém que ainda nunca nos
/// mandou nada — o caso normal de quem acabou de instalar a app.
pub fn chaves_para(dados: &std::path::Path, impressao_curta: &str) -> Option<PublicIdentity> {
    Conhecidos::carregar(dados.join("conhecidos.json"))
        .identidade(impressao_curta)
        .or_else(|| Conhecidos::carregar(dados.join(VISTOS)).identidade(impressao_curta))
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
        self.mapa.get(&hex::encode(quem.fingerprint())).map(Registado::nome)
    }

    /// A identidade pública de quem tem esta impressão digital curta.
    ///
    /// É o que torna possível enviar pelo relay: sem a chave pública não há
    /// envelope, e sem envelope o ficheiro teria de ir em claro. Devolve `None`
    /// para quem só cá está no formato antigo — nesse caso só se envia depois de
    /// voltar a haver contacto direto, e é isso que se diz a quem envia.
    pub fn identidade(&self, impressao_curta: &str) -> Option<PublicIdentity> {
        self.mapa.iter().find_map(|(fp, r)| match r {
            Registado::Completo(p) if fp.starts_with(impressao_curta) => Some(p.clone()),
            _ => None,
        })
    }

    /// Grava o dispositivo como conhecido.
    pub fn registar(&mut self, quem: &PublicIdentity) -> Result<()> {
        self.mapa.insert(
            hex::encode(quem.fingerprint()),
            Registado::Completo(quem.clone()),
        );

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
    fn o_formato_antigo_continua_a_ler_se() {
        // Ficheiros de antes de o relay existir só tinham o nome. Perder a
        // memória de quem já cá passou faria o número de segurança reaparecer
        // para toda a gente — e ensinar a ignorá-lo é o pior que se lhe pode
        // fazer.
        let c = temp("antigo");
        std::fs::create_dir_all(c.parent().unwrap()).unwrap();
        let quem = Identity::generate("Telemóvel do avô").public();
        let fp = hex::encode(quem.fingerprint());
        std::fs::write(&c, format!(r#"{{"{fp}": "Telemóvel do avô"}}"#)).unwrap();

        let mut reg = Conhecidos::carregar(c.clone());
        assert_eq!(reg.nome_anterior(&quem), Some("Telemóvel do avô"));
        // Mas sem chave pública não se pode enviar pelo relay.
        assert!(reg.identidade(&fp[..16]).is_none());

        // No reencontro seguinte, sobe a completo.
        reg.registar(&quem).unwrap();
        assert!(Conhecidos::carregar(c.clone()).identidade(&fp[..16]).is_some());
        let _ = std::fs::remove_dir_all(c.parent().unwrap());
    }

    #[test]
    fn a_chave_publica_volta_pela_impressao_curta() {
        // É esta a peça que torna possível enviar pelo relay: fora da rede local
        // não há a quem pedir a chave, e o relay não a pode saber.
        let c = temp("chave");
        let quem = Identity::generate("Tablet").public();
        let mut reg = Conhecidos::carregar(c.clone());
        reg.registar(&quem).unwrap();

        let curta = &hex::encode(quem.fingerprint())[..16];
        assert_eq!(Conhecidos::carregar(c.clone()).identidade(curta), Some(quem));
        assert!(reg.identidade("0000000000000000").is_none());
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
