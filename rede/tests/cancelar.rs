//! Cancelar um envio a meio, e o que fica depois.
//!
//! O que importa provar: cancelar pára mesmo, é reportado como cancelamento e
//! não como falha, e o que já passou fica em disco — retomar tem de continuar a
//! valer.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering::Relaxed},
};
use std::time::Duration;

use pombo_correio_core::identity::Identity;
use pombo_correio_rede::{Evento, Metricas, receber, enviar};

/// Diretório temporário que se limpa sozinho.
struct Temp(std::path::PathBuf);
impl Temp {
    fn novo(n: &str) -> Self {
        let d = std::env::temp_dir().join(format!("pombo-cancel-{n}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Self(d)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Recolhe os eventos por tipo, para o teste os poder interrogar.
#[derive(Default)]
struct Registo {
    tipos: Mutex<Vec<String>>,
}
impl Registo {
    fn ouvinte(self: &Arc<Self>) -> pombo_correio_rede::Ouvinte {
        let eu = self.clone();
        Arc::new(move |e: Evento| {
            let nome = match e {
                Evento::AEscutar { .. } => "a_escutar",
                Evento::Oferta { .. } => "oferta",
                Evento::Destino { .. } => "destino",
                Evento::Progresso { .. } => "progresso",
                Evento::Recebido { .. } => "recebido",
                Evento::Enviado { .. } => "enviado",
                Evento::Cancelado { .. } => "cancelado",
                Evento::Falhou { .. } => "falhou",
            };
            eu.tipos.lock().unwrap().push(nome.into());
        })
    }
    fn viu(&self, tipo: &str) -> bool {
        self.tipos.lock().unwrap().iter().any(|t| t == tipo)
    }
}

// Runtime de várias threads de propósito: o cancelamento é accionado por uma
// tarefa à parte, e numa thread só ela ficava esfomeada pelo trabalho da
// transferência quando os testes correm em paralelo, disparando tarde demais.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelar_para_e_guarda_o_que_ja_passou() {
    let dir = Temp::novo("meio");
    let recetor = Identity::generate("recetor");
    let reg_r = Arc::new(Registo::default());
    let reg_e = Arc::new(Registo::default());

    // Servidor numa porta escolhida pelo sistema.
    let escuta = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let porta = escuta.local_addr().unwrap().port();
    drop(escuta);

    let destino_dir = dir.0.join("recebidos");
    let conhecidos = dir.0.join("conhecidos.json");
    let ouvinte_r = reg_r.ouvinte();
    tokio::spawn(async move {
        let _ = receber::servir(
            recetor,
            destino_dir,
            conhecidos,
            porta,
            ouvinte_r,
            Default::default(),
            pombo_correio_rede::Metricas::desligada(),
        )
        .await;
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Grande o suficiente para durar bem mais do que os 50 ms até ao
    // cancelamento — senão, em loopback, acabava antes.
    let origem = dir.0.join("grande.bin");
    std::fs::write(&origem, vec![7u8; 150_000_000]).unwrap();

    let emissor = Identity::generate("emissor");
    let cancelar = Arc::new(AtomicBool::new(false));

    // Dispara o cancelamento pouco depois do arranque.
    let gatilho = cancelar.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        gatilho.store(true, Relaxed);
    });

    let addr = format!("127.0.0.1:{porta}").parse().unwrap();
    enviar::enviar(&emissor, &origem, addr, reg_e.ouvinte(), cancelar, &Metricas::desligada())
        .await
        .unwrap();

    // Foi reportado como cancelamento, e não como falha nem como sucesso.
    assert!(reg_e.viu("cancelado"), "não reportou cancelamento");
    assert!(!reg_e.viu("enviado"), "reportou envio completo apesar do cancelamento");
    assert!(!reg_e.viu("falhou"), "reportou falha em vez de cancelamento");

    // O ficheiro final não existe — foi interrompido — mas o parcial ficou, o
    // que é o que torna a retoma possível.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !dir.0.join("recebidos/grande.bin").exists(),
        "o ficheiro apareceu completo apesar de cancelado"
    );
    let ha_parcial = std::fs::read_dir(dir.0.join("recebidos"))
        .map(|d| d.filter_map(Result::ok).any(|e| e.path().extension().is_some_and(|x| x == "parcial")))
        .unwrap_or(false);
    assert!(ha_parcial, "não guardou o que já tinha passado — retomar deixa de valer");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelar_no_recetor_para_quem_envia() {
    // O buraco encontrado a testar no Pi: cancelar do lado de quem recebe não
    // parava quem enviava. Agora o recetor regista um sinal por transferência,
    // e accioná-lo fecha a ligação, o que faz o envio terminar.
    let dir = Temp::novo("recetor");
    let recetor = Identity::generate("recetor");
    let reg_e = Arc::new(Registo::default());

    // Registo de canceláveis partilhado com o servidor, como faz a app.
    let cancelaveis: pombo_correio_rede::Cancelaveis = Default::default();

    let escuta = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let porta = escuta.local_addr().unwrap().port();
    drop(escuta);

    let dd = dir.0.join("recebidos");
    let cc = dir.0.join("conhecidos.json");
    let canc_servidor = cancelaveis.clone();
    tokio::spawn(async move {
        let _ = receber::servir(
            recetor,
            dd,
            cc,
            porta,
            pombo_correio_rede::silencio(),
            canc_servidor,
            Metricas::desligada(),
        )
        .await;
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Grande o suficiente para a transferência durar bem mais do que a latência
    // do cancelamento — senão, em loopback, acabava antes de o vigia disparar.
    let origem = dir.0.join("grande.bin");
    std::fs::write(&origem, vec![3u8; 200_000_000]).unwrap();

    // Assim que a transferência aparecer no registo, aciona o cancelamento —
    // é o que o botão da app faz, pelo identificador.
    let vigia = cancelaveis.clone();
    tokio::spawn(async move {
        for _ in 0..2000 {
            if let Some(sinal) = vigia.lock().unwrap().values().next() {
                sinal.store(true, Relaxed);
                return;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    });

    let addr = format!("127.0.0.1:{porta}").parse().unwrap();
    // O envio tem de TERMINAR — é o que falhava, ficava preso a comunicar.
    let r = tokio::time::timeout(
        Duration::from_secs(10),
        enviar::enviar(&Identity::generate("e"), &origem, addr, reg_e.ouvinte(), Default::default(), &Metricas::desligada()),
    )
    .await;
    assert!(r.is_ok(), "o envio não terminou — ficou preso apesar do cancelamento no recetor");

    // O importante: não ficou preso (já provado pelo timeout) e não reportou
    // sucesso. Quem envia chega a um estado terminal — cancelado, se o 409 do
    // recetor lhe chegou, ou falhou, se a ligação caiu antes (o comum a meio de
    // um upload grande). Ambos são aceitáveis; ficar pendurado ou dar por
    // enviado não são.
    assert!(!reg_e.viu("enviado"), "reportou envio completo apesar de cancelado no recetor");
    assert!(
        reg_e.viu("cancelado") || reg_e.viu("falhou"),
        "quem envia não chegou a nenhum estado terminal"
    );
}

#[tokio::test]
async fn cancelar_antes_de_comecar_nao_rebenta() {
    // Cancelar com o sinal já ligado não deve estoirar nem enviar nada.
    let dir = Temp::novo("cedo");
    let recetor = Identity::generate("r");
    let reg = Arc::new(Registo::default());

    let escuta = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let porta = escuta.local_addr().unwrap().port();
    drop(escuta);
    let dd = dir.0.join("recebidos");
    let cc = dir.0.join("conhecidos.json");
    tokio::spawn(async move {
        let _ = receber::servir(
            recetor,
            dd,
            cc,
            porta,
            pombo_correio_rede::silencio(),
            Default::default(),
            Metricas::desligada(),
        )
        .await;
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    let origem = dir.0.join("f.bin");
    std::fs::write(&origem, vec![7u8; 5_000_000]).unwrap();

    let addr = format!("127.0.0.1:{porta}").parse().unwrap();
    let ja = Arc::new(AtomicBool::new(true));
    enviar::enviar(&Identity::generate("e"), &origem, addr, reg.ouvinte(), ja, &Metricas::desligada())
        .await
        .unwrap();

    assert!(reg.viu("cancelado"));
    assert!(!reg.viu("enviado"));
}
