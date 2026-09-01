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
    chunk::{Manifest, ManifestBuilder},
    crypto::{Envelope, SEAL_OVERHEAD, TransferKey, open_chunk, open_manifest, seal_chunk, seal_manifest},
    identity::{Identity, PublicIdentity},
    protocolo::{Oferta, mensagem_assinada},
};
use wasm_bindgen::prelude::*;

/// A identidade do dispositivo, para quando o `ui/` corre sem Tauri (S8 P2).
///
/// Gerar e assinar já não tocam em I/O — é por isso que `core::identity` sempre
/// compilou para wasm32. O que falta só é a fronteira: o `localStorage` é quem
/// persiste (não há sistema de ficheiros no browser), por isso os bytes
/// secretos saem daqui em hexadecimal e voltam pela mesma via — o mesmo desenho
/// do `secret_bytes()`/`from_secret_bytes()` que o Tauri já usa, só que ali o
/// destino é um ficheiro `0600` e aqui é o `localStorage` da origem.
#[wasm_bindgen]
pub struct IdentidadeWasm {
    interna: Identity,
}

#[wasm_bindgen]
impl IdentidadeWasm {
    /// Gera uma identidade nova. Só deve ser chamado uma vez por dispositivo —
    /// quem chama grava logo o segredo, senão a próxima visita gera outra.
    pub fn gerar(nome: String) -> IdentidadeWasm {
        IdentidadeWasm { interna: Identity::generate(nome) }
    }

    /// Reconstrói a partir do que ficou em `localStorage`.
    #[wasm_bindgen(js_name = deSegredo)]
    pub fn de_segredo(signing_hex: &str, secret_hex: &str, nome: String) -> Result<IdentidadeWasm, String> {
        let signing: [u8; 32] = hex::decode(signing_hex)
            .map_err(erro)?
            .try_into()
            .map_err(|_| "chave de assinatura guardada não tem 32 bytes".to_string())?;
        let secret: [u8; 32] = hex::decode(secret_hex)
            .map_err(erro)?
            .try_into()
            .map_err(|_| "chave de cifra guardada não tem 32 bytes".to_string())?;
        Ok(IdentidadeWasm { interna: Identity::from_secret_bytes(signing, secret, nome) })
    }

    pub fn nome(&self) -> String {
        self.interna.label.clone()
    }

    #[wasm_bindgen(js_name = numeroSeguranca)]
    pub fn numero_seguranca(&self) -> String {
        self.interna.public().safety_number()
    }

    /// A impressão digital de 16 dígitos hexadecimais — a que o relay pede em
    /// `--autorizados` e que a interface mostra em destaque no primeiro
    /// arranque, para o André a poder copiar para lá.
    #[wasm_bindgen(js_name = impressaoCurta)]
    pub fn impressao_curta(&self) -> String {
        hex::encode(self.interna.public().fingerprint())[..16].to_string()
    }

    #[wasm_bindgen(js_name = segredoSigning)]
    pub fn segredo_signing(&self) -> String {
        hex::encode(self.interna.secret_bytes().0)
    }

    #[wasm_bindgen(js_name = segredoSecret)]
    pub fn segredo_secret(&self) -> String {
        hex::encode(self.interna.secret_bytes().1)
    }

    /// A identidade pública, como JSON — o que entra no campo `de` de uma
    /// [`Oferta`] (S8 P5).
    #[wasm_bindgen(js_name = publicaJson)]
    pub fn publica_json(&self) -> Result<String, String> {
        serde_json::to_string(&self.interna.public()).map_err(erro)
    }

    /// Assina uma mensagem qualquer e devolve a assinatura em hexadecimal.
    /// Usado para os pedidos ao relay (S8 P5) — `assinarPedido` já monta a
    /// mensagem certa; isto é o primitivo por trás.
    pub fn assinar(&self, mensagem: &str) -> String {
        hex::encode(self.interna.sign(mensagem.as_bytes()))
    }

    /// Os três cabeçalhos `x-pombo-*` que um pedido assinado ao relay precisa,
    /// como JSON — a mesma mensagem que `relay::autenticado::verificar`
    /// exige do outro lado (`core::protocolo::mensagem_assinada`), para não
    /// haver uma quarta cópia do formato em JavaScript.
    ///
    /// `instante` vem de fora (`Date.now()/1000`) porque o `SystemTime` do
    /// Rust não tem uma fonte de tempo garantida em wasm32 fora do browser, e
    /// o JavaScript já tem uma sem precisar de nenhuma.
    #[wasm_bindgen(js_name = assinarPedido)]
    pub fn assinar_pedido(&self, acao: &str, extra: &str, instante: u64) -> Result<String, String> {
        let p = self.interna.public();
        let impressao = self.impressao_curta();
        let assinatura = hex::encode(
            self.interna
                .sign(mensagem_assinada(acao, &impressao, extra, instante).as_bytes()),
        );
        #[derive(serde::Serialize)]
        struct Cabecalhos {
            identidade: String,
            instante: String,
            assinatura: String,
        }
        serde_json::to_string(&Cabecalhos {
            identidade: format!("{}{}", hex::encode(p.verifying), hex::encode(p.encrypting)),
            instante: instante.to_string(),
            assinatura,
        })
        .map_err(erro)
    }
}

/// Uma identidade pública a partir das duas chaves em hexadecimal — o
/// destinatário de um depósito (S8 P5). A etiqueta fica vazia: não entra na
/// cifra nem na assinatura, só no envelope é que não faz falta nenhuma.
fn destinatario_de(verifying_hex: &str, encrypting_hex: &str) -> Result<PublicIdentity, String> {
    let verifying: [u8; 32] = hex::decode(verifying_hex)
        .map_err(erro)?
        .try_into()
        .map_err(|_| "chave de verificação do destinatário não tem 32 bytes".to_string())?;
    let encrypting: [u8; 32] = hex::decode(encrypting_hex)
        .map_err(erro)?
        .try_into()
        .map_err(|_| "chave de cifra do destinatário não tem 32 bytes".to_string())?;
    Ok(PublicIdentity { verifying, encrypting, label: String::new() })
}

/// A impressão digital de 16 dígitos hexadecimais de um destinatário
/// qualquer — o que vai no `?para=` de um depósito (S8 P5). Irmã de
/// `IdentidadeWasm::impressaoCurta`, mas para uma identidade que não é a
/// nossa: quem envia precisa de saber a impressão de quem vai receber.
#[wasm_bindgen(js_name = impressaoCurtaDe)]
pub fn impressao_curta_de(verifying_hex: &str, encrypting_hex: &str) -> Result<String, String> {
    let destinatario = destinatario_de(verifying_hex, encrypting_hex)?;
    Ok(hex::encode(destinatario.fingerprint())[..16].to_string())
}

/// Quantos bytes tem um bloco em claro — o `CHUNK_SIZE` do `core/`, exposto
/// para quem tem de partir um `File` do browser nesse tamanho antes de o
/// poder empurrar para o `ManifestoBuilderWasm` (S8 P5). Uma função em vez de
/// duplicar o número em TypeScript: divergir aqui daria um manifesto que o
/// `core/` nativo não reproduziria.
#[wasm_bindgen(js_name = tamanhoBloco)]
pub fn tamanho_bloco() -> u32 {
    pombo_correio_core::chunk::CHUNK_SIZE as u32
}

fn chave_de_hex(chave_hex: &str) -> Result<TransferKey, String> {
    let bytes: [u8; 32] = hex::decode(chave_hex)
        .map_err(erro)?
        .try_into()
        .map_err(|_| "chave de transferência não tem 32 bytes".to_string())?;
    Ok(TransferKey::from_bytes(bytes))
}

/// Gera uma chave de transferência nova, em hexadecimal (S8 P5).
///
/// Fica em hexadecimal, não num tipo `TransferKey` exposto ao wasm-bindgen: a
/// chave viaja por três funções deste ficheiro (selar blocos, selar o
/// manifesto, montar o envelope) e todas já falam hexadecimal com o
/// `localStorage` e o JSON — inventar um tipo opaco só obrigava o
/// TypeScript a passá-lo de um lado para o outro sem lhe poder tocar.
#[wasm_bindgen(js_name = gerarChaveTransferencia)]
pub fn gerar_chave_transferencia() -> String {
    hex::encode(TransferKey::generate().to_bytes())
}

/// Sela um bloco em claro para subir ao relay (S8 P5).
#[wasm_bindgen(js_name = selarBloco)]
pub fn selar_bloco(chave_hex: &str, indice: u32, dados: &[u8]) -> Result<Vec<u8>, String> {
    Ok(seal_chunk(&chave_de_hex(chave_hex)?, indice, dados))
}

/// Constrói um manifesto bloco a bloco — ver [`ManifestBuilder`] no `core/`.
/// Não é `Clone`/`Copy`: cada bloco só se pode empurrar uma vez, na ordem.
#[wasm_bindgen]
pub struct ManifestoBuilderWasm(ManifestBuilder);

#[wasm_bindgen]
impl ManifestoBuilderWasm {
    #[wasm_bindgen(constructor)]
    pub fn nova() -> ManifestoBuilderWasm {
        ManifestoBuilderWasm(Manifest::builder())
    }

    pub fn push(&mut self, dados: &[u8]) {
        self.0.push(dados);
    }

    /// Fecha o manifesto e devolve-o como JSON — pronto para selar
    /// (`selarManifesto`) e assinar.
    pub fn finalizar(self, nome: String) -> Result<String, String> {
        serde_json::to_string(&self.0.finish(nome)).map_err(erro)
    }
}

impl Default for ManifestoBuilderWasm {
    fn default() -> Self {
        Self::nova()
    }
}

/// Sela o manifesto já fechado (S8 P5).
#[wasm_bindgen(js_name = selarManifesto)]
pub fn selar_manifesto(chave_hex: &str, manifesto_json: &str) -> Result<Vec<u8>, String> {
    Ok(seal_manifest(&chave_de_hex(chave_hex)?, manifesto_json.as_bytes()))
}

/// Monta a [`Oferta`] completa, assinada, pronta a mandar ao relay (S8 P5).
///
/// Mesma forma do `depositar_em` nativo (`rede/src/relay.rs`): sela o
/// manifesto, embrulha a chave para o destinatário, assina o manifesto
/// selado. Devolvê-la já como JSON (via `serde_json`, não montado à mão em
/// JavaScript) é o que garante que o formato bate certo com o que um
/// `serde_json::from_str::<Oferta>` do outro lado espera — divergir aqui
/// só apareceria quando alguém tentasse ler o depósito.
///
/// Para um "link sem instalação" (sem destinatário conhecido), quem chama
/// passa as chaves de uma identidade gerada e deitada fora — o mesmo truque
/// do `depositar_ligacao` nativo.
#[wasm_bindgen(js_name = construirOferta)]
pub fn construir_oferta(
    remetente: &IdentidadeWasm,
    chave_hex: &str,
    manifesto_json: &str,
    destinatario_verifying_hex: &str,
    destinatario_encrypting_hex: &str,
) -> Result<String, String> {
    let chave = chave_de_hex(chave_hex)?;
    let destinatario = destinatario_de(destinatario_verifying_hex, destinatario_encrypting_hex)?;

    let manifesto_selado = seal_manifest(&chave, manifesto_json.as_bytes());
    let oferta = Oferta {
        de: remetente.interna.public(),
        envelope: Envelope::seal(&chave, &destinatario),
        assinatura: remetente.interna.sign(&manifesto_selado),
        manifesto_selado,
        // Sem ligação de volta possível a partir do browser — nunca há
        // servidor de receção no Safari.
        porta_resposta: 0,
    };
    serde_json::to_string(&oferta).map_err(erro)
}

/// Uma descida em curso: sabe o manifesto e por que bloco vai.
#[wasm_bindgen]
pub struct Descarga {
    chave: TransferKey,
    manifesto: Manifest,
    /// A identidade completa de quem enviou, não só a etiqueta — o S8 P5 usa
    /// o resto (`remetenteJson`) para a página do link ensinar essa
    /// identidade à PWA instalada, via `localStorage` (mesma origem).
    remetente: pombo_correio_core::identity::PublicIdentity,
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
        Self::verificar(&oferta)?;
        Self::de_oferta_e_chave(oferta, chave)
    }

    /// Prepara a descida a partir da caixa do relay (S8 P3) — o equivalente
    /// ao `trazer()` nativo (`rede/src/relay.rs`), até este ponto: aqui a
    /// chave não vem de um fragmento de URL, vem de abrir o envelope com a
    /// **identidade do dispositivo**, porque é a ele que o depósito está
    /// endereçado. Dali para a frente é o mesmo `Descarga` de sempre — as
    /// mesmas verificações por bloco, os mesmos testes.
    #[wasm_bindgen(js_name = daCaixa)]
    pub fn da_caixa(remetente: &IdentidadeWasm, oferta_json: &str) -> Result<Descarga, String> {
        let oferta: Oferta = serde_json::from_str(oferta_json).map_err(erro)?;
        // Autenticar antes de abrir seja o que for — mesma ordem do `nova` e
        // do `trazer()` nativo: sem isto estaríamos a decifrar o envelope de
        // material de qualquer um.
        Self::verificar(&oferta)?;
        let chave = oferta
            .envelope
            .open(&remetente.interna)
            .map_err(|_| "este depósito não é para mim".to_string())?;
        Self::de_oferta_e_chave(oferta, chave)
    }

    fn verificar(oferta: &Oferta) -> Result<(), String> {
        oferta
            .de
            .verify(&oferta.manifesto_selado, &oferta.assinatura)
            .map_err(|_| "a assinatura de quem enviou não bate certo".to_string())
    }

    /// A parte partilhada pelos dois construtores, já com a assinatura
    /// verificada e a `chave` em mãos — só muda como se chegou a ela.
    fn de_oferta_e_chave(oferta: Oferta, chave: TransferKey) -> Result<Descarga, String> {
        let bruto = open_manifest(&chave, &oferta.manifesto_selado)
            .map_err(|_| "a chave deste depósito não abre este ficheiro".to_string())?;
        let manifesto: Manifest = serde_json::from_slice(&bruto).map_err(erro)?;
        manifesto.validate().map_err(erro)?;

        Ok(Descarga {
            chave,
            manifesto,
            remetente: oferta.de,
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
        self.remetente.label.clone()
    }

    /// O número de segurança de quem enviou — o que se lê em voz alta ao
    /// telefone para confirmar um dispositivo novo (S8 P3). Sem isto, o
    /// aviso de "dispositivo novo" ficava com o número em branco: a única
    /// parte da interface cujo trabalho é ser lida ao telefone, vazia.
    #[wasm_bindgen(js_name = numeroSegurancaDe)]
    pub fn numero_seguranca_de(&self) -> String {
        self.remetente.safety_number()
    }

    /// A identidade completa de quem enviou (as duas chaves públicas, mais a
    /// etiqueta), como JSON — para quem quiser guardá-la e passar a poder
    /// depositar de volta para esta pessoa (S8 P5: é assim que a PWA aprende
    /// o primeiro dispositivo de casa, ao abrir um link).
    #[wasm_bindgen(js_name = remetenteJson)]
    pub fn remetente_json(&self) -> Result<String, String> {
        serde_json::to_string(&self.remetente).map_err(erro)
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
    fn o_que_o_lado_de_enviar_produz_o_lado_de_receber_le(){
        // Interop de ponta a ponta, dentro de um só teste: as funções de
        // envio novas (S8 P5) e a `Descarga` que já existia (S7 R5) só se
        // encontram na rede a sério — aqui é onde se prova, sem precisar de
        // um browser, que produzem e consomem exatamente o mesmo formato.
        let remetente = IdentidadeWasm::gerar("iPhone da Ana".to_string());
        let destinatario = Identity::generate("PC da sala");

        let dados = conteudo(pombo_correio_core::chunk::CHUNK_SIZE + 777);
        let chave_hex = gerar_chave_transferencia();

        let mut builder = ManifestoBuilderWasm::nova();
        let mut blocos_selados = Vec::new();
        for (i, bloco) in dados.chunks(pombo_correio_core::chunk::CHUNK_SIZE).enumerate() {
            builder.push(bloco);
            blocos_selados.push(selar_bloco(&chave_hex, i as u32, bloco).unwrap());
        }
        let manifesto_json = builder.finalizar("ferias.jpg".to_string()).unwrap();

        let dp = destinatario.public();
        let oferta_json = construir_oferta(
            &remetente,
            &chave_hex,
            &manifesto_json,
            &hex::encode(dp.verifying),
            &hex::encode(dp.encrypting),
        )
        .unwrap();

        // O lado de receber não sabe que isto veio do wasm — só vê a `Oferta`
        // JSON e a chave, exatamente como viriam de um link a sério.
        let mut descarga = Descarga::nova(&chave_hex, &oferta_json).unwrap();
        assert_eq!(descarga.nome(), "ferias.jpg");
        assert_eq!(descarga.de(), "iPhone da Ana");
        assert_eq!(descarga.tamanho(), dados.len() as f64);

        let mut saida = Vec::new();
        for selado in blocos_selados {
            assert_eq!(descarga.proximo_tamanho() as usize, selado.len());
            saida.extend(descarga.bloco(&selado).unwrap());
        }
        assert_eq!(saida, dados);
    }

    #[test]
    fn da_caixa_abre_com_a_identidade_em_vez_da_chave_do_link() {
        // S8 P3, sentido contrário do teste acima: um PC de casa deposita
        // (endereçado à PWA a sério, não a um "ninguém" de link), e a PWA
        // abre-o com a própria identidade — não com nenhuma chave que tenha
        // vindo de um URL, porque não há URL nenhum neste caminho.
        let remetente = Identity::generate("PC da sala");
        let destinatario = IdentidadeWasm::gerar("iPhone da Ana".to_string());

        let dados = conteudo(5000);
        let manifesto = Manifest::from_bytes("nota.pdf", &dados);
        let chave = TransferKey::generate();
        let manifesto_selado = seal_manifest(&chave, &serde_json::to_vec(&manifesto).unwrap());
        let dp: PublicIdentity = serde_json::from_str(&destinatario.publica_json().unwrap()).unwrap();

        let oferta = Oferta {
            de: remetente.public(),
            envelope: Envelope::seal(&chave, &dp),
            assinatura: remetente.sign(&manifesto_selado),
            manifesto_selado,
            porta_resposta: 0,
        };
        let oferta_json = serde_json::to_string(&oferta).unwrap();

        let mut descarga = Descarga::da_caixa(&destinatario, &oferta_json).unwrap();
        assert_eq!(descarga.nome(), "nota.pdf");
        assert_eq!(descarga.de(), "PC da sala");
        // O que se lê em voz alta no aviso de dispositivo novo — vazio aqui
        // já foi um bug real (App.svelte mostrava o cartão sem número).
        assert_eq!(descarga.numero_seguranca_de(), remetente.public().safety_number());

        let selado = seal_chunk(&chave, 0, &dados);
        assert_eq!(descarga.bloco(&selado).unwrap(), dados);
    }

    #[test]
    fn da_caixa_recusa_um_envelope_que_nao_e_para_esta_identidade() {
        // O caso central de segurança do P3: um depósito endereçado a outra
        // pessoa não pode abrir só porque calhou de aparecer na caixa errada.
        let remetente = Identity::generate("PC da sala");
        let outro = Identity::generate("outro dispositivo qualquer");
        let eu = IdentidadeWasm::gerar("iPhone da Ana".to_string());

        let chave = TransferKey::generate();
        let manifesto_selado = seal_manifest(&chave, b"{}");
        let oferta = Oferta {
            de: remetente.public(),
            envelope: Envelope::seal(&chave, &outro.public()),
            assinatura: remetente.sign(&manifesto_selado),
            manifesto_selado,
            porta_resposta: 0,
        };

        assert!(Descarga::da_caixa(&eu, &serde_json::to_string(&oferta).unwrap()).is_err());
    }

    #[test]
    fn assinar_pedido_bate_com_a_verificacao_do_relay() {
        // O mesmo `mensagem_assinada` que o relay usa para verificar — se
        // este teste passar sozinho mas a assinatura ainda assim não bater
        // num pedido a sério, a divergência está no lado do relay, não aqui.
        let eu = IdentidadeWasm::gerar("iPhone".to_string());
        let cabecalhos: serde_json::Value =
            serde_json::from_str(&eu.assinar_pedido("caixa", "", 1_700_000_000).unwrap()).unwrap();

        let msg = mensagem_assinada("caixa", &eu.impressao_curta(), "", 1_700_000_000);
        let assinatura: [u8; 64] = hex::decode(cabecalhos["assinatura"].as_str().unwrap())
            .unwrap()
            .try_into()
            .unwrap();
        assert!(eu.interna.public().verify(msg.as_bytes(), &assinatura).is_ok());
    }

    #[test]
    fn identidade_wasm_sobrevive_a_ida_e_volta_pelo_hexadecimal() {
        // O caminho real: gerar, guardar os dois segredos em `localStorage`
        // (aqui, só as strings), e reconstruir na visita seguinte.
        let original = IdentidadeWasm::gerar("iPhone da Ana".to_string());
        let signing = original.segredo_signing();
        let secret = original.segredo_secret();

        let restaurada = IdentidadeWasm::de_segredo(&signing, &secret, "iPhone da Ana".to_string()).unwrap();
        assert_eq!(original.numero_seguranca(), restaurada.numero_seguranca());
        assert_eq!(original.impressao_curta(), restaurada.impressao_curta());
    }

    #[test]
    fn identidade_wasm_recusa_segredo_com_tamanho_errado() {
        assert!(IdentidadeWasm::de_segredo("ab", "cd", "x".to_string()).is_err());
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
