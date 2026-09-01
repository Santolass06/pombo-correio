//! Quem envia passa a saber as chaves de quem recebeu — sem o dar por conhecido.
//!
//! O buraco: só se conseguia deixar um ficheiro no relay para alguém que já nos
//! **tivesse enviado** alguma coisa, porque só quem recebe é que registava as
//! chaves. Na prática, ninguém conseguia mandar nada a quem tinha acabado de
//! instalar a app.
//!
//! A armadilha da correção óbvia é o segundo teste deste ficheiro, e é a razão
//! de existirem dois registos em vez de um.

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use pombo_correio_core::identity::Identity;
use pombo_correio_rede::{Metricas, Ouvinte, conhecidos, enviar, receber};

struct Temp(PathBuf);

impl Temp {
    fn nova(nome: &str) -> Self {
        let d = std::env::temp_dir().join(format!(
            "pombo-chaves-{}-{}-{nome}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&d).unwrap();
        Self(d)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[derive(Default)]
struct Registo(Mutex<Vec<String>>);

impl Registo {
    fn ouvinte(self: &Arc<Self>) -> Ouvinte {
        let eu = self.clone();
        Arc::new(move |e| {
            let nome = match e {
                pombo_correio_rede::Evento::Enviado { .. } => "enviado",
                pombo_correio_rede::Evento::Recebido { .. } => "recebido",
                pombo_correio_rede::Evento::Falhou { .. } => "falhou",
                pombo_correio_rede::Evento::Oferta { novo, .. } => {
                    if novo { "oferta-nova" } else { "oferta" }
                }
                _ => return,
            };
            eu.0.lock().unwrap().push(nome.into());
        })
    }

    fn viu(&self, q: &str) -> bool {
        self.0.lock().unwrap().iter().any(|x| x == q)
    }
}

/// Põe um recetor de pé e devolve (porta, identidade **pública** dele).
///
/// A privada fica no servidor e não sai — a `Identity` nem é `Clone`, de
/// propósito.
async fn recetor_de_pe(
    dir: &Temp,
    nome: &str,
    ouvinte: Ouvinte,
) -> (u16, pombo_correio_core::identity::PublicIdentity) {
    let quem = Identity::generate(nome);
    let publica = quem.public();

    let escuta = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let porta = escuta.local_addr().unwrap().port();
    drop(escuta);

    let dd = dir.0.join("recebidos");
    let cc = dir.0.join("conhecidos.json");
    tokio::spawn(async move {
        let _ = receber::servir(
            quem,
            dd,
            cc,
            porta,
            ouvinte,
            Default::default(),
            Metricas::desligada(),
        )
        .await;
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    (porta, publica)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enviar_ensina_as_chaves_de_quem_recebeu() {
    // O caso de casa: a mãe instala a app, eu mando-lhe uma foto pela rede
    // local, e a partir daí consigo deixar-lhe coisas no relay mesmo com o
    // telemóvel dela desligado.
    let casa_dele = Temp::nova("recebe");
    let casa_minha = Temp::nova("envia");
    let reg = Arc::new(Registo::default());

    let (porta, ele) = recetor_de_pe(&casa_dele, "Telemóvel da mãe", pombo_correio_rede::silencio()).await;
    let curta = &hex::encode(ele.fingerprint())[..16];

    // Antes do envio não se sabe nada dele — é exatamente o buraco.
    assert!(
        conhecidos::chaves_para(&casa_minha.0, curta).is_none(),
        "sabia as chaves de alguém com quem nunca falou"
    );

    let origem = casa_minha.0.join("foto.jpg");
    std::fs::write(&origem, vec![3u8; 300_000]).unwrap();
    let addr = format!("127.0.0.1:{porta}").parse().unwrap();
    enviar::enviar(
        &Identity::generate("o meu portátil"),
        &origem,
        addr,
        reg.ouvinte(),
        Default::default(),
        &Metricas::desligada(),
        0,
        Some(&casa_minha.0),
    )
    .await
    .unwrap();
    assert!(reg.viu("enviado"), "o envio não chegou ao fim");

    // E agora sabe-se, com as chaves certas — as mesmas que fazem a impressão.
    let aprendida = conhecidos::chaves_para(&casa_minha.0, curta)
        .expect("não aprendeu as chaves de quem acabou de receber");
    assert_eq!(aprendida, ele);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn aprender_as_chaves_nao_cala_o_aviso_de_dispositivo_novo() {
    // A armadilha da correção óbvia, e a razão de haver dois ficheiros.
    //
    // Se as chaves aprendidas a enviar fossem parar ao `conhecidos.json`, um
    // aparelho escolhido por um nome anunciado no mDNS — que não autentica
    // ninguém — passaria a ser "conhecido", e o número de segurança deixaria de
    // aparecer quando ele nos enviasse alguma coisa pela primeira vez. Trocava
    // um incómodo por um buraco.
    let casa_minha = Temp::nova("aviso-envio");
    let casa_dele = Temp::nova("aviso-recebe");

    // Eu envio-lhe primeiro.
    let (porta_dele, ele) = recetor_de_pe(&casa_dele, "Tablet", pombo_correio_rede::silencio()).await;
    let origem = casa_minha.0.join("x.bin");
    std::fs::write(&origem, vec![9u8; 200_000]).unwrap();
    let eu = Identity::generate("o meu portátil");
    enviar::enviar(
        &eu,
        &origem,
        format!("127.0.0.1:{porta_dele}").parse().unwrap(),
        pombo_correio_rede::silencio(),
        Default::default(),
        &Metricas::desligada(),
        0,
        Some(&casa_minha.0),
    )
    .await
    .unwrap();

    // Aprendi as chaves dele...
    let curta_dele = hex::encode(ele.fingerprint())[..16].to_string();
    assert!(
        conhecidos::chaves_para(&casa_minha.0, &curta_dele).is_some(),
        "não aprendeu as chaves — o resto do teste não provaria nada"
    );

    // ...mas o `conhecidos.json` — o que decide o aviso — continua sem existir.
    assert!(
        !casa_minha.0.join("conhecidos.json").exists(),
        "enviar escreveu no registo que decide o aviso de dispositivo novo"
    );

    // A prova a sério: agora é ELE que me envia, e eu tenho de ser avisado de
    // que é um dispositivo desconhecido.
    let reg_meu = Arc::new(Registo::default());
    let (minha_porta, _) = recetor_de_pe(&casa_minha, "eu", reg_meu.ouvinte()).await;

    let origem2 = casa_dele.0.join("y.bin");
    std::fs::write(&origem2, vec![1u8; 200_000]).unwrap();
    enviar::enviar(
        &Identity::generate("Tablet"),
        &origem2,
        format!("127.0.0.1:{minha_porta}").parse().unwrap(),
        pombo_correio_rede::silencio(),
        Default::default(),
        &Metricas::desligada(),
        0,
        Some(&casa_dele.0),
    )
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;

    assert!(
        reg_meu.viu("oferta-nova"),
        "o aviso de dispositivo novo não apareceu — saber as chaves de alguém passou a valer por o ter recebido"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn receber_ja_ensina_a_chave_de_quem_nunca_recebeu_de_nos() {
    // Parecia um buraco por fechar (ficou escrito assim no PLANO.md, a
    // acompanhar o teste do Android) mas não é: `chaves_para` já consulta o
    // `conhecidos.json`, e esse ficheiro já é escrito, incondicional, em
    // qualquer oferta assinada que se receba — antes mesmo deste teste
    // existir. Confirmado ao tentar "corrigir": comentar a escrita extra em
    // `vistos.json` que ia acrescentar aqui não fez este teste falhar. Fica o
    // teste, sem o código morto, a proteger o comportamento real.
    let casa_minha = Temp::nova("so-recebe");
    let casa_dele = Temp::nova("so-envia");

    let (minha_porta, _) = recetor_de_pe(&casa_minha, "eu", pombo_correio_rede::silencio()).await;

    let eu_quem_envia = Identity::generate("Tablet");
    let a_chave_dele = eu_quem_envia.public();
    let curta_dele = hex::encode(a_chave_dele.fingerprint())[..16].to_string();

    // Antes de ele me enviar nada, não sei as chaves dele.
    assert!(
        conhecidos::chaves_para(&casa_minha.0, &curta_dele).is_none(),
        "já sabia as chaves de alguém com quem nunca falou"
    );

    let origem = casa_dele.0.join("z.bin");
    std::fs::write(&origem, vec![7u8; 200_000]).unwrap();
    enviar::enviar(
        &eu_quem_envia,
        &origem,
        format!("127.0.0.1:{minha_porta}").parse().unwrap(),
        pombo_correio_rede::silencio(),
        Default::default(),
        &Metricas::desligada(),
        0,
        Some(&casa_dele.0),
    )
    .await
    .unwrap();

    // Nunca lhe enviei nada — só recebi. Ainda assim, tenho de saber a chave.
    let aprendida = conhecidos::chaves_para(&casa_minha.0, &curta_dele)
        .expect("não aprendeu a chave de quem só me enviou, nunca recebeu");
    assert_eq!(aprendida, a_chave_dele);
}
