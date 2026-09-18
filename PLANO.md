# Pombo-Correio

Partilha de ficheiros entre os dispositivos de casa, sem perder qualidade e sem
depender de nenhum serviço de terceiros.

Este ficheiro é **só o plano**: contexto, decisões, arquitetura, e o que já
está feito ou por fazer, sprint a sprint. As lições, dificuldades e a
descrição de como cada peça foi implementada e validada vivem em
**`BACKLOG.md`** — consulta-o quando quiseres o *porquê* por trás de uma
decisão, não para saberes o que fazer a seguir.

---

## Contexto

Em casa há Android, iOS, iPadOS, macOS, Windows e Linux (x86-64 e ARM64). O PC
Windows não tem Bluetooth, o que inutiliza o Quick Share, e não existe hoje
nenhuma forma única de enviar um ficheiro entre todos. As alternativas de
consumo recomprimem fotos e vídeos, o que é inaceitável.

Alojamento em casa, em duas máquinas fracas e redundantes entre si:

| Máquina | Arquitetura | Papel |
|---|---|---|
| AMD A10-6700 (~2013) | x86-64 | Depósito de blobs, trabalho pesado |
| Raspberry Pi 5 | ARM64 | Sempre ligado (~5 W): presença e sinalização |

Ambas têm aceleração AES por hardware — o A10 é Piledriver com AES-NI, o
Cortex-A76 tem as extensões criptográficas ARMv8 — por isso TLS a gigabit não é
problema em nenhuma delas.

### Requisitos

- Todas as plataformas acima, incluindo Linux ARM64.
- Simples ao ponto de um familiar sem conhecimentos técnicos conseguir usá-lo.
- Transferência **byte-exata**. Sem transcodificação, nunca.
- Vários caminhos independentes, com degradação automática.
- Tailscale **opcional** — tem de haver caminho para quem não o tem.
- O esforço fica nos dispositivos que transferem, não nos servidores.

### Decisões fechadas

| Questão | Decisão |
|---|---|
| Conta Apple Developer | Não. iOS recebe PWA no ecrã principal. |
| Stack | Rust + Tauri v2 + interface web. Ver justificação abaixo. |
| Exposição | Servidores na mesma rede, porta aberta possível, ambos em Tailscale. |
| Ordem | Linux → Windows → Android → **iOS por último**, já polido. |
| Onde se implementa | Máquina local na rede de casa. O ambiente web só planeia e revê. |

---

## Workflow

O guia operacional condensado — o que seguir a cada sessão — está em
**CLAUDE.md** (carregado automaticamente). Esta secção guarda a versão completa,
com a fundamentação.

1. Trabalho dividido em **sprints**. Cada sprint acaba em algo que corre e se
   testa — não em código a meio.
2. **Commits por unidade coerente**, não só no fim do sprint. Um sprint são
   dias; um único commit gigante torna o `git bisect` inútil.
3. Fim de sprint: testes verdes, este documento atualizado, e uma **tag**
   `sprint-N` a marcar a fronteira.
4. Este ficheiro é a **única** fonte de verdade do plano. **`BACKLOG.md`** é a
   fonte de verdade das lições e do histórico de implementação — atualizados
   os dois à medida que se executa. Ficheiros em `~/.claude/plans/` são
   efémeros (reescritos a cada sessão de planeamento nova) e nunca são a
   referência para nada que precise de sobreviver à sessão.
5. **Implementação manual, sem delegar a modelos mais fracos.** Cada subagente
   arranca a frio e volta a derivar o contexto, o que aqui é mais caro, não mais
   barato. Delegar compensa em trabalho mecânico e paralelo (o mesmo padrão em
   dezenas de ficheiros); este projeto é protocolo e cifra, onde os erros são
   silenciosos e passam nos testes que um modelo mais fraco escreveria.
   Reavaliar se aparecer fan-out genuinamente mecânico.
6. **Implementação na máquina local, não no ambiente web/remoto.** O código dos
   sprints é escrito, corrido, testado e commitado a partir de uma máquina na
   rede de casa. O ambiente remoto é efémero, não tem rota para a LAN, o Pi 5
   nem o Tailscale, e é reciclado por inatividade — serve apenas para planear e
   rever. Não é preferência: o critério do S3 (validar LAN, Tailscale e
   x86-64↔ARM64 no Pi 5) é impossível a partir do remoto.
7. Cada bocado de lógica não-trivial deixa **um teste que falha se a lógica
   partir**. Sem frameworks e sem suites por função.

8. **Desenhar antes de implementar, para o que envolve duas partes, protocolo,
   estado ou timing.** Escreve-se, por extenso e com trechos de código se
   ajudar, como a coisa funciona — o **caminho feliz e os de falha** — antes de
   codificar, e adapta-se ao que se encontrar. Não é para tudo: uma correção
   local trivial dispensa-o. É para o que tem duas pontas. A pergunta que **tem**
   de ser respondida no desenho: *o que é que cada lado observa, e quando?*
   (ver `BACKLOG.md`, S3/S4, o cancelamento que nasceu partido por saltar isto).

9. **Pré-mortem numa linha para qualquer mudança de protocolo:** *se esta ligação
   morrer agora, o que é que cada lado conclui?*. Barata, e apanha desenhos que
   dependem de a ligação sobreviver ao que a está a matar.

10. **Ser honesto sobre onde uma coisa foi provada.** Cada afirmação de "feito"
    diz *onde* foi verificada, e **loopback, build debug, ou uma só máquina não
    contam como prova de comportamento distribuído nem de desempenho real**.
    "Lógica verificada em loopback" e "validado entre máquinas" são estados
    diferentes. Um sprint com uma ponta distribuída só fecha com validação em
    máquinas reais, como o S3 já exige.

---

## Arquitetura

### Um só formato de fio

A simplificação central, e a que mais trabalho poupa: **os transportes não
diferem no protocolo, só na forma de alcançar o outro lado.**

Tudo é HTTP simples, e os browsers falam-no nativamente — o que o caminho sem
instalação exige de qualquer maneira.

Há duas metades, e ambas são precisas:

- **Subir** — quem envia empurra num `PUT` único com o corpo em fluxo. Retoma
  com `?de=N`. É o que liga dispositivo a dispositivo, e também dispositivo ao
  relay. Feito em S3.
- **Descer** — quem recebe puxa com um `GET` e `Range`. Só é preciso do relay
  para quem recebe. Feito em S7.

| Caminho | Como se alcança o outro lado | Passa pelo servidor? |
|---|---|---|
| **LAN** | Descoberta mDNS → IP local | Não |
| **Tailscale** | IP 100.x | Não |
| **Relay** | URL do nó de casa, conteúdo cifrado, com TTL | Sim, cifrado |
| **Link sem instalação** | URL curto enviado por WhatsApp/SMS | Sim, cifrado |

O Tailscale sai praticamente de graça: entrega endereços IP realmente
encaminháveis, por isso é o caminho da LAN sem alterar uma linha.

QUIC fica de fora até haver medição que o justifique. Traria multiplexação e
melhor comportamento em ligações com perdas, mas custa um segundo formato de
fio e não é falado por browsers. O LocalSend usa HTTP simples e chega-lhe.

**Os blocos são a unidade de verificação e retoma, não de pedidos.** Um pedido
HTTP por bloco seria o desenho ingénuo e mataria o débito pela internet: a 1 MiB
por bloco e gigabit são ~125 pedidos por segundo, e com 30 ms de latência
sequencialmente não passa de ~33 Mbps. Em vez disso é **um pedido só, com o
corpo em fluxo**, deixando o TCP encher o tubo; os blocos vão sendo verificados
à medida que passam. Retomar é recomeçar na fronteira do primeiro bloco por
verificar.

### Onde está mesmo o gargalo

Por ordem, do mais apertado ao mais folgado:

1. **Upload de casa** nos caminhos pela internet (~50–500 Mbps).
2. **Disco.** No Pi 5, um cartão microSD faz ~50–100 MB/s e é o teto real; um
   NVMe pelo HAT PCIe passa dos 800 MB/s.
3. **LAN gigabit** — 125 MB/s.
4. **Cifra e hash** — BLAKE3 e ChaCha20 andam nos 500+ MB/s, 4 a 40× acima do
   fio. Nunca é o gargalo.

A RAM não entra na lista. O Pi tem 15 GB, o que não altera nada do desenho: o
que sobra vai para page cache do Linux, que é melhor uso do que bufferizar em
espaço de utilizador. Encher memória com ficheiros inteiros só tiraria cache ao
sistema e serializaria ler → cifrar → enviar, em vez de os sobrepor.

Nota sobre a escolha da cifra: com aceleração por hardware o AES-GCM bate o
ChaCha20 nas duas máquinas de casa, mas ambos estão muito acima do débito da
rede, por isso é indiferente. O desempate é o iOS — em WASM não há acesso às
instruções AES, e aí o ChaCha20 ganha com folga. Daí XChaCha20-Poly1305.

### Modelo de segurança

- Por dispositivo: um par **Ed25519** (assinar) e um par **X25519** (cifrar).
  Separados de propósito — reutilizar a mesma chave para assinatura e acordo de
  chaves é possível mas frágil, e a conversão Ed25519→X25519 abre um flanco
  desnecessário quando gerar dois pares custa microssegundos.
- Emparelhamento por QR ou por comparação do número de segurança em voz alta.
- Chave aleatória por transferência, embrulhada para a chave pública de cada
  destinatário. **O relay só vê texto cifrado.** Isto importa precisamente
  porque o relay está exposto à internet: comprometê-lo não expõe ficheiro
  nenhum.
- Link sem instalação: `https://casa.exemplo/d/<id>#<chave>`. O fragmento `#`
  nunca é enviado ao servidor pelos browsers, por isso o modelo aguenta-se mesmo
  com o destinatário num browser. PIN opcional à parte.

### Alta disponibilidade sem replicar ficheiros

Os ficheiros em trânsito são **efémeros e reenviáveis**. Construir replicação de
blobs entre duas máquinas fracas seria pagar caro por nada.

- Os dois nós correm o mesmo binário, ativo-ativo, diferindo só na configuração.
- O cliente conhece ambos os endpoints e corre-os em paralelo, ficando com o que
  responder primeiro.
- **Blobs não são replicados.** O remetente carrega para o nó que estiver de pé;
  se esse cair a meio, reenvia para o outro.
- Só o registo de dispositivos seria replicado, se existisse — ver R2 no S7.

### Porquê Rust + Tauri v2

A escolha de PWA para iOS decidiu tudo. Como a interface web **tem de existir de
qualquer forma** para o iPhone, a opção certa é a framework que reutiliza essa
mesma interface no desktop e no Android. O Flutter obrigaria a manter duas
interfaces para o mesmo produto.

```
core/     Rust. Protocolo, identidade, cifra, blocos. Compila nativo E wasm32.
ui/       TypeScript + Svelte. UMA interface, quatro consumidores:
          Tauri desktop · Tauri Android · PWA iOS · página de download.
app/      Invólucro Tauri v2: Windows, macOS, Linux x64+arm64, Android.
relay/    Rust (axum + tokio). Binários estáticos musl amd64 e arm64.
```

O `core/` compilado para WASM faz o iPhone correr exatamente a mesma lógica de
cifra e blocos que o Linux.

### Restrições descobertas na investigação

1. **Os bytes nunca atravessam o IPC do Tauri.** O IPC serializa tudo para
   string JSON e colapsa com payloads grandes. O Rust é dono do ficheiro do
   princípio ao fim; a WebView recebe apenas eventos de progresso. Regra
   arquitetural, não otimização.
2. **O Safari no iOS transcodifica HEIC→JPEG** quando o ficheiro vem da
   Biblioteca de Fotos via `<input type="file">`. Colide de frente com o
   requisito de não perder qualidade. Mitigação: encaminhar para a app
   **Ficheiros**, e não incluir `image/heic` no `accept`. Correção verdadeira só
   com app nativa.
3. **Android precisa de plugins**: `tauri-plugin-android-fs` (SAF e URIs
   `content://`) e `tauri-plugin-background-service` (foreground service).
4. **PWAs no ecrã principal funcionam na UE.** A Apple anunciou que as ia
   remover no iOS 17.4 e reverteu duas semanas depois, em março de 2024.

### O que ficou deliberadamente de fora

- **Bluetooth.** O iOS não expõe transferência de ficheiros arbitrários a apps
  de terceiros, e onde funciona dá ~1–2 Mbit/s. Manutenção em cinco plataformas
  para cobrir "zero rede partilhada", que em casa não acontece. A LAN já cobre
  "estamos os dois em casa".
- **BitTorrent.** O seu valor é *muitos seeders para um ficheiro popular*; em
  casa a fonte é sempre uma. As partes úteis — blocos, retoma, integridade,
  ligações múltiplas — estão dentro do protocolo via Merkle BLAKE3, sem arrastar
  `libtorrent`.
- **Furo de NAT / WebRTC.** Era o degrau mais caro do plano original e é apenas
  uma *otimização* sobre o relay: poupa largura de banda de casa quando os dois
  lados estão fora. Adiado até o relay doer. `ponytail: relay gasta o upload de
  casa duas vezes quando ambos estão remotos; WebRTC se isso incomodar.`
- **QUIC.** Ver acima.

---

## Ideias por implementar

Não são sprints. São direções discutidas e consideradas fazíveis, guardadas
para não se perderem — com as armadilhas já marcadas, para a ideia não voltar
mais bonita do que é.

### Relay como capacidade, não como dispositivo

Hoje o relay são "os dois servidores de casa". A generalização certa: **qualquer
nó estável e alcançável anuncia a capacidade de relay**, e a rede escolhe quem
está de pé. O A10 e o Pi são só os que mais provavelmente qualificam. Encaixa no
desenho ativo-ativo que o S7 já tem — é a forma certa de o desenhar, não uma
funcionalidade à parte.

- **Ranquear por adequação**, não aceitar qualquer voluntário: tempo de
  atividade, alcançabilidade, e *não estar a bateria*.
- **Telemóvel-como-relay é permitido em último recurso, mas não se desenha a UX
  à volta dele.** Falha exatamente no cenário que o motiva — "sem energia em
  casa, dou host no Android" é o telemóvel a bateria, em dados móveis atrás de
  CGNAT, com o SO a matar processos em segundo plano. No iOS nem servidor pode
  correr. É o pior nó possível precisamente quando lhe pegarias.

### Mandar remotamente um dispositivo de confiança começar a dar host

Anda em cima do canal de notificações autenticado que o S7 precisa de ter, por
isso não é grátis mas está no caminho. **Inegociável: o comando tem de ser
assinado por um dispositivo de confiança.** Abrir serviços por ordem remota é
uma superfície de ataque apetitosa; a assinatura Ed25519 que já temos resolve.
Depois do S7 existir.

### Suporte remoto: ver o ecrã + voz

Caso de uso concreto e valioso: **ver o ecrã de um familiar e falar com ele para
o ajudar**. É um segundo produto, mas reutiliza a base de identidade,
emparelhamento e descoberta (`core/` e parte do `rede/`) — só a camada de
transporte é que muda.

- **Precisa de WebRTC** (o mesmo que adiámos no furo de NAT — passa a ter dois
  motivos para existir) e de plugins nativos de captura de ecrã no Android.
- **A voz é quase de borla** depois de haver vídeo: é mais uma faixa na mesma
  ligação. O instinto de juntar voice chat está certo.
- **Vários a assistir funciona para números pequenos.** Um emissor para N
  familiares são N ligações diretas; para uma família chega. Só dezenas de
  espetadores exigiriam um servidor a redistribuir, que o A10 e o Pi não
  aguentariam — mas não é o caso.
- **O muro é o iPhone.** Uma PWA no iOS **não captura o próprio ecrã** — o Safari
  deixa *ver* um ecrã partilhado, não *partilhar* o dele. Ajudar com o Windows
  ou o Android de alguém: fácil. Ajudar com o *iPhone* dele: exige app iOS
  nativa (99 €/ano). É o dispositivo onde mais se precisa de ajuda e o único que
  a PWA não alcança — e é isto, não a partilha de ficheiros, que justificaria a
  conta Apple.

### Reforços à autorização automática por convite

A autorização automática pelo link de pareamento em si **já está construída**
(S10 T3, revista pela Review4 — ver S11). O que fica aqui são só dois
reforços do André para mais tarde, guardados mas não urgentes: **expirar
dispositivos sem uso há 30+ dias** (remove-os da lista sozinho) e um
**segredo pessoal tipo PIN/número de faculdade** que torna um dispositivo
"não suspeito" por um número de dias após ações fora do padrão. Ambas trazem
estado novo a gerir (temporizadores, um segredo extra) — sem urgência
enquanto a família continuar pequena.

### Controlo remoto — decisão separada, marcada de propósito

"Ajudar o avô" tende a virar *"clicar por ele"*, e isso é **controlo remoto**,
não partilha de ecrã. Salto enorme de complexidade e de superfície de segurança,
e proibido pela raiz no iOS. Ver+voz resolve ~70% dos casos ("carrega no botão
azul em cima"); controlar resolve o resto mas é outro monstro. Fica marcado à
parte para não entrar disfarçado de screen sharing.

---

## TODO — ASAP

- [x] **Não listar o próprio dispositivo na descoberta.** `procurar`
  recebe a impressão digital própria (`impressao_curta`) e descarta o anúncio que
  coincide. Pela impressão, não pelo IP (múltiplo e mutável) nem pelo nome (que
  se repete entre aparelhos distintos).
- [x] **Saber as chaves de quem nunca nos enviou nada.** Ver `BACKLOG.md`
  ("Geral") para o desenho e a correção posterior.

---

## Auditorias externas

- **Review0** (`audit/Review0.md`, 01/09/2026): maioritariamente positiva, sem
  falhas críticas. Cinco achados, verificados contra o código. Um deles gerou
  trabalho real (zeroização de `TransferKey`, feito), outro ficou agendado
  (limite de débito no relay, ver "Antes do S8"). Detalhe completo em
  `BACKLOG.md`.
- **Review1** (`audit/Review1.md`, 01/09/2026): validou a separação
  PLANO/BACKLOG, o F-01 e os testes de `Conhecidos` — sem ação nova aí. Duas
  sugestões concretas para o S8, incorporadas nas fases P1-P4 abaixo: os
  limites de memória do Safari em ficheiros grandes (F-06) e o formato da
  interface dupla `api.ts` (F-07, já era o plano do P2 — confirma, não muda).
  **Uma sugestão rejeitada:** o `getrandom = { version = "0.2", features =
  ["js"] }` que a Review1 propõe para o P2 já não se aplica — o `core/`
  já usa `getrandom = "0.4.3"` com a feature `wasm_js` (não `js`, que é da
  0.2), e o build para wasm32 já compila sem isto. Verificado antes de
  aceitar, não só copiado.
- **Review2** (`audit/Review2.md`, 01/09/2026): auditoria do S8 completo
  (VAPID/RFC 8292, camada wasm, dual backend, resiliência iOS, design
  responsivo) — sem achados críticos nem médios, três de severidade baixa/
  informativa (F-08 a F-10). Nenhum é um bug: são trabalho a agendar.
  F-10 (segredos em `localStorage`) só pedia documentação — feito, ver
  "Riscos". F-08 (retoma de descarga na PWA) e F-09 (limite de débito, já
  agendado desde a Review0) e as quatro sugestões da secção 7 formam o S9
  abaixo.
- **Review3** (`audit/Review3.md`, 02/09/2026): auditoria do S9 completo —
  "aprovado com recomendações". Quatro achados, nenhum criptográfico. **F-11**
  (média/alta) é o único com peso real: o `PeerIpKeyExtractor` do Q3 já não
  vê o IP de quem pede — o `nginx` do Q4 fala com o relay por
  `127.0.0.1:53319`, por isso todo o tráfego público de `pombo-
  correio.duckdns.org` cai no **mesmo balde** de 5 pedidos de rajada. Verificado
  contra o código, não só a palavra da auditoria: o comentário em
  `rotas_publicas()` ainda diz "não há proxy nenhum... nem depois do Q4",
  frase escrita antes do `nginx` existir e nunca revista. F-12 (mesma
  verificação: `FcmService.onMessageReceived` está mesmo vazio) esvazia o
  Q5 na prática — sem notificação nenhuma, ninguém sabe que chegou nada até
  abrir a app por iniciativa própria, o cenário exato que o Q5 existia para
  cobrir. F-13 (retoma de upload na PWA) e F-14 (autorização manual) são
  trabalho a agendar, não bugs. Os três primeiros formam o S10 abaixo.
- **Review4** (`audit/Review4.md`, 04/09/2026): auditoria do S10 completo,
  postura de "advogado do diabo" — "aprovado com recomendações". T1 e T2 do
  S10 saem confirmados sem reparo; o T3 (autorização por convite) rende seis
  achados novos (F-15 a F-20), um só de severidade ALTA. **F-15** é o único
  com peso real: `convidar()` lê e decide sobre a `Ficha` do convite, e só
  apaga o `de` num pedido de escrita à parte depois — sem trinco nenhum
  entre os dois passos, dois pedidos concorrentes para o mesmo link (o
  cenário natural de um link reencaminhado e aberto por duas pessoas ao
  mesmo tempo) autorizam-se os dois, o mesmo furo que o T3 achou e corrigiu
  para "ao longo do tempo" continua aberto para "ao mesmo tempo". F-16 a
  F-20 (perda silenciosa de convites na PWA em erros HTTP não-2xx,
  notificação FCM presa na barra, convite órfão sem CTA para a app, cache de
  OAuth2 e limite de débito em falta em `/convite`, `autorizados.txt` sem
  quebra de linha garantida) são achados reais mas menores. Formam o S11
  abaixo — a cache de OAuth2 fica de fora, decisão já tomada e documentada
  em `fcm.rs`, reafirmada por não haver tráfego novo que a justifique.
- **Review5** (`audit/Review5.md`, 04/09/2026): revisão do *planeamento* do
  S11 antes de arrancar (nenhum código ainda existia) — "aprovado com
  ressalvas críticas". Achou uma inconsistência real no desenho do T1:
  apagar `de` **antes** de `autorizar()` partia a própria retentativa que
  o F-16 pede — um 500 a meio deixava o convite já sem `de`, por isso a
  retentativa da PWA caía num 403 definitivo em vez de repetir a
  autorização (o "Ghost Burn", corrigido no desenho abaixo: autorizar
  primeiro, apagar `de` só depois de confirmado). Mais quatro ajustes ao
  T2/T3: CTA permanente no rodapé além do pós-descarga (F-17), limpeza da
  notificação especificamente em `onResume()` (F-18), isolamento dos
  testes de `/convite` face ao balde partilhado (F-19), escrita única no
  `append` do `autorizados.txt` (F-20). Incorporados abaixo antes de
  qualquer linha de código do S11 ser escrita.

---

## Sprints

- [x] **S1 — Primitivas do núcleo**
- [x] **S2 — Cifra e identidade em disco**
- [x] **S3 — Transferência sobre HTTP**
- [x] **S4 — Descoberta e dispositivos conhecidos**
- [x] **S5 — Interface e desktop** (em Windows a app corre e recebe; falta olhar para o ecrã)
- [x] **S6 — Android**
- [ ] **S7 — Relay e link sem instalação**
- [x] **S8 — PWA iOS** (as seis fases construídas; P1/P2/P5/P3/P6 provados no iPhone real; P4 provado contra o Chrome/FCM a sério, falta confirmar a entrega da Apple)
- [x] **S9 — Distribuição e exposição pública** (Q1 a Q5 feitos e verificados de ponta a ponta — musl, retoma de descarga, limitador de débito, DDNS pública, push FCM Android)
- [x] **S10 — Fechar o que a Review3 encontrou** (T1, T2 e T3 feitos e verificados de ponta a ponta — limitador de débito corrigido, notificação FCM a acordar a app, autorização automática por link de pareamento)
- [x] **S11 — Fechar o que a Review4 encontrou** (T1 corrige a corrida no consumo do convite e a perda silenciosa de convites na PWA; T2 fecha o ciclo de vida da descoberta e da notificação; T3 dá limite de débito a `/convite` e escrita segura ao `autorizados.txt`)
- [ ] **S12 — Ficheiros grandes (>1 GB) e emparelhamento por QR** (T1 e T2 implementados — código escrito, typecheck e build limpos, por provar em hardware real (regra 3), ver handoff; T3, a validação com ficheiro real >1 GB e queda de rede a sério, continua em aberto)

### S1 — Primitivas do núcleo ✅

Identidade e endereçamento por conteúdo. 18 testes.

`core/src/identity.rs` · `core/src/chunk.rs` · `core/src/error.rs`

**Feito**
- Ed25519 + X25519, com serialização para persistir.
- Impressão digital derivada de **ambas** as chaves.
- Número de segurança em 6 grupos de 5 dígitos, ao estilo do Signal.
- Derivação de chave partilhada com separação por contexto.
- Blocos de 1 MiB, Merkle BLAKE3, manifesto com validação e verificação por
  bloco.

### S2 — Cifra e identidade em disco ✅

Selagem de blocos, envelope para o destinatário, identidade persistida.
35 testes no total.

`core/src/crypto.rs` · `core/src/identity.rs`

**Feito**
- Chave aleatória por transferência, com bytes do CSPRNG do sistema.
- Selagem de blocos com XChaCha20-Poly1305, nonce aleatório por bloco e o
  índice como dados associados.
- Envelope com par efémero, à maneira do `age`.
- Identidade gravada em disco a `0600`, com `load_or_create`.

### S3 — Transferência sobre HTTP ✅

`cli/src/protocolo.rs` · `cli/src/receber.rs` · `cli/src/enviar.rs` ·
`cli/src/progresso.rs` · `cli/src/main.rs` · `cli/tests/e2e.rs`

**Feito**
- [x] Servidor `axum` no lado de quem recebe: `/identidade`, `/transferencia`,
      `/transferencia/{id}/blocos`.
- [x] CLI: `pombo receber`, `pombo enviar <ficheiro> <destino>`,
      `pombo identidade`.
- [x] Retoma, com o identificador da transferência a ser a raiz de Merkle.
- [x] Teste de integração com dois processos e um socket.
- [x] Progresso ao vivo nos dois lados: percentagem, blocos e débito.
- [x] **Validado em rede real, PC x86-64 → Raspberry Pi 5 ARM64.**

### S4 — Descoberta e dispositivos conhecidos ✅

Acabaram-se os endereços escritos à mão na rede local.

`cli/src/descoberta.rs` · `cli/src/conhecidos.rs`

**Feito**
- [x] mDNS: anunciar ao receber, descobrir ao enviar.
- [x] `pombo dispositivos` lista quem está à espera.
- [x] `pombo enviar <ficheiro> "Nome do dispositivo"` — sem endereço.
- [x] Registo de dispositivos conhecidos, com aviso na primeira vez.
- [ ] QR — adiado para S5, onde há ecrã para o mostrar.

### S5 — Interface e desktop ✅ (em Windows corre e recebe; falta ver o ecrã)

Aplicação gráfica a funcionar em Linux. Recebe e envia, com progresso ao vivo.

`ui/src/App.svelte` · `ui/src/lib/api.ts` · `ui/src/app.css` ·
`ui/src-tauri/src/lib.rs` · `rede/` (extraída de `cli/`)

**Feito**
- [x] `rede/` extraída de `cli/`, comunicando por eventos em vez de imprimir.
- [x] Tauri v2 + Svelte 5, com o desenho convertido.
- [x] Lista de dispositivos, envio, progresso, número de segurança.
- [x] Tema claro e escuro, com botão que ganha sobre a preferência do sistema.
- [x] **Bytes nunca passam pelo IPC** — só nomes, tamanhos e contagens.
- [x] Verificado: 40 MB do CLI para a aplicação, byte a byte idênticos.
- [x] **Correr em Windows ✅** — ver `BACKLOG.md` para a saga completa.

**Ecrãs do desenho ainda sem suporte no núcleo:** histórico, emparelhamento por
QR, dispositivos de confiança, prazo dos links, modo simples. Ficaram de fora em
vez de aparecerem sem função — o histórico e o QR entram quando houver onde os
guardar, os links entraram em S7.

### S6 — Android ✅

Testado num OnePlus Pad (OPD2403), Android 16, ARM64, por USB (adb).

**Feito e provado**
- [x] Compila para ARM64, instala e arranca no tablet.
- [x] A interface **renderiza** (verificado pelo DOM via DevTools da WebView — o
      `screencap` mostra preto porque não captura a camada de hardware da
      WebView, mas o conteúdo está lá, tema escuro incluído).
- [x] O servidor escuta em `0.0.0.0:53318`.
- [x] **Recebe um ficheiro do PC byte-exato** (hash confirmado), com métricas por
      fase a funcionar.
- [x] Foreground service nativo (Kotlin) + `MulticastLock` — recebe com o ecrã
      desligado e sem isenção de bateria.
- [x] Enviar do Android via `content://` (SAF), com abstração `Fonte` em
      `rede/`.
- [x] Partilhar de outra app para o Pombo-Correio (folha de partilha do
      sistema).

**Limitação conhecida e aceite, não corrigida:** o Android redige as
coordenadas GPS de fotos partilhadas sem a permissão `ACCESS_MEDIA_LOCATION` —
a app não a pede, por decisão de privacidade do André, e por isso não é
byte-exata em fotos com GPS vindas do Android. Ver `BACKLOG.md` para a
investigação completa.

### S7 — Relay e link sem instalação

Em branch própria (`s7-relay`), com o `main` sempre verde: o S7 mexe no formato
de fio e tem uma ponta que só se valida com os servidores de casa de pé. O `main`
tem de continuar a ser o que se instala nos aparelhos da família a meio disto.

#### O que o relay é — e o que não pode ser

**Não pode ser um `receber::servir`.** O recetor de hoje abre o envelope com a
sua identidade e verifica cada bloco contra o manifesto; o relay não é o
destinatário, não tem chave, e não pode ter. O relay é mais burro de propósito:
guarda **bytes opacos** com um identificador, e devolve-os. Não sabe o nome do
ficheiro (vai no manifesto selado), não sabe o tamanho em claro, não distingue um
vídeo de um PDF. Comprometê-lo não expõe ficheiro nenhum — é a razão de existir
deste desenho, e é ele que está exposto à internet.

Quem verifica continua a ser quem recebe: `open_chunk` + `verify_chunk` contra o
manifesto, exatamente como na LAN. **A garantia byte-exata não muda de dono.**

#### O identificador no relay não é a raiz de Merkle

Na LAN o identificador é a raiz de Merkle, e é bom que seja: duas tentativas do
mesmo ficheiro reencontram o trabalho a meio. No relay seria um erro por duas
razões:

1. **Confirmação de conteúdo.** Quem desconfie de que passou por ali um ficheiro
   concreto calcula-lhe a raiz e pergunta. Num relay exposto à internet, isso
   deita fora metade do que o ciframento comprou.
2. **Colisão entre pessoas.** Dois envios do mesmo ficheiro para destinatários
   diferentes partilhariam depósito.

Logo: **identificador aleatório de 128 bits**, gerado pelo relay. É também o que
vai no link sem instalação, e é por ser imprevisível que o link se aguenta.

#### Retoma: blocos de um lado, bytes do outro

A subida para o relay retoma como hoje (`?de=N`, em blocos) porque quem envia
conhece o manifesto. A descida retoma por `Range` **em bytes**, porque o relay
não sabe o que é um bloco. Quem recebe converte: o bloco *k* começa no byte
`k * (chunk_size + SEAL_OVERHEAD)` — todos os blocos selados têm tamanho fixo
menos o último. É esta conta que mantém o relay ignorante.

#### Peças, por ordem de dependência

- [x] **R1 — depósito opaco.** `relay/` em axum: `POST /deposito` (devolve o
      id), `PUT /deposito/{id}?de=N` (subir, retomável), `GET /deposito/{id}`
      (oferta selada e bytes já lá), `GET /deposito/{id}/blocos` com `Range`
      (descer, retomável), validade com limpeza de hora a hora.
- [x] **R2 — caixa autenticada.** `GET /caixa` assinada com Ed25519 e um
      instante, `POST /deposito` também assinado, mais uma lista de quem é de
      casa (`--autorizados`). Ver `BACKLOG.md` para a simplificação que isto
      trouxe (o registo de dispositivos do plano original deixou de ser
      preciso).
- [x] **R3 — o cliente desce a escada.** Direto → relay, e ao abrir a app
      pergunta pela caixa. O "destino não disponível" deixou de ser um beco.
- [x] **R4 — dois nós ativo-ativo.** De pé no A10 e no Pi 5, cada um como
      serviço de utilizador do systemd com `Restart=always` e `linger`.
      Validado entre as duas máquinas reais, na LAN — ver `BACKLOG.md` para a
      matriz completa. **Por medir:** nada disto saiu da LAN.
- [x] **R5 — página sem instalação.** `core/` em WASM + `pagina/`, servida
      pelo relay em `--pagina`. Validado em Chromium, loopback e LAN entre os
      dois nós. Ver `BACKLOG.md` para o desenho completo e o que ainda não
      estava provado nesta fase (Safari, `showSaveFilePicker`, fora da LAN).
- [ ] **R6 — push.** Web Push no iOS **feito** (P4 do S8, `relay/src/vapid.rs`
      + `push.rs` — VAPID/RFC 8292, provado a sério contra o FCM do Chrome,
      falta só a confirmação da Apple). **Falta FCM no Android** — precisa da
      conta e chaves de serviço Firebase do André. Ver "Antes do S9".
- [x] **R7 — TLS, nos dois nós.** ✅ Confirmado a sério em 01/09/2026:
      `curl https://raspberrypi.tail455bb7.ts.net/saude` e
      `curl https://andresantos-m11bb.tail455bb7.ts.net/saude` → 200 nos dois,
      certificado Let's Encrypt válido. Saga completa (o bug do lado do
      Tailscale) em `BACKLOG.md`.
- [ ] Binários musl para amd64 e arm64. Ver "Antes do S8".

**Onde é que cada coisa está provada, sem arredondar:** o protocolo, a retoma
nos dois sentidos, a opacidade do relay e o TLS estão provados **entre o A10
e o Pi 5, na LAN**; **pela internet não está provado nada**, e é assim que
fica escrito até alguém o correr de fora de casa.

### Antes do S8

Análise feita em 01/09/2026, com o estado de cada máquina confirmado a
sério antes de escrever a tabela — a ordem de trabalho combinada com o André
para desbloquear o S8.

| Item | Bloqueia o S8? | De quem é a próxima ação | Ação concreta |
|---|---|---|---|
| ~~TLS no A10~~ | — | — | ✅ Fechado, 01/09/2026 — confirmado nos dois nós |
| ~~iPhone na tailnet~~ | — | — | ✅ Fechado, 01/09/2026 — `iphone` visível em `tailscale status` |
| Binários musl amd64+arm64 | Não | Eu | Instalar targets, configurar `relay/` |
| DDNS (DuckDNS) | Não — só para quem não tem Tailscale | André depois eu | André cria a conta e dá-me o token; ao expor o relay, junto ao `--autorizados` entra também um limitador de débito por IP (Review0 F-02 — `tower_governor`, sobre o `axum` que já temos) |
| Push — Firebase (Android) | Não para arrancar o S8; sim para R6/P4 | André depois eu | André cria o projeto Firebase e a conta de serviço; eu integro |
| Push — VAPID (iOS) | Não para arrancar o S8; sim para R6/P4 | Eu | Gero o par de chaves, sem depender de conta nenhuma |
| Janela da app em Windows | Não | André | Olhar para ela depois de `npm run app:build`, enviar algo de lá |

**Os dois bloqueadores do arranque estão fechados — o S8 pode começar.** As
restantes linhas correm em paralelo, sem bloquear.

Já fechados nesta análise, sem código a mudar mais: o buraco dos `Conhecidos`
(depositar para quem só recebeu, já funcionava) e a zeroização de
`TransferKey` (Review0 F-01, feito). Detalhe de ambos em `BACKLOG.md`.

### S8 — PWA iOS

Decisão de arquitetura: a app do iPhone é a **mesma interface Svelte**
(`ui/`) a correr sem Tauri — não uma página à parte — para o design
(`design/claude-design/`) e a lógica viverem num sítio só, partilhados por
desktop, Android e iOS. `api.ts` ramifica por `eTauri()` só nas funções com
a mesma forma nos dois lados; a app instalável fica servida **só pelo Pi**
(`localStorage` é por origem), com os dois nós de casa (R4) acedidos por
`fetch()` com CORS.

- [x] **P1 — Fundação instalável.** `manifest.json`, ícones, service worker
      mínimo, `--app <ui/dist>` no relay.
- [x] **P2 — Identidade sem Tauri.** `wasm-backend.ts` gera e persiste a
      identidade em `localStorage`; `pagina/` ganhou `IdentidadeWasm`.
- [x] **P5 — Enviar do iPhone.** Sobe **só ao relay** (mixed content bloqueia
      HTTPS→HTTP direto); o destino aprende-se ao abrir um link criado no
      desktop.
- [x] **P3 — Receber ao abrir a app.** `Descarga.daCaixa`, o mesmo caminho do
      R5; até 500 MB juntam-se em memória (limite não medido no iPhone),
      acima falha com aviso — sem proxy de streaming.
- [x] **P6 — Design responsivo.** `design/claude-design/` aplicado a `ui/`,
      390px a 4K, herdado de graça por desktop e Android.
- [x] **P4 — Push (VAPID/RFC 8292).** `relay/src/vapid.rs` + `/subscricao`;
      provado a sério contra o FCM do Chrome. **Por confirmar:** a entrega
      real do serviço de push da Apple — só ela pode fechar isto.

**Confirmado a sério no iPhone real (01/09/2026)**, não só em Chromium: P1,
P2, P5, P3, P6 — instalação no ecrã principal, `localStorage` a sobreviver,
envio e receção nos dois sentidos byte a byte, descarga disparada por
script sem gesto de clique. Dois bugs só visíveis fora do Chromium,
corrigidos (`buscarNoRelay()` só corria uma vez ao abrir a app; o ecrã
vazio de dispositivos prometia deteção automática que a PWA não tem). Ver
`BACKLOG.md` para o desenho completo de cada fase e a prova de ponta a
ponta.

### S9 — Distribuição e exposição pública

Nasce da Review2 (secção 7) mais o que ficava por fechar do S7: FCM no
Android (a outra metade do R6, Web Push iOS já fechado no S8) e os
binários musl.

- [x] **Q1 — Binários musl estáticos (amd64 + arm64).** Com `cross`
      (`:edge`, não `:0.2.5` — `glibc` velha demais). Corridos a sério fora
      da LAN: `aarch64` no Pi 5 sem `musl` instalada, `x86_64` num Alpine.
- [x] **Q2 — Retoma de descarga na PWA (Review2 F-08).** Mesmo desenho do
      cliente nativo (`trazer()`): não recriar o `Descarga` wasm entre
      tentativas basta para reter o progresso, sem `IndexedDB` nem estado
      novo. Provado a sério contra uma queda de rede real, não simulada por
      interceção.
- [x] **Q3 — Limitador de débito (Review0 F-02 / Review2 F-09).**
      `tower_governor` nas rotas públicas sem assinatura, chave por IP do
      socket TCP.
- [x] **Q4 — DDNS pública (DuckDNS).** `pombo-correio.duckdns.org`, TLS a
      sério (`acme.sh`, desafio DNS-01) atrás de um `nginx` novo no Pi.
      Verificado de fora a sério (`check-host.net`, não só loopback/LAN).
      Os quatro dispositivos de casa autorizados a sério em
      `--autorizados`.
- [x] **Q5 — Push FCM (Android), o resto do R6.** `relay/src/fcm.rs` — JWT
      RS256 → access token OAuth2 → FCM HTTP v1, sem carga (mensagem
      vazia, só acorda a app, prioridade alta). Confirmado a sério na
      OnePlus Pad 2: token real, subscrição registada, entrega
      `ENTREGUE` pela API da Google.

**Sprint 9 completo, Q1 a Q5 verificados de ponta a ponta** — nada só
provado em loopback. Ver `BACKLOG.md` para o desenho, as armadilhas
(sintomas enganadores em Q2/Q3, builds Android mortas em Q4/Q5) e a prova
de cada fase.

### Antes do S9

| Item | Bloqueia o S9? | De quem é a próxima ação | Ação concreta |
|---|---|---|---|
| Binários musl | Não | Eu | `rustup target add` + build, sem depender de ninguém |
| Retoma de descarga na PWA | Não | Eu | Regra 8 primeiro, depois código |
| Limitador de débito | Não | Eu | Testável em loopback já |
| DDNS (DuckDNS) | Feito | — | — |
| Push — Firebase (Android) | Feito (credenciais dadas) | — | Falta só provar a build Android a sério, sem bloqueio nenhum do André |
| Confirmação do P4 no iPhone (dívida do S8, não do S9) | Não | André | Definições → Ativar notificações, esperar por um depósito |

### S10 — Fechar o que a Review3 encontrou

Nasce da `audit/Review3.md` (02/09/2026) — F-11 já era um bug de produção a
sério (o limitador de débito deixara de ver o IP real de quem pedia, atrás
do `nginx` do Q4), não só uma sugestão para mais tarde.

- [x] **T1 — Limitador de débito atrás do `nginx` (F-11).** `IpConfiavel`,
      `KeyExtractor` próprio: só confia em `X-Real-IP` quando o IP do
      socket é mesmo o `127.0.0.1` do `nginx` local — a porta 53319 direta
      continua a usar o IP do socket, sem furo de confiança cega. Confirmado
      a sério no Pi: rajada pública leva 429, o próprio Pi continua a 200.
- [x] **T2 — Notificação local ao acordar por FCM (F-12).**
      `FcmService.onMessageReceived` deixou de estar vazio — canal
      `pombo_push`, `PendingIntent` para a `MainActivity`, pedido runtime de
      `POST_NOTIFICATIONS`. Confirmado a sério no tablet (processo morto a
      sério, não por `force-stop` — a diferença importou, ver
      `BACKLOG.md`). **Por confirmar:** tocar na notificação abre a app —
      a ligação USB à tablet caiu a meio da verificação.
- [x] **T3 — Autorização automática pelo link de pareamento (F-14).**
      `Ficha` ganhou `de` (quem criou o depósito); `POST /convite` autoriza
      quem prova ter aberto um link cujo criador já é de casa. Confirmado
      com Playwright a sério: um link aberto por uma identidade nova
      resgatou-se sozinho, depois usado para enviar um ficheiro real.
      **Furo achado e corrigido antes de fechar, não hipotético:** nada
      consumia o convite — um único link reencaminhado autorizava um
      número sem limite de dispositivos. Corrigido com
      `Deposito::consumir_convite`; reverificado com duas identidades
      diferentes, a segunda falha visivelmente na interface. **A Review4
      (04/09/2026) revisitou este T3 com escrutínio adicional — ver S11.**

**Deliberadamente fora:** F-13 (retoma de upload na PWA) e WebRTC/
hole-punching — nenhum resolve uma dor que exista hoje. Ver `BACKLOG.md`
para o desenho, o porquê de cada correção e a prova completa.

---

### S11 — Fechar o que a Review4 encontrou

Nasce da `audit/Review4.md` (04/09/2026), auditoria do S10 completo em postura
de "advogado do diabo". Sem achado criptográfico, mas com o **T3 do S10
revisitado a sério**: seis achados (F-15 a F-20), um só de severidade ALTA —
o resto do S10 (T1, T2) sai confirmado sem reparo. A forma repete-se: uma
certeza que deixou de ser verdade assim que se pensou nela a fundo — desta
vez não é "não há proxy" (F-11), é "só um pedido de cada vez lê a ficha do
convite" (F-15), implícito no código, nunca escrito, e falso desde que
`convidar()` existe.

Três fases, T1 primeiro por ser a única de severidade ALTA e a única com
desenho de estado a sério (regra 8); T2 e T3 são independentes entre si e
de T1:

- [x] **T1 — Corrigir a corrida no consumo do convite (Review4 F-15, F-16).**
      `convidar()` lê a `Ficha` (com `de` preenchido), decide, autoriza, e só
      **depois** apaga o `de` num pedido de escrita à parte — quatro passos,
      nenhum trinco entre eles. Dois pedidos concorrentes para o mesmo `id`
      (o cenário exato para que um link existe: reencaminhado, aberto por
      duas pessoas ao mesmo tempo) leem os dois o `de` preenchido antes de
      qualquer um o apagar, e saem os dois autorizados — o mesmo furo que o
      T3 do S10 corrigiu para "reencaminhado ao longo do tempo" continua
      aberto para "reencaminhado ao mesmo tempo". Agravante: o `consumir_
      convite` de hoje ignora o erro de escrita (`let _ = ...`) — se falhar,
      o convite fica ilimitado para sempre, sem aviso nenhum.

      **Desenho (regra 8):** juntar tudo — ler, decidir, autorizar, apagar
      `de` — numa só função em `Deposito`
      (`resgatar_convite(id, impressao, autorizados: &Autorizados)`, a
      assinatura implementada — passar `&Autorizados` em vez do
      `autorizado: impl Fn(&str) -> bool` do rascunho inicial, porque o
      passo 3 também precisa de **mutar** por `autorizar()`, não só
      consultar `deixa()`), atrás de um `tokio::sync::Mutex` novo no
      próprio `Deposito` — não por
      `id` (adicionar um mapa de trincos para um caminho tão pouco
      frequente é estado a mais para o mesmo resultado, o mesmo raciocínio
      que já afastou a cache de OAuth2 do FCM). Dentro do trinco, por esta
      ordem exata:

      1. Se `autorizados.deixa(&prova.impressao)` já for verdade (outra aba
         do mesmo browser já correu isto primeiro), devolver sucesso sem
         tocar em mais nada — cobre duas abas concorrentes sem as precisar
         distinguir de um ataque.
      2. Confirmar `para == CAIXA_LIGACAO` e `de` autorizado; senão, o 403
         de sempre.
      3. Chamar `autorizados.autorizar(&prova.impressao)`. **Se isto
         falhar (erro de I/O), devolver 500 e sair sem tocar em `de`** — o
         convite continua vivo para a próxima tentativa.
      4. Só depois de `autorizar()` ter tido sucesso, apagar o `de` da
         ficha e gravar.
      5. Largar o trinco.

      `convidar()` chama `resgatar_convite` uma vez só; deixa de existir
      uma segunda chamada a `consumir_convite` à parte.

      **Porque a ordem importa — "Ghost Burn" (achado da Review5 sobre
      este próprio plano, antes de qualquer código ser escrito):** a
      primeira versão deste desenho apagava `de` **antes** de autorizar,
      "o convite gasta-se primeiro". Isso partia a retentativa que o F-16
      pede a seguir: um 500 transitório (disco a meio) deixava o convite
      já sem `de`; a *segunda* tentativa da PWA já não encontrava convite
      nenhum e levava um 403 definitivo, que a PWA lê como recusa e apaga
      do `localStorage` para sempre — a retentativa do F-16 tornava-se
      inútil exatamente no caso para que foi desenhada. Autorizar primeiro
      e só apagar `de` depois de confirmado é a única ordem em que um erro
      500 deixa o convite intacto para a retentativa ter algo para repetir.

      **Pré-mortem (regra 9):** se dois pedidos chegarem ao mesmo tempo
      para o mesmo link, o trinco serializa-os. Duas abas do mesmo browser:
      a primeira autoriza e apaga o `de`; a segunda, já dentro do trinco,
      vê o requerente já autorizado no passo 1 e sai também com sucesso —
      nunca as duas em 403. Um estranho a repetir o pedido depois de o
      link já ter sido gasto por outra pessoa: `de` já está vazio quando
      ele entra no trinco, passo 2 falha, cai no 403 de sempre — nunca
      autorizado.

      **F-16, na mesma fase por tocar o mesmo par pedido/resposta:**
      `resgatarConvites()` em `wasm-backend.ts` só entra no `catch` (e
      preserva o convite) se o `fetch()` falhar mesmo — DNS, rede em baixo.
      Uma resposta HTTP de erro (502 com o `nginx` de pé mas o relay a
      reiniciar, 500 de disco, 429 do limitador) chega como sucesso da
      Promise e o código apaga o convite na mesma, perdendo-o para sempre
      sem nunca ter sido resgatado a sério. Corrigir para olhar a
      `res.status`: 5xx/429 preserva (tenta no próximo arranque); qualquer
      outro caso apaga (sucesso, ou 403/404 definitivo — repetir não ia
      resultar). Seguro por construção: `convidar()` já responde 204 a quem
      já está autorizado, por isso um pedido perdido em trânsito mas que
      chegou a ser processado no relay não duplica nada ao repetir-se — o
      retry cai direto nesse caminho.

      **Implementado e testado** (`relay/tests/deposito_http.rs`, módulo
      `convite`): `duas_identidades_diferentes_a_disputar_o_mesmo_link_so_
      uma_ganha` prova a corrida do F-15 a sério (duas identidades
      *diferentes*, `tokio::join!`, corrido oito vezes seguidas para
      confirmar que não é sorte) — confirmado que falha, de facto, sem o
      trinco (sabotagem temporária local, revertida). `falha_a_autorizar_
      nao_apaga_o_convite_ghost_burn` prova o F-16 sem mocks: torna
      `autorizados.txt` só de leitura, confirma 500 sem apagar `de`, repõe
      a escrita e confirma que a retentativa tem sucesso.

- [x] **T2 — Ciclo de vida da descoberta e da notificação (Review4 F-17,
      F-18).** Duas pontas soltas, nenhuma protocolar — polimento de
      interface, sem desenho de estado.
      - **F-17:** quem abre um link pela `pagina/www/` descarrega o ficheiro
        e fecha o separador — nada convida a visitar a app principal (`/`),
        por isso o convite que `pagina.js` já guardou em `pombo-conhecidos`
        nunca chega a ser resgatado na prática, e ao fim dos 7 dias de
        retenção o `id` já nem existe para tentar. **Dois níveis, não um só
        (Review5):** um link permanente e discreto no rodapé do cartão
        ("O Pombo-Correio é livre e encriptado de ponta a ponta. Abrir a
        aplicação") — presente mesmo quando a descarga **falha**
        (ficheiro grande em rede com quota, armazenamento cheio: o caso em
        que o CTA só-pós-sucesso nunca aparecia, e exatamente o caso em que
        o convite mais precisa de outra via) — e, depois da descarga
        concluir com sucesso, um destaque dinâmico a substituir o botão
        ("Ficheiro guardado ✓. Queres enviar ficheiros de volta? Abrir o
        Pombo-Correio"). Mesmo padrão do resto da `pagina/`, sem framework.
        **Provado a sério:** `pombo-relay` real (`--pagina pagina/www`),
        link real criado por `pombo ligacao`, aberto num Chromium a sério
        por Playwright — o link permanente aparece antes de qualquer
        descarga, o destaque dinâmico substitui o botão depois de guardar
        (testado pelo caminho de memória, o do Safari/Firefox — o de
        `showSaveFilePicker` não foi tocado por esta fase), e o ficheiro
        descarregado bate byte a byte com o original.
      - **F-18:** a notificação local do T2 do S10 (`FcmService`, canal
        `pombo_push`, id `2`) só se limpa da barra do sistema se o
        utilizador tocar nela; abrir a app pelo ícone do ecrã principal
        deixa-a presa lá. **Tem de ser em `onResume()`, não `onCreate()`
        (Review5):** tocar no ícone com a app já em segundo plano (na
        gaveta de processos) invoca só `onStart()`→`onResume()` — o
        Android não volta a chamar `onCreate()` numa Activity que já
        existe, por isso "no arranque" sozinho falhava o caso mais comum.
        `NotificationManagerCompat.from(this).cancel(2)` em `onResume()`
        cobre arranque a frio e regresso com um só sítio.
        **Provado a sério, tablet OnePlus Pad real:** build assinada
        (`tauri android build --target aarch64 --apk`, `zipalign` +
        `apksigner` com o `debug.keystore`) instalada por `adb install`.
        Arranque a frio → `am start` outra vez com a app já em segundo
        plano (confirmado antes por `dumpsys activity activities`, task
        viva com `visible=false`) devolveu **"Warning: Activity not
        started, its current task has been brought to the front"** — a
        confirmação inequívoca de que só `onNewIntent`/`onResume` correram,
        nunca `onCreate`, exatamente o caso que a versão anterior do plano
        falhava. `logcat` sem `FATAL EXCEPTION` à volta do evento: o
        `cancel(2)` novo corre sem rebentar no sítio certo. O `cancel()`
        em si (que uma notificação id=2/canal `pombo_push` realmente
        desaparece) não foi refeito de água em pé — já validado a sério no
        mesmo tablet no S10 T2; o que esta fase mudava era só o timing, e
        é o timing que ficou provado.

- [x] **T3 — Robustez do relay (Review4 F-19 parte 2, F-20).** Dois arranjos
      pequenos e sem risco, cada um isolado no seu ficheiro.
      - **`/convite` sem limite de débito:** ao contrário de `/saude`,
        `/vapid` e `/d/{id}`, a rota fica de fora de `rotas_publicas()` e por
        isso do `GovernorLayer` — nada impede uma rajada de pedidos a forçar
        leituras de disco repetidas (a `Deposito::ficha` de cada tentativa).
        Corrige-se movendo a rota para dentro de `rotas_publicas()`: exige
        assinatura como já exige, mas passa a partilhar o balde por IP como
        as outras rotas alcançáveis sem autorização prévia. **Cuidado nos
        testes (Review5):** em `relay/tests/deposito_http.rs` todos os
        pedidos caem em `127.0.0.1` (o `IpConfiavel` não distingue em
        teste), e o balde de `rotas_publicas()` só aguenta 5 de rajada por
        segundo — um teste que dispare mais de 5 convites em sequência
        rápida contra a mesma instância leva 429 em vez do código
        esperado. Cada teste que exercite `/convite` várias vezes precisa
        da sua própria instância (porta efémera, já o padrão de
        `relay_com_lista`), nunca partilhar uma instância entre casos.
      - **`autorizados.txt` sem quebra de linha garantida:** `Autorizados::
        autorizar` faz `append` presumindo que o ficheiro já acaba em `\n`.
        Um ficheiro editado à mão sem essa quebra final funde a próxima
        impressão com a última linha existente, e ambas as identidades
        perdem o acesso ao reiniciar. **Numa escrita só (Review5):** não
        ler o último byte e depois fazer duas chamadas a `write` (uma para
        o `\n` em falta, outra para a impressão) — isso reabre, dentro do
        próprio ficheiro, a janela de corrida que o `O_APPEND` existe para
        fechar. Calcular `precisa_nl` a partir do último byte, montar a
        linha completa numa `String` só, e emitir um único `write_all()` —
        o kernel só garante atomicidade a uma escrita `O_APPEND`, não a
        duas seguidas.

      **Implementado e testado:** `cargo clippy --workspace --all-targets`
      dá zero, e `relay/src/autenticado.rs` ganhou o teste `autorizar_nao_
      funde_linhas_quando_o_ficheiro_nao_acaba_em_quebra_de_linha`; o
      isolamento de testes do `/convite` já era o padrão existente
      (`relay_com_lista`, porta efémera por teste) — nada a mudar aí.

      **Deliberadamente fora desta fase, revisto e reafirmado:** a
      recomendação de cache do *access token* OAuth2 do FCM (parte 1 do
      F-19) já é uma decisão tomada, escrita no próprio `fcm.rs` — "um
      depósito não é frequente o suficiente para a troca extra pesar; cache
      seria estado a mais para o mesmo resultado". A Review4 não trouxe
      nada de novo que mude essa conta (nenhum número de tráfego a sério
      desde então); fica de fora outra vez, pela mesma razão, não por
      esquecimento. **A Review5 reavaliou e concorda:** ~300ms por
      notificação sem estado persistente compensa a simplificação, dado o
      baixo volume de transferências concorrentes de um produto doméstico.

**Transição do S11 para o S12:** o upload fragmentado na PWA (Review3 F-13)
e o emparelhamento por QR Code foram agendados para o **S12** logo a seguir ao
S11 fechar, motivados pela intenção expressa de partilhar ficheiros de vídeo
e imagens pesadas acima de 1 GB.

---

### S12 — Ficheiros grandes (>1 GB) e emparelhamento por QR

Nasce da necessidade operacional de partilhar ficheiros volumosos (>1 GB,
como vídeos em resolução nativa) através da PWA, onde o upload monolítico
atual colapsa os limites de memória do navegador, aliado ao emparelhamento
visual sem atrito entre ecrãs desktop e telemóveis.

Três fases:

- [x] **T1 — Upload fragmentado e retomável na PWA (Review3 F-13 / Ficheiros >1 GB).**
      O upload monolítico atual (`wasm-backend.ts:depositarEm`) acumula todos os
      blocos de 1 MiB selados num único array `partes` no heap do JavaScript,
      gerando um único `Blob` gigante e um único `PUT /deposito/{id}?de=0`.
      Para ficheiros >1 GB, isto esgota a quota de memória do WebKit no iOS
      (limite Jetsam de ~1.4 GB), causando o crash fatal da aba (*"Aconteceu um
      problema com esta página web"*). Além disso, qualquer queda de ligação
      móvel ou bloqueio de ecrã perto do final perde todo o envio, exigindo
      recomeçar dos 0 MB.

      **Desenho:** o relay já suporta nativamente `PUT /deposito/{id}?de=N`
      (`servidor.rs:subir` e `deposito.rs:abrir_para_subir`). Adaptar a PWA
      para enviar o ficheiro em pedaços de tamanho moderado (ex: 16 MiB por
      pedido `PUT`), lendo do ficheiro via `File.slice()` sob demanda, selando
      os blocos correspondentes e libertando a memória RAM logo a seguir. O
      consumo de memória mantém-se plano em ~20 MB mesmo para ficheiros de 5 GB.
      Em caso de falha de rede, consultar `GET /deposito/{id}` para saber
      quantos bytes já estão no relay e retomar exatamente no offset certo.

      **Implementado, código escrito e verificado por leitura + typecheck —
      por provar em rede real (regra 3):** `depositarEm` em `wasm-backend.ts`
      agora sela e sobe em pedaços de `TAM_PEDACO_PUT` (16 MiB), com um ciclo
      de tentativas (`TENTATIVAS`, o mesmo número já usado na descida) por
      fora. **Pré-mortem — se a ligação morrer a meio de um `PUT`, o que
      conclui cada lado?** O relay pode já ter escrito parte dos bytes desse
      pedido (`escrever()` grava em fluxo, não atómico por pedido); o browser
      não sabe quantos. Por isso nunca se assume que o pedido falhado não
      escreveu nada — pergunta-se ao relay (`GET /deposito/{id}`, campo
      `bytes`) e retoma-se em `Math.floor(bytes / tamBlocoSelado)`, a fronteira
      de bloco completo mais próxima sem passar à frente do confirmado. Isto
      usa o próprio contrato documentado em `abrir_para_subir` — `de` menor do
      que o que lá está trunca o meio-bloco a mais, `de` maior dá erro — pelo
      que reenviar sempre a partir de uma fronteira de bloco é seguro por
      construção, sem exigir que `selarBloco` seja determinístico entre
      tentativas. Cada pedaço enviado emite `progresso` (mesmo `Evento` que a
      descida já usa), pelo que a UI ganha uma barra de progresso na subida
      sem nenhuma mudança no `App.svelte`. **Não corrido** com um ficheiro
      real >1 GB nem com uma queda de rede a sério — por decisão do André,
      ver T3 (ainda em aberto) e a nota de handoff.

      **Bug do lado do relay apanhado ao rever, e corrigido a sério (com
      teste, `cargo test`/`clippy` verdes):** este T1 torna comum um caminho
      que antes quase nunca acontecia — repetir um `PUT` ao mesmo `id` a
      seguir a uma ligação cair. `servidor.rs::subir` marcava o `id` em
      `a_subir` e só o largava numa instrução a seguir a um `.await`; se a
      ligação cai a sério a meio (o futuro do handler é cancelado pelo hyper,
      não devolve um erro normal), essa instrução nunca corria e a marca
      ficava presa até o relay reiniciar — a retoma automática receberia
      sempre 409. Corrigido com um `Drop` (`MarcaSubida`, novo `struct` em
      `servidor.rs`) que larga a marca em qualquer saída, incluindo
      cancelamento; `a_subir` passou de `tokio::sync::Mutex` a
      `std::sync::Mutex` (nunca se detém através de um `.await`, e um `Drop`
      não pode ser `async`). Teste
      `marca_subida_larga_ao_cair_mesmo_sem_remove_explicito` confirmado a
      falhar com o `Drop` sabotado de propósito (posto a não fazer nada) e
      revertido a seguir — 73 testes do `relay` continuam verdes.

- [x] **T2 — Emparelhamento por QR Code na interface.**
      Abrir links de ligação gerados no computador (`pombo ligacao` ou botão
      "Criar link" na UI) exige atualmente copiar e colar o URL através de
      aplicações de terceiros (WhatsApp, e-mail) para chegar ao telemóvel de
      um familiar.
      
      **Desenho:** exibir na interface (Desktop e Web) um QR Code contendo o
      URL do link de pareamento (`https://pombo-correio.duckdns.org/d/{id}#...`)
      gerado com uma biblioteca JS SVG/canvas leve e sem dependências nativas.
      O familiar não precisa de scanner na app: basta apontar a **câmara
      nativa do telemóvel** (iOS Camera ou Android Google Lens) ao ecrã do
      computador para abrir o link no navegador e desencadear a auto-autorização
      (construída em S10 T3 e S11 T1). Sem permissões de câmara (`CAMERA`) a
      pedir na app.

      **Implementado, por provar visualmente num telemóvel a sério (regra
      3):** biblioteca `qrcode-svg` (zero dependências, só gera SVG,
      confirmada na ponytail — nenhuma alternativa "stdlib"/já instalada
      existia). `App.svelte` gera o QR do `ligacao` corrente com `join: true`
      (um único `<path>`, não centenas de `<rect>`) e `xmlDeclaration: false`
      (a declaração XML por omissão não pertence a HTML injetado por
      `{@html}`) — confirmado por inspeção direta do SVG gerado (24,7 KB,
      bem formado). Sem tipos publicados pelo pacote: `qrcode-svg.d.ts`
      próprio, só com a forma usada. `npm run build` e `svelte-check` limpos.
      **Não verificado** apontar a câmara de um iPhone/Android a sério ao QR
      no ecrã — por decisão do André, ver a nota de handoff.

- [ ] **T3 — Validação e teste real de carga.**
      Transferir um ficheiro real de vídeo ou imagem com mais de 1 GB entre a
      PWA no iPhone e o relay no Raspberry Pi 5. Confirmar que o consumo de
      RAM permanece estável, simular corte de rede aos 50% e verificar que a
      retoma conclui com sucesso com hash BLAKE3 final idêntico.

---

## Verificação

- **Integridade:** hash BLAKE3 comparado nas duas pontas em todas as combinações
  de plataforma. Uma foto HEIC e um vídeo têm de chegar com o mesmo hash — é
  este o teste que valida o requisito central.
- **Degradação:** forçar cada caminho isoladamente (sem mDNS, sem Tailscale, sem
  acesso direto) e confirmar que o envio chega na mesma.
- **Falha de nó:** desligar cada servidor a meio de uma transferência; a app tem
  de recuperar sozinha.
- **Retoma:** cortar a rede a meio de um ficheiro grande; tem de continuar do
  bloco onde ia.
- **Usabilidade real:** um familiar não-técnico envia uma foto sem instruções
  faladas. Se falhar, é a interface que está errada, não o utilizador. É o
  critério que mais importa.

## Riscos

| Risco | Mitigação |
|---|---|
| HEIC transcodificado no iOS via Biblioteca de Fotos | Encaminhar para a app Ficheiros. Correção real exige app nativa (99 €/ano). |
| Instalar a PWA no iOS é pouco óbvio | Instruções com imagens; instalar pessoalmente nos dispositivos da família. |
| Tauri no Android é mais recente que no desktop | S6 isolado, com plugins já validados. Falha cedo e visivelmente. |
| Upload de casa é o gargalo com ambos os lados remotos | Aceite por agora. WebRTC se doer. |
| iOS pode despejar o armazenamento da PWA | Nada de estado crítico só no iOS; identidade recuperável por reemparelhamento. |
| A identidade (`SigningKey`, `StaticSecret`) vive em `localStorage`, legível por qualquer script que corra na origem da PWA (Review2 F-10) | Seguro no modelo atual — a origem (`raspberrypi...ts.net`) só serve o `ui/dist` do próprio relay, sem scripts de terceiros. Fica escrito aqui para não se tornar óbvio-depois-do-facto: **nunca meter analytics, CDN de terceiros ou qualquer `<script src>` externo nessa origem.** |
