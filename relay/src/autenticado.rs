//! Provar ao relay que se é quem se diz — sem o relay guardar segredo nenhum.
//!
//! A ideia é simples e evita um passo de registo inteiro: **a impressão digital
//! deriva das chaves**. Quem pergunta manda as suas chaves públicas e uma
//! assinatura; o relay confirma que a impressão digital sai mesmo dessas chaves
//! e que a assinatura bate. Não precisa de ter visto o dispositivo antes, nem de
//! guardar nada.
//!
//! **Isto não substitui TLS.** Uma assinatura interceptada pode ser repetida
//! dentro da janela de tolerância, e o conteúdo do pedido vai em claro. O que
//! isto fecha é a listagem da caixa por quem simplesmente sabe a impressão
//! digital de alguém — que é pública, anda anunciada na rede local. O sigilo do
//! canal é do TLS, que o relay exposto à internet leva de qualquer maneira.

use axum::http::{HeaderMap, StatusCode};
use pombo_correio_core::{identity::PublicIdentity, protocolo::mensagem_assinada};

/// Diferença tolerada entre o relógio de quem pede e o do relay.
///
/// Cinco minutos é folgado para relógios em NTP e apertado para quem repita um
/// pedido gravado. Sem janela nenhuma, um telemóvel com o relógio um segundo à
/// frente deixava de conseguir usar o relay — falha invisível e enlouquecedora.
const JANELA: u64 = 300;

pub struct Prova {
    /// A impressão digital de quem pede, já confirmada.
    pub impressao: String,
}

fn agora() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn cabecalho<'a>(h: &'a HeaderMap, nome: &str) -> Option<&'a str> {
    h.get(nome)?.to_str().ok()
}

/// Reconstrói uma identidade pública a partir das duas chaves em hexadecimal.
///
/// O nome legível fica de fora de propósito: não entra na impressão digital nem
/// na verificação da assinatura, por isso o relay não tem que saber como se
/// chama o telemóvel de ninguém.
fn identidade_de(hexa: &str) -> Option<PublicIdentity> {
    let b = hex::decode(hexa).ok()?;
    if b.len() != 64 {
        return None;
    }
    Some(PublicIdentity {
        verifying: b[..32].try_into().ok()?,
        encrypting: b[32..].try_into().ok()?,
        label: String::new(),
    })
}

/// Confirma quem está do outro lado, ou diz porquê não.
pub fn verificar(
    cabecalhos: &HeaderMap,
    acao: &str,
    extra: &str,
) -> Result<Prova, (StatusCode, String)> {
    let recusa = |m: &str| (StatusCode::UNAUTHORIZED, m.to_string());

    let quem = cabecalho(cabecalhos, "x-pombo-identidade")
        .and_then(identidade_de)
        .ok_or_else(|| recusa("identidade em falta ou ilegível"))?;

    let instante: u64 = cabecalho(cabecalhos, "x-pombo-instante")
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| recusa("instante em falta"))?;

    let agora = agora();
    if instante.abs_diff(agora) > JANELA {
        return Err(recusa("o teu relógio está muito longe do meu"));
    }

    let assinatura: [u8; 64] = cabecalho(cabecalhos, "x-pombo-assinatura")
        .and_then(|s| hex::decode(s).ok())
        .and_then(|v| v.try_into().ok())
        .ok_or_else(|| recusa("assinatura em falta ou ilegível"))?;

    let impressao = hex::encode(quem.fingerprint())[..16].to_string();
    quem.verify(mensagem_assinada(acao, &impressao, extra, instante).as_bytes(), &assinatura)
        .map_err(|_| recusa("assinatura não bate"))?;

    Ok(Prova { impressao })
}

/// Quem pode depositar aqui.
///
/// A assinatura prova que alguém é dono de uma chave — **não** que tenha alguma
/// coisa a ver connosco: gerar um par novo custa microssegundos. Sem esta lista,
/// qualquer um na internet enchia o disco de casa. Um ficheiro de texto com uma
/// impressão digital por linha chega e sobra; é pequeno, muda quando entra um
/// aparelho novo, e copia-se para os dois nós à mão — ou, desde o S10 T3, sozinho
/// por convite (`POST /convite`).
///
/// `RwLock`, não `tokio::sync::Mutex`: `deixa()` é chamado de código síncrono
/// nos dois handlers que já existiam, e passar isso a `async` só por causa de
/// `autorizar()` — uma escrita rara, só no convite — espalhava `.await` sem
/// necessidade nenhuma.
#[derive(Default)]
pub struct Autorizados {
    lista: Option<std::sync::RwLock<std::collections::HashSet<String>>>,
    /// Onde persistir uma adição automática. `None` em modo aberto (nada a
    /// escrever, tudo já é permitido) — nunca `None` quando `lista` é `Some`.
    caminho: Option<std::path::PathBuf>,
}

impl Autorizados {
    /// Sem ficheiro, aceita-se qualquer identidade válida. É o que serve para
    /// correr na rede de casa, onde ninguém de fora chega.
    pub fn abertos() -> Self {
        Self { lista: None, caminho: None }
    }

    pub fn do_ficheiro(caminho: &std::path::Path) -> anyhow::Result<Self> {
        let texto = std::fs::read_to_string(caminho)?;
        let lista = texto
            .lines()
            .map(|l| l.split('#').next().unwrap_or("").trim().to_lowercase())
            .filter(|l| !l.is_empty())
            .collect();
        Ok(Self {
            lista: Some(std::sync::RwLock::new(lista)),
            caminho: Some(caminho.to_path_buf()),
        })
    }

    pub fn deixa(&self, impressao: &str) -> bool {
        match &self.lista {
            Some(lista) => lista.read().unwrap().contains(&impressao.to_lowercase()),
            None => true,
        }
    }

    /// Acrescenta `impressao` à lista, em memória e no ficheiro em disco — sem
    /// a segunda parte, um reinício do relay esquecia quem foi autorizado a
    /// meio (S10 T3), e o dispositivo voltava a levar 403 do nada. Em modo
    /// aberto não há nada a fazer: tudo já é permitido.
    pub fn autorizar(&self, impressao: &str) -> std::io::Result<()> {
        let (Some(lista), Some(caminho)) = (&self.lista, &self.caminho) else {
            return Ok(());
        };
        let impressao = impressao.to_lowercase();
        {
            let mut lista = lista.write().unwrap();
            if !lista.insert(impressao.clone()) {
                return Ok(()); // já lá estava — nada a escrever de novo
            }
        }

        use std::io::Write;
        let escrita = (|| -> std::io::Result<()> {
            writeln!(
                std::fs::OpenOptions::new().append(true).create(true).open(caminho)?,
                "{impressao} # autorizado automaticamente por convite"
            )
        })();

        // Sem isto, uma escrita falhada (disco cheio) deixava a autorização
        // só em memória — válida até o relay reiniciar, e depois 403 sem
        // explicação nenhuma, porque o ficheiro nunca a teve.
        if escrita.is_err() {
            lista.write().unwrap().remove(&impressao);
        }
        escrita
    }
}

#[cfg(test)]
mod testes {
    use super::*;
    use pombo_correio_core::identity::Identity;

    fn cabecalhos(eu: &Identity, acao: &str, extra: &str, instante: u64) -> HeaderMap {
        let p = eu.public();
        let impressao = hex::encode(p.fingerprint())[..16].to_string();
        let sig = eu.sign(mensagem_assinada(acao, &impressao, extra, instante).as_bytes());

        let mut h = HeaderMap::new();
        h.insert(
            "x-pombo-identidade",
            format!("{}{}", hex::encode(p.verifying), hex::encode(p.encrypting))
                .parse()
                .unwrap(),
        );
        h.insert("x-pombo-instante", instante.to_string().parse().unwrap());
        h.insert("x-pombo-assinatura", hex::encode(sig).parse().unwrap());
        h
    }

    #[test]
    fn quem_assina_prova_a_sua_impressao() {
        let eu = Identity::generate("teste");
        let esperada = hex::encode(eu.public().fingerprint())[..16].to_string();
        let p = verificar(&cabecalhos(&eu, "caixa", "", agora()), "caixa", "").unwrap();
        assert_eq!(p.impressao, esperada);
    }

    #[test]
    fn assinatura_de_outra_acao_nao_serve() {
        // Sem a ação dentro da mensagem, uma assinatura apanhada a listar a caixa
        // servia para depositar.
        let eu = Identity::generate("teste");
        let h = cabecalhos(&eu, "caixa", "", agora());
        assert!(verificar(&h, "deposito", "").is_err());
    }

    #[test]
    fn assinatura_de_outro_deposito_nao_serve() {
        let eu = Identity::generate("teste");
        let h = cabecalhos(&eu, "deposito", "aaa|100", agora());
        assert!(verificar(&h, "deposito", "bbb|100").is_err());
    }

    #[test]
    fn assinatura_velha_nao_serve() {
        // O que impede repetir um pedido gravado ontem.
        let eu = Identity::generate("teste");
        let h = cabecalhos(&eu, "caixa", "", agora() - JANELA - 10);
        assert!(verificar(&h, "caixa", "").is_err());
    }

    #[test]
    fn nao_se_pode_pedir_a_caixa_de_outro() {
        // A impressão sai das chaves que vêm no pedido, não de um parâmetro:
        // não há sequer forma de pedir a de outra pessoa.
        let eu = Identity::generate("eu");
        let ele = Identity::generate("ele");
        let p = verificar(&cabecalhos(&eu, "caixa", "", agora()), "caixa", "").unwrap();
        assert_ne!(p.impressao, hex::encode(ele.public().fingerprint())[..16]);
    }

    #[test]
    fn chaves_trocadas_nao_passam() {
        // Mandar as chaves de outra pessoa e assinar com as suas: a impressão
        // deixa de bater com a chave que verifica.
        let eu = Identity::generate("eu");
        let ele = Identity::generate("ele");
        let mut h = cabecalhos(&eu, "caixa", "", agora());
        let p = ele.public();
        h.insert(
            "x-pombo-identidade",
            format!("{}{}", hex::encode(p.verifying), hex::encode(p.encrypting))
                .parse()
                .unwrap(),
        );
        assert!(verificar(&h, "caixa", "").is_err());
    }

    #[test]
    fn lista_de_autorizados_filtra_quem_nao_e_de_casa() {
        // Assinar prova posse de uma chave, não pertença a esta casa: um par
        // novo custa microssegundos.
        let d = std::env::temp_dir().join(format!("pombo-aut-{}", std::process::id()));
        std::fs::write(&d, "AB12CD34ef567890  # tablet\n\n# um comentário\n").unwrap();
        let a = Autorizados::do_ficheiro(&d).unwrap();
        assert!(a.deixa("ab12cd34ef567890"));
        assert!(!a.deixa("0000000000000000"));
        assert!(Autorizados::abertos().deixa("0000000000000000"));
        std::fs::remove_file(&d).unwrap();
    }

    #[test]
    fn autorizar_entra_em_memoria_e_sobrevive_a_reler_o_ficheiro() {
        // Sem a escrita a sério em disco, um reinício do relay esquecia quem
        // foi autorizado a meio (S10 T3) — o próprio motivo de existir do
        // método, não só o efeito em memória.
        let d = std::env::temp_dir().join(format!("pombo-aut-autorizar-{}", std::process::id()));
        std::fs::write(&d, "ab12cd34ef567890 # tablet\n").unwrap();
        let a = Autorizados::do_ficheiro(&d).unwrap();
        assert!(!a.deixa("1111111111111111"));

        a.autorizar("1111111111111111").unwrap();
        assert!(a.deixa("1111111111111111"));

        let relido = Autorizados::do_ficheiro(&d).unwrap();
        assert!(relido.deixa("1111111111111111"));
        assert!(relido.deixa("ab12cd34ef567890"), "não pode ter apagado quem já lá estava");
        std::fs::remove_file(&d).unwrap();
    }

    #[test]
    fn autorizar_duas_vezes_nao_duplica_a_linha() {
        let d = std::env::temp_dir().join(format!("pombo-aut-dup-{}", std::process::id()));
        std::fs::write(&d, "").unwrap();
        let a = Autorizados::do_ficheiro(&d).unwrap();
        a.autorizar("2222222222222222").unwrap();
        a.autorizar("2222222222222222").unwrap();
        let texto = std::fs::read_to_string(&d).unwrap();
        assert_eq!(texto.matches("2222222222222222").count(), 1);
        std::fs::remove_file(&d).unwrap();
    }

    #[test]
    fn autorizar_em_modo_aberto_nao_escreve_nada() {
        // Sem ficheiro, tudo já é permitido — não há onde persistir, e não
        // faz falta.
        Autorizados::abertos().autorizar("qualquer").unwrap();
    }
}
