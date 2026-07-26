//! Descoberta na rede local, por mDNS.
//!
//! Acaba com os endereços escritos à mão. Cada dispositivo à espera anuncia-se,
//! e quem envia procura pelo nome — `pombo enviar foto.jpg "Pi da sala"` em vez
//! de decorar `192.168.1.232:53318`.
//!
//! Só cobre a rede local, por desenho: o mDNS é multicast e não atravessa
//! routers. Pelo Tailscale ou pela internet o endereço continua a escrever-se,
//! até o relay de S7 trazer um registo próprio.

use std::{collections::HashMap, net::SocketAddr, time::Duration};

use anyhow::{Context, Result};
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use pombo_correio_core::identity::PublicIdentity;

/// Tipo de serviço. O sufixo `._tcp.local.` é o que o mDNS exige.
const SERVICO: &str = "_pombo._tcp.local.";

/// Chave TXT com a impressão digital, para distinguir dois dispositivos com o
/// mesmo nome — dois telemóveis "Android" na mesma casa não são improváveis.
const CHAVE_FP: &str = "fp";

/// Um dispositivo encontrado na rede.
pub struct Descoberto {
    pub nome: String,
    pub endereco: SocketAddr,
    /// Impressão digital anunciada. **Não é de confiar**: qualquer um pode
    /// anunciar o que quiser. Serve para distinguir e para mostrar, nunca para
    /// autenticar — quem autentica é o envelope, que só o dono da chave abre.
    pub impressao: Option<String>,
}

/// Mantém o anúncio vivo enquanto existir.
pub struct Anuncio {
    daemon: ServiceDaemon,
    nome_completo: String,
}

impl Drop for Anuncio {
    fn drop(&mut self) {
        // Sem isto o dispositivo continuaria a aparecer na lista dos outros
        // durante minutos depois de desligado, e quem enviasse falhava sem
        // perceber porquê.
        let _ = self.daemon.unregister(&self.nome_completo);
        let _ = self.daemon.shutdown();
    }
}

/// Impressão digital curta, tal como vai no anúncio.
///
/// Um só sítio a decidir a forma, para que o anúncio e o filtro do próprio
/// dispositivo comparem exatamente a mesma coisa.
pub fn impressao_curta(eu: &PublicIdentity) -> String {
    hex::encode(eu.fingerprint())[..16].to_string()
}

/// Anuncia este dispositivo na rede local.
pub fn anunciar(eu: &PublicIdentity, porta: u16) -> Result<Anuncio> {
    let daemon = ServiceDaemon::new().context("não consegui arrancar o mDNS")?;

    // O nome de anfitrião tem de ser único na rede; o nome legível não serve,
    // porque pode repetir-se. A impressão digital resolve, e como só entram
    // dígitos hexadecimais não há caracteres a escapar.
    let fp = impressao_curta(eu);
    let anfitriao = format!("pombo-{fp}.local.");

    let propriedades: HashMap<String, String> =
        HashMap::from([(CHAVE_FP.to_string(), fp)]);

    // Endereço vazio: a biblioteca deteta sozinha os das interfaces ativas.
    // Fixar um IP aqui partiria em qualquer máquina com Wi-Fi e cabo, ou com o
    // Tailscale a somar mais uma interface.
    let info = ServiceInfo::new(SERVICO, &eu.label, &anfitriao, "", porta, propriedades)
        .context("dados de anúncio inválidos")?
        .enable_addr_auto();

    let nome_completo = info.get_fullname().to_string();
    daemon.register(info).context("não consegui anunciar")?;

    Ok(Anuncio { daemon, nome_completo })
}

/// Preferência de um endereço: menor é melhor.
///
/// A ordem importa muito mais do que parece. Um dispositivo na mesma casa
/// anuncia o IP da LAN (`192.168.x`, `10.x`) **e** o do Tailscale (`100.x`). Se
/// se escolher o do Tailscale estando ambos na LAN, o tráfego sai de casa e
/// volta pelo relay do Tailscale — uns míseros ~2 MB/s — em vez dos ~100 MB/s
/// da rede local. Por isso a gama do Tailscale (CGNAT, 100.64.0.0/10) fica
/// **abaixo** de qualquer outro IPv4. O IPv6 fica por último: numa rede
/// doméstica depende do que o router e a firewall deixam passar.
fn preferencia(ip: &std::net::IpAddr) -> u8 {
    match ip {
        std::net::IpAddr::V4(a) => {
            let o = a.octets();
            // 100.64.0.0/10 — a gama que o Tailscale usa.
            if o[0] == 100 && (64..=127).contains(&o[1]) {
                1
            } else {
                0
            }
        }
        std::net::IpAddr::V6(_) => 2,
    }
}

/// Escolhe com que endereço falar, de entre os que um dispositivo anuncia.
///
/// A biblioteca entrega-os num conjunto sem ordem; sem uma ordenação estável,
/// pegar no primeiro daria endereços diferentes a cada execução, sentido como
/// falhas ao calhas. Ordena-se pela preferência acima e, dentro dela, pelo
/// texto, só para o resultado ser reproduzível.
///
/// O `loopback` é descartado: um dispositivo que anuncie `127.0.0.1` está a
/// falar de si próprio, e enviar para lá nunca sairia da máquina de origem.
fn escolher_ip(enderecos: impl Iterator<Item = std::net::IpAddr>) -> Option<std::net::IpAddr> {
    let mut ips: Vec<_> = enderecos.filter(|i| !i.is_loopback()).collect();
    ips.sort_by_key(|i| (preferencia(i), i.to_string()));
    ips.into_iter().next()
}

/// Procura dispositivos durante `prazo`, sem contar o próprio.
///
/// É preciso um prazo porque o mDNS não tem fim: as respostas vão chegando e
/// nada indica que chegaram todas. Espera-se um bocado e mostra-se o que houver.
///
/// `ignorar` é a impressão digital deste dispositivo: como cada aparelho também
/// se vê a si próprio na rede, filtra-se pela impressão — nunca pelo IP, que é
/// múltiplo (Wi-Fi, cabo, Tailscale) e muda, nem pelo nome, que se pode repetir
/// entre aparelhos distintos. A impressão deriva das chaves e é única.
pub fn procurar(prazo: Duration, ignorar: Option<&str>) -> Result<Vec<Descoberto>> {
    let daemon = ServiceDaemon::new().context("não consegui arrancar o mDNS")?;
    let recetor = daemon.browse(SERVICO).context("não consegui procurar")?;

    let fim = std::time::Instant::now() + prazo;
    let mut achados: HashMap<String, Descoberto> = HashMap::new();

    while let Some(restante) = fim.checked_duration_since(std::time::Instant::now()) {
        let Ok(evento) = recetor.recv_timeout(restante) else {
            break;
        };

        if let ServiceEvent::ServiceResolved(servico) = evento {
            let impressao = servico
                .txt_properties
                .get_property_val_str(CHAVE_FP)
                .map(str::to_string);

            // Sou eu próprio — não me mostro a mim mesmo. O `is_some` evita que
            // um `ignorar` a `None` coincida com um anúncio sem impressão e o
            // esconda por engano.
            if ignorar.is_some() && impressao.as_deref() == ignorar {
                continue;
            }

            let Some(ip) = escolher_ip(servico.addresses.iter().map(|s| s.to_ip_addr())) else {
                continue;
            };

            let nome = servico
                .fullname
                .strip_suffix(&format!(".{SERVICO}"))
                .unwrap_or(&servico.fullname)
                .to_string();

            achados.insert(
                servico.fullname.clone(),
                Descoberto { nome, endereco: SocketAddr::new(ip, servico.port), impressao },
            );
        }
    }

    let _ = daemon.shutdown();

    let mut lista: Vec<_> = achados.into_values().collect();
    lista.sort_by(|a, b| a.nome.cmp(&b.nome));
    Ok(lista)
}

/// Traduz o que o utilizador escreveu num endereço.
///
/// Um endereço literal usa-se tal e qual — é o que serve o Tailscale e tudo o
/// que esteja fora da rede local. Só se recorre ao mDNS quando o que veio não
/// se parece com um endereço, o que evita esperar pela rede à toa.
pub fn resolver(destino: &str, porta_omissao: u16, ignorar: Option<&str>) -> Result<SocketAddr> {
    if let Ok(a) = destino.parse::<SocketAddr>() {
        return Ok(a);
    }
    if let Ok(ip) = destino.parse::<std::net::IpAddr>() {
        return Ok(SocketAddr::new(ip, porta_omissao));
    }

    println!("À procura de \"{destino}\" na rede local...");
    let encontrados = procurar(Duration::from_secs(3), ignorar)?;

    let iguais: Vec<_> = encontrados
        .iter()
        .filter(|d| d.nome.eq_ignore_ascii_case(destino))
        .collect();

    match iguais.as_slice() {
        [um] => {
            println!("Encontrado: {} em {}", um.nome, um.endereco);
            Ok(um.endereco)
        }
        [] if encontrados.is_empty() => {
            anyhow::bail!(
                "não encontrei \"{destino}\" nem nenhum outro dispositivo.\n\
                 Confirma que o outro lado tem `pombo receber` a correr e que estão na mesma rede.\n\
                 Fora da rede local (Tailscale, internet) escreve o endereço."
            )
        }
        [] => {
            let nomes: Vec<_> = encontrados.iter().map(|d| d.nome.as_str()).collect();
            anyhow::bail!(
                "não encontrei \"{destino}\". Estão à espera: {}",
                nomes.join(", ")
            )
        }
        varios => {
            // Não se escolhe por adivinhação: enviar um ficheiro ao dispositivo
            // errado é pior do que falhar com uma pergunta.
            let detalhe: Vec<_> = varios
                .iter()
                .map(|d| format!("{} ({})", d.endereco, d.impressao.as_deref().unwrap_or("?")))
                .collect();
            anyhow::bail!(
                "há {} dispositivos chamados \"{destino}\": {}.\n\
                 Escreve o endereço de qual queres.",
                varios.len(),
                detalhe.join(", ")
            )
        }
    }
}

/// Diz se em `endereco` está mesmo o dispositivo esperado.
///
/// Devolve `false` a qualquer falha. Não vale a pena distinguir "não respondeu"
/// de "respondeu outra coisa": em ambos os casos o caminho certo é o mesmo —
/// deitar a dica fora e procurar.
///
/// O tempo de ligação é curto de propósito. Um IP que já não tem ninguém não
/// recusa a ligação, fica calado, e o sistema só desiste ao fim de minutos —
/// esperar isso seria muito pior do que a busca que se está a evitar.
async fn quem_esta_em(endereco: SocketAddr, impressao: Option<&str>) -> bool {
    let Ok(cliente) = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(1))
        .timeout(Duration::from_secs(3))
        .build()
    else {
        return false;
    };
    let Ok(resposta) = cliente
        .get(format!("http://{endereco}/identidade"))
        .send()
        .await
    else {
        return false;
    };
    let Ok(deles) = resposta.json::<PublicIdentity>().await else {
        return false;
    };
    match impressao {
        Some(esperada) => impressao_curta(&deles) == esperada,
        // Sem impressão para comparar, ter respondido é quanto se pode exigir.
        None => true,
    }
}

/// Resolve um destino aproveitando o endereço que a lista já conhece.
///
/// A lista de dispositivos vem da descoberta e já traz o endereço de cada um;
/// mandar o [`resolver`] procurá-lo outra vez custava os 3 segundos inteiros da
/// janela de mDNS **antes** de o envio sequer começar — no Android, antes de o
/// seletor de ficheiros abrir, que é onde mais se notava.
///
/// Aqui esse endereço entra como dica e confirma-se com um pedido curto. Só se
/// a dica não servir — a máquina mudou de IP, ou o IP passou entretanto para
/// outro dispositivo — é que se paga a busca.
///
/// A impressão comparada aqui **não autentica** ninguém (ver [`Descoberto`]):
/// serve para não entregar o ficheiro ao dispositivo errado por o endereço
/// estar velho. Quem autentica continua a ser o envelope, que só o dono da
/// chave abre.
pub async fn resolver_com_dica(
    dica: Option<SocketAddr>,
    destino: &str,
    impressao: Option<&str>,
    porta_omissao: u16,
    ignorar: Option<&str>,
) -> Result<SocketAddr> {
    if let Some(d) = dica
        && quem_esta_em(d, impressao).await
    {
        return Ok(d);
    }

    // A busca é bloqueante (espera a janela toda), por isso sai da thread do
    // runtime.
    let destino = destino.to_string();
    let ignorar = ignorar.map(str::to_string);
    tokio::task::spawn_blocking(move || resolver(&destino, porta_omissao, ignorar.as_deref()))
        .await
        .context("a busca pelo destino não chegou ao fim")?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;

    fn ips(l: &[&str]) -> Vec<IpAddr> {
        l.iter().map(|s| s.parse().unwrap()).collect()
    }

    #[test]
    fn ipv4_ganha_ao_ipv6() {
        let escolhido = escolher_ip(ips(&["2a01:11:a31::1", "192.168.1.50"]).into_iter());
        assert_eq!(escolhido, Some("192.168.1.50".parse().unwrap()));
    }

    #[test]
    fn ipv6_serve_quando_e_o_unico() {
        let escolhido = escolher_ip(ips(&["2a01:11:a31::1"]).into_iter());
        assert_eq!(escolhido, Some("2a01:11:a31::1".parse().unwrap()));
    }

    #[test]
    fn loopback_e_descartado() {
        // Anunciar 127.0.0.1 é falar de si próprio; enviar para lá nunca sairia
        // da máquina de origem.
        let escolhido = escolher_ip(ips(&["127.0.0.1", "192.168.1.50"]).into_iter());
        assert_eq!(escolhido, Some("192.168.1.50".parse().unwrap()));
        assert_eq!(escolher_ip(ips(&["127.0.0.1", "::1"]).into_iter()), None);
    }

    #[test]
    fn a_escolha_nao_depende_da_ordem_de_entrada() {
        // O conjunto que a biblioteca entrega não tem ordem definida; sem esta
        // garantia o mesmo dispositivo dava endereços diferentes a cada corrida.
        let a = escolher_ip(ips(&["192.168.1.50", "192.168.1.20", "10.0.0.5"]).into_iter());
        let b = escolher_ip(ips(&["10.0.0.5", "192.168.1.50", "192.168.1.20"]).into_iter());
        assert_eq!(a, b);
    }

    #[test]
    fn lan_ganha_ao_tailscale() {
        // O bug do ~2 MB/s: com ambos na mesma casa, escolher o 100.x do
        // Tailscale mandava o tráfego pelo relay em vez da rede local. O IP da
        // LAN tem de ganhar, apesar de "100..." vir antes de "192..." no texto.
        for lan in ["192.168.1.50", "10.0.0.5", "172.16.3.4"] {
            let escolhido = escolher_ip(ips(&["100.100.20.30", lan]).into_iter());
            assert_eq!(escolhido, Some(lan.parse().unwrap()), "não escolheu a LAN sobre o Tailscale");
        }
    }

    #[test]
    fn tailscale_serve_quando_e_o_unico() {
        // Fora de casa, o Tailscale é o único caminho — e aí tem de ser usado.
        let escolhido = escolher_ip(ips(&["100.100.20.30"]).into_iter());
        assert_eq!(escolhido, Some("100.100.20.30".parse().unwrap()));
    }

    #[test]
    fn sem_enderecos_nao_ha_escolha() {
        assert_eq!(escolher_ip(std::iter::empty()), None);
    }

    #[test]
    fn impressao_curta_e_estavel_e_deriva_da_identidade() {
        use pombo_correio_core::identity::Identity;
        let id = Identity::generate("x");
        let a = impressao_curta(&id.public());
        // Estável entre chamadas...
        assert_eq!(a, impressao_curta(&id.public()));
        assert_eq!(a.len(), 16);
        // ...e diferente para outra identidade — é isto que garante que o filtro
        // do próprio dispositivo nunca esconde um aparelho diferente.
        assert_ne!(a, impressao_curta(&Identity::generate("x").public()));
    }

    #[test]
    fn endereco_literal_dispensa_a_rede() {
        // Escrever o endereço tem de continuar a funcionar: é o caminho do
        // Tailscale e de tudo o que esteja fora da rede local.
        assert_eq!(
            resolver("192.168.1.50:9999", 53318, None).unwrap(),
            "192.168.1.50:9999".parse().unwrap()
        );
        assert_eq!(
            resolver("192.168.1.50", 53318, None).unwrap(),
            "192.168.1.50:53318".parse().unwrap()
        );
        assert_eq!(
            resolver("100.64.0.1", 53318, None).unwrap(),
            "100.64.0.1:53318".parse().unwrap()
        );
    }
}
