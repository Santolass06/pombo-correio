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

### Autorização automática pelo link de pareamento

Discutido em 02/09/2026, ao ligar o `--autorizados` a sério pela primeira vez
(S9 Q4). Hoje `POST /deposito` exige a impressão digital de **quem envia**
já estar na lista, sem distinguir "estranho da internet" de "família que o
André já convidou por link" — cada telemóvel novo obriga a copiar a
impressão à mão para o ficheiro e reiniciar o relay. Sustentável para os
poucos dispositivos de hoje; não para o dia a dia com mais família.

**O que não serve:** aceitar a impressão ao primeiro contacto (TOFU) não
resolve nada aqui — o ataque que o `--autorizados` existe para travar (um
estranho a encher o disco) *também* é "uma primeira ligação"; não há como
distinguir os dois só pela ordem de chegada. TOFU protege contra uma chave
que *muda* depois de já confiada (o caso do SSH), não contra deixar entrar
quem chegar primeiro.

**O que já serve, e só falta ligar:** o link de pareamento (R5/P5) já é um
segredo de 128 bits que só o André partilha, pessoa a pessoa — só quem o
recebeu consegue abri-lo. Isso já *é* a autorização; falta o relay tratá-la
como tal. Direção a desenhar a sério (regra 8) quando arrancar: abrir um
link criado pelo André, ou ser o destinatário de um depósito assinado por
um dispositivo já autorizado, acrescenta essa nova impressão ao
`--autorizados` sozinho, sem o André mexer em ficheiro nenhum nem reiniciar
o serviço. **Pré-mortem, uma linha:** se dois links forem abertos ao mesmo
tempo por pessoas diferentes, cada um só pode adicionar a *sua própria*
impressão — nunca a de outro — senão um link vira forma de qualquer um se
autoautorizar.

Duas ideias do André para reforçar isto mais tarde, guardadas mas não
urgentes: **expirar dispositivos sem uso há 30+ dias** (remove-os da lista
sozinho) e um **segredo pessoal tipo PIN/número de faculdade** que torna um
dispositivo "não suspeito" por um número de dias após ações fora do padrão.
Ambas trazem estado novo a gerir (temporizadores, um segredo extra) — fazem
sentido depois da autorização automática existir, não antes.

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
- [ ] **S9 — Distribuição e exposição pública** (Q1 musl, Q2 retoma de descarga, Q3 limitador de débito e Q4 DDNS pública feitos e verificados de fora; Q5 FCM Android bloqueado no André)

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

**Levantamento de 01/09/2026, antes de arrancar:** `ui/src/lib/api.ts` — as
12 funções que a interface usa (`identidade`, `dispositivos`,
`enviarFicheiro`, `buscarNoRelay`, `criarLigacao`, `cancelar`, os eventos…) são
**todas** `invoke()`/`listen()` do Tauri. Nenhuma existe fora de um WebView
com Rust por trás — um Safari a abrir a app hoje não tem onde bater. O
`pagina/` (R5) só resolve "abrir um link": sem identidade persistente, sem
selar, sem subir nada. O S8 é maior do que as três balas que aqui estavam.

**Decisão de arquitetura:** a app do iPhone é a **mesma interface Svelte**
(`ui/`) a correr sem Tauri — não uma página à parte em JavaScript simples.
Só assim o design (`design/claude-design/`) vive num sítio só e serve os três
consumidores (desktop, Android, iOS) de vez, como o desenho original sempre
previu ("ui/ ... quatro consumidores").

Fases pequenas, ao estilo do S6/S7 — cada uma a acabar em algo que corre e se
prova **no iPhone a sério**, não só em Chromium (regra 10; o R5 já mostrou
que Chromium esconde coisas, como o `showSaveFilePicker`).

**Desenho, antes de mexer (regra 8):**

- **Um único sítio serve a app.** `localStorage`/`IndexedDB` são por origem —
  uma PWA instalada a partir do Pi e outra a partir do A10 seriam duas apps
  com identidades diferentes. A app fica servida **só pelo Pi**
  (`raspberrypi...ts.net`, o nó sempre ligado); o A10 continua a ser só um
  relay. A HA dos dois nós (R4) é para onde os **depósitos** vivem, não para
  onde a app é servida — a `WasmBackend` fala com os dois por `fetch()`.
- **CORS entra por causa disto.** A app do Pi vai chamar `/caixa` e
  `/deposito*` no A10. Sem `Access-Control-Allow-Origin` (mais os cabeçalhos
  `x-pombo-*` e `Range` no preflight), a chamada ao segundo nó falha em
  silêncio no Safari e a HA parece funcionar até o Pi cair. Entra no P2/P3/P5,
  onde o `fetch()` é escrito.
- **Ordem real: P1 → P2 → P5 → P3 → P4 → P6**, não a ordem de leitura da
  lista abaixo. `chaves_para()` só resolve depois de um `conhecidos.json` ter
  sido escrito, e isso só acontece ao **receber** uma oferta assinada — nunca
  aconteceu vindo do iPhone. Por isso ninguém consegue depositar *para* o
  iPhone (P3) antes de o iPhone ter enviado *de* si próprio uma vez (P5). A
  lista de tarefas fica pela ordem lógica de leitura; a ordem de trabalho é
  esta.
  - **Pré-mortem, uma linha:** se o depósito P5→relay morrer a meio da
    subida, quem envia (o iPhone) vê `Falhou` e tenta de novo — exatamente
    como já acontece no `depositar_em` de sempre; nada disto é novo código,
    só um chamador novo.
- **A primeira identidade do iPhone não está em `--autorizados`.** Gerada no
  primeiro arranque (P2), o relay recusa-lhe o primeiro depósito com 403 até
  o André a acrescentar aos dois nós e reiniciar o `pombo-relay` — o
  `Autorizados` só se lê uma vez, ao arrancar (`montar()`). A interface tem de
  mostrar a impressão digital em destaque no primeiro arranque para isto ser
  uma instrução, não um mistério.

- [x] **P1 — Fundação instalável.** `manifest.json`, ícones (`apple-touch-icon`
      180×180 gerado sem canal alfa, confirmado a bytes — verificado com
      `PIL.getextrema()`, não só de olho), meta tags do Safari
      (`apple-mobile-web-app-capable` + a versão sem `apple-` que o Chrome
      pede, `black-translucent`, `viewport-fit=cover`), service worker
      mínimo (rede-primeiro-cai-para-cache, sem pré-carregar nomes de
      ficheiro do build). `relay/` ganhou `--app <ui/dist>`: rota
      `fallback()` do axum, que só dispara depois de todas as rotas
      exatas — sem colisão possível com `/saude`, `/deposito*`, `/caixa`,
      `/d/{id}`. **Provado em Chromium/loopback** (`sw.register` regista com
      `scope: "/"`, manifest e ícones servidos com o `content-type` certo, 5
      testes novos no `relay`); **HTTPS real, Add-to-Home-Screen e o modo
      standalone no Safari continuam por provar no iPhone** — só aí o R7
      (TLS) e este P1 se encontram a sério. Decisão tomada aqui, sem
      perguntar: a app fica servida **só pelo Pi** (ver "Desenho" acima).
- [x] **P2 — A `ui/` corre sem Tauri (identidade).** `api.ts` deteta
      `window.__TAURI_INTERNALS__` e passa a chamar `invoke`/`listen` ou o
      novo `wasm-backend.ts`, função a função — sem uma classe `PomboBackend`
      formal: `enviarFicheiro`/`criarLigacao` têm assinaturas que só fazem
      sentido no Tauri (caminho de ficheiro vs. `File` do browser), por isso
      só as funções com a mesma forma nos dois lados (`identidade`,
      `dispositivos`, `cancelar`, `buscarNoRelay`, `aoAcontecer`) ganharam
      ramo — abstrair as outras à força seria a mesma interface a fingir que
      dois desenhos diferentes são um só. `pagina/` ganhou `IdentidadeWasm`
      (`gerar`/`deSegredo`/`numeroSeguranca`/`impressaoCurta`), bindings finos
      sobre o `Identity` que já compilava para wasm32; a persistência em si
      (ida e volta ao `localStorage`) fica em TypeScript — não há
      `web-sys` novo, é a mesma API que qualquer página usa. **Provado em
      Chromium/loopback:** a app carrega sem Tauri, gera uma identidade nova
      na primeira visita, mostra o número de segurança real no separador
      Definições, e mantém a mesma identidade a recarregar a página (bytes
      de `localStorage` idênticos antes/depois). **Confirmado no iPhone
      real (01/09/2026):** `localStorage` sobrevive à instalação como app no
      ecrã principal — o computador aprendido por link antes de instalar
      continuou lá depois. Não foi preciso `IndexedDB`. Mostrar a
      impressão digital (`impressaoCurta()`) em destaque na interface fica
      para o P5, que é quando ela passa a ser precisa (`--autorizados`).
**Desenho do P3, antes de mexer (regra 8):**

- **`Descarga` ganha um segundo construtor, não um struct novo.** O de
  sempre (`nova`) abre o envelope com a `TransferKey` do fragmento do URL;
  o da caixa (`daCaixa`) abre-o com a `Identity` do dispositivo
  (`Envelope::open`). Os dois chegam ao mesmo sítio — um `Descarga` com o
  manifesto validado — e daí para a frente é o código que o R5 já tem e já
  testa (`bloco`, `proximo_tamanho`, o índice como AAD): sem isto duplicava-
  -se a lógica de abrir blocos, que é a parte que mais importa não errar.
- **Igual ao `trazer()` nativo, minus a escrita em disco.** Consulta
  `/caixa` (assinado), para cada id por completar salta (`trazer()` também
  salta — "um depósito que ainda esteja a subir fica para a próxima"),
  descarrega `/deposito/{id}/blocos`, decifra bloco a bloco.
- **Memória, com um limite explícito, não "até rebentar".** Sem
  `showSaveFilePicker` fiável (é o mesmo R5 que já sabia disto), os blocos
  decifrados juntam-se num `Blob`. Um ficheiro maior do que
  `LIMITE_MEMORIA` (o mesmo número do `pagina.js`, 500 MB, por
  consistência — **não medido em iPhone real**) não se tenta: falha com uma
  mensagem clara em vez de arriscar a aba. O proxy de streaming por
  service worker (para lá desse limite) fica de fora desta fase — o
  próprio texto do S8 já dizia que o limiar certo só se mede no aparelho a
  sério, e construir o proxy sem poder validar o número não prova nada a
  mais; entra se/quando um ficheiro real do tamanho de um vídeo do iPhone
  mostrar que faz falta.
- **Descarregar sem gesto de clique.** `buscarNoRelay()` corre ao abrir a
  app (não a um clique) — um `<a download>` disparado por script chega para
  gravar o ficheiro, mas não é `showSaveFilePicker` e por isso não pede
  onde guardar. **Confirmado no iPhone real:** o Safari deixa passar um
  download disparado por script, sem bloquear e sem gesto nenhum.
- **Pré-mortem, uma linha:** se a descida cair a meio, o depósito fica
  incompleto no relay (não se apaga) e a próxima `buscarNoRelay()` tenta-o
  de novo do princípio — sem retoma nesta fase, pela mesma razão do P5
  (`ponytail`, mesma nota).

- [x] **P3 — Receber pelo relay ao abrir a app.** Equivalente ao
      `buscarNoRelay()` do Android, em wasm+`fetch()`: consulta `/caixa`,
      decifra e verifica com o `core/` (`Descarga.daCaixa`, que reutiliza
      toda a verificação por bloco do R5). Ficheiros até 500 MB juntam-se
      num `Blob` e descarregam-se sozinhos (`<a download>` clicado por
      script); acima disso falha com aviso em vez de arriscar a aba —
      **o proxy de streaming por service worker fica de fora**, por decisão
      registada acima, não por esquecimento. **Provado em
      Chromium/loopback**, processos a sério (relay, `pombo enviar` nativo,
      PWA a receber por `buscarNoRelay()`), byte a byte idêntico.
      **E confirmado no iPhone real (01/09/2026):** ficheiro depositado do
      computador chegou igual ao iPhone, descarregado sozinho ao abrir a
      app, sem gesto de clique e sem o Safari bloquear nada. O limite exato
      de memória continua por medir (não houve ficheiro perto de 500 MB à
      mão), mas o caminho abaixo desse limite está provado.
      **Correção pós-P5/P3, apanhada pelo `advisor()` a rever a sessão:** o
      cartão de dispositivo novo mostrava o número de segurança em branco
      — `Descarga` ganhou `numeroSegurancaDe()`, testado.

**Ordem revista: P6 antes do P4.** O P4 (push) não tem uma única linha
verificável a partir daqui — o `PushManager` só existe depois de instalar
no ecrã principal, e a entrega passa pelo serviço de push da Apple até a um
aparelho real. O P6 (design responsivo) é o inverso: prova-se por inteiro
em Chromium (`browser_resize` a 720p–4K, sem servidor nenhum de terceiros
envolvido), e o André pediu-o explicitamente à parte. Trocada a ordem por
isso — não por o P4 ser menos importante.

- [x] **P4 — Push.** Construído a pedido explícito do André, depois de eu ter
      proposto adiar (o bloqueio original: `pushManager.subscribe()` só
      existe depois de instalar no ecrã principal, e é essa chamada que dá o
      endpoint e o público a que o relay assina o JWT — não havia como
      escrever a assinatura ES256 sem a primeira execução a sério ser contra
      um serviço de push de verdade). O André decidiu avançar mesmo assim.
      **VAPID (ES256, RFC 8292):** `relay/src/vapid.rs` gera o par P-256 na
      primeira vez e guarda o escalar em `<dados>/vapid_chave`, `0600` — o
      mesmo `--dados` que já guarda os depósitos, já fora do `git` de
      propósito, por isso sem flag nova nenhuma a inventar. `GET /vapid`
      devolve a chave pública (sem segredo, sem autenticação); `POST
      /subscricao` guarda a subscrição do browser, assinado como tudo o
      resto (`autenticado::verificar`, ação `"subscricao"`, `extra` = o
      `endpoint` — sem isso, uma assinatura apanhada a subscrever servia
      para registar o endpoint de outra pessoa em nome de quem a apanhou).
      **O aviso sai só quando o depósito fica completo** (`subir()`, não
      `criar()`) — avisar mais cedo mandava quem recebe atrás de um
      manifesto que a `caixa` já lista mas que ainda não fecha (P3/P5 não
      sabem retomar). Um `410`/`404` do serviço de push apaga a subscrição
      morta sozinho.
      **Simplificação deliberada (`ponytail:` no código):** o service worker
      **não** pergunta ao `/caixa` sozinho — mostra sempre o mesmo aviso
      genérico ("Chegou um ficheiro novo"). O desenho original queria isso
      sem carga nenhuma, mas o service worker não tem acesso a
      `localStorage` (só a `IndexedDB`), e ir lá buscar a identidade para
      assinar um pedido a sério dobrava o âmbito desta fase por pouco — o
      clique na notificação já abre a app, que faz exatamente essa pergunta
      sozinha (P3, já construído). Teto: nenhuma pré-visualização do que
      chegou na própria notificação; sobe quando isso for pedido a sério.
      **Provado, não só em Chromium simulado — contra o serviço de push a
      sério do Chrome (FCM), com o botão "Ativar notificações" a sério em
      Definições:** `pushManager.subscribe()` aceitou a chave pública VAPID
      e devolveu um `endpoint` genuíno da Google; `POST /subscricao`
      guardou-a (`204`). Cinco testes de integração
      (`relay/tests/deposito_http.rs`, módulo `push`) provam a escada
      completa contra um serviço de push falso que captura o pedido a
      sério: o aviso só sai depois de o depósito ficar completo (não a meio
      de uma subida cortada), o cabeçalho `Authorization: vapid t=…, k=…`
      está bem formado, o `aud` do JWT bate com a origem do **serviço de
      push**, não com a do relay, um `410` apaga a subscrição, e um depósito
      sem subscrição continua a entregar-se na mesma. Um teste de
      auto-verificação (`vapid::testes`) assina um JWT e confirma-o com a
      própria chave pública — é o que teria apanhado um erro de codificação
      da assinatura (DER em vez de r‖s) que um serviço de push a sério
      recusaria em silêncio com 401, sem dizer porquê.
      **Achado a rever isto antes de escrever código:** o `relays` dos dois
      nós de casa tinha o A10 primeiro — "ao depositar, fica-se com o
      primeiro que responder ao `/saude`" — e a PWA só verifica a própria
      origem (o Pi) ao buscar. Um envio nativo para um contacto aprendido
      por link caía sempre no A10, onde a PWA nunca olharia; sem isso
      corrigido, o P4 não teria nada de real para desencadear. Corrigido
      (Pi primeiro nos dois `relays`) e documentado no `COMO_CORRER.md`.
      **Por confirmar no iPhone real:** tudo — a Apple é um serviço de push
      diferente do Chrome/FCM, e o `PushManager` só existe depois da
      instalação. O que muda em relação ao resto do S8: desta vez a
      mecânica de fio (formato do JWT, do pedido, da chave pública) já foi
      testada contra infraestrutura a sério, só não a da Apple — o risco que
      sobra é mais estreito do que "nada disto correu fora de um mock".
      `CLAUDE.md` diz para não fechar sem máquina real.
**Desenho do P5, antes de mexer (regra 8) — revisto depois de ler o código a
sério, não só o resumo do plano:**

- **O caminho direto está fora, para sempre, não "por agora".** A PWA corre
  em HTTPS (tem de correr, para ser instalável); o `receber::servir` de um
  familiar fala HTTP simples na LAN. Uma página HTTPS não pode fazer
  `fetch()` a HTTP — é *mixed content*, bloqueado pelo browser, não uma
  falta de código nossa. A escada do iPhone tem **um degrau só: o relay.**
  Isto fecha a dúvida que ficava aberta no texto antigo da bala P5
  ("direto se houver destino alcançável").
- **`depositar_ligacao` não ensina nada a ninguém — verificado no código,
  não assumido.** Corrigindo uma leitura anterior deste plano: pensava-se
  que um link criado do iPhone via `CAIXA_LIGACAO` bastava para desbloquear
  o `chaves_para()` do lado de casa. Não basta: um depósito `CAIXA_LIGACAO`
  nunca aparece em nenhum `/caixa` (o filtro é por impressão digital, e a
  `CAIXA_LIGACAO` não é uma), por isso `trazer()` — a única função que
  escreve em `conhecidos.json` — nunca lhe toca. Criar um link não regista
  o iPhone em lado nenhum.
- **O que regista, de facto: `trazer()` em `rede/src/relay.rs:459-462`**,
  chamado quando um dispositivo de casa faz `buscar_no_relay()` e encontra
  um depósito **endereçado à sua própria identidade**. Para isso acontecer,
  o iPhone precisa de saber, à partida, a `PublicIdentity` completa de
  **um** dispositivo de casa — não só o nome, as duas chaves públicas.
- **De onde vem essa identidade, sem inventar uma segunda cerimónia de
  confiança:** a família já tem uma — o link sem instalação (R5). O André
  cria um link no desktop (`criarLigacao`, já existe) e abre-o no iPhone.
  A oferta que o `/d/{id}` serve já traz a `PublicIdentity` completa de
  quem criou o link (`Oferta.de`); só faltava a página do lado do browser
  guardá-la. Dá-se ao `Descarga` (em `pagina/`) um acessor novo para a
  identidade completa (não só a etiqueta que `.de()` já dá), e o
  `pagina.js` grava-a num registo próprio em `localStorage`
  (`pombo-conhecidos`) — o mesmo ficheiro `pombo-relay` a servir os dois
  papéis (a app instalada e a página do link), por isso é a mesma origem e
  o mesmo `localStorage`. **Por confirmar no iPhone real:** se uma PWA
  instalada no ecrã principal partilha mesmo o `localStorage` com o Safari
  normal para a mesma origem — é a mesma incerteza do ITP já registada no
  P2, agora com uma segunda consequência se for "não": sem partilha, este
  truque de aprendizagem não chega à app instalada e passa a ser preciso
  outra via (ainda por desenhar).
- **Duas passagens pelo `File`, não uma — e é a mesma forma do caminho
  nativo.** O relay exige o manifesto completo (raiz de Merkle) antes de
  aceitar o `POST /deposito`; o manifesto só fecha depois de ver todos os
  blocos. Por isso: 1ª passagem sobre o `File` (via `file.slice()`, sem
  carregar o ficheiro todo — o `File` já vive fora da memória do
  JavaScript) só para resumir cada bloco e fechar o manifesto; 2ª passagem
  sela e sobe cada bloco. É exatamente o que `depositar_em` já faz em
  Rust nativo (`Manifest::from_reader` primeiro, `fluxo_selado` depois,
  reabrindo o ficheiro) — não é um desenho novo, é o mesmo com `File.slice`
  no lugar de `std::fs::File`.
- **A fórmula da assinatura passa a viver num sítio só.** Existia
  duplicada — `relay/src/autenticado.rs::mensagem()` (quem verifica) e
  `rede/src/relay.rs::assinado()` (quem assina, nativo) — e o wasm ia ser
  uma terceira cópia. Três cópias de um formato de string
  (`"pombo-{acao}-v1|{impressao}|{extra}|{instante}"`) seria o tipo de
  coisa que diverge um dia sem ninguém notar até uma assinatura deixar de
  bater. Move-se para `core::protocolo::mensagem_assinada`, e os dois
  sítios existentes passam a chamá-la.
- **CORS entra a sério aqui, não é só teoria do P1.** A PWA fica servida
  pelo Pi (decisão do S8); os pedidos assinados ao A10 são de outra
  origem. Sem `Access-Control-Allow-Origin` e sem os cabeçalhos `x-pombo-*`
  no preflight, o segundo nó fica inacessível a partir do Safari — a HA do
  R4 degrada-se para um nó só, em silêncio, sem erro nenhum que o diga.
  Como a autorização já vem da assinatura (não do CORS), abrir largo aqui
  não abre nada que a assinatura já não guardasse.
- **A impressão digital sai já no P5, não fica adiada.** É aqui que o
  primeiro depósito do iPhone pode levar 403 (não está em
  `--autorizados`) — a interface mostra `impressaoCurta()` em destaque
  antes de se tentar depositar, não só depois de falhar.
- **Pré-mortem, uma linha:** se o `PUT` do depósito morrer a meio da
  segunda passagem, o iPhone vê `Falhou` (o mesmo evento de sempre) e o
  utilizador tenta outra vez — a retoma por `?de=N` já existe do lado do
  relay; o P5 não teve de inventar nada aí, só usar.

- [x] **P5 — Enviar do iPhone.** `<input type=file>` escondido faz de
      seletor (o Tauri `open()` não existe fora do Tauri); duas passagens
      pelo `File` (resumir com `ManifestoBuilderWasm`, depois selar com
      `selarBloco`/`construirOferta` e subir por um único `PUT` com corpo
      `Blob`); sobe **só ao relay**, nunca direto — confirmado, não só
      previsto: mixed content bloqueia mesmo `fetch()` HTTPS→HTTP. O
      destino aprende-se ao abrir um link criado no desktop: `Descarga`
      ganhou `remetenteJson()`, e o `pagina.js` grava a `PublicIdentity` de
      quem criou o link em `localStorage` (`pombo-conhecidos`), que a app
      instalada lê em `listarDispositivos()`. A fórmula de assinatura dos
      pedidos ao relay (`core::protocolo::mensagem_assinada`) deixou de
      estar duplicada — o `relay/` e o `rede/` nativo passaram a usá-la
      também. O relay ganhou CORS (`tower-http`, largo de propósito: quem
      autoriza é a assinatura) para a HA entre os dois nós continuar a
      valer a partir do Safari.
      **Provado de ponta a ponta, com processos a sério, não só em
      loopback simulado:** `pombo-relay --app ui/dist`, um `pombo ligacao`
      nativo aberto no Chromium (aprende o destinatário), um ficheiro
      enviado da PWA por `fetch()`, e `pombo caixa` nativo a recebê-lo —
      **byte a byte idêntico** (`diff` limpo) — com o aviso de "dispositivo
      novo" a aparecer corretamente do lado de quem recebe. O "criar link"
      a partir da PWA também foi provado (a página do link abre, decifra o
      manifesto e mostra nome/tamanho corretos). **Simplificações
      deliberadas** (`ponytail:` no código): uma só tentativa de subida,
      sem retoma; um único relay (a própria origem da PWA), sem HA do
      lado do emissor. **Confirmado no iPhone real (01/09/2026):** o
      `localStorage` partilhado entre a PWA instalada e o Safari normal (de
      que o aprender-por-link depende) sobrevive à instalação — envio e
      receção nos dois sentidos, entre o iPhone a sério e a app do
      computador, byte a byte iguais, com uma correção no caminho: ver
      "bug do `buscarNoRelay()` só ao arranque" na secção seguinte.
- [x] **P6 — O design a sério, responsivo.** A extração do design empacotado
      (`design/claude-design/_ds/nocturne-.../styles.css` — o sistema já
      linkado a partir do `.dc.html`, não a página composta) confirmou que
      o `app.css` do S5 já usava a folha certa; o que faltava era o `@font-
      face` do "Inter" que só existe resolvido dentro da ferramenta de
      design (a raiz do `OTS parsing error` do P1) — **removido**, com
      `system-ui` (já era o segundo nome em `--font-heading`/`--font-body`)
      a fazer de letra, sem ficheiro nenhum a servir. `ui/src/App.svelte`
      ganhou uma grelha responsiva para "Os teus dispositivos"
      (`grid-template-columns: repeat(auto-fill, minmax(240px, 1fr))` —
      1 coluna num telemóvel, várias num ecrã largo, sem pontos de quebra à
      mão) e `.app` passou de 780px fixo para `clamp(780px, 70vw, 1400px)`
      (texto continua estreito e legível; a grelha ganha espaço em ecrãs
      grandes). Dois bugs visuais **encontrados a testar, não hipotéticos**:
      o cartão "Para quem não tem a app" sobrepunha-se a si próprio em
      telemóvel (`.linha:has(.btn-block-mobile)` empilha agora abaixo de
      480px) e `--autorizados` partia a meio do "--" ao fim da linha
      (`code { white-space: nowrap }`). **Provado em Chromium**:
      `browser_resize` a 390 (telemóvel), 768 (tablet), 1280×720, 1920×1080,
      2560×1440 e 3840×2160, nos dois temas — sem deslocamento horizontal
      em nenhum (`scrollWidth === clientWidth` confirmado), grelha a 1→7
      colunas conforme a largura. Por ser o mesmo `ui/` em todo o lado, o
      desktop e o Android herdam o resultado de graça — nenhuma das duas
      builds foi corrida nesta verificação (fora do alcance daqui), mas não
      tocaram em nada que as distinga da PWA.

Cada fase segue a regra 8 (desenhar antes de implementar) quando arrancar a
sério — este roteiro é a ordem de trabalho, não o desenho protocolar de cada
peça.

### Estado do S8 (01/09/2026) — as seis fases construídas, cinco provadas no iPhone real

**Cinco fases fecharam com prova de ponta a ponta entre aparelhos a sério —
o computador (app nativa, A10) e um iPhone real, não só Chromium/loopback:**
P1, P2, P5, P3, P6. Envio e receção nos dois sentidos, byte a byte iguais;
instalação no ecrã principal, modo isolado; `localStorage` sobrevive à
instalação (a incerteza repetida em P2/P3/P5, fechada); descarga disparada
por script no Safari, sem gesto de clique, sem bloqueio.

**O P4 (push) fechou depois, a pedido explícito do André** — eu tinha
proposto adiá-lo (ver a bala do P4 para o porquê). Provado contra o serviço
de push a sério do Chrome (não o da Apple — ver a bala) e por cinco testes
de integração contra um serviço de push falso que captura o pedido HTTP
real. Ainda por confirmar no iPhone: tudo o que só a Apple pode confirmar.

**Dois bugs reais, só visíveis a testar com o iPhone a sério, não hipotéticos:**

- **`buscarNoRelay()` só corria uma vez, ao abrir a app** (`App.svelte`,
  tanto no Tauri como na PWA) — quem já tinha a app aberta antes de o outro
  lado enviar nunca reparava no depósito novo, sem reiniciar. O comentário
  no Rust já dizia "chamado ao abrir a app e **de vez em quando** pela
  interface", mas nada chamava o "de vez em quando". Passou a correr no
  mesmo temporizador de 4 s que já refrescava a lista de dispositivos.
- **O texto do ecrã vazio de dispositivos mentia em modo PWA:** "Abre o
  Pombo-Correio noutro aparelho, na mesma rede. Aparece aqui sozinho" — verdade
  no Tauri (mDNS), falso na PWA (sem mDNS possível num browser; só se
  conhecem por link). Corrigido para um texto próprio nesse modo.

Os dois foram corrigidos, reconstruídos e reimplantados (Pi + A10) antes da
confirmação final do André.

**O S8 está construído por inteiro.** Falta um guião de verificação no
iPhone para o P4 — pedir permissão de notificações em Definições, esperar
por um depósito, confirmar que a notificação chega e que tocar nela abre a
app — o único passo que só a Apple pode fechar.

### S9 — Distribuição e exposição pública

Nasce da Review2 (secção 7) mais o que já ficava por fechar do S7: FCM no
Android (metade do R6, a outra metade — Web Push iOS — já fechou no S8) e os
binários musl que nunca chegaram a arrancar. Nenhuma das cinco fases é um
bug — a Review2 não encontrou nenhum achado crítico nem médio no S8; isto é
trabalho novo, não reparação.

**Duas fases não têm bloqueador nenhum** (musl, retoma de descarga) e podem
arrancar já; as outras três dependem de uma decisão ou conta do André. Ver
"Antes do S9".

- [x] **Q1 — Binários musl estáticos (amd64 + arm64).** Feito com `cross`
      (docker por trás) em vez de instalar toolchains cruzados à mão — o
      `apt` só tem `musl-tools` para `x86_64`, nada para `aarch64-musl`. As
      imagens `:0.2.5` por omissão do `cross` têm `glibc` velha demais para
      o `rustc` atual (falha a compilar `libc`/`zeroize_derive`); fixadas as
      `:edge` num `Cross.toml` novo na raiz. **Provado real, não só em
      loopback**: o binário `x86_64` corre dentro de um `alpine` (zero
      `glibc`) e responde a `/saude`; o `aarch64` foi copiado a sério para o
      Pi 5 (Debian, sem `musl` instalada) e correu lá, também a responder a
      `/saude` — `ldd` confirma "não é um executável dinâmico" nos dois.
      Comandos em `COMO_CORRER.md`.
- [x] **Q2 — Retoma de descarga na PWA (Review2 F-08).**

      **Desenho (regra 8), antes de mexer:** o cliente nativo
      (`rede/src/relay.rs::trazer`) já resolve isto — três tentativas
      (`TENTATIVAS`), cada uma a retomar do byte da fronteira do último
      bloco completo (`byte_do_bloco`, arredondado **para baixo**: o que o
      relay tenha a mais é meio bloco selado que ninguém abre). A PWA fica
      com o mesmo desenho, mas mais simples de escrever: o objeto wasm
      `Descarga` (`d`) já guarda internamente quantos blocos decifrou —
      **não recriar `d` entre tentativas** é suficiente para reter o
      progresso, sem precisar de `IndexedDB` nem de nenhum estado novo. O
      `feitos` que o código já conta para os eventos de progresso dá o
      deslocamento: `feitos * (tamBloco + SEAL_OVERHEAD)`, o mesmo cálculo
      do `byte_do_bloco` nativo — `tamBloco` é fixo (`m.tamanhoBloco()`,
      só o último bloco é mais pequeno, e nunca é o que fica a meio de uma
      retoma porque é sempre o último a pedir-se).

      **O que cada lado observa:** a rede cai a meio de um `leitor.read()`
      → a promessa nunca resolve com `done` limpo, ou rejeita — o buffer
      parcial desse bloco é descartado (uma variável local, nunca chega a
      `d.bloco()`), a próxima tentativa pede `Range: bytes=N-` a partir do
      último bloco **completo**, o relay responde com esses bytes outra
      vez (no máximo um bloco redundante, nunca dados corrompidos). Se a
      rede cair **exatamente** na fronteira de um bloco selado, é o mesmo
      caso: nenhum bloco parcial ficou pendurado, só a tentativa seguinte
      continua dali. Se as três tentativas falharem, cai-se no
      comportamento que já existe hoje: `trazer()` devolve `false`, e o
      próximo `buscarNoRelay()` (4 s depois, no temporizador do
      `App.svelte`) recomeça do zero com um `d` novo — para isto se tornar
      relevante a rede tinha de estar em baixo minutos a fio, o que já era
      verdade antes desta fase. Do lado do relay não muda nada: um `GET`
      com `Range` não mexe no temporizador de limpeza horária (R1); se o
      depósito expirar a meio das três tentativas (só possível se a rede
      esteve mesmo muito tempo em baixo), a tentativa seguinte recebe 404,
      cai no mesmo "falhou" genérico de sempre — não é um caso novo.

      **Corrida encontrada a desenhar isto, não hipotética:** o
      `setInterval` de 4 s em `App.svelte` **não espera** o
      `buscarNoRelay()` anterior acabar. Hoje isso quase nunca importa
      porque uma descida sem retoma é rápida; com três tentativas a
      esticar quanto tempo um `trazer()` pode ficar em curso num Wi-Fi
      mau, duas chamadas concorrentes ao **mesmo** `idRelay` deixam de ser
      só teóricas — cada uma criaria o seu próprio `d`, descarregaria o
      ficheiro a dobrar e tentaria apagar o depósito duas vezes. Corrigido
      com um `Set<string>` module-level em `wasm-backend.ts`: `trazer()`
      salta um `idRelay` já em curso, em vez de arrancar uma segunda
      descida. Sem isto, esta fase tornava um bug latente e raro num bug
      frequente.

      **Pré-mortem, uma linha:** se a ligação morrer a meio de uma
      tentativa, o browser conclui "falhou esta tentativa, tenta a
      próxima com o que já tem" — nunca "perdi tudo, recomeça do zero" a
      não ser que as três falhem; o relay não conclui nada, porque não
      distingue uma descida normal de uma retomada.

      **Provado a sério, contra o relay real, não um mock:** o `page.route()`
      do Playwright não intercetou o `fetch()` em stream de `.../blocos`
      nesta configuração (confirmado com três abordagens diferentes — ver
      `BACKLOG.md`), por isso a queda de rede foi simulada a direito num
      script que corre a mesma sequência do `trazerUmaVez()` (byte a byte
      igual, correndo contra o `pombo_correio_pagina.js` a sério) dentro do
      browser, contra um depósito real de ~2,5 blocos (2 621 440 bytes)
      auto-endereçado. Cortada a 1ª tentativa a meio do 2º bloco: a 2ª
      tentativa pediu exatamente `Range: bytes=1048616-` (a fronteira do 1º
      bloco selado, `tamBloco + SEAL_OVERHEAD`), recuperou os 3 blocos, e o
      ficheiro remontado bateu byte a byte com o original — a autenticação
      AEAD do `d.bloco()` teria falhado sozinha se a retoma tivesse desviado
      um único byte. O caminho feliz (sem queda nenhuma) continua provado
      pela app a sério, sem interceção nenhuma: depósito, temporizador de
      4 s a apanhá-lo, descarga e `DELETE` automático, como antes desta
      fase. **Não testado ao vivo**: o `Set` `emCurso` (revisto por leitura,
      não por uma corrida provocada a sério).
- [x] **Q3 — Limitador de débito (Review0 F-02 / Review2 F-09).**
      `tower_governor` sobre um sub-`Router` só com as rotas públicas sem
      assinatura nenhuma (`/saude`, `/vapid`, `GET /d/{id}`) — as outras já
      custam uma identidade Ed25519 gerada, não um pedido HTTP, por isso
      ficam de fora. Chave por IP do TCP (`PeerIpKeyExtractor`), não por
      `X-Forwarded-For`: não há proxy nenhum entre o cliente e o relay,
      nem antes nem depois do Q4. `ponytail:` 5 de rajada, 1/s a reencher —
      números de partida, por afinar quando o Q4 puser o relay mesmo
      alcançável de fora da tailnet.

      **Armadilha encontrada a testar, não hipotética:** `PeerIpKeyExtractor`
      precisa do IP real do TCP, que só chega ao `axum` com
      `into_make_service_with_connect_info::<SocketAddr>()` em vez do
      `Router` nu. Mudei o `axum::serve` de produção (`servir()`) mas
      esqueci o dos testes (`servir_em_porta_livre_com_pagina_e_app`) — sem
      o `ConnectInfo`, o extrator falha a **todos** os pedidos, não só aos
      que excedem o limite: 5 dos 51 testes do `relay` começaram a falhar,
      todos os que tocam em `/saude`/`/vapid`/`/d/{id}`, um só pedido cada,
      bem dentro da rajada de 5 — o sintoma certo apontava para a causa
      errada (parecia "o limite é baixo demais", era "o extrator nunca
      encontra o IP"). Corrigido nos dois sítios; os 51 testes voltaram a
      passar, mais dois novos (`rajada_no_saude_acaba_por_levar_429`,
      `a_mesma_rajada_nao_atinge_uma_rota_autenticada`, este último a provar
      que martelar `/saude` não arrasta `/caixa` com ele). `cargo clippy -p
      pombo-correio-relay --all-targets`: zero avisos.
- [x] **Q4 — DDNS pública (DuckDNS).** Expor o relay a quem não tem
      Tailscale instalado. Bloqueado no token do André (ver "Antes do S9").
      Entra depois ou junto do Q3 — nunca antes, para não abrir a porta sem
      o limitador de débito de pé.

      **Estado a 02/09/2026:** o encaminhamento de porta (TCP 53319, router
      NOS → Pi) já está criado e ativo — o Q4 chegou antes do DuckDNS
      propriamente dito, porque o `--autorizados` (que já era pré-requisito
      escrito desde a análise "Antes do S8") passou de teórico a urgente
      assim que a porta abriu. Feito antes de continuar: `--autorizados` a
      sério no Pi (`~/.local/share/pombo-relay/autorizados`), com as
      impressões atuais de A10 (CLI e app) e do iPhone; testado a sério — um
      depósito real do A10 passou.

      **Bug real encontrado a autorizar a tablet Android, não hipotético:**
      a app não tinha onde mostrar a própria impressão digital — nem numa
      build nova nem numa antiga. `impressaoDigital()` (`api.ts`) sempre
      devolveu `null` no Tauri de propósito, com um comentário a dizer
      "desktop/Android ainda não precisam de a mostrar" — verdade em S8,
      quando só a PWA precisava de se autorizar sozinha; deixou de o ser
      assim que o `--autorizados` se tornou obrigatório para toda a gente
      (S9 Q4). Corrigido com um comando Tauri novo,
      `impressao_digital` (`src-tauri/src/lib.rs`, reaproveita
      `descoberta::impressao_curta` que já existia para o `dispositivos`
      se filtrar a si próprio), ligado em `api.ts`. O cartão "Impressão
      digital" em Definições passou a aparecer em todas as plataformas — só
      teve de se separar do cartão "Notificações" (P4), que continua a só
      fazer sentido na PWA (`{#if impressao && !eTauri()}`), senão o
      Android ganhava um botão de push que não faz nada. Confirmado real:
      apk assinado (`zipalign`+`apksigner`, chave de debug) instalado a
      sério na OnePlus Pad 2 (`adb`), a impressão digital apareceu no ecrã,
      o André leu-a e acrescentou-se ao `--autorizados`. **Todos os quatro
      dispositivos de casa autorizados** (A10 CLI, A10 app, iPhone,
      OnePlus Pad 2).

      **Fechado a 02/09/2026:** domínio `pombo-correio.duckdns.org` (token do
      André), IP mantido atual por um `duckdns-update.timer` no Pi (5 em 5
      min, `curl` ao endpoint de update do DuckDNS). TLS a sério emitido por
      `acme.sh --issue --dns dns_duckdns` (desafio DNS-01, sem abrir mais
      nenhuma porta para isso) e instalado num `nginx` novo no Pi, que
      termina TLS e reencaminha para o relay em `127.0.0.1:53319` — a mesma
      ideia do "TLS pelo Tailscale" do R7 (nunca código de TLS nosso), só que
      aqui é a ferramenta equivalente para um domínio público em vez da
      tailnet. Ficou a faltar encaminhar a porta 443 no router para o Pi
      (igual ao que já se tinha feito para a 53319); o André criou as duas
      regras. `nginx` teve de ficar a ouvir em `192.168.1.232:443` e não em
      `0.0.0.0:443` — o `tailscaled` já tinha a wildcard-equivalente ocupada
      na sua própria interface, e um bind específico conflita com um bind
      universal na mesma porta.

      **Verificado de fora a sério, não só em loopback** (regra 3): local no
      Pi deu 200 contra o hostname real; de fora, `check-host.net` (TCP
      connect a `pombo-correio.duckdns.org:443` e depois um pedido HTTPS
      completo a `/saude`) confirmou de nós na Áustria, Portugal e Ucrânia —
      ligação aceite e `200 OK` com o certificado a validar contra o IP
      público real (`89.153.98.18`). Os nós no Irão falharam a ligar
      (bloqueio de rede do próprio país, não do nosso lado). **Q4 fechado.**
      O que resta do S9 é só o Q5, bloqueado no Firebase do André.
- [ ] **Q5 — Push FCM (Android), o resto do R6.** Estende `relay/src/push.rs`
      para enviar também para o Firebase Cloud Messaging, ao lado do Web
      Push já feito para iOS. Bloqueado na conta Firebase e credenciais de
      serviço do André (ver "Antes do S9"); a forma do código deve poder
      partilhar o `Subscricoes`/o disparo em `subir()` já existentes,
      só a acrescentar um segundo transporte.

### Antes do S9

| Item | Bloqueia o S9? | De quem é a próxima ação | Ação concreta |
|---|---|---|---|
| Binários musl | Não | Eu | `rustup target add` + build, sem depender de ninguém |
| Retoma de descarga na PWA | Não | Eu | Regra 8 primeiro, depois código |
| Limitador de débito | Não | Eu | Testável em loopback já |
| DDNS (DuckDNS) | Feito | — | — |
| Push — Firebase (Android) | Sim, para o Q5 | André | Criar o projeto Firebase, gerar a conta de serviço, dar-mos |
| Confirmação do P4 no iPhone (dívida do S8, não do S9) | Não | André | Definições → Ativar notificações, esperar por um depósito |

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
