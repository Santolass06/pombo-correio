//! Endereçamento por conteúdo com árvore de Merkle.
//!
//! Um ficheiro é partido em blocos de tamanho fixo, cada bloco é resumido com
//! BLAKE3, e sobre esses resumos constrói-se uma árvore binária cuja raiz
//! identifica o ficheiro inteiro.
//!
//! É isto que substitui o BitTorrent no desenho. Dá as três propriedades que
//! interessavam:
//!
//! - **Retoma** — sabendo que blocos já chegaram, continua-se de onde ia.
//! - **Verificação por bloco** — cada bloco confirma-se à chegada, sem esperar
//!   pelo ficheiro todo, por isso corrupção detecta-se cedo e não no fim.
//! - **Multi-fonte** — como a identidade de um bloco é o seu conteúdo, blocos
//!   iguais vindos de origens diferentes são intermutáveis. É o que permitirá
//!   mais tarde puxar de vários dispositivos ao mesmo tempo.

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// 1 MiB. Compromisso entre o peso do manifesto e a granularidade da retoma:
/// um vídeo de 4 GB dá ~4096 blocos, ou seja ~128 KB de hashes.
pub const CHUNK_SIZE: usize = 1 << 20;

/// Resumo de um bloco.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkHash(pub [u8; 32]);

impl ChunkHash {
    pub fn to_hex(self) -> String {
        hex::encode(self.0)
    }
}

/// Resume um bloco, ligando-o à sua posição.
///
/// O índice entra no resumo de propósito: sem ele, blocos com o mesmo conteúdo
/// seriam indistinguíveis e alguém poderia reordenar um ficheiro mantendo todos
/// os hashes válidos.
pub fn hash_chunk(index: u32, data: &[u8]) -> ChunkHash {
    let mut h = blake3::Hasher::new_derive_key("pombo-chunk-v1");
    h.update(&index.to_le_bytes());
    h.update(data);
    ChunkHash(*h.finalize().as_bytes())
}

/// Combina dois nós internos da árvore.
///
/// A chave de derivação difere da das folhas para que um nó interno nunca possa
/// ser confundido com uma folha — sem isso, a árvore fica exposta ao ataque
/// clássico de segunda pré-imagem.
fn hash_pair(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut h = blake3::Hasher::new_derive_key("pombo-node-v1");
    h.update(left);
    h.update(right);
    *h.finalize().as_bytes()
}

/// Raiz de Merkle sobre os resumos dos blocos.
///
/// Quando um nível tem um número ímpar de nós, o último sobe inalterado em vez
/// de ser duplicado. Duplicar seria a escolha do Bitcoin e traz consigo a
/// ambiguidade CVE-2012-2459, em que duas listas diferentes produzem a mesma
/// raiz.
pub fn merkle_root(chunks: &[ChunkHash]) -> [u8; 32] {
    if chunks.is_empty() {
        // Um ficheiro vazio ainda precisa de identidade estável.
        return *blake3::Hasher::new_derive_key("pombo-empty-v1")
            .finalize()
            .as_bytes();
    }

    let mut level: Vec<[u8; 32]> = chunks.iter().map(|c| c.0).collect();

    while level.len() > 1 {
        level = level
            .chunks(2)
            .map(|p| if p.len() == 2 { hash_pair(&p[0], &p[1]) } else { p[0] })
            .collect();
    }

    level[0]
}

/// Descrição completa de um ficheiro, suficiente para o pedir e verificar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// Nome original. Nunca usado como caminho sem ser saneado primeiro.
    pub name: String,
    pub size: u64,
    pub chunk_size: u32,
    pub root: [u8; 32],
    pub chunks: Vec<ChunkHash>,
}

impl Manifest {
    /// Constrói o manifesto de um buffer em memória.
    pub fn from_bytes(name: impl Into<String>, data: &[u8]) -> Self {
        let chunks: Vec<ChunkHash> = data
            .chunks(CHUNK_SIZE)
            .enumerate()
            .map(|(i, c)| hash_chunk(i as u32, c))
            .collect();

        Self {
            name: name.into(),
            size: data.len() as u64,
            chunk_size: CHUNK_SIZE as u32,
            root: merkle_root(&chunks),
            chunks,
        }
    }

    /// Constrói o manifesto lendo em streaming, sem carregar o ficheiro todo.
    ///
    /// É este o caminho usado na prática: um vídeo de 4 GB não cabe
    /// confortavelmente em memória num Raspberry Pi.
    pub fn from_reader<R: std::io::Read>(name: impl Into<String>, mut r: R) -> Result<Self> {
        let mut chunks = Vec::new();
        let mut buf = vec![0u8; CHUNK_SIZE];
        let mut size: u64 = 0;
        let mut index: u32 = 0;

        loop {
            // `read` pode devolver menos do que foi pedido sem estar no fim do
            // ficheiro, por isso enche-se o buffer até ao limite antes de cortar
            // o bloco. Sem isto, os blocos sairiam com tamanhos irregulares e o
            // manifesto não reproduziria.
            let mut filled = 0;
            while filled < CHUNK_SIZE {
                match r.read(&mut buf[filled..])? {
                    0 => break,
                    n => filled += n,
                }
            }

            if filled == 0 {
                break;
            }

            chunks.push(hash_chunk(index, &buf[..filled]));
            size += filled as u64;
            index += 1;

            if filled < CHUNK_SIZE {
                break;
            }
        }

        Ok(Self {
            name: name.into(),
            size,
            chunk_size: CHUNK_SIZE as u32,
            root: merkle_root(&chunks),
            chunks,
        })
    }

    /// Constrói um `ManifestBuilder` vazio — ver o seu próprio comentário.
    pub fn builder() -> ManifestBuilder {
        ManifestBuilder::default()
    }

    /// Número de blocos esperado a partir do tamanho declarado.
    pub fn expected_chunks(&self) -> u32 {
        self.size.div_ceil(self.chunk_size as u64) as u32
    }

    /// Tamanho em claro do bloco `index`.
    ///
    /// Todos os blocos têm `chunk_size` menos o último, que leva o resto. Quem
    /// recebe precisa disto para enquadrar o fluxo: sem saber onde acaba cada
    /// bloco selado, não consegue separá-los para os abrir.
    pub fn plain_len(&self, index: u32) -> Option<usize> {
        let n = self.expected_chunks();
        if index >= n {
            return None;
        }
        Some(if index + 1 < n {
            self.chunk_size as usize
        } else {
            // O último bloco leva o que sobra; se o tamanho for múltiplo exato
            // do bloco, sobra um bloco inteiro e não zero.
            match self.size % self.chunk_size as u64 {
                0 => self.chunk_size as usize,
                resto => resto as usize,
            }
        })
    }

    /// Confirma que o manifesto é internamente coerente.
    ///
    /// Deve correr-se em **todo** o manifesto recebido pela rede antes de se
    /// alocar seja o que for com base nele.
    pub fn validate(&self) -> Result<()> {
        if self.chunk_size == 0 {
            return Err(Error::BadManifest("chunk_size é zero".into()));
        }
        if self.chunks.len() != self.expected_chunks() as usize {
            return Err(Error::BadManifest(format!(
                "declara {} bytes ({} blocos) mas traz {} hashes",
                self.size,
                self.expected_chunks(),
                self.chunks.len()
            )));
        }
        if merkle_root(&self.chunks) != self.root {
            return Err(Error::RootMismatch);
        }
        Ok(())
    }

    /// Verifica um bloco recebido contra o manifesto.
    pub fn verify_chunk(&self, index: u32, data: &[u8]) -> Result<()> {
        let expected = self
            .chunks
            .get(index as usize)
            .ok_or(Error::ChunkMismatch { index })?;

        if hash_chunk(index, data) != *expected {
            return Err(Error::ChunkMismatch { index });
        }
        Ok(())
    }
}

/// Constrói um [`Manifest`] bloco a bloco, para quando quem chama já traz os
/// bytes em blocos e não tem um `std::io::Read` para dar ao `from_reader`.
///
/// É o caso do wasm da PWA (S8 P5): um `File` do browser lê-se por
/// `file.slice(...).arrayBuffer()`, que dá exatamente um bloco de cada vez —
/// e sem `std::io::Read` no browser, `from_reader` não serve. Aceitar blocos
/// já prontos, em vez de reimplementar aqui a leitura, mantém o hash de cada
/// bloco no mesmo sítio (`hash_chunk`) que o `from_reader` já usa, para as
/// duas formas de construir um manifesto continuarem a dar a mesma raiz.
#[derive(Default)]
pub struct ManifestBuilder {
    chunks: Vec<ChunkHash>,
    size: u64,
}

impl ManifestBuilder {
    /// Resume mais um bloco. **Pela ordem**: o índice é a posição na
    /// sequência de chamadas, não algo que se possa saltar ou repetir — tal
    /// como o `from_reader`, que também não admite blocos fora de ordem.
    pub fn push(&mut self, dados: &[u8]) {
        let indice = self.chunks.len() as u32;
        self.chunks.push(hash_chunk(indice, dados));
        self.size += dados.len() as u64;
    }

    /// Fecha o manifesto. Não valida `chunk_size` contra os blocos que
    /// entraram — quem chama é responsável por lhes dar sempre o mesmo
    /// tamanho, exceto o último; um manifesto construído com blocos
    /// irregulares falha depois em `validate()`, não aqui.
    pub fn finish(self, name: impl Into<String>) -> Manifest {
        Manifest {
            name: name.into(),
            size: self.size,
            chunk_size: CHUNK_SIZE as u32,
            root: merkle_root(&self.chunks),
            chunks: self.chunks,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dados(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i % 251) as u8).collect()
    }

    #[test]
    fn manifesto_do_builder_reproduz_o_de_memoria() {
        // O caminho do wasm (S8 P5) é este: se divergisse do `from_bytes`, um
        // ficheiro enviado do iPhone teria uma raiz diferente da que o
        // `from_reader` calcularia do outro lado, e a verificação rejeitava
        // um ficheiro correto.
        let d = dados(CHUNK_SIZE * 2 + 4321);
        let esperado = Manifest::from_bytes("v.mp4", &d);

        let mut b = Manifest::builder();
        for bloco in d.chunks(CHUNK_SIZE) {
            b.push(bloco);
        }
        assert_eq!(b.finish("v.mp4"), esperado);
    }

    #[test]
    fn manifesto_reproduz_entre_memoria_e_streaming() {
        // O caminho de streaming é o usado em produção; se divergir do simples,
        // ficheiros grandes deixam de validar.
        let d = dados(CHUNK_SIZE * 2 + 12_345);
        let a = Manifest::from_bytes("v.mp4", &d);
        let b = Manifest::from_reader("v.mp4", std::io::Cursor::new(&d)).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn leitura_fragmentada_nao_altera_o_manifesto() {
        /// Leitor que devolve poucos bytes de cada vez, como um socket lento.
        struct Lento(Vec<u8>, usize);
        impl std::io::Read for Lento {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let n = buf.len().min(1000).min(self.0.len() - self.1);
                buf[..n].copy_from_slice(&self.0[self.1..self.1 + n]);
                self.1 += n;
                Ok(n)
            }
        }

        let d = dados(CHUNK_SIZE + 5000);
        let esperado = Manifest::from_bytes("x", &d);
        let obtido = Manifest::from_reader("x", Lento(d, 0)).unwrap();
        assert_eq!(esperado, obtido);
    }

    #[test]
    fn conteudos_diferentes_dao_raizes_diferentes() {
        assert_ne!(
            Manifest::from_bytes("a", b"abc").root,
            Manifest::from_bytes("a", b"abd").root
        );
    }

    #[test]
    fn ficheiro_vazio_e_valido() {
        let m = Manifest::from_bytes("vazio", b"");
        assert_eq!(m.size, 0);
        assert!(m.chunks.is_empty());
        assert!(m.validate().is_ok());
    }

    #[test]
    fn validate_aceita_manifesto_intacto() {
        assert!(Manifest::from_bytes("x", &dados(CHUNK_SIZE * 3)).validate().is_ok());
    }

    #[test]
    fn validate_deteta_raiz_adulterada() {
        let mut m = Manifest::from_bytes("x", &dados(5000));
        m.root[0] ^= 1;
        assert!(matches!(m.validate(), Err(Error::RootMismatch)));
    }

    #[test]
    fn validate_deteta_contagem_incoerente() {
        let mut m = Manifest::from_bytes("x", &dados(CHUNK_SIZE * 2));
        m.chunks.pop();
        assert!(matches!(m.validate(), Err(Error::BadManifest(_))));
    }

    #[test]
    fn verify_chunk_aceita_o_bloco_certo_e_recusa_o_alterado() {
        let d = dados(CHUNK_SIZE + 100);
        let m = Manifest::from_bytes("x", &d);

        assert!(m.verify_chunk(0, &d[..CHUNK_SIZE]).is_ok());
        assert!(m.verify_chunk(1, &d[CHUNK_SIZE..]).is_ok());

        let mut mau = d[..CHUNK_SIZE].to_vec();
        mau[0] ^= 1;
        assert!(m.verify_chunk(0, &mau).is_err());
    }

    #[test]
    fn bloco_certo_no_indice_errado_e_recusado() {
        // A ligação bloco↔posição é o que impede a reordenação do ficheiro.
        let d = dados(CHUNK_SIZE * 2);
        let m = Manifest::from_bytes("x", &d);
        assert!(m.verify_chunk(1, &d[..CHUNK_SIZE]).is_err());
    }

    #[test]
    fn indice_fora_do_alcance_e_recusado() {
        let m = Manifest::from_bytes("x", b"abc");
        assert!(matches!(
            m.verify_chunk(99, b"abc"),
            Err(Error::ChunkMismatch { index: 99 })
        ));
    }

    #[test]
    fn plain_len_soma_ao_tamanho_total() {
        for tamanho in [1, 5000, CHUNK_SIZE, CHUNK_SIZE + 1, CHUNK_SIZE * 3 + 77] {
            let m = Manifest::from_bytes("x", &dados(tamanho));
            let soma: usize = (0..m.expected_chunks()).map(|i| m.plain_len(i).unwrap()).sum();
            assert_eq!(soma as u64, m.size, "falhou com {tamanho} bytes");
        }
    }

    #[test]
    fn multiplo_exato_do_bloco_nao_da_ultimo_vazio() {
        // O caso que o `%` apanha mal: com tamanho múltiplo exato, o resto é
        // zero mas o último bloco está cheio, não vazio.
        let m = Manifest::from_bytes("x", &dados(CHUNK_SIZE * 2));
        assert_eq!(m.expected_chunks(), 2);
        assert_eq!(m.plain_len(1), Some(CHUNK_SIZE));
        assert_eq!(m.plain_len(2), None);
    }

    #[test]
    fn merkle_distingue_listas_de_comprimento_impar() {
        // Guarda contra a ambiguidade que surge de duplicar o último nó: estas
        // duas listas têm de dar raízes distintas.
        let a = [ChunkHash([1; 32]), ChunkHash([2; 32]), ChunkHash([3; 32])];
        let b = [
            ChunkHash([1; 32]),
            ChunkHash([2; 32]),
            ChunkHash([3; 32]),
            ChunkHash([3; 32]),
        ];
        assert_ne!(merkle_root(&a), merkle_root(&b));
    }
}
