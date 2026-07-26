//! O depósito: onde ficam os bytes à espera de quem os vai buscar.
//!
//! **Isto não é um recetor.** O recetor de `rede/` abre o envelope com a sua
//! identidade e verifica cada bloco contra o manifesto; o relay não é o
//! destinatário, não tem chave, e não pode ter. Aqui guardam-se bytes opacos com
//! um identificador. O relay não sabe o nome do ficheiro (vai no manifesto
//! selado), não sabe o tamanho em claro, e não distingue um vídeo de um PDF.
//!
//! Quem verifica continua a ser quem recebe. A garantia byte-exata não muda de
//! dono por passar por aqui.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// Ficha de um depósito: tudo o que o relay sabe, que é pouco de propósito.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ficha {
    /// Impressão digital de quem vai buscar. Pública — é o que a rede local já
    /// anuncia — e serve só para a caixa. Não autentica nada.
    pub para: String,
    /// Tamanho total dos bytes selados, declarado por quem envia. Serve para
    /// saber se o depósito está completo e para recusar o que não cabe.
    pub tamanho: u64,
    /// Segundos desde a época. Não é `SystemTime` para o ficheiro em disco ser
    /// legível e comparável sem surpresas de fuso.
    pub criado: u64,
    /// A oferta, tal e qual como quem envia a escreveu. O relay nunca a
    /// interpreta — só a guarda e a devolve. É o que permite mudar o formato de
    /// fio sem lhe tocar.
    pub oferta: String,
}

/// Estado de um depósito, para quem pergunta.
#[derive(Debug, Serialize, Deserialize)]
pub struct Estado {
    pub oferta: String,
    /// Bytes selados já cá dentro. É por aqui que uma subida interrompida
    /// retoma.
    pub bytes: u64,
    pub tamanho: u64,
    pub completo: bool,
}

/// Um depósito não pode crescer sem fim: o relay corre em casa, em duas máquinas
/// fracas, e um disco cheio derruba os dois caminhos ao mesmo tempo.
pub const TAMANHO_MAX: u64 = 32 * 1024 * 1024 * 1024;

/// Quanto tempo um depósito espera por quem o venha buscar.
pub const VALIDADE: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 3600);

pub struct Deposito {
    raiz: PathBuf,
}

fn agora() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Um identificador que ninguém adivinha.
///
/// **Não é a raiz de Merkle**, ao contrário do que se faz na LAN, e a diferença
/// é deliberada: na raiz, quem desconfiasse de que passou por aqui um ficheiro
/// concreto calculava-lha e perguntava, e num servidor exposto à internet isso
/// deitaria fora metade do que o ciframento comprou. Além disso, dois envios do
/// mesmo ficheiro para pessoas diferentes partilhariam depósito.
///
/// 128 bits: é também o que vai no link sem instalação, e é por ser
/// imprevisível que o link se aguenta sozinho.
fn novo_id() -> String {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).expect("sem fonte de aleatoriedade do sistema");
    hex::encode(b)
}

/// Recusa qualquer coisa que não seja um identificador nosso.
///
/// Vem do URL, e sem isto um `..` escreveria ou leria fora da pasta. Exigir
/// exatamente 32 dígitos hexadecimais é mais apertado — e mais fácil de ler — do
/// que tentar limpar o que vem.
fn valido(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|c| c.is_ascii_hexdigit())
}

impl Deposito {
    pub fn novo(raiz: impl Into<PathBuf>) -> Result<Self> {
        let raiz = raiz.into();
        std::fs::create_dir_all(&raiz).context("não consegui criar a pasta dos depósitos")?;
        Ok(Self { raiz })
    }

    fn ficha_em(&self, id: &str) -> PathBuf {
        self.raiz.join(format!("{id}.json"))
    }

    pub fn blocos_em(&self, id: &str) -> PathBuf {
        self.raiz.join(format!("{id}.blocos"))
    }

    /// Abre espaço para um depósito novo e devolve o seu identificador.
    pub async fn criar(&self, para: &str, tamanho: u64, oferta: String) -> Result<String> {
        if tamanho == 0 || tamanho > TAMANHO_MAX {
            bail!("tamanho fora do que este relay aceita");
        }

        let id = novo_id();
        let ficha = Ficha {
            para: para.to_string(),
            tamanho,
            criado: agora(),
            oferta,
        };
        tokio::fs::write(self.ficha_em(&id), serde_json::to_vec(&ficha)?).await?;
        Ok(id)
    }

    pub async fn ficha(&self, id: &str) -> Result<Ficha> {
        if !valido(id) {
            bail!("identificador inválido");
        }
        let bruto = tokio::fs::read(self.ficha_em(id)).await.context("depósito desconhecido")?;
        Ok(serde_json::from_slice(&bruto)?)
    }

    /// Quantos bytes selados já cá estão.
    pub async fn bytes(&self, id: &str) -> u64 {
        tokio::fs::metadata(self.blocos_em(id)).await.map(|m| m.len()).unwrap_or(0)
    }

    pub async fn estado(&self, id: &str) -> Result<Estado> {
        let ficha = self.ficha(id).await?;
        let bytes = self.bytes(id).await;
        Ok(Estado {
            oferta: ficha.oferta,
            bytes,
            completo: bytes >= ficha.tamanho,
            tamanho: ficha.tamanho,
        })
    }

    /// Prepara o ficheiro para receber a partir de `de`.
    ///
    /// **`de` menor do que o que cá está trunca**, e é essa a regra que faz a
    /// retoma funcionar: quem envia só pode recomeçar na fronteira de um bloco,
    /// e o que cá estava a mais é meio bloco que ninguém consegue abrir. Pedir
    /// para começar *à frente* do que cá está é que não tem remédio — deixaria um
    /// buraco no meio — e dá erro.
    ///
    /// Devolve o ficheiro **já posicionado** em `de`. Truncar sem posicionar
    /// deixava a escrita a começar no zero e o conteúdo saía trocado — foi o que
    /// o teste `retomar_atras_trunca_o_que_ficou_a_meio` apanhou.
    pub async fn abrir_para_subir(&self, id: &str, de: u64) -> Result<tokio::fs::File> {
        let ficha = self.ficha(id).await?;
        let tem = self.bytes(id).await;
        if de > tem {
            bail!("pediu para começar no byte {de} mas só tenho {tem}");
        }
        if de >= ficha.tamanho {
            bail!("já está tudo cá");
        }

        use tokio::io::AsyncSeekExt;

        let caminho = self.blocos_em(id);
        let mut f = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&caminho)
            .await?;
        f.set_len(de).await?;
        f.seek(std::io::SeekFrom::Start(de)).await?;
        Ok(f)
    }

    pub async fn apagar(&self, id: &str) -> Result<()> {
        if !valido(id) {
            bail!("identificador inválido");
        }
        let _ = tokio::fs::remove_file(self.blocos_em(id)).await;
        tokio::fs::remove_file(self.ficha_em(id)).await.context("depósito desconhecido")?;
        Ok(())
    }

    /// O que está à espera de alguém. Devolve identificadores, do mais antigo ao
    /// mais recente.
    pub async fn caixa_de(&self, impressao: &str) -> Result<Vec<String>> {
        let mut achados: Vec<(u64, String)> = Vec::new();
        let mut it = tokio::fs::read_dir(&self.raiz).await?;
        while let Some(e) = it.next_entry().await? {
            let nome = e.file_name().to_string_lossy().into_owned();
            let Some(id) = nome.strip_suffix(".json") else { continue };
            let Ok(ficha) = self.ficha(id).await else { continue };
            if ficha.para == impressao {
                achados.push((ficha.criado, id.to_string()));
            }
        }
        achados.sort();
        Ok(achados.into_iter().map(|(_, id)| id).collect())
    }

    /// Deita fora o que passou da validade. Devolve quantos foram.
    ///
    /// Vive aqui, e não numa tarefa periódica escondida, para ter teste: a
    /// limpeza é o que impede o disco de encher, e um disco cheio derruba os dois
    /// nós ao mesmo tempo.
    pub async fn limpar(&self, validade: std::time::Duration) -> Result<usize> {
        let limite = agora().saturating_sub(validade.as_secs());
        let mut fora = 0;
        let mut it = tokio::fs::read_dir(&self.raiz).await?;
        while let Some(e) = it.next_entry().await? {
            let nome = e.file_name().to_string_lossy().into_owned();
            let Some(id) = nome.strip_suffix(".json") else { continue };
            let Ok(ficha) = self.ficha(id).await else { continue };
            if ficha.criado < limite {
                let _ = self.apagar(id).await;
                fora += 1;
            }
        }
        Ok(fora)
    }

    pub fn raiz(&self) -> &Path {
        &self.raiz
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    fn temporaria(nome: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pombo-dep-{}-{nome}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn identificador_de_fora_nao_sai_da_pasta() {
        // Vem do URL. Sem isto, `..` lia ou escrevia onde lhe apetecesse.
        assert!(!valido("../../etc/passwd"));
        assert!(!valido("abc"));
        assert!(!valido(&"g".repeat(32)));
        assert!(valido(&"a1".repeat(16)));
    }

    #[tokio::test]
    async fn dois_depositos_do_mesmo_ficheiro_nao_se_misturam() {
        // O identificador é aleatório, não a raiz de Merkle: o mesmo ficheiro
        // para duas pessoas tem de dar dois depósitos.
        let d = Deposito::novo(temporaria("distintos")).unwrap();
        let a = d.criar("aaa", 10, "{}".into()).await.unwrap();
        let b = d.criar("bbb", 10, "{}".into()).await.unwrap();
        assert_ne!(a, b);
    }

    #[tokio::test]
    async fn retomar_atras_trunca_o_que_ficou_a_meio() {
        // Uma subida cortada deixa meio bloco em disco, que ninguém consegue
        // abrir. Quem envia recomeça na fronteira do bloco anterior, e o que
        // estava a mais tem de desaparecer — senão o ficheiro fica com lixo pelo
        // meio e só se descobre na verificação, no fim.
        use tokio::io::AsyncWriteExt;

        let d = Deposito::novo(temporaria("truncar")).unwrap();
        let id = d.criar("aaa", 100, "{}".into()).await.unwrap();

        let mut f = d.abrir_para_subir(&id, 0).await.unwrap();
        f.write_all(&[1u8; 60]).await.unwrap();
        f.flush().await.unwrap();
        drop(f);
        assert_eq!(d.bytes(&id).await, 60);

        let mut f = d.abrir_para_subir(&id, 40).await.unwrap();
        f.write_all(&[2u8; 10]).await.unwrap();
        f.flush().await.unwrap();
        drop(f);

        let bytes = tokio::fs::read(d.blocos_em(&id)).await.unwrap();
        assert_eq!(bytes.len(), 50);
        assert_eq!(&bytes[..40], &[1u8; 40]);
        assert_eq!(&bytes[40..], &[2u8; 10]);
    }

    #[tokio::test]
    async fn retomar_a_frente_e_recusado() {
        // Deixaria um buraco no meio que nada preencheria depois.
        let d = Deposito::novo(temporaria("buraco")).unwrap();
        let id = d.criar("aaa", 100, "{}".into()).await.unwrap();
        assert!(d.abrir_para_subir(&id, 10).await.is_err());
    }

    #[tokio::test]
    async fn a_caixa_so_mostra_o_que_e_para_a_pessoa() {
        let d = Deposito::novo(temporaria("caixa")).unwrap();
        let meu = d.criar("euu", 10, "{}".into()).await.unwrap();
        let _outro = d.criar("elee", 10, "{}".into()).await.unwrap();

        assert_eq!(d.caixa_de("euu").await.unwrap(), vec![meu]);
        assert!(d.caixa_de("ninguem").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn o_que_passa_da_validade_desaparece_com_os_bytes() {
        use tokio::io::AsyncWriteExt;

        let d = Deposito::novo(temporaria("validade")).unwrap();
        let velho = d.criar("aaa", 10, "{}".into()).await.unwrap();
        let mut f = d.abrir_para_subir(&velho, 0).await.unwrap();
        f.write_all(&[9u8; 10]).await.unwrap();
        f.flush().await.unwrap();
        drop(f);

        // Envelhecer a ficha à mão em vez de esperar: o que se testa é a regra,
        // não o relógio.
        let mut ficha = d.ficha(&velho).await.unwrap();
        ficha.criado = agora().saturating_sub(3600);
        tokio::fs::write(d.ficha_em(&velho), serde_json::to_vec(&ficha).unwrap()).await.unwrap();

        let novo = d.criar("aaa", 10, "{}".into()).await.unwrap();

        assert_eq!(d.limpar(std::time::Duration::from_secs(60)).await.unwrap(), 1);
        assert!(d.ficha(&velho).await.is_err());
        // Os bytes vão com a ficha: deixá-los seria o disco a encher em silêncio.
        assert!(!d.blocos_em(&velho).exists());
        assert!(d.ficha(&novo).await.is_ok());
    }

    #[tokio::test]
    async fn tamanho_absurdo_e_recusado_a_entrada() {
        let d = Deposito::novo(temporaria("tamanho")).unwrap();
        assert!(d.criar("aaa", 0, "{}".into()).await.is_err());
        assert!(d.criar("aaa", TAMANHO_MAX + 1, "{}".into()).await.is_err());
    }
}
