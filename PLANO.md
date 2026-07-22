# Pombo-Correio

Partilha de ficheiros entre os dispositivos de casa, sem perder qualidade e sem
depender de nenhum serviço de terceiros.

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

1. Trabalho dividido em **sprints**. Cada sprint acaba em algo que corre e se
   testa — não em código a meio.
2. **Commits por unidade coerente**, não só no fim do sprint. Um sprint são
   dias; um único commit gigante torna o `git bisect` inútil.
3. Fim de sprint: testes verdes, este documento atualizado, e uma **tag**
   `sprint-N` a marcar a fronteira.
4. Este ficheiro é a **única** fonte de verdade do plano. Atualizado à medida
   que se executa: o que ficou feito, o que se descobriu, o que se decidiu
   mudar. O plano original em `~/.claude/plans/` está morto.
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
  para quem recebe. Entra em S7.

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
- Só o registo de dispositivos é replicado — é pequeno e muda pouco.

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

## Sprints

- [x] **S1 — Primitivas do núcleo**
- [x] **S2 — Cifra e identidade em disco**
- [x] **S3 — Transferência sobre HTTP**
- [ ] **S4 — Descoberta e emparelhamento**
- [ ] **S5 — Interface e desktop**
- [ ] **S6 — Android**
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

**Dificuldades e detalhes**

- **`read()` curto não é fim de ficheiro.** Um `Read` pode devolver menos bytes
  do que os pedidos sem ter acabado — acontece sempre com sockets. A versão
  ingénua cortaria blocos irregulares e o manifesto não reproduziria entre o
  remetente e o destinatário. O leitor enche o buffer até ao limite antes de
  fechar um bloco. Fixado pelo teste `leitura_fragmentada_nao_altera_o_manifesto`,
  com um leitor artificialmente lento.
- **O índice do bloco entra no hash.** Sem isso, blocos de conteúdo igual são
  intermutáveis e o ficheiro pode ser reordenado mantendo todos os hashes
  válidos. Teste: `bloco_certo_no_indice_errado_e_recusado`.
- **Em níveis ímpares o último nó sobe inalterado, não duplicado.** Duplicar é o
  que o Bitcoin faz e arrasta a ambiguidade CVE-2012-2459, em que duas listas
  diferentes produzem a mesma raiz. Teste:
  `merkle_distingue_listas_de_comprimento_impar`.
- **Folhas e nós internos usam chaves de derivação distintas**
  (`pombo-chunk-v1` vs `pombo-node-v1`), contra segunda pré-imagem.
- **A impressão digital cobre a chave de cifra.** Se cobrisse só a de
  assinatura, seria possível trocar a chave de cifra sem a impressão digital
  mudar — e a verificação em voz alta deixaria de proteger contra substituição.
  Teste: `fingerprint_cobre_a_chave_de_cifra`.
- **Contexto:** `rustup` estava instalado sem toolchain default. Resolvido com
  `rustup default stable` (1.97.1).
- **Contexto:** o `git init` inicial foi feito em `Desktop/Projects/`, que é a
  pasta-mãe de vários projetos. Desfeito e refeito dentro de `pombo-correio/`.
  Nada se perdeu — não havia commits.

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

**Dificuldades e detalhes**

- **Par efémero em vez de DH estático-estático.** Quem envia gera um par novo
  para cada envelope e destrói-o a seguir. Isto dá sigilo futuro: quem venha a
  roubar a chave estática do destinatário mais tarde não reabre o que já passou
  pelo relay. Custou dez linhas e é o desenho do `age`.
- **Nonce aleatório por bloco, e não um contador.** É a decisão que mais
  interage com a retoma. Um contador obrigaria a persistir o seu estado entre
  sessões, e retomar uma transferência ao fim de dias com o contador errado
  repetiria um nonce — falha catastrófica num AEAD. Os 192 bits do nonce do
  XChaCha20 tornam a escolha aleatória segura e eliminam o estado por completo.
  É esta a razão de ser XChaCha20 e não ChaCha20 simples.
- **O índice do bloco entra como dados associados**, autenticado sem ser
  cifrado. É o que impede trocar de posição blocos já selados, sem os conseguir
  abrir. Teste: `indice_errado_nao_abre`.
- **As permissões são postas na criação, não depois.** `OpenOptions::mode(0o600)`
  em vez de gravar e fazer `chmod` a seguir — a segunda ordem deixaria uma
  janela em que o material secreto esteve legível a toda a gente. Teste:
  `ficheiro_de_identidade_fica_0600`.
- **Formato da identidade em disco:** 32 bytes de assinatura, 32 de cifra, e o
  nome a seguir em bruto. Sem serializador nem cabeçalho — dois campos fixos e
  um resto. Há teste com acentos, porque o nome real é "Telemóvel do avô".
- **API da `chacha20poly1305` 0.11 mudou.** `generate_nonce` e
  `XNonce::from_slice` deixaram de existir, e a `rand` 0.10 tirou o `OsRng` de
  `rand::rngs`. Resolvido chamando `getrandom::fill` diretamente: é o CSPRNG do
  sistema sem camadas pelo meio, e deixa de haver acoplamento entre as versões
  da `rand` e da `aead`.

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

**Matriz validada em máquinas reais**

| | Linux x86-64 | Linux ARM64 (Pi 5) | Windows |
|---|---|---|---|
| envia | ✅ | ✅ | ✅ |
| recebe | ✅ | ✅ | ✅ |
| retoma | ✅ | ✅ | ✅ |

Mais: LAN, Tailscale, dois envios em simultâneo, matar cada um dos lados a meio,
e nomes com espaços e com caracteres que o Windows recusa.

**Medido**
- Localhost: 400 MB a ~127 MB/s.
- LAN, PC → Pi 5: `1GB.txt` cortado e retomado no bloco 321 de 1024.
- LAN, Windows → PC: 164 MB a 112,9 MB/s.
- Matar o recetor a meio: 160 de 1145 blocos preservados, retomado, idêntico.
- Os 48 testes passam em ARM64 e em Windows sem alterações. A precaução de usar
  `to_le_bytes` explicitamente em toda a parte pagou-se: nada a corrigir entre
  arquiteturas.

**Duas coisas que só apareceram fora do Linux**

- **O Windows nem arrancava.** `pasta_dados` procurava `HOME` ou
  `XDG_DATA_HOME`, que o Windows não define. Passou a `%APPDATA%`, e o nome do
  dispositivo a `%COMPUTERNAME%` em vez de `/etc/hostname` — senão todas as
  máquinas Windows apareceriam como "dispositivo" na lista, que é o que torna a
  lista inútil.
- **Nomes com `:` não gravam em Windows.** Carimbos temporais como
  `2026-06-22 23:04:58.mkv` saem assim de gravadores de ecrã em Linux e Android.
  Sem tratamento, a transferência corria até ao fim, verificava todos os blocos,
  e falhava só no `rename` final. Também se tratam pontos e espaços finais, que
  o Windows remove em silêncio, e os nomes de dispositivo do MS-DOS. Aplicado só
  em Windows — em Unix estes caracteres são legítimos — mas compilado em toda a
  parte, para a lógica ter testes onde é mais cómodo corrê-los.

**Em aberto:** em Windows a identidade fica com as permissões por omissão da
pasta, porque o `0600` é `#[cfg(unix)]`. O `%APPDATA%` já é por utilizador, mas
não é a mesma garantia.

**Progresso: onde é contado, e porquê importa**

No emissor o `passo` vem **depois** do `yield`. O fluxo é preguiçoso, por isso
só se retoma a execução quando o bloco anterior saiu mesmo pela ligação. Contar
antes do `yield` mostraria o ritmo a que se lê do disco — números bonitos e
falsos, que esconderiam exatamente o problema que o progresso serve para
apanhar.

Fora de um terminal não se desenha nada. O `\r` só faz sentido num ecrã; num
ficheiro de registo encheria tudo de lixo, e o teste de integração lê esta saída
linha a linha.

**Desvios ao plano, e porquê**

- **Empurrar em vez de puxar com `Range`.** O plano previa quem recebe a puxar
  com pedidos `Range`. Ficou ao contrário: só quem recebe tem servidor, e quem
  envia empurra num `PUT` único com o corpo em fluxo. A retoma faz-se com um
  parâmetro `?de=N` em vez de um cabeçalho `Range`.
  A razão é que empurrar é necessário de qualquer maneira — no caminho do relay
  quem envia também empurra — enquanto puxar só é preciso do relay para quem
  recebe. Fazer primeiro a metade que serve os dois casos evita escrever dois
  servidores. O `GET` com `Range` entra em S7, para a descida do relay.
  O princípio que interessava mantém-se: **um pedido só, com o corpo em fluxo**,
  em vez de um pedido por bloco.
- **Sem TLS nesta fase.** O conteúdo já vai selado ponta-a-ponta desde S2, o
  Tailscale já cifra por WireGuard, e a LAN é a rede de casa. TLS aqui só
  acrescentaria gestão de certificados sem tapar nenhum buraco real. Entra em
  S7, onde o relay fica exposto à internet e existe um certificado a sério.
- **O manifesto passou a ir selado.** Consequência direta de dispensar o TLS:
  sem isso, o nome do ficheiro viajaria em claro mesmo com os blocos cifrados —
  e o nome costuma dizer mais sobre o conteúdo do que os próprios bytes. O
  envelope continua em claro, porque é a chave embrulhada e só o destinatário a
  abre.

**Dificuldades e detalhes**

- **O nome do ficheiro é reduzido ao último componente.** Vem de outra máquina,
  e um `..` ou uma barra à frente escreveriam fora da pasta de destino.
- **A assinatura é verificada antes de abrir seja o que for.** A oferta traz
  uma assinatura Ed25519 sobre o manifesto selado; sem a validar primeiro, o
  servidor estaria a decifrar material de qualquer um.
- **O fluxo não chega alinhado com os blocos.** Os pedaços que saem do corpo
  HTTP têm tamanhos arbitrários e não coincidem com as fronteiras dos blocos
  selados. O recetor acumula até ter um bloco inteiro antes de tentar abri-lo —
  o mesmo problema do `read` curto, noutra camada.
- **A retoma é keyed pela raiz de Merkle**, não pelo nome. Duas tentativas do
  mesmo ficheiro reencontram o trabalho a meio mesmo que o nome mude, e
  ficheiros diferentes com o mesmo nome não se misturam.
- **Blocos completos contam-se pelo tamanho do ficheiro parcial**
  (`tamanho / chunk_size`), sem ficheiro de estado à parte. Só é correto porque
  os blocos se escrevem em ordem e todos têm tamanho fixo menos o último.
- **`plain_len` no múltiplo exato.** Com um ficheiro de tamanho múltiplo exato
  do bloco, `size % chunk_size` dá zero mas o último bloco está cheio, não
  vazio. Teste: `multiplo_exato_do_bloco_nao_da_ultimo_vazio`.
- **Teste a fechar o tubo do stdout.** A primeira versão do teste lia a saída do
  recetor até encontrar a porta e largava o leitor. O `println!` seguinte do
  recetor apanhava EPIPE e o processo morria a meio da transferência. O teste
  passou a drenar a saída numa thread.
- **`try_stream!` não aceita anotação de tipos** e o `wrap_stream` da `reqwest`
  é genérico demais para os inferir. Resolvido com uma função identidade tipada.

### S4 — Descoberta e emparelhamento

- [ ] mDNS (`mdns-sd`): anunciar e descobrir na LAN.
- [ ] Emparelhamento por QR e por número de segurança.
- [ ] Registo de dispositivos de confiança, persistido.

### S5 — Interface e desktop

- [ ] `ui/` em Svelte. Um botão de envio, lista de dispositivos, mais nada.
- [ ] `app/` em Tauri v2. Alvos Linux e Windows.
- [ ] **Confirmar que os bytes não passam pelo IPC** — só eventos de progresso.

### S6 — Android

- [ ] `tauri-plugin-android-fs` e `tauri-plugin-background-service`.
- [ ] Partilha pelo share sheet do sistema.
- [ ] Transferência com o ecrã desligado.

### S7 — Relay e link sem instalação

- [ ] `relay/` em axum: depósito cifrado com TTL, registo de dispositivos.
- [ ] Ativo-ativo no A10 e no Pi 5; cliente corre ambos em paralelo.
- [ ] Sincronização do registo por vetor de versões.
- [ ] Página de descarregamento com a chave no fragmento do URL.
- [ ] Notificações push.
- [ ] Binários musl para amd64 e arm64.

### S8 — PWA iOS

- [ ] `core/` para WASM.
- [ ] Service worker, Web Push, instalação no ecrã principal.
- [ ] Encaminhamento explícito para a app Ficheiros (contorna o HEIC→JPEG).

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
