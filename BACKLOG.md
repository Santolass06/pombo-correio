# Pombo-Correio — Backlog

Lições, dificuldades, desenhos escritos antes de codificar, e como cada peça
foi validada. **O plano em si — o que fazer, e o que já está feito — vive em
`PLANO.md`.** Este ficheiro é só história: útil para perceber *porquê* uma
decisão ficou como ficou, não para saber o que fazer a seguir.

Separado por sprint, na mesma ordem do `PLANO.md`.

---

## Geral (antes dos sprints numerados)

### Chaves de quem nunca nos enviou — desenho

**O sintoma, em linguagem de casa:** a mãe instala a app, tu mandas-lhe uma foto,
o telemóvel dela está desligado. Não vai. Só depois de **ela** te ter mandado
alguma coisa é que passas a conseguir deixar-lhe ficheiros no relay.

**A causa, e ela é pior do que parecia.** A primeira leitura foi *"quem envia
nunca guarda as chaves"*. Errado: o CLI e a app **guardavam**, num `Evento::Destino`
que chamava `Conhecidos::registar` — mas no **`conhecidos.json`**, que é o ficheiro
que decide mostrar o número de segurança, e **mal o endereço respondia**, antes de
o ficheiro seguir sequer.

Ou seja, a correção "óbvia" que este documento ia recomendar **já estava no
código**, com o buraco que se lhe apontava. Bastava alcançar um endereço que se
dissesse chamar *"Telemóvel da mãe"* — e o mDNS não autentica ninguém — para esse
aparelho ficar por conhecido e o aviso de dispositivo novo nunca aparecer quando
ele nos enviasse alguma coisa.

Só não se notava no tablet porque o A10 nunca lhe tinha enviado nada pelo CLI.
**O buraco de usabilidade tapava um buraco de segurança.**

**Porque não se corrige onde parece.** A tentação é registar no envio. Mas o
`Conhecidos` responde hoje a uma pergunta só — *"já recebi deste antes?"* — e é
essa resposta que decide mostrar o número de segurança. Registar por ter enviado
faria um aparelho escolhido por um **nome anunciado no mDNS**, que não autentica
ninguém, calar o aviso de dispositivo novo numa receção futura. Trocava um
incómodo por um buraco de segurança.

**A separação, então:** duas perguntas, dois ficheiros.

| ficheiro | pergunta | quem escreve | quem lê |
|---|---|---|---|
| `conhecidos.json` | *já recebi deste?* | só quem **recebe** | o aviso de dispositivo novo |
| `vistos.json` | *sei as chaves dele?* | quem **envia**, e só depois de o envio direto ter **acabado bem** | o envelope do relay |

O caminho do aviso não muda **nada** — continua a ler só o `conhecidos.json`. É a
propriedade que interessa não partir, e o teste é esse.

**Porquê só depois de um envio direto que acabou bem**, e não a partir da simples
descoberta: um envio concluído significa que já lhe entregaste um ficheiro em
claro àquelas chaves. Selar-lhe outro mais tarde não revela nada que ainda não
tenhas dado. Guardar chaves de quem apenas se anunciou na rede seria deixar
qualquer aparelho da LAN semear-se para depósitos futuros.

**Pré-mortem — *se a ligação morrer a meio do envio, o que fica escrito?*** Nada:
só se grava no fim, com o ficheiro entregue. Um envio a meio não ensina chave
nenhuma, e a próxima tentativa volta a passar pelo mesmo sítio.

O que isto **não** resolve, e fica dito: continua a não haver forma de mandar
para alguém com quem nunca houve contacto direto nenhum. Isso é emparelhamento
por QR ou código, e é outra peça.

##### Feito, e onde foi provado

`vistos.json` novo, escrito por `guardar_chaves` no fim de um envio bem sucedido;
`conhecidos::chaves_para` lê os dois ficheiros; e **os dois `registar` do
`Evento::Destino` foram apagados** — do CLI e da app.

Dois testes em `rede/tests/chaves_de_quem_recebe.rs`. O segundo é o que interessa,
e **foi visto a falhar**: apontei o `guardar_chaves` ao `conhecidos.json` de
propósito e ele apanhou-o na hora (*"enviar escreveu no registo que decide o aviso
de dispositivo novo"*). Um teste que nunca falhou não prova nada.

Entre o A10 e o tablet, com binários de release:

| passo | resultado |
|---|---|
| Depositar pela impressão, sem contacto anterior | recusa, com a frase nova |
| Envio direto pela LAN | escreve **só** o `vistos.json` |
| `conhecidos.json` depois do envio | intocado — a porta do aviso não se abriu |
| Depositar pela impressão, agora | passa |

**Uma lição de método, que é o que fica.** O teste em `rede/` passava e o buraco
continuava lá, porque quem o abria era o **chamador** — o ouvinte do CLI e o da
app — e não a camada testada. Foi preciso olhar para os ficheiros em disco de
máquinas a sério para dar por isso. *Testar a camada onde a lógica vive não chega
quando o comportamento errado está na de cima.*

#### O "buraco" da secção do Android, revisitado — já estava fechado

A secção do S7/Android descrevia isto como um problema em
aberto: *só se consegue depositar para quem já nos enviou algo*, e "quem
envia nunca guarda as chaves de quem recebeu" (ver S7, R5/Android). Fui desenhar a correção —
aprender a chave também ao **receber**, não só ao enviar — e antes de a
escrever a sério, o passo de sempre: um teste que prove a falha primeiro
(`receber_ja_ensina_a_chave_de_quem_nunca_recebeu_de_nos`, em
`rede/tests/chaves_de_quem_recebe.rs`).

**Não falhou.** `chaves_para` já consulta o `conhecidos.json` antes do
`vistos.json`, e o `conhecidos.json` já é escrito, incondicional, em
`propor()`, para **qualquer** oferta com assinatura válida — desde a correção
original acima. Ou seja: assim que A
recebe um ficheiro de B, mesmo que A nunca lhe tenha enviado nada, A já fica
com a chave de B em `conhecidos.json` — e é aí que `chaves_para` a encontra.
**O caso "só recebi, nunca enviei" já funcionava.**

Cheguei a implementar a escrita extra em `vistos.json` no `propor()` (com o
desenho todo, pré-mortem incluído) e o teste passou **igual** com essa linha
comentada — a prova de que era código morto. Revertido antes de commitar.

**O que realmente falta**, e continua a ser o que a secção original já dizia:
depositar para quem nunca houve contacto nenhum, em nenhum dos dois sentidos.
Isso é emparelhamento por QR ou código — está fora de alcance de qualquer
correção nos registos, porque não há chave nenhuma para aprender.

**Lição:** o texto da secção do Android ficou escrito no calor do teste
manual (o "programa descartável" que fabricava o registo), sem confirmar se o
`conhecidos.json` já resolvia o caso por si — o programa a fabricar o
registo escrevia-o diretamente, sem passar pelo `chaves_para`, por isso não
provava que o caminho normal falhava. O passo "escrever um teste que falhe
primeiro" já teria apanhado isto na altura.

### Review0 — auditoria externa

`audit/Review0.md`, 01/09/2026, de um agente à parte que passa a ser
responsável por testar, validar e sugerir correções ao código. Maioritariamente
positiva, sem falhas críticas nem bugs bloqueantes. Cinco achados (F-01 a
F-05); confirmei cada um contra o código antes de agir — a experiência do
buraco dos `Conhecidos` acima já tinha mostrado que um relatório escrito sem
correr o teste pode descrever um buraco que não existe.

| Achado | Confirmado? | O que se fez |
|---|---|---|
| F-01 — zeroização de memória em falta | Parcial: `SigningKey`/`StaticSecret`/`EphemeralSecret` já zeroizam sozinhas (`ZeroizeOnDrop`, confirmado na fonte do `x25519-dalek`/`ed25519-dalek` 3.0.0). Só `TransferKey` (`core/src/crypto.rs`) era um `[u8; 32]` nosso, sem `Drop` nenhum | ✅ `TransferKey` passou a `Zeroize`/`ZeroizeOnDrop`. `zeroize` já era dependência transitiva (via `x25519-dalek`), passou a direta. Teste `transfer_key_zeroiza_ao_sair_de_ambito` — asserção em tempo de compilação, não leitura de memória pós-`drop` (seria UB) |
| F-02 — sem limite de débito no `PUT /deposito` do relay | Confirmado: só há validação de tamanho declarado e o portão `--autorizados` na criação; nada limita repetição de pedidos | Agendado, não implementado — só importa quando o relay for exposto para lá da tailnet. Ver "Antes do S8" no `PLANO.md` |
| F-03 — sem TLS, sem `isSecureContext` | Já resolvido no Pi 5 (ver S7 abaixo); a auditoria só confirma o que já se sabia | Sem ação nova |
| F-04 — throttling do progresso a 100 ms | A própria auditoria diz "manter como padrão" | Sem ação |
| F-05 — interface Svelte | "N/A" na própria auditoria | Sem ação |

### Diagnóstico

Registo de métricas em `<dados>/metricas/eventos.jsonl` (`rede/src/metricas.rs`),
uma linha JSON por acontecimento. Existe porque validar entre dispositivos é
difícil: quem desenvolve não vê o Pi nem a rede de casa. A peça central é o
**tempo por fase** da receção — `rede_ms`, `decifra_ms`, `verifica_ms`,
`disco_ms` — que aponta o gargalo sem adivinhação. Ver `COMO_CORRER.md`.

Foi criado precisamente para o problema descrito a seguir, em "Resolvido".

### Resolvido

- **Velocidade a 2 MB/s entre o Pi e a máquina.** Era o tráfego a passar pelo
  relay do Tailscale em vez da rede local. Cada dispositivo anuncia por mDNS o IP
  da LAN (`192.168.x`) **e** o do Tailscale (`100.x`), e `escolher_ip` ordenava
  por texto — `"100..."` vem antes de `"192..."`, por isso escolhia sempre o
  Tailscale. Estando ambos na mesma casa, o tráfego saía e voltava pelo DERP do
  Tailscale (~2 MB/s) em vez dos ~100 MB/s da LAN. A pista decisiva foi ser
  **igual nos dois sentidos**: disco ou CPU seriam assimétricos, a rede não.
  Corrigido em `preferencia`: a gama CGNAT do Tailscale fica abaixo de qualquer
  outro IPv4, mas continua a ser usada quando é o único caminho (fora de casa).

### Notas de comportamento

- **Cancelar reflete-se nos dois lados, por um canal à parte.** O aviso de
  cancelamento não pode viajar pela ligação da transferência — é essa que está a
  ser cortada, e o aviso morreria com ela (foi o bug relatado). Cada dispositivo
  corre um servidor, por isso quem cancela abre uma ligação **nova** ao outro
  (`POST /cancelar/{id}`) para lho dizer. Quem envia anuncia a sua porta de
  servidor na oferta (`porta_resposta`), para quem recebe o saber avisar; o CLI a
  enviar não tem servidor e manda porta 0 (não é avisado — mas o Ctrl+C basta).
  O 409 fica como reforço para o caso de a resposta ainda chegar. Provado por
  `cancelar_reflete_nos_dois_lados_pelo_canal`, com dois servidores reais.

---

## S1 — Primitivas do núcleo

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

## S2 — Cifra e identidade em disco

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

## S3 — Transferência sobre HTTP

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
  servidores. O `GET` com `Range` entrou em S7, para a descida do relay.
  O princípio que interessava mantém-se: **um pedido só, com o corpo em fluxo**,
  em vez de um pedido por bloco.
- **Sem TLS nesta fase.** O conteúdo já vai selado ponta-a-ponta desde S2, o
  Tailscale já cifra por WireGuard, e a LAN é a rede de casa. TLS aqui só
  acrescentaria gestão de certificados sem tapar nenhum buraco real. Entrou em
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

## S4 — Descoberta e dispositivos conhecidos

**Decisões**

- **O endereço literal continua a funcionar e nem toca no mDNS.** É o caminho do
  Tailscale e de tudo o que esteja fora da rede local — o mDNS é multicast e não
  atravessa routers. Só se procura na rede quando o que o utilizador escreveu
  não se parece com um endereço, o que também evita esperar três segundos à toa.
- **O número de segurança só aparece na primeira vez.** Mostrá-lo a cada
  ficheiro de um dispositivo já conhecido ensinaria a ignorá-lo, que é a pior
  coisa que se pode fazer a uma verificação. Se o nome mudar, diz-se qual era
  antes; se a identidade for outra, volta a ser um dispositivo novo.
- **O registo é indexado pela impressão digital, nunca pelo nome.** O nome é
  escolhido pelo dono e muda quando lhe apetecer; a impressão digital deriva das
  chaves e só muda se o aparelho for realmente outro — que é o caso a detetar.
  Teste: `a_identidade_manda_e_nao_o_nome`.
- **Falhar a anunciar não impede receber.** Numa rede que bloqueie multicast,
  escrever o endereço continua a funcionar; abortar seria trocar uma comodidade
  perdida por uma falha total.
- **A impressão digital anunciada no TXT não serve para autenticar** — qualquer
  um pode anunciar o que quiser. Serve para distinguir dois dispositivos com o
  mesmo nome. Quem autentica é o envelope, que só o dono da chave abre.

**Dificuldades e detalhes**

- **O conjunto de endereços não tem ordem.** Uma máquina com Wi-Fi, cabo e
  Tailscale anuncia vários, e a biblioteca entrega-os num `HashSet`. Pegar no
  primeiro dava endereços diferentes a cada execução — na primeira corrida saiu
  um IPv6 global, na seguinte podia sair outro. Agora escolhe-se IPv4 primeiro,
  com desempate textual: numa rede doméstica o IPv4 funciona sempre, enquanto o
  IPv6 depende do router e da firewall. O `loopback` é descartado, porque um
  dispositivo que anuncie `127.0.0.1` está a falar de si próprio.
  Teste: `a_escolha_nao_depende_da_ordem_de_entrada`.
- **O nome de anfitrião mDNS tem de ser único na rede**, e o nome legível não
  serve porque pode repetir-se. Usa-se `pombo-<16 dígitos da impressão>.local.`,
  que além de único não tem caracteres a escapar.
- **O anúncio é retirado ao sair** (`Drop`). Sem isso, um dispositivo desligado
  continuaria a aparecer na lista dos outros durante minutos, e quem lhe
  enviasse falhava sem perceber porquê.
- **Um ficheiro de conhecidos corrompido não impede receber.** Perde-se a
  memória de quem já cá passou, o que é um incómodo e não uma falha.

## S5 — Interface e desktop

### Windows, finalmente (Andre_Desktop, Ryzen 5 8400F, Windows 11 build 26200)

Deixou de estar bloqueado no momento em que passou a haver SSH para lá
(`ssh andre@100.88.104.68`). O repositório é privado e o `git` remoto não tem
terminal para credenciais, por isso o código foi por `git archive` + `scp` — 1 MB.

| | |
|---|---|
| `cargo build --release -p pombo-correio` | ✅ 1m00s, sem um único aviso |
| `cargo test --workspace` | ✅ **122 passaram, 0 falharam** (123 em Linux; a diferença é o teste das permissões `0600`, que é Unix) |
| A10 → Windows, 5 MB | ✅ `sha256` idêntico |
| Windows → A10, 4 MB | ✅ `sha256` idêntico, com o aviso de dispositivo novo a aparecer |
| Windows → tablet Android, 3 MB | ✅ descoberto **pelo nome**, aceite. *Hash não confirmado* — o ADB já estava desligado |
| `pombo dispositivos` em Windows | ✅ encontrou os três: tablet, A10 e Pi |
| Caminhos de dados (`%APPDATA%\pombo`) | ✅ o ramo Windows do `pasta_dados` está certo |

**O que Windows destapou, e não se via em mais lado nenhum:**

- **A rede local está classificada como *Pública*.** A Ethernet de casa aparece
  como `Public` e só o Tailscale como `Private`. Não impediu nada nos testes, mas
  é a classificação que o Windows usa para decidir o que deixa entrar.
- ~~**A NordVPN bloqueia a entrada pela LAN.**~~ **Errado, e corrigido no mesmo
  dia.** O sintoma foi real: com o recetor a escutar em `0.0.0.0` e uma regra de
  firewall explícita para a porta 53318 em **todos** os perfis, a ligação a
  `192.168.1.64:53318` dava *timeout*, e pelo Tailscale entrava à primeira. A
  causa que lhe atribuí é que não se aguenta. Horas depois, com a **NordLynx
  ligada**, sem regra nenhuma de firewall para a app (confirmado: zero regras por
  nome e zero por caminho do programa) e sem nenhuma regra de bloqueio de entrada
  activa, o tablet enviou pela LAN e o A10 abriu `192.168.1.64:53318` à primeira.
  **Não se reproduz e não sei a causa.** O que mudou entretanto foi a app passar a
  correr instalada em vez do CLI lançado por SSH — pode ser aí, mas isso é
  hipótese, não conclusão. Fica escrito como está: aconteceu, e a explicação que
  dei estava errada.
- **O IPv6 anda a fazer trabalho não creditado.** O tablet descobriu o Windows
  primeiro por IPv6 global (`2a01:11:…`) e só depois por `192.168.1.64`. Da
  Ethernet cá de casa esse IPv6 não responde, o que quer dizer que a lista de
  dispositivos chega a mostrar endereços que não servem para todos os pares.
  Ninguém reparou porque a escada desce sozinha — mas custa uma tentativa
  falhada antes de descer.
- **Não foi preciso mexer na firewall.** Cheguei a criar uma regra para a porta;
  desactivei-a e o Tailscale continuou a entrar. Foi removida — a firewall ficou
  exactamente como estava.

### A janela em Windows (12/08/2026)

O `npm run app:build` correu na máquina do André. O que ficou provado, e como:

| | |
|---|---|
| O instalador saiu e foi usado | ✅ instalada em `%LOCALAPPDATA%\Pombo-Correio\pombo-correio-app.exe` |
| O processo arranca e serve | ✅ pid 22416 a escutar em `0.0.0.0:53318` |
| O lado Rust correu o `setup` | ✅ `%APPDATA%\pt.pombo.correio` criado com `identidade`, `recebidos`, `metricas` |
| **A WebView2 é filha da app** | ✅ `browser` 27204 com pai 22416 — e não uma qualquer das outras que já lá corriam |
| **Houve página para desenhar** | ✅ debaixo dela, `renderer`, `gpu-process`, `crashpad-handler` e duas `utility` |
| A WebView escreveu o seu perfil | ✅ `%LOCALAPPDATA%\pt.pombo.correio\EBWebView\`, com `GrShaderCache` e `GPUPersistentCache` |
| A10 → app do Windows, 40 MB, Tailscale | ✅ `eac20c8e…663d8a0d` nas duas pontas |
| Registou quem enviou | ✅ `conhecidos.json` com `andresantos-M11BB` |

A app tem identidade própria, separada da do CLI (`%APPDATA%\pombo` contra
`%APPDATA%\pt.pombo.correio`), e apresenta-se como `ANDRE_DESKTOP` — o
`COMPUTERNAME` tal e qual, em maiúsculas.

**A árvore de processos é a melhor prova que se consegue por SSH, e quase
chega.** Contar `msedgewebview2` não valeria nada — havia cinco de outras apps a
correr desde o dia anterior. O que vale é a **ascendência**: o processo `browser`
tem por pai a nossa app, e por baixo dele há um `renderer`, que só nasce quando
há documento para desenhar, e um `gpu-process` que deixou *shader cache* escrito
em disco. Isso é rasterização a sério, não um processo vazio à espera.

Continua a não ser "apareceu no ecrã" — uma janela escondida também rasteriza. O
SSH em Windows corre na sessão 0 e não vê o ambiente de trabalho, por isso
**captura de ecrã não é caminho**: sai preta. O que faltava era do André, e era pouco:
olhar para a janela, ver se a lista de dispositivos se enche, e **enviar a partir
dela** — que é a metade que nada disto tocava.

### Estrutura que a interface obrigou a criar

A lógica de transferência vivia em `cli/`, e a interface precisava exatamente
da mesma. Adiei a separação em S3 de propósito — nessa altura o CLI era o único
consumidor — e fiz-a em S5, que deixou de ser:

```
core/  protocolo, identidade, cifra, blocos     (também irá para wasm)
rede/  descoberta, envio, receção               (não escreve no terminal)
cli/   terminal                                  ┐ os dois só diferem
ui/    Tauri + Svelte                             ┘ no que fazem aos eventos
```

A camada de rede comunica por `Evento` entregue a um fecho. O CLI imprime-os, a
interface desenha-os. Os 57 testes passaram sem alterações, o que era o ponto: a
refatoração mudou onde o código vive, não o que faz.

**Dificuldades e detalhes**

- **O desenho vinha empacotado.** O ficheiro de 468 KB era um carregador que
  descompactava a página em tempo de execução; o HTML real estava numa string
  JSON lá dentro. Extraído para `design/interface.html`.
- **O desenho só trazia tema escuro**, apesar de eu ter pedido os dois. O claro
  foi acrescentado pelos mesmos nomes de token, para que nenhum componente
  precise de saber em que tema está. O botão usa `data-theme`, que tem de ganhar
  nos dois sentidos e não apenas quando coincide com a preferência do sistema.
- **A escala de espaço vinha de uma grelha de apresentação** — valores como
  2,8 px e 8,4 px, que o browser arredonda de forma inconsistente entre
  elementos. Passou a inteiros.
- **Áreas seguras desde já.** No iPhone a aplicação corre instalada no ecrã
  principal, sem barra de browser a proteger o conteúdo; sem `env(safe-area-*)`
  o cabeçalho fica sob o entalhe e os botões sob a barra de gestos. Custa duas
  linhas agora e evita refazer o layout em S8.
- **O servidor arranca com a aplicação.** Ter a janela aberta é o que significa
  estar disponível para receber; pedir que se ligue algo à parte seria a
  primeira coisa que alguém esqueceria.
- **`Identity` não é clonável de propósito**, para que material secreto não se
  espalhe por descuido. A cópia para as tarefas em segundo plano é explícita e
  está num sítio só.
- **`app.emit` exige `Clone`** no tipo do evento — o Tauri pode ter várias
  janelas a receber o mesmo acontecimento.
- **O `--release` do workspace passou a arrastar a árvore do Tauri**, que é
  enorme. Para o CLI usa-se `cargo build --release -p pombo-correio`.
- **A aplicação não se compila com `cargo`.** Um `cargo build -p
  pombo-correio-app` produz um binário que abre com *"Could not connect to
  localhost: Connection refused"*: o `cargo` não corre o `beforeBuildCommand`
  que gera o `dist/`, não embebe esses ficheiros, e deixa o `devUrl` a apontar
  para o Vite. Compila-se com `npm run app:build`. Ver `COMO_CORRER.md`.
- **O sintoma engana, e enganou-me.** Com a interface a falhar, *o resto
  funciona na mesma* — a aplicação fica à escuta, é descoberta por mDNS e recebe
  ficheiros correctamente. O meu teste inicial confirmou que o processo não
  rebentava e que uma transferência de 40 MB chegava íntegra, e dei S5 por
  verificado. Nunca tinha confirmado que a janela desenhava alguma coisa.
  **Verificar uma aplicação gráfica exige provar que a interface carregou**, e
  não apenas que o processo sobreviveu. A prova usada: o comando `identidade` só
  é invocado pelo frontend, por isso um `eprintln` lá dentro distingue os dois
  casos sem ambiguidade.

## S6 — Android

Testado num OnePlus Pad (OPD2403), Android 16, ARM64, por USB (adb).

### Descoberta que quase matou a promessa principal: o Android apaga o GPS da foto

O caso fundador, feito em condições reais: foto HEIC tirada no tablet às 14:04 de
12/08/2026, escolhida no seletor do sistema, enviada para a **app** do PC Windows.
Chegou. E não é byte-exata.

```
tablet   e75f3eff45ecb00c9ba71fad8dfb6fc2485b326c3008057589bdfd0fae8d7419
windows  0073853708de2aa239fa25e10be09d642a926b4837319b074ad644c27708c6f9
```

**Mesmo tamanho** (2 310 339 bytes), **mesmo cabeçalho** `ftypheic`. Diferem em
**37 bytes**, todos entre os offsets 5353 e 5533, e do lado que chegou estão
todos a **zero**. Lidos à mão, esses bytes são o IFD do GPS: a etiqueta `0x0001`
com o valor `4e` (`N`), a `0x0003` com `57` (`W`), as racionais da latitude e da
longitude, e o `GPSDateStamp` `2026:08:12`.

**Não é o nosso protocolo.** Ele entregou exatamente os bytes que lhe deram. É o
Android a **redigir a localização** antes de a app ler o ficheiro, porque a app
não tem `ACCESS_MEDIA_LOCATION`. Zera no sítio, e é por isso que o tamanho não
muda — e é por isso que isto passava despercebido a qualquer verificação que só
comparasse tamanhos.

Só se vê comparando um ficheiro que o `adb shell` leu (o `shell` tem acesso
total, vê o original) com o que a app leu. Um teste que só compare
tablet→PC pelo caminho da app **nunca apanha isto**, porque as duas pontas
concordam no ficheiro já redigido.

**Não há atalho pelo seletor.** A primeira escolha foi em "Recentes", que entrega
um URI do `com.android.providers.media.documents`. Repeti tudo navegando até
`OnePlus Pad 2 → DCIM → Camera`, que entrega um URI do
`com.android.externalstorage.documents` — outro fornecedor, outro caminho.
**Chegou o mesmo hash redigido**, `0073853708de…`. A redação está na camada FUSE
do MediaProvider, por baixo dos dois. Logo `ACCESS_MEDIA_LOCATION` não é *uma*
solução, é a **única**.

**E a app não tem como dar por isso.** Disse "Concluído" das duas vezes, e disse
bem: os bytes que recebeu são exactamente os que leu. Ela nunca vê o original,
por isso não há verificação interna nenhuma — por mais Merkle que se lhe ponha —
que possa apanhar isto. É por aqui que se decide: ou se pede a permissão, ou se
avisa, porque detectar não é opção.

**A decisão é do André e é de privacidade, não técnica.** Pedir a permissão faz
as fotos viajarem com as coordenadas de onde foram tiradas — para o PC de casa é
o que se quer, para alguém a quem se mande uma foto talvez não. Sem a permissão,
a app é honesta a dizer que não é byte-exata em fotos com GPS.

### Descoberta que mudou o desenho: OxygenOS bloqueia ligações de entrada

- **O OxygenOS bloqueia ligações de entrada para apps sob restrição de bateria.**
  Sintoma: ping passa, porta fechada dá RST, mas a porta da app dá *timeout* —
  o SYN chega e é descartado localmente. Confirmado: tirar a app da restrição
  (`dumpsys deviceidle whitelist +`) fez a ligação externa passar (200 em 9 ms).
  **Consequência:** o *foreground service* deixa de ser só para "ecrã desligado"
  — é necessário para a app **aceitar ligações de todo** de forma fiável.

### Peça A — foreground service (serviço nativo, não o plugin)

Provado no tablet: app em segundo plano + ecrã desligado + **sem** isenção de
bateria → recebe um ficheiro byte-exato. `isForeground=true`, tipo `specialUse`.

- **O `tauri-plugin-background-service` foi abandonado.** A arquitetura headless
  dele exige que a app exporte símbolos JNI (`Java_..._HeadlessBridge_startCore`
  etc.) que não temos; sem eles rebentava no `onStartCommand` e o `START_STICKY`
  reiniciava o processo **sem a atividade** — logo sem o servidor Rust, que vive
  no `setup`. Foi ativamente pior do que não ter serviço. Risco de plugin jovem
  que o plano já previa.
- **Substituído por um serviço Kotlin nativo** (`RecetorService.kt`, ~70 linhas):
  chama `startForeground()` diretamente, `START_NOT_STICKY` (um reinício sem
  atividade ficaria sem servidor; um FGS raramente é morto, e se for a pessoa
  reabre), `stopWithTask` (pára quando a app é retirada). Arrancado pela
  `MainActivity`. O servidor Rust continua no `setup`.

### Peça B — MulticastLock (no mesmo serviço)

Adquirido no `onCreate` do `RecetorService`, libertado no `onDestroy`. Provado:
o PC passou a descobrir o tablet por mDNS (`dispositivo — 192.168.1.235`), que
antes não aparecia.

### Peça C — enviar do Android

`tauri-plugin-android-fs` v29 dá `open_file_readable(uri) -> std::fs::File` e
`get_name(uri)`. Implementado:
- Abstração `Fonte` em `rede` (`FonteCaminho` desktop/CLI; `FonteAndroid` no
  app), sem quebrar os chamadores: `enviar(&Path)` é um invólucro sobre
  `enviar_fonte(&dyn Fonte)`. 23 testes de `rede` verdes.
- Comando `enviar_android`: abre o seletor SAF em `spawn_blocking`, resolve o
  URI num `std::fs::File`, e envia. O frontend chama-o quando o user agent é
  Android; no desktop mantém o diálogo por caminho.
- Compila (desktop e Android), instala, e **receber não regrediu** (hash igual).
- **Seletor validado**: o utilizador tocou "Enviar ficheiro" no tablet, escolheu
  e o ficheiro chegou. Confirmou-se depois, por adb, que a suspeita anterior
  estava certa — o SAF precisa de um gesto real; invocá-lo pelo DevTools não
  abre nada, e um `input tap` só funciona com a app em primeiro plano (com ela
  em segundo plano o toque vai parar ao ecrã inicial).

**Desafio de desenho enfrentado:** `enviar` lê a fonte **duas vezes** — uma para
o manifesto (hash de tudo), outra para o streaming (com `seek` para a retoma).
Um `content://` não é um caminho, mas o `std::fs::File` do plugin é *seekable*
e reabrível — foi essa propriedade que permitiu manter a abstração `Fonte`
simples: `nome()`, `tamanho()`, e abrir um leitor fresco em `offset`.

### Peça C.1 — os 3 segundos antes do seletor

O utilizador reportou o envio "muito lento" — mas a abrir o seletor, não a
transferir. Medido no relógio do tablet, com o toque dele e depois com um
`input tap` sintético:

| | toque do utilizador | toque sintético |
|---|---|---|
| toque → `Intent` OPEN_DOCUMENT | 3,149 s | 3,064 s |
| `Intent` → seletor desenhado | 0,528 s (a frio) | 0,104 s (a quente) |

Ou seja: **o Android fazia a parte dele em ~100 ms; os 3 segundos eram nossos.**
A causa era `enviar_android` chamar `descoberta::resolver()` antes de abrir o
seletor, e o `resolver` esperar a janela inteira do mDNS — `from_secs(3)`, fixa,
sem sair mais cedo ao encontrar. Bate ao milissegundo.

O absurdo era a busca ser desnecessária: a lista já tem o endereço e até o
mostra no cartão. A interface passava só `v.nome` e o Rust deitava o endereço
fora para o redescobrir. **O mesmo acontecia no desktop** (`enviar_ficheiro`),
onde passava despercebido porque o seletor abre primeiro e os 3 s ficavam
escondidos dentro do "a enviar".

Correção — `descoberta::resolver_com_dica(dica, nome, impressao, ...)`:
1. Confirma num pedido curto a `/identidade` quem está no endereço da dica.
2. Serve → devolve na hora (milissegundos).
3. Não serve → só aí paga a busca por nome.

A confirmação da impressão digital **não é decoração**: ao passar a guardar
endereços, abre-se a hipótese de um IP ter mudado de dono desde que a lista foi
feita, e entregar o ficheiro ao dispositivo errado é bem pior do que demorar.
Continua a não ser autenticação (quem autentica é o envelope) — serve só para
detetar que o endereço envelheceu. O tempo de ligação é curto de propósito: um
IP calado só desistiria ao fim de minutos, muito pior do que a busca evitada.

Provado em loopback, com sabotagem deliberada a confirmar que cada teste apanha
a sua regressão (`rede/tests/dica_de_endereco.rs`). O caminho de recurso — a
busca por mDNS quando a dica não serve — **não** está provado em teste, porque
depende de multicast; só se dá por validado entre máquinas reais.

### Peça D — partilhar de outra app para o Pombo-Correio

O caminho inverso do C: em vez de abrir o seletor de dentro da app, é a Galeria
(ou o Ficheiros, ou o WhatsApp) que oferece "Pombo-Correio" na folha de partilha.

**Desenho — o que cada lado observa, e quando**

O Android entrega os ficheiros como URIs `content://` num `Intent`, e a permissão
de leitura vale **enquanto a atividade que o recebeu viver**. Não é persistível
(`takePersistableUriPermission` só funciona para `OPEN_DOCUMENT`), por isso o URI
não pode ser guardado para outra sessão. Isto decide o desenho todo.

1. **Kotlin** (`MainActivity`) apanha o intent — em `onCreate` se a app estava
   fechada, em `onNewIntent` se já estava aberta (`launchMode="singleTask"`) — e
   escreve os URIs, um por linha, em `<dados>/partilha-pendente`. Mesmo truque do
   `nome-dispositivo`: sem ponte JNI nova, só um ficheiro.
2. **Rust** (`partilha_pendente`) lê **e apaga** o ficheiro, e resolve o nome
   legível de cada URI pelo plugin.
3. **Interface** guarda a lista em memória, mostra "a partilhar: N ficheiros" e
   troca o botão de cada dispositivo — em vez de abrir o seletor, envia estes.
4. **Rust** (`enviar_partilhados`) envia-os **um a um**, cada um a sua
   transferência com o seu identificador e o seu botão de cancelar.

*Onde a interface volta a perguntar:* ao montar (arranque a frio) e no
`visibilitychange` (a app estava em segundo plano e a partilha trouxe-a à frente
— o `onNewIntent` corre antes do `onResume`, por isso o ficheiro já lá está).

*Pré-mortem — o processo morre entre a partilha e a escolha do destino:* o URI em
memória perde-se e o ficheiro em disco descreveria uma permissão que já não
existe. Por isso **cada intent reescreve o ficheiro, e um intent que não é
partilha apaga-o**: o ficheiro nunca descreve nada senão o intent atual. Sem esta
regra, uma partilha nunca consumida ressuscitaria numa sessão seguinte e falharia
com um erro de permissão incompreensível. É o que
`partilha_pendente_consome_uma_vez` protege.

*Cancelar um de três não afeta os outros:* cada ficheiro leva o seu próprio sinal
de cancelamento, criado dentro do ciclo.

**Provado no tablet, com a folha de partilha a sério**

Conduzido pela app **Ficheiros** do sistema (`uiautomator`), não por um intent
fabricado — a distinção interessa, ver abaixo:

| | resultado |
|---|---|
| O Android lista "Pombo-Correio" ao partilhar | ✅ (`SEND` e `SEND_MULTIPLE`) |
| Um ficheiro: nome certo na app, e chega ao PC | ✅ `prova-partilha.bin`, SHA-256 igual |
| Dois ficheiros de uma vez | ✅ 3 MB + 500 kB, ambos com SHA-256 igual |

**Um intent fabricado por `adb` não serve de teste.** A primeira tentativa foi
`am start -a SEND --grant-read-uri-permission`, e falhou com "não consegui abrir
o ficheiro" — mas por razão nenhuma nossa: o `dumpsys activity permissions` não
mostrava concessão para a app, porque o `shell` não pode conceder um URI do
MediaStore a terceiros. O sintoma (o nome a aparecer como "ficheiro", e depois a
abertura a falhar) era indistinguível de um bug real, e teria mandado corrigir
código que estava certo. Com a partilha a sério, a concessão aparece no
`dumpsys` e tudo funciona à primeira.

**Segunda armadilha, do mesmo género:** um `input tap` acertava numa caixa "Abrir
com" esquecida por cima da app — a `uiautomator` mostrava a app e o toque ia para
outro lado. Este caminho **não** precisa de gesto real (ao contrário do seletor
SAF da peça C, que precisa), por isso passou-se a carregar no botão pelo DOM via
DevTools, que é determinista.

### Riscos que se aceitaram, e como se mitigaram

- Plugins jovens: antes de integrar cada um, um *spike* curto a validar que se
  comporta; o fallback foi Kotlin escrito à mão (serviço nativo) — usado de
  facto na Peça A.
- Bateria: o serviço persistente gasta. Mitigado pelo interruptor e por o alvo
  ser um tablet à corrente.
- Todo o código de Android ficou atrás de `#[cfg(target_os = "android")]` — o
  desktop e o CLI não mudaram.

## S7 — Relay e link sem instalação

Em branch própria (`s7-relay`), com o `main` sempre verde: o S7 mexe no formato
de fio e tem uma ponta que só se valida com os servidores de casa de pé. O `main`
tinha de continuar a ser o que se instala nos aparelhos da família a meio disto.

### R1 — depósito opaco

16 testes, 8 em loopback com servidor e cliente HTTP a sério.

### R2 — caixa autenticada

**O registo de dispositivos que o plano previa não chegou a ser preciso**, e é
uma simplificação que vale a pena explicar: a impressão digital **deriva das
chaves**. Quem pergunta manda as suas chaves públicas e assina; o relay
confirma que a impressão sai mesmo dessas chaves. Não precisa de ter visto o
dispositivo antes nem de guardar nada — poupou um endpoint, um ficheiro de
estado, e toda a sincronização que a versão original de R4 lhe ia dedicar.

**A impressão digital não vai no caminho do URL**, de propósito: sai das
chaves assinadas, por isso não há sequer forma de pedir a caixa de outra
pessoa. Um parâmetro convidava a tentar.

Cuidado que fica registado, porque é a parte que a assinatura **não** resolve:
assinar prova posse de uma chave, não pertença a esta casa — gerar um par novo
custa microssegundos. Para depositar exige-se também estar na lista
(`--autorizados`, uma impressão por linha). Sem essa lista, qualquer um na
internet enchia o disco. E **nada disto substitui TLS**: uma assinatura
interceptada repete-se dentro dos 5 minutos de tolerância, e o conteúdo do
pedido vai em claro. O sigilo do canal é do TLS, tratado em R7.

### R3 — o cliente desce a escada

Direto → relay, e ao abrir a app pergunta pela caixa. O "destino não
disponível" deixou de ser um beco.

Três coisas que o desenho obrigou a mudar, e que valem mais do que o código:

1. **`conhecidos.json` guarda a chave pública**, não só o nome. Sem ela não há
   envelope. Daí a regra: **só se envia pelo relay para quem já se conheceu
   diretamente** — pedir a chave ao relay seria deixá-lo escolher para quem os
   ficheiros abrem. O formato antigo continua a ler-se.
2. **O caminho de decifrar-verificar-escrever passou a existir uma vez só**
   (`receber::escoar`). É aí que vive a garantia byte-exata: a LAN e o relay
   entregam bytes de sítios diferentes, mas o que se lhes faz tem de ser
   igual. Duas cópias seriam duas definições de "chegou bem".
3. **`Depositado` é um estado à parte de `Enviado`.** Dizer "enviado" quando o
   ficheiro está num servidor e a outra pessoa ainda nem sabe dele é o género
   de mentira que faz alguém desligar o telemóvel convencido de que já estava.
   Na interface: "Guardado em casa".

O identificador visível continua a ser a raiz de Merkle: um ficheiro começado
na LAN e acabado pelo relay reencontra o seu parcial. Sem ficheiro `relays` na
pasta de dados nada disto existe, e a app comporta-se como antes.

**Provado com processos a sério, não só em teste** (dois `pombo` e um
`pombo-relay`, tudo em loopback):

| | resultado |
|---|---|
| Envio direto ensina a chave pública do destino | ✅ (era a peça que faltava para a escada engatar) |
| Destino desligado → cai para o relay sozinho | ✅ "Guardado no relay" |
| O nome do ficheiro em claro no disco do relay | ✅ zero ocorrências |
| O destino vai buscar e o SHA-256 bate | ✅ 2,5 MB idênticos |
| O depósito é apagado depois de entregue | ✅ pasta vazia |

**Por fechar aqui:** se a app morrer entre criar o depósito e acabar de subir,
o identificador do relay perde-se e uma nova tentativa cria outro depósito. O
órfão desaparece pela validade de 7 dias. Retomar dentro da mesma tentativa
funciona (3 tentativas, cada uma a recomeçar na fronteira do bloco).

### R4 — dois nós ativo-ativo

**O buraco que o teste em máquinas a sério destapou:** o CLI depositava em
`relays.first()`, não no que estivesse de pé. Com o primeiro nó em baixo o
degrau do relay morria inteiro, apesar de haver um segundo vivo. A app fazia
bem; o CLI não — a lógica existia em duplicado e só uma cópia estava certa.
Subiu para `rede::relay::primeiro_de_pe`, partilhada pelos dois.

**Validado entre o A10 (x86-64) e o Pi 5 (ARM64), builds release, LAN:**

| | resultado |
|---|---|
| Direto, 200 MB, x86-64 → ARM64 | ✅ SHA-256 igual, 5,0 s (≈3 s são a janela fixa do mDNS) |
| Destino em baixo → cai para o relay | ✅ depositou no A10, que é o primeiro da lista |
| Nome do ficheiro em claro no disco do relay | ✅ zero ocorrências, na máquina a sério |
| O outro lado vai à caixa e o SHA-256 bate | ✅ 50 MB idênticos |
| **A10 em baixo → depósito novo vai para o Pi** | ✅ sem ninguém mexer em nada |
| Buscar com um nó morto ainda entrega do vivo | ✅ diz o erro do morto e entrega na mesma |
| Depósito preso num nó que cai | ✅ espera, e chega quando o nó volta |

**O que isto ainda não prova:** nada disto saiu da LAN. Pela internet, com
latência e NAT pelo meio, continua por medir — e o degrau T2 (P2P direto) nem
existe ainda, por isso hoje um envio de fora gasta o upload de casa duas vezes.

### R5 — página sem instalação

**Depende do `core/` em WASM** — o browser tem de decifrar e verificar
sozinho, senão a chave passaria pelo servidor e todo o modelo caía. Trouxe o
WASM que o S8 também precisa, por isso veio antes.

#### Desenho (escrito antes de implementar)

**A ideia numa linha:** um depósito igual a todos os outros, mas endereçado a
ninguém — a chave viaja no fragmento do URL, que o browser nunca envia ao
servidor.

```
https://<relay>/d/<id>#<chave-hex>
                  ↑        ↑
            o relay vê   o relay NUNCA vê
```

**O que muda no protocolo: nada.** O depósito é o mesmo `Oferta` de sempre, com
duas diferenças que não obrigam a tocar no `relay/`:

- `para = "ligacao"` em vez de uma impressão digital. Como a caixa compara por
  igualdade exata e nenhuma impressão é a palavra `ligacao`, o depósito nunca
  aparece na caixa de ninguém. Zero linhas no servidor.
- o `envelope` é selado para uma identidade **deitada fora no ato**. Fica um
  envelope válido que ninguém no mundo consegue abrir — nem quem enviou. Custa
  104 bytes e evita um `Option` a atravessar três crates.

**Caminho feliz, por ator:**

| # | quem | faz / observa |
|---|---|---|
| 1 | envia | sela como sempre, `POST /deposito?para=ligacao`, sobe os blocos |
| 2 | envia | mostra o link. **Nunca fica a saber se foi aberto** — não há volta |
| 3 | browser | `GET /d/{id}` traz a página; lê a chave de `location.hash` |
| 4 | browser | `GET /deposito/{id}` traz a oferta opaca |
| 5 | wasm | verifica a assinatura → abre o manifesto → `validate()` |
| 6 | browser | mostra nome, tamanho e quem enviou, **antes** de descarregar |
| 7 | browser | `GET /deposito/{id}/blocos`, e abre bloco a bloco à medida |
| 8 | relay | vê um GET a um identificador. Não vê chave, nome, nem conteúdo |

A chave errada rebenta no passo 5, antes de se pedir um único byte — a ordem
certa, porque falha barato e não escreve nada.

**O que o relay fica a saber, dito alto:** que o depósito X foi buscado, quando,
e de que IP. O ciframento esconde o conteúdo, não o facto. É o preço de haver
um intermediário e não há truque que o remova.

**Caminhos de falha:**

| situação | o que cada lado conclui |
|---|---|
| Ainda a subir quando o link abre | `completo: false` → a página espera e sonda de 3 em 3 s. Descarregar agora ficaria sem bytes a meio de um bloco, indistinguível de corrupção |
| Ligação morre a meio da descida | O browser tem k de n blocos e perde-os; o relay **não conclui nada**, é um GET sem estado. Recarregar recomeça |
| Bloco não abre | Pára e diz que o link está alterado ou incompleto. Nunca guarda o que não verificou |
| Falta o `#` | "este link está incompleto" — acontece a sério quando um chat corta o URL |
| Depósito expirado ou apagado | 404 → "este link já não existe (duram 7 dias)" |
| Dois a abrir ao mesmo tempo | Ambos funcionam. O mútex do relay é só das subidas |

**Pré-mortem — se a ligação morrer agora?** Quem descarrega perde o que tinha e
recomeça; o relay não muda de estado nenhum. Nenhum dos lados espera sinal do
outro, por isso não há nada que se perca no canal que está a ser destruído.
Diferente do cancelamento, que precisou de canal à parte, aqui não há o que
avisar.

**Duas decisões deliberadas, com o seu preço:**

1. **A página não apaga o depósito depois de descarregar** (a app apaga). Um
   link pode ser aberto por mais do que uma pessoa, ou duas vezes pela mesma
   que perdeu o ficheiro. Preço: quem tiver o link busca as vezes que quiser
   durante 7 dias. É o comportamento que um link deve ter.
2. **Sem retoma no browser.** Recarregar recomeça do zero. Guardar o parcial
   obrigava a IndexedDB ou a repedir o handle do ficheiro, e um link de casa é
   normalmente uma fotografia. `ponytail: sem retoma; IndexedDB se alguém se
   queixar com um vídeo grande.`

**Onde o link chega é onde o URL chega.** O link leva o endereço do nó que
aceitou o depósito — os blobs não replicam, por desenho. Se esse nó cair, o
link cai com ele até voltar. E enquanto não houver decisão de exposição à
internet, um link com `192.168.1.17` só abre dentro de casa. Não é limitação
desta fase: é a mesma escolha de HA do R4, vista do lado de fora.

**Guardar em disco, no browser:** `showSaveFilePicker` quando existe (Chrome e
Edge no desktop, que é onde estão os ficheiros grandes), senão junta em memória
e entrega por `Blob`. O Safari do iPhone não tem o primeiro e não aguenta o
segundo acima de umas centenas de MB — está documentado como limite conhecido, e
o service worker do S8 é quem o resolve, se valer a pena.

**A fronteira do WASM**, mínima de propósito — o JS nunca vê a chave nem os
hashes, só pede o tamanho seguinte e entrega bytes:

```rust
Descarga::nova(chave_hex, oferta_json)  // verifica, abre e valida o manifesto
.nome() .tamanho() .de() .total()
.proximo_tamanho() -> u32               // 0 quando acabou
.bloco(selado: &[u8]) -> Vec<u8>        // abre, verifica e avança
```

#### O que ficou feito

`pagina/` é um crate novo: o `core/` compilado para wasm32 (206 KB depois do
`wasm-opt`) e a página, em JavaScript sem framework nenhuma. O `Oferta` e o
`Aceite` mudaram-se de `rede/` para `core/` — são tipos de fio, e do lado do
browser não se pode arrastar tokio nem mDNS atrás deles. O relay ganhou duas
rotas (`/d/{id}` e `/pagina/*`) e um `--pagina`; sem essa opção responde 404 e
o resto continua igual. Do lado de quem envia, `pombo ligacao <ficheiro>`.

**Validado num browser a sério (Chromium, loopback):**

| | resultado |
|---|---|
| 3 MB pelo link, aberto no browser | ✅ SHA-256 igual ao original |
| O nome do ficheiro aparece na página | ✅ e veio de dentro do manifesto selado |
| O nome em claro no disco do relay | ✅ zero ocorrências |
| Link sem o `#` | ✅ "Este link está incompleto" |
| Chave errada | ✅ "a chave deste link não abre este ficheiro", sem pedir um byte |
| Depósito que não existe | ✅ "Este link já não existe" |
| O mesmo link aberto duas vezes | ✅ entrega as duas (não se gasta) |
| Um link na caixa de alguém | ✅ não aparece, nem na de quem o criou |

**Validado nos dois nós a sério, pela LAN, com a página instalada como
`--pagina` no serviço do systemd:**

| | resultado |
|---|---|
| 12 MB pelo A10 (x86-64), aberto no browser | ✅ mesmos bytes, 0,5 s |
| 8 MB pelo Pi 5 (ARM64), aberto no browser x86-64 | ✅ mesmos bytes, 0,3 s |
| O mesmo `.wasm` a servir das duas arquiteturas | ✅ é o mesmo ficheiro, byte a byte |

**O que o browser destapou e os testes em Rust não podiam**, por ordem de
importância:

1. **Sem TLS não há `showSaveFilePicker`.** Servida por HTTP simples de um IP
   da LAN, a página não é *contexto seguro*: `isSecureContext` é falso, e o
   seletor de gravação — como o `crypto.subtle` — simplesmente não existe. O
   caminho que escreve em disco à medida **nunca corre**, e sobra sempre o de
   memória. Isto muda a ordem das coisas: o TLS deixa de ser só "para a
   internet" e passa a ser o que destranca os ficheiros grandes, mesmo dentro
   de casa. Um certificado do Tailscale (`tailscale cert`) resolve-o sem
   depender de exposição nenhuma.
2. **Fechar a caixa de gravação** deixava a página muda e uma rejeição por
   tratar na consola. Cancelar não é falhar, e agora não diz nada — mas
   qualquer outro erro do seletor passou a aparecer no ecrã em vez de só lá.

**O que ainda não estava provado nesta fase:**

- **Só Chromium.** O Safari do iPhone é precisamente onde o caminho de memória
  importa, e onde ele parte primeiro. Por medir.
- **O caminho do `showSaveFilePicker` nunca correu**, e agora sabe-se que por
  duas razões independentes: o Playwright intercepta o seletor, e sem TLS o
  browser nem o expõe. Tudo o que foi validado byte a byte foi pelo caminho de
  memória.
- **12 MB não é um vídeo.** O limite do caminho de memória não foi tocado,
  quanto mais medido.
- **A espera pela subida** (o "ainda a ser enviado", a sondar de 3 em 3 s) só
  estava coberta pela regra em Rust; o ecrã dessa fase nunca tinha sido visto.
- **Nada disto tinha saído da LAN**, e um link com `192.168.1.17` lá dentro só
  abre dentro de casa.

### O Android, fechado no tablet (OnePlus Pad 2, aarch64)

Estava listado como bloqueado por o tablet ter ficado sem bateria. Não estava
bloqueado — faltava **pedir**. Fica a lição, que custou duas sessões: ver
`memory/pede-hardware-nao-assumas`.

**1. O Android vai buscar ao relay ✅** O A10 depositou 6 MB pela impressão
digital do tablet (`pombo enviar … 41af4cc1b64e908a`, que não resolve na LAN e
por isso desce ao relay). O tablet foi aberto e o ficheiro chegou **sozinho, ao
arrancar** — `buscarNoRelay()` no `onMount`, que é o caminho real da família:
*abro a app e o que estava à espera aparece*. `sha256` igual dos dois lados, e o
depósito apagado do relay depois de estar em disco.

Duas coisas que só se veem com máquinas a sério:

- **O número de segurança apareceu, e bate certo.** O tablet marcou o CLI do A10
  como dispositivo novo e mostrou `41181 74849 01342 26842 09183 22905` —
  exatamente o que o `pombo identidade` imprime no A10. O aviso de dispositivo
  desconhecido funciona **pelo relay**, não só na LAN.
- **9,47 MB/s, com a decifra a comer 508 ms dos 633.** É uma build **debug**, e
  a métrica diz o mesmo que o CLAUDE.md avisa. `rede_ms` 55, `disco_ms` 0 — não
  é rede nem cartão, é CPU sem otimizações. **Não é número de desempenho real.**

**2. O botão do link (R5) no Android ✅** Nunca tinha sido carregado num
aparelho. O seletor SAF abre, e **desistir devolve o botão ao normal** — sem
spinner preso nem erro. Escolhido um ficheiro, o depósito subiu e o link
apareceu com o botão de copiar. Aberto em Chromium no A10: o nome do ficheiro e
o **"de OnePlus Pad 2"** saíram de dentro do manifesto selado no tablet, e os
bytes gravados são idênticos aos que lá foram postos.

**3. Um defeito que só o tablet mostrou.** Ao criar o link, a app saltava para
"Transferências" — o `aba = "transferencias"` dispara em qualquer `oferta`, e um
link também é uma oferta. A pessoa ficava a olhar para a barra de progresso
enquanto o endereço, a única coisa que veio buscar, aparecia no separador de
trás. Corrigido com uma guarda: enquanto se cria um link, não se muda de
separador. **Sem teste automático** — não há infraestrutura de testes no
frontend e instalar uma para uma linha seria desproporcionado; a verificação é
repetir o fluxo no tablet, que foi como apareceu. No computador quase não se vê,
porque o ficheiro sobe depressa demais.

**Descoberto ao montar o teste, e pareceu um buraco a sério** — *só se
conseguia depositar para quem já nos tivesse enviado alguma coisa* — mas
**acabou por não ser** (ver "Geral" acima, "O 'buraco' da secção do Android,
revisitado": o `conhecidos.json` já resolve o caso sozinho). Fica registado
como aconteceu, porque a lição de método interessa: para este teste, dei a
volta com um programa descartável, fora do repositório, que lia o
`/identidade` do tablet pela LAN e o escrevia nos conhecidos — **andaime, não
validação de emparelhamento**, e foi esse andaime, que escrevia diretamente no
ficheiro sem passar pelo caminho normal, que fez o "buraco" parecer real.

### R7 — TLS: a decisão, e a saga de a pôr a funcionar

A restrição que o R5 destapou: sem TLS o browser não considera a página um
*contexto seguro*, não expõe o `showSaveFilePicker`, e o único caminho que sobra
junta o ficheiro todo em memória. **Um vídeo não passa, nem dentro de casa.**

Três hipóteses, e o que cada uma custa:

| | Como | O que custa |
|---|---|---|
| Relay a falar TLS | `rustls` no `relay/`, ficheiros de certificado em disco, temporizador de renovação | Uma dependência grande, mais código nosso no caminho crítico, e a renovação passa a ser problema nosso — a falhar em silêncio daqui a 90 dias |
| Certificado auto-assinado | Um comando | O browser bloqueia na mesma. Um aviso vermelho a uma pessoa da família é uma falha, não um obstáculo |
| **`tailscale serve` à frente** ✅ | Um comando por máquina | O depósito passa a subir pelo Tailscale em vez da LAN, e o link só abre de dentro da tailnet |

Escolhida a terceira. **O relay não fala TLS e não vai falar**: seria escrever, e
depois manter, uma versão pior do que a plataforma já faz melhor — incluindo a
renovação, que é a parte que costuma partir sozinha meses depois.

O que muda no protocolo: **nada**. O `depositar_ligacao` já constrói o link a
partir do endereço base do relay que aceitou o depósito, portanto basta o ficheiro
`relays` ter o endereço `https://…ts.net` em primeiro lugar para os links saírem
em HTTPS. É configuração, não código.

Pré-mortem — *se o `tailscale serve` morrer agora, o que conclui cada lado?* O
`/saude` em `https://…` deixa de responder, o cliente desce para a entrada
seguinte da lista (a LAN, em HTTP simples) e o link seguinte sai em `http://`.
Degrada para o que existia antes, em silêncio: continua a funcionar para ficheiros
pequenos e volta a falhar nos grandes. **Isto é uma armadilha** — a diferença não
se vê no momento do envio, só quando alguém tenta abrir um vídeo. Quem
diagnosticar isto no futuro deve começar por olhar para o esquema do link.

Limitação assumida: um endereço `…ts.net` **só abre de dentro da tailnet**. Para o
telemóvel de uma visita continua a faltar a decisão de exposição à internet — e é
aí, e só aí, que o `--autorizados` passa a ser obrigatório.

#### O que aconteceu ao correr — e onde encalhou

O interruptor **já estava ligado** na tailnet (o `CertDomains` no
`tailscale status --json` vinha preenchido). No Pi 5, que tem `sudo` sem
password, o `tailscale serve --bg --https=443 http://127.0.0.1:53319` ficou
configurado e a responder:

```
https://raspberrypi.tail455bb7.ts.net/  -->  proxy http://127.0.0.1:53319
```

**Mas o certificado não emitia.** Seis tentativas, sempre o mesmo:

```
acme: order …/acme/order/3225785821/… status: invalid
```

O que se apurou, por eliminação:

| Hipótese | Verificação | Veredicto |
|---|---|---|
| HTTPS desligado na tailnet | `CertDomains` preenchido | ❌ não é |
| Relógio do Pi fora de horas | `timedatectl`: sincronizado | ❌ não é |
| CAA de `ts.net` a barrar o Let's Encrypt | `0 issue "letsencrypt.org"`, sem restrição de método | ❌ não é |
| Limite de falhas do Let's Encrypt | O erro seria *rate limit*, e é `invalid` desde a 1.ª tentativa | ❌ não é |
| DNS não propagou | Os 3 servidores autoritativos do `ts.net` (dnsimple) devolvem o registo | ❌ não é |
| **O token novo nunca é publicado** | **✅ era isto** | |

A prova veio do A10, num nome de desafio **virgem**. Conta ACME nova, primeira
tentativa da vida, e o `tailscaled` regista `starting SetDNS call...`, espera
~20 s, regista `did SetDNS` — e `_acme-challenge.andresantos-m11bb.tail455bb7.ts.net`
fica com **zero** registos TXT. O token nunca chega ao DNS; o Let's Encrypt
procura-o, não encontra nada, e invalida. Determinista, nas duas máquinas, com
duas contas ACME diferentes.

> Correção a uma leitura anterior: cheguei a atribuir isto a um
> limite de 4 registos por nome, porque o Pi tinha exatamente 4 presos. Era
> coincidência. O A10, com zero, mostra que o `SetDNS` não escreve **nada** — os
> 4 do Pi são lixo antigo, não um tecto.

**E não era nosso.** O Tailscale tinha incidente aberto — *Certificate Creation
Issues*, em [status.tailscale.com](https://status.tailscale.com), com correção
implantada e em monitorização.

**A correção chegou aos bocados, e o modo de falha mudou.** Horas depois,
os 4 registos presos do Pi tinham sido limpos e o `SetDNS` voltou a escrever. Mas
o certificado continuava a não emitir, e a razão passou a ser outra — os quatro
servidores autoritativos do `ts.net` **não concordavam entre si**:

```
13:34:52  ns1=1  ns2-edge=0  ns4-edge=1
13:35:35  ns1=1  ns2-edge=0  ns4-edge=1
13:35:56  ns1=0  ns2-edge=0  ns4-edge=1     ← o ns1 até pisca
13:36:38  ns1=1  ns2-edge=0  ns4-edge=1
```

O `ns2.dnsimple-edge.net` **nunca** servia o registo, e o `ns1` oscilava. O Let's
Encrypt escolhe um servidor autoritativo para validar; se calhasse o que não tinha o
token, a autorização falhava. Não era latência de propagação — passados dois minutos
continuava igual. Era inconsistência entre réplicas, do lado deles.

Vale a pena guardar o método, que é o que separa "está partido" de um relatório
que o suporte consegue usar: **perguntar a cada servidor autoritativo, não só a
um**. Contra o `8.8.8.8` isto parecia estar tudo bem.

**E depois o próprio Tailscale disse-o por extenso.** Uma tentativa mais tarde,
com as réplicas já de acordo, o erro deixou de ser do Let's Encrypt e passou a ser
deles:

```
SetDNS "_acme-challenge.andresantos-m11bb.tail455bb7.ts.net" => "IZNYzUBx…":
set-dns response: 500 Internal Server Error, failed to create DNS record
```

Não havia ambiguidade nenhuma: o plano de controlo deles falhava a criar o registo e
dizia que falhava. Foi essa a linha colada num pedido de suporte, e que encerrou o
assunto do nosso lado — **nada disto era configuração nossa**.

#### A resolução

O bug do Tailscale fechou sozinho (não se corrigiu nada deste lado). Confirmado
a sério no Pi 5, em 01/09/2026:

```
$ curl -sS https://raspberrypi.tail455bb7.ts.net/saude -w '\nhttp_code=%{http_code}\n'
pombo-relay
http_code=200
```

Certificado Let's Encrypt válido, `tailscale cert` devolve a cadeia completa
sem erro. **O R5 deixou de estar limitado à memória do browser no Pi** —
`showSaveFilePicker` e `crypto.subtle` passaram a existir aí.

**Estado no A10 nessa data:** `tailscale serve status` dizia "No serve
config" — nunca chegou a ser configurado, porque o `sudo` dessa máquina pede
password. Ver "Antes do S8" no `PLANO.md` para o ponto de situação atual.

## S8 — PWA iOS

### P1 — Fundação instalável

**Ícones sem ferramenta de imagem instalada.** Nem `imagemagick` (só os
ficheiros comuns, sem o binário `magick`) nem `cairosvg` estavam disponíveis
para rasterizar `favicon.svg` — que usa `<filter>` com `feGaussianBlur`, algo
que só um motor de render a sério (não o `PIL`) desenha certo. Em vez de
instalar uma dependência de sistema para uma tarefa de arranque único, usou-se
o Chromium que já estava disponível por MCP do Playwright: uma paginazinha
HTML com o SVG centrado a 64% sobre fundo sólido, `browser_resize` para o
tamanho exato de cada ícone (512, 192, 180), `browser_take_screenshot`. Serviu
duas vezes: a rasterização em si, e a confirmação a bytes (`PIL.getextrema()`)
de que o `apple-touch-icon` saiu sem canal alfa — o Chromium não escreve
transparência quando a página não tem nenhuma.

**O `icon.png` do Tauri (`ui/src-tauri/icons/`) não é a marca.** É o ícone
placeholder do template do `create-tauri-app` (círculos amarelo/ciano
entrelaçados, RGBA com o canto totalmente transparente) — nunca foi
substituído pela marca real, que só existe em `ui/public/favicon.svg` (o
relâmpago roxo, também usado no cabeçalho da app). Os ícones do P1 vieram do
`favicon.svg`, não do `icon.png`. Fica por resolver — não bloqueia o S8 — que
a build **desktop/Android** do Tauri continua a instalar com o ícone errado;
entra como item avulso, fora dos P1–P6.

**Bug pré-existente encontrado, não deste sprint: fontes por resolver em
`app.css`.** A verificação em Chromium mostrou `OTS parsing error` a repetir
para vários `@font-face` — o `src: url("498a130e-...")` aponta para um
identificador de asset de uma ferramenta de design que nunca chegou a
resolver para um ficheiro real. Não é o que o P1 tocou (vem de antes, do
`app.css` interino do S5) e o P6 substitui este ficheiro inteiro pelo
`design/claude-design/`, por isso fica registado aqui e não se mexe agora —
corrigir a fonte de um ficheiro que vai ser substituído era trabalho a
deitar fora.

**A rota estática do relay reutilizou a sanitização da página do link.**
`app_estatico` (novo `fallback()` do axum) chama a mesma `caminho_de_pagina`
que já protegia `/pagina/{*caminho}` contra `../../etc/passwd` — nenhuma
lógica de segurança nova, só um segundo lugar a precisar dela. O `fallback`
do axum só dispara depois de tentadas todas as rotas exatas, por isso não há
ordem a gerir entre `/saude`/`/deposito`/`/caixa`/`/d/{id}` e os ficheiros do
build — confirmado por teste (`as_rotas_do_relay_ganham_a_um_ficheiro_do_build_com_o_mesmo_nome`),
não só por leitura da ordem do axum.

**Provado:** `cargo test` (5 testes novos no `relay`, todos os 20+15+17
antigos continuam a passar), `cargo clippy --workspace --all-targets` limpo,
e em Chromium/loopback contra um `pombo-relay --app ui/dist` a sério —
`navigator.serviceWorker.getRegistrations()` devolve 1 registo com
`scope: "/"`, o `manifest.json` e os ícones respondem com o `content-type`
certo. **Por provar:** tudo o que só o Safari do iPhone decide — HTTPS real
(o R7 resolve a parte do certificado, falta juntar as duas pontas),
Add-to-Home-Screen, e o modo `standalone` sem a barra do Safari.

### P2 — A `ui/` corre sem Tauri (identidade)

**A interface `PomboBackend` que o PLANO.md e a Review1 desenhavam não
sobreviveu ao confronto com o código real.** `enviarFicheiro` no Tauri recebe
um caminho de ficheiro (`string`); o equivalente no browser vai receber um
`File` do `<input type=file>` — não há assinatura comum possível sem fingir
(por exemplo, aceitar `string | File` e desmontar por dentro, o que só move a
divergência para dentro da função em vez de a admitir). A decisão tomada:
`api.ts` mantém as mesmas funções exportadas de sempre — nada em `App.svelte`
mudou — e só ramificam por `eTauri()` as que têm **a mesma forma nos dois
lados**: `identidade`, `dispositivos`, `cancelar`, `buscarNoRelay`,
`aoAcontecer`. As que não têm (`enviarFicheiro`, `criarLigacao`) continuam
Tauri-only até o P5 desenhar o que fazem no browser — não é dívida técnica,
é adiar uma decisão de desenho para quando há informação para a tomar.

**`wasm-pack build --target web` não passa pelo bundler do Vite — de
propósito.** O `pagina/www/` já provava isto (R5): o glue-code do
`wasm-bindgen` é servido como ficheiro estático e importado em runtime, não
compilado pelo Vite. Copiar o padrão evitou meter o Vite a tentar perceber
`import.meta.url` dentro de um `.wasm` — mas trouxe dois problemas próprios,
os dois resolvidos sem novas dependências:

1. **`tsc` tentava resolver `"/wasm/pombo_correio_pagina.js"` como módulo do
   projeto** (falha: não existe nesse caminho, é servido à parte) mesmo com
   `/* @vite-ignore */` — esse comentário é só para o Vite, o `tsc` não o lê.
   Resolvido guardando o caminho numa `const` antes do `import()`: uma
   `string` normal (não um literal) faz o TypeScript inferir `Promise<any>`
   em vez de tentar encontrar o módulo.
2. **`InstanceType<ModuloWasm["IdentidadeWasm"]>` não compilava** — o
   `wasm-bindgen` gera `private constructor()` de propósito (só se cria por
   `gerar`/`deSegredo`), e `InstanceType` exige um construtor público.
   Resolvido com `ReturnType<...["gerar"]>`, que dá o mesmo tipo sem pedir
   acesso ao construtor.

**O tipo do módulo wasm precisa do `.d.ts` gerado para existir em disco.**
`type ModuloWasm = typeof import("../../public/wasm/....js")` só compila
depois de um `wasm-pack build` — exatamente como o `pagina/www/wasm/*.d.ts`
já exigia para quem mexesse ali. Documentado em `COMO_CORRER.md`, secção nova
("App instalável (PWA, S8)"): compilar o wasm **antes** do `npm run build`
final, senão o Vite copia `ui/public/` sem ele.

**A persistência da identidade ficou em TypeScript, não em Rust.** O
`IdentidadeWasm` só expõe os bytes secretos em hexadecimal
(`segredoSigning`/`segredoSecret`); é o `wasm-backend.ts` que lê e escreve o
`localStorage`. Dar ao wasm acesso direto ao `localStorage` (via `web-sys`)
teria sido mais código Rust para chamar uma API que o TypeScript já tem de
graça — o mesmo raciocínio do S7 R5, onde a página também decide o que fazer
com os bytes do outro lado da fronteira, não o wasm.

**Provado em Chromium/loopback:** `npx tsc --noEmit`, `npx svelte-check`, e
os testes Rust novos (`identidade_wasm_sobrevive_a_ida_e_volta_pelo_hexadecimal`,
`identidade_wasm_recusa_segredo_com_tamanho_errado`) — mas sobretudo, a app a
correr a sério: 0 erros na consola (antes do P2, `identidade()` rebentava
com "Cannot read properties of undefined (reading 'invoke')"), identidade
nova gerada e gravada em `localStorage` na primeira visita, número de
segurança real mostrado em Definições, e a mesma identidade a sobreviver a
um `location.reload()`. **Por confirmar no iPhone real:** se o `localStorage`
sobrevive à instalação como PWA no ecrã principal (o ITP do Safari expira
`localStorage` de páginas web ao fim de 7 dias sem visita — mas há isolamento
para apps instaladas; só se sabe testando).

---
