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

---

## Sprints

- [x] **S1 — Primitivas do núcleo**
- [x] **S2 — Cifra e identidade em disco**
- [x] **S3 — Transferência sobre HTTP**
- [x] **S4 — Descoberta e dispositivos conhecidos**
- [x] **S5 — Interface e desktop** (em Windows a app corre e recebe; falta olhar para o ecrã)
- [x] **S6 — Android**
- [ ] **S7 — Relay e link sem instalação**
- [ ] **S8 — PWA iOS**

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
- [ ] **R6 — push.** FCM no Android, Web Push no iOS. Precisa de contas e chaves
      de serviço — decisão do utilizador, não técnica. Ver "Antes do S8".
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

- [ ] **P1 — Fundação instalável.** `manifest.json`, ícones (incluindo
      `apple-touch-icon`), meta tags do Safari (`apple-mobile-web-app-*`), um
      service worker mínimo. Servido por HTTPS — já resolvido pelo R7. Prova:
      instalar no ecrã principal do iPhone, abrir como app isolada.
- [ ] **P2 — A `ui/` corre sem Tauri.** `api.ts` ganha duas implementações
      por trás da mesma assinatura — deteta `window.__TAURI_INTERNALS__` e
      escolhe. O lado browser precisa de bindings wasm novos (a estender o
      `core/`, no padrão do `pagina/`): gerar e persistir uma `Identity`
      (bytes para `localStorage`; `secret_bytes()` já existe no `core/`).
      Sem isto, nada do resto tem quem o assine.
- [ ] **P3 — Receber pelo relay ao abrir a app.** Equivalente ao
      `buscarNoRelay()` do Android, em wasm+`fetch()`: consulta `/caixa`,
      decifra e verifica com o `core/`, oferece a gravação (o caminho do R5,
      que já cai para memória sem `showSaveFilePicker`). O caso central do
      pedido original — a mãe abre a app e o que estava à espera aparece.
- [ ] **P4 — Push.** Service worker subscreve Web Push (VAPID), o relay
      guarda a subscrição e avisa quando há depósito novo. Sem isto, P3 só
      corre quando alguém lembra de abrir a app.
- [ ] **P5 — Enviar do iPhone.** Seletor de ficheiros (`<input type=file>`,
      sem `accept="image/heic"`, a incentivar a app Ficheiros em vez da
      Biblioteca de Fotos). Sela com o `core/` em wasm, sobe por `fetch()` —
      direto se houver destino alcançável, ao relay senão. Sem mDNS possível
      no Safari: o destino escolhe-se da lista de conhecidos ou por
      endereço, nunca por descoberta automática.
- [ ] **P6 — O design a sério, responsivo.** Aplicar
      `design/claude-design/` ao `ui/App.svelte`/`app.css` — a mesma
      extração que o S5 já fez uma vez ("o desenho vinha empacotado"), agora
      com breakpoints de 720p a 4K, testado em telemóvel, tablet, laptop e
      desktop. Por ser o mesmo `ui/` em todo o lado, o desktop e o Android
      herdam o resultado de graça.

Cada fase segue a regra 8 (desenhar antes de implementar) quando arrancar a
sério — este roteiro é a ordem de trabalho, não o desenho protocolar de cada
peça.

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
