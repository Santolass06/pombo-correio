//! Onde ficam as subscrições de push, uma por dispositivo (impressão digital).
//!
//! Poucas dezenas de entradas, quando muito — um ficheiro JSON reescrito por
//! inteiro a cada mudança chega e sobra; a mesma escolha do `conhecidos.json`
//! nativo. Sem fila, sem base de dados.

use std::{collections::HashMap, path::PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chaves {
    pub p256dh: String,
    pub auth: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subscricao {
    pub endpoint: String,
    pub keys: Chaves,
}

pub struct Subscricoes {
    caminho: PathBuf,
    mapa: Mutex<HashMap<String, Subscricao>>,
}

impl Subscricoes {
    pub fn carregar(dados: &std::path::Path) -> Result<Self> {
        let caminho = dados.join("subscricoes.json");
        let mapa = match std::fs::read(&caminho) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(_) => HashMap::new(),
        };
        Ok(Self { caminho, mapa: Mutex::new(mapa) })
    }

    /// Substitui a subscrição de `impressao` pela mais recente — o browser
    /// pode trocar de `endpoint` a qualquer momento (rotação do serviço de
    /// push), e não há aqui histórico nenhum que valha a pena guardar.
    pub async fn guardar(&self, impressao: &str, sub: Subscricao) -> Result<()> {
        let mut mapa = self.mapa.lock().await;
        mapa.insert(impressao.to_string(), sub);
        std::fs::write(&self.caminho, serde_json::to_vec(&*mapa)?)?;
        Ok(())
    }

    pub async fn obter(&self, impressao: &str) -> Option<Subscricao> {
        self.mapa.lock().await.get(impressao).cloned()
    }

    /// Chamado quando o serviço de push diz que o endpoint morreu (410/404):
    /// sem isto, cada depósito futuro tentava a mesma entrega já recusada.
    pub async fn remover(&self, impressao: &str) -> Result<()> {
        let mut mapa = self.mapa.lock().await;
        if mapa.remove(impressao).is_some() {
            std::fs::write(&self.caminho, serde_json::to_vec(&*mapa)?)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    fn temporaria(nome: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("pombo-subs-{}-{nome}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn exemplo() -> Subscricao {
        Subscricao {
            endpoint: "https://push.exemplo.org/abc".into(),
            keys: Chaves { p256dh: "chave".into(), auth: "segredo".into() },
        }
    }

    #[tokio::test]
    async fn guarda_e_relê_do_disco() {
        let d = temporaria("relê");
        let s = Subscricoes::carregar(&d).unwrap();
        s.guardar("ab12", exemplo()).await.unwrap();

        let relido = Subscricoes::carregar(&d).unwrap();
        assert_eq!(relido.obter("ab12").await.unwrap().endpoint, exemplo().endpoint);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[tokio::test]
    async fn remover_o_que_nao_existe_nao_falha() {
        let d = temporaria("remover");
        let s = Subscricoes::carregar(&d).unwrap();
        s.remover("nada-aqui").await.unwrap();
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[tokio::test]
    async fn subscrever_de_novo_substitui_a_anterior() {
        let d = temporaria("substitui");
        let s = Subscricoes::carregar(&d).unwrap();
        s.guardar("ab12", exemplo()).await.unwrap();
        let mut nova = exemplo();
        nova.endpoint = "https://push.exemplo.org/outro".into();
        s.guardar("ab12", nova.clone()).await.unwrap();
        assert_eq!(s.obter("ab12").await.unwrap().endpoint, nova.endpoint);
        std::fs::remove_dir_all(&d).unwrap();
    }
}
