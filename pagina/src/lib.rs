//! O que corre dentro do browser de quem recebe um link.
//!
//! A razão de existir: **a chave nunca pode passar pelo servidor.** Ela vai no
//! fragmento do URL — a parte depois do `#`, que os browsers não enviam — e é
//! aqui, no dispositivo de quem recebe, que abre o manifesto e os blocos. Um
//! relay que decifrasse por conveniência deitava fora o modelo todo.
//!
//! O JavaScript do outro lado desta fronteira nunca vê a chave nem os hashes:
//! pergunta de quantos bytes é o bloco seguinte, entrega-os, e recebe conteúdo
//! já verificado. Tudo o que decide se algo é legítimo fica deste lado.

use pombo_correio_core::{
    chunk::Manifest,
    crypto::{SEAL_OVERHEAD, TransferKey, open_chunk, open_manifest},
    protocolo::Oferta,
};
use wasm_bindgen::prelude::*;

/// Uma descida em curso: sabe o manifesto e por que bloco vai.
#[wasm_bindgen]
pub struct Descarga {
    chave: TransferKey,
    manifesto: Manifest,
    de: String,
    proximo: u32,
}

/// Erros atravessam como texto: o `JsError` do wasm-bindgen não se constrói fora
/// do wasm, e isso deixava estes testes só correr no browser. A página apanha-os
/// com `try` e mostra a frase tal e qual.
fn erro(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[wasm_bindgen]
impl Descarga {
    /// Prepara a descida a partir da chave do fragmento e da oferta opaca que o
    /// relay guardou.
    ///
    /// Faz as três verificações **antes de se pedir um único byte**, e por esta
    /// ordem: a assinatura de quem enviou (senão estaríamos a decifrar material
    /// de qualquer um), a abertura do manifesto (é aqui que uma chave errada
    /// rebenta, barato), e a coerência interna do manifesto (senão alocava-se
    /// memória a partir de números que vieram da rede).
    #[wasm_bindgen(constructor)]
    pub fn nova(chave_hex: &str, oferta_json: &str) -> Result<Descarga, String> {
        let bytes: [u8; 32] = hex::decode(chave_hex.trim())
            .map_err(|_| "a chave do link não é hexadecimal".to_string())?
            .try_into()
            .map_err(|_| "a chave do link não tem 32 bytes".to_string())?;
        let chave = TransferKey::from_bytes(bytes);

        let oferta: Oferta = serde_json::from_str(oferta_json).map_err(erro)?;

        oferta
            .de
            .verify(&oferta.manifesto_selado, &oferta.assinatura)
            .map_err(|_| "a assinatura de quem enviou não bate certo".to_string())?;

        let bruto = open_manifest(&chave, &oferta.manifesto_selado)
            .map_err(|_| "a chave deste link não abre este ficheiro".to_string())?;
        let manifesto: Manifest = serde_json::from_slice(&bruto).map_err(erro)?;
        manifesto.validate().map_err(erro)?;

        Ok(Descarga {
            chave,
            manifesto,
            de: oferta.de.label,
            proximo: 0,
        })
    }

    /// O nome original. Vinha selado dentro do manifesto — o relay nunca o viu.
    pub fn nome(&self) -> String {
        self.manifesto.name.clone()
    }

    /// Tamanho em claro. Em `f64` porque é assim que um número atravessa para o
    /// JavaScript sem `BigInt`; até 2^53 bytes não há perda, e isso são 9 PB.
    pub fn tamanho(&self) -> f64 {
        self.manifesto.size as f64
    }

    /// Etiqueta de quem enviou, já com a assinatura verificada.
    pub fn de(&self) -> String {
        self.de.clone()
    }

    pub fn total(&self) -> u32 {
        self.manifesto.expected_chunks()
    }

    /// Quantos bytes selados o bloco seguinte ocupa, ou zero se acabou.
    ///
    /// É isto que enquadra o fluxo: os bytes chegam sem fronteiras marcadas, e
    /// sem saber onde acaba cada bloco não haveria como os abrir. Todos têm o
    /// mesmo tamanho menos o último.
    pub fn proximo_tamanho(&self) -> u32 {
        match self.manifesto.plain_len(self.proximo) {
            Some(n) => (n + SEAL_OVERHEAD) as u32,
            None => 0,
        }
    }

    /// Abre o bloco seguinte e avança.
    ///
    /// Duas verificações, e ambas contam: o selo autentica o bloco na sua posição
    /// (o índice entra como dados associados, por isso trocar blocos de sítio não
    /// passa), e o resumo confirma-o contra o manifesto. Só se avança se as duas
    /// passarem — um bloco que não abre nunca se escreve em lado nenhum.
    pub fn bloco(&mut self, selado: &[u8]) -> Result<Vec<u8>, String> {
        let i = self.proximo;
        if self.manifesto.plain_len(i).is_none() {
            return Err("já não faltam blocos".to_string());
        }

        let claro = open_chunk(&self.chave, i, selado)
            .map_err(|_| "este bloco não abre — o ficheiro foi alterado".to_string())?;
        self.manifesto
            .verify_chunk(i, &claro)
            .map_err(|_| "este bloco não corresponde ao ficheiro anunciado".to_string())?;

        self.proximo = i + 1;
        Ok(claro)
    }
}

#[cfg(test)]
mod testes {
    use super::*;
    use pombo_correio_core::{
        crypto::{Envelope, seal_chunk, seal_manifest},
        identity::Identity,
    };

    /// Constrói o que o relay guardaria para um link, e devolve a chave em
    /// hexadecimal como ela viajaria no fragmento do URL.
    fn deposito(dados: &[u8]) -> (String, String) {
        let eu = Identity::generate("Portátil da Ana");
        let manifesto = Manifest::from_bytes("ferias.jpg", dados);
        let chave = TransferKey::generate();
        let manifesto_selado = seal_manifest(&chave, &serde_json::to_vec(&manifesto).unwrap());

        let oferta = Oferta {
            de: eu.public(),
            envelope: Envelope::seal(&chave, &Identity::generate("ninguém").public()),
            assinatura: eu.sign(&manifesto_selado),
            manifesto_selado,
            porta_resposta: 0,
        };

        (
            hex::encode(chave.to_bytes()),
            serde_json::to_string(&oferta).unwrap(),
        )
    }

    fn selados(chave_hex: &str, dados: &[u8]) -> Vec<Vec<u8>> {
        let b: [u8; 32] = hex::decode(chave_hex).unwrap().try_into().unwrap();
        let chave = TransferKey::from_bytes(b);
        dados
            .chunks(pombo_correio_core::chunk::CHUNK_SIZE)
            .enumerate()
            .map(|(i, c)| seal_chunk(&chave, i as u32, c))
            .collect()
    }

    fn conteudo(n: usize) -> Vec<u8> {
        (0..n).map(|i| ((i * 31 + i / 97) % 256) as u8).collect()
    }

    #[test]
    fn o_ficheiro_sai_do_link_byte_a_byte() {
        // Mais do que um bloco, para o último sair mais curto — é onde as contas
        // de enquadramento partem.
        let dados = conteudo(pombo_correio_core::chunk::CHUNK_SIZE + 4321);
        let (chave, oferta) = deposito(&dados);

        let mut d = Descarga::nova(&chave, &oferta).unwrap();
        assert_eq!(d.nome(), "ferias.jpg");
        assert_eq!(d.tamanho(), dados.len() as f64);
        assert_eq!(d.de(), "Portátil da Ana");
        assert_eq!(d.total(), 2);

        let mut saida = Vec::new();
        for selado in selados(&chave, &dados) {
            // O enquadramento tem de bater certo com o que está no fluxo.
            assert_eq!(d.proximo_tamanho() as usize, selado.len());
            saida.extend(d.bloco(&selado).unwrap());
        }

        assert_eq!(saida, dados);
        assert_eq!(d.proximo_tamanho(), 0, "devia ter acabado");
    }

    #[test]
    fn chave_errada_rebenta_antes_de_pedir_bytes() {
        // O ponto de fazer isto na construção: falha barato, sem descarregar nada.
        let (_, oferta) = deposito(b"segredo");
        let outra = hex::encode([7u8; 32]);
        assert!(Descarga::nova(&outra, &oferta).is_err());
    }

    #[test]
    fn link_truncado_diz_o_que_se_passa() {
        // O caso real: um chat corta o URL e a chave chega a meio.
        let (chave, oferta) = deposito(b"x");
        assert!(Descarga::nova(&chave[..30], &oferta).is_err());
        assert!(Descarga::nova("", &oferta).is_err());
    }

    #[test]
    fn manifesto_adulterado_no_relay_nao_passa() {
        // O relay é a peça exposta. Mexer-lhe no manifesto tem de partir a
        // assinatura, e ela é verificada antes de se abrir o que quer que seja.
        let (chave, oferta) = deposito(b"conteudo importante");
        let mut o: serde_json::Value = serde_json::from_str(&oferta).unwrap();
        o["manifesto_selado"].as_array_mut().unwrap()[0] = serde_json::json!(9);
        assert!(Descarga::nova(&chave, &o.to_string()).is_err());
    }

    #[test]
    fn bloco_trocado_de_posicao_e_recusado() {
        // O índice entra no selo e no resumo. Sem isso, quem estivesse no meio
        // reordenava o ficheiro sem partir nada.
        let dados = conteudo(pombo_correio_core::chunk::CHUNK_SIZE * 2);
        let (chave, oferta) = deposito(&dados);
        let blocos = selados(&chave, &dados);

        let mut d = Descarga::nova(&chave, &oferta).unwrap();
        assert!(d.bloco(&blocos[1]).is_err(), "o bloco 1 não é o bloco 0");
    }

    #[test]
    fn bloco_com_um_bit_mexido_e_recusado() {
        let dados = conteudo(5000);
        let (chave, oferta) = deposito(&dados);
        let mut blocos = selados(&chave, &dados);
        let ultimo = blocos[0].len() - 1;
        blocos[0][ultimo] ^= 1;

        let mut d = Descarga::nova(&chave, &oferta).unwrap();
        assert!(d.bloco(&blocos[0]).is_err());
    }

    #[test]
    fn pedir_um_bloco_a_mais_nao_rebenta() {
        let dados = conteudo(100);
        let (chave, oferta) = deposito(&dados);
        let blocos = selados(&chave, &dados);

        let mut d = Descarga::nova(&chave, &oferta).unwrap();
        d.bloco(&blocos[0]).unwrap();
        assert_eq!(d.proximo_tamanho(), 0);
        assert!(d.bloco(&blocos[0]).is_err());
    }
}
