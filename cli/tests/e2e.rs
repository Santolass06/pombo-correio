//! Ida e volta real: dois processos, um socket, um ficheiro.
//!
//! Cobre o que os testes de unidade não alcançam — o enquadramento do fluxo no
//! recetor, que é a parte do protocolo mais fácil de partir sem dar por isso.

use std::io::{BufRead, BufReader, Write};

/// Diretório temporário que se limpa sozinho.
struct Temp(std::path::PathBuf);

impl Temp {
    fn novo(nome: &str) -> Self {
        let d = std::env::temp_dir().join(format!("pombo-e2e-{nome}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Self(d)
    }
    fn join(&self, n: &str) -> std::path::PathBuf {
        self.0.join(n)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Mata o processo quando o teste acaba, mesmo se falhar a meio.
struct Processo(std::process::Child);

impl Drop for Processo {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Arranca o recetor e espera até saber em que porta ficou.
///
/// Pedir a porta 0 deixa o sistema escolher uma livre; adivinhar um número
/// fixo tornaria o teste dependente do que mais estivesse a correr na máquina.
fn arrancar_recetor(dados: &std::path::Path) -> (Processo, u16) {
    let mut filho = std::process::Command::new(env!("CARGO_BIN_EXE_pombo"))
        .args(["--dados", dados.to_str().unwrap(), "receber", "--porta", "0"])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("arrancar o recetor");

    let saida = BufReader::new(filho.stdout.take().unwrap());
    let (envia, recebe) = std::sync::mpsc::channel();

    // O tubo tem de continuar a ser drenado depois de encontrada a porta. Se o
    // leitor fosse largado aqui, o `println!` seguinte do recetor apanhava
    // EPIPE e rebentava o processo a meio da transferência.
    std::thread::spawn(move || {
        let mut anunciada = false;
        for linha in saida.lines().map_while(Result::ok) {
            if !anunciada && let Some((_, p)) = linha.rsplit_once("porta ") {
                anunciada = true;
                let _ = envia.send(p.trim().parse::<u16>().expect("porta numérica"));
            }
        }
    });

    let porta = recebe
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("o recetor não anunciou a porta");

    (Processo(filho), porta)
}

fn enviar(dados: &std::path::Path, ficheiro: &std::path::Path, porta: u16) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_pombo"))
        .args([
            "--dados",
            dados.to_str().unwrap(),
            "enviar",
            ficheiro.to_str().unwrap(),
            &format!("127.0.0.1:{porta}"),
        ])
        .output()
        .expect("correr o emissor")
}

#[test]
fn ficheiro_atravessa_a_rede_byte_a_byte() {
    let emissor = Temp::novo("emissor");
    let recetor = Temp::novo("recetor");

    // Tamanho escolhido para dar vários blocos e um último bloco incompleto:
    // é onde o enquadramento do fluxo se parte.
    let tamanho = pombo_correio_core::chunk::CHUNK_SIZE * 3 + 12_345;
    let conteudo: Vec<u8> = (0..tamanho).map(|i| (i % 251) as u8).collect();

    let origem = emissor.join("ferias.bin");
    std::fs::File::create(&origem)
        .unwrap()
        .write_all(&conteudo)
        .unwrap();

    let (_guarda, porta) = arrancar_recetor(&recetor.0);
    let saida = enviar(&emissor.0, &origem, porta);
    assert!(
        saida.status.success(),
        "envio falhou: {}",
        String::from_utf8_lossy(&saida.stderr)
    );

    let chegou = std::fs::read(recetor.join("recebidos/ferias.bin")).expect("ficheiro recebido");
    assert_eq!(chegou.len(), conteudo.len(), "tamanho diferente");
    assert!(chegou == conteudo, "conteúdo diferente");
}

#[test]
fn reenviar_o_mesmo_ficheiro_nao_o_duplica() {
    // A segunda tentativa encontra a transferência já completa e não escreve
    // nada de novo — é o caso degenerado da retoma.
    let emissor = Temp::novo("emissor2");
    let recetor = Temp::novo("recetor2");

    let origem = emissor.join("nota.txt");
    std::fs::write(&origem, b"conteudo pequeno").unwrap();

    let (_guarda, porta) = arrancar_recetor(&recetor.0);
    assert!(enviar(&emissor.0, &origem, porta).status.success());
    assert!(enviar(&emissor.0, &origem, porta).status.success());

    assert_eq!(
        std::fs::read(recetor.join("recebidos/nota.txt")).unwrap(),
        b"conteudo pequeno"
    );
}
