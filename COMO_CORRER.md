# Como correr

## Aplicação gráfica

**Compila-se pela CLI do Tauri, nunca pelo `cargo` directamente.**

```sh
cd ui
npm install          # só da primeira vez
npm run app          # desenvolvimento, com recarga automática
npm run app:build    # versão final
```

O binário fica em `target/release/pombo-correio-app`, e os pacotes `.deb`,
`.rpm` e `.AppImage` em `target/release/bundle/`.

### Para usar a sério, corre a versão final

**`npm run app` compila em modo debug, e a criptografia em debug é dezenas de
vezes mais lenta** — a transferência cai para ~2–5 MB/s em vez de ~90. Basta um
dos lados estar em debug para arrastar os dois: quem cifra devagar deixa o outro
à espera.

Para uso real, em **todas** as máquinas:

```sh
npm run app:build
./target/release/pombo-correio-app
```

O `npm run app` (dev) serve para desenvolver a interface, com recarga
automática; um perfil no `Cargo.toml` otimiza as dependências de criptografia
mesmo em debug para o tornar tolerável, mas nunca iguala a versão final.

### Porque não `cargo build`

Um `cargo build -p pombo-correio-app` compila e corre — mas abre uma janela com
**"Could not connect to localhost: Connection refused"**.

O `cargo` não sabe nada sobre a interface. Não executa o `beforeBuildCommand`
que gera o `dist/`, não embebe esses ficheiros no binário, e deixa lá o
`devUrl` a apontar para o servidor de desenvolvimento do Vite — que não está a
correr. A CLI do Tauri é que trata das três coisas.

O sintoma engana porque **o resto da aplicação funciona à mesma**: fica à
escuta, é descoberta na rede e recebe ficheiros correctamente. Só a janela é que
está vazia. Um teste que só verifique que o processo não rebenta não apanha
isto — tem de se confirmar que a interface desenha.

## Linha de comandos

Esta sim, compila-se com o `cargo`:

```sh
cargo build --release -p pombo-correio

./target/release/pombo receber
./target/release/pombo dispositivos
./target/release/pombo enviar foto.jpg "Pi da sala"
./target/release/pombo enviar foto.jpg 100.64.0.5:53318   # fora da rede local
./target/release/pombo identidade
```

Usa `-p pombo-correio`: sem isso o `--release` arrasta a árvore de dependências
do Tauri, que é enorme e demora minutos.

## Testes

```sh
cargo test --workspace     # 57 testes
cargo clippy --workspace --all-targets
cd ui && npm run check
```

## Métricas de diagnóstico

Cada transferência regista o que fez em `<dados>/metricas/eventos.jsonl` — uma
linha JSON por acontecimento. Serve para diagnosticar problemas entre
dispositivos, que quem desenvolve não consegue observar de outra forma.

O acontecimento mais útil é o `rececao_fim` (e as amostras `rececao_amostra` a
cada segundo), com o **tempo por fase**:

```
{"tipo":"rececao_fim","mb_por_s":99.4,"rede_ms":366,"decifra_ms":1146,"verifica_ms":197,"disco_ms":158,...}
```

Como ler: a fase que domina é o gargalo.
- `disco_ms` alto num Raspberry Pi → o cartão SD é o limite (troca por NVMe).
- `rede_ms` alto → a ligação, ou o outro lado a enviar devagar.
- `decifra_ms` alto → CPU (esperado que seja a maior fatia; só é problema se o
  total for lento e as outras fases forem baixas).

Para inspecionar, do lado que recebe:

```sh
tail -n 5 ~/.local/share/pt.pombo.correio/metricas/eventos.jsonl        # app
tail -n 5 ~/.local/share/pombo/metricas/eventos.jsonl                   # CLI
```

## Onde ficam os dados

| Sistema | Caminho |
|---|---|
| Linux | `~/.local/share/pombo` (CLI) · `~/.local/share/pt.pombo.correio` (app) |
| Windows | `%APPDATA%\pombo` · `%APPDATA%\pt.pombo.correio` |

Lá dentro: `identidade` (chaves privadas, `0600` em Unix), `conhecidos.json` e
`recebidos/`.

Apagar a `identidade` cria um dispositivo novo aos olhos dos outros, e o número
de segurança muda — os outros voltam a avisar que é um dispositivo desconhecido.

## Relay de casa (S7)

O relay guarda ficheiros cifrados até o destinatário os vir buscar — serve
quando os dois dispositivos não se alcançam, ou o destinatário está desligado.
**Só vê texto cifrado**: não sabe o nome do ficheiro nem o consegue abrir.

Está instalado em **duas** máquinas de casa, cada uma como serviço de
utilizador do systemd (`~/.config/systemd/user/pombo-relay.service`), com
`Restart=always` e `linger` ligado — sobem sozinhas depois de um reinício, sem
ninguém entrar na sessão.

| máquina | endereço LAN | Tailscale |
|---|---|---|
| A10 (`andresantos-M11BB`) | `192.168.1.17:53319` | `andresantos-m11bb` |
| Pi 5 (`raspberrypi`) | `192.168.1.232:53319` | `raspberrypi` |

Para instalar noutra máquina:

```sh
cargo build --release -p pombo-correio-relay
mkdir -p ~/.local/share/pombo-relay ~/.config/systemd/user
cat > ~/.config/systemd/user/pombo-relay.service <<'EOF'
[Unit]
Description=Relay do Pombo-Correio
After=network-online.target

[Service]
ExecStart=%h/CAMINHO/target/release/pombo-relay --dados %h/.local/share/pombo-relay --porta 53319
Restart=always
RestartSec=5

[Install]
WantedBy=default.target
EOF
systemctl --user daemon-reload && systemctl --user enable --now pombo-relay
sudo loginctl enable-linger $USER     # senão morre ao sair da sessão
```

Ver como está: `systemctl --user status pombo-relay`, ou
`curl http://localhost:53319/saude` (responde `pombo-relay`).

O `--autorizados` é um ficheiro com uma impressão digital por linha (a curta, 16
dígitos hexadecimais; `#` faz comentário). **Passa a ser obrigatório assim que o
relay esteja acessível de fora**: assinar prova posse de uma chave, não pertença
a esta casa, e sem a lista qualquer um na internet enche o disco. A impressão de
cada dispositivo vê-se em `pombo identidade`.

Do lado dos clientes, um ficheiro `relays` na pasta de dados, um URL por linha:

```sh
# App: ~/.local/share/pt.pombo.correio/relays · CLI: ~/.local/share/pombo/relays
cat > ~/.local/share/pombo/relays <<'EOF'
# O nó que serve a PWA primeiro (ver "App instalável" abaixo), depois LAN,
# depois Tailscale. `#` faz comentário.
http://192.168.1.232:53319
http://raspberrypi:53319
http://192.168.1.17:53319
http://andresantos-m11bb:53319
EOF
```

Ao **depositar**, fica-se com o primeiro que responder ao `/saude` — é para
isso que são dois. Ao **buscar**, passa-se por todos: um nó em baixo não pode
impedir o outro de entregar, e um depósito preso num nó que caiu fica lá à
espera de ele voltar (os blobs não se replicam, de propósito). **Sem este ficheiro não há relay nenhum**, e a app comporta-se
exatamente como antes.

**O nó que serve a PWA tem de vir primeiro nesta lista, sempre (S8).** Uma
PWA só verifica a própria origem ao buscar (`location.origin` — não há
`relays` num browser); se o "depositar" escolher outro nó primeiro, um envio
endereçado a um contacto no iPhone cai onde a PWA nunca vai olhar. Os
dispositivos nativos não sofrem com isto (verificam todos ao buscar) — só a
ordem dos dois primeiros da lista importa, e só por causa do S8.

Os depósitos duram 7 dias e são limpos de hora a hora. Um ficheiro entregue é
apagado do relay assim que está em disco do outro lado.

### Binários musl estáticos (S9 Q1)

Para instalar numa máquina sem a `glibc` certa (distros minimalistas,
containers) — `pombo-relay` sem dependência dinâmica nenhuma:

```sh
cargo install cross --locked   # uma vez; usa Docker por trás
rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl

cross build --release --target x86_64-unknown-linux-musl  -p pombo-correio-relay
cross build --release --target aarch64-unknown-linux-musl -p pombo-correio-relay
```

Os binários ficam em `target/<alvo>/release/pombo-relay`. O `Cross.toml` na
raiz fixa as imagens `:edge` — as `:0.2.5` (a tag por omissão do `cross`
0.2.5) têm uma `glibc` velha demais para o `rustc` atual e falham a compilar
o `libc`/`zeroize_derive` com erros crípticos de versão. **Provado real**: o
binário `aarch64` corre no Pi 5 (Debian/glibc, sem `musl` instalada) e o
`x86_64` corre dentro de um `alpine` (sem `glibc` nenhuma) — os dois
respondem a `/saude`, `ldd` confirma "não é um executável dinâmico" nos
dois.

## Link sem instalação (R5)

Para quem não tem a app — o browser decifra sozinho, porque a chave vai depois
do `#` e essa parte nunca chega ao servidor.

```sh
pombo ligacao ~/Imagens/ferias.jpg
# → http://192.168.1.17:53319/d/a1b2…#3f4e…   (dura 7 dias)
```

Na app é o cartão **"Para quem não tem a app"**, no fim do separador dos
dispositivos: escolhe o ficheiro, e no fim aparece o endereço com um botão de
copiar.

O relay precisa da página construída. **É preciso reconstruí-la sempre que o
`core/` mudar** — é código Rust compilado para wasm, e não se recompila sozinho
com o resto:

```sh
cd pagina && wasm-pack build --target web --out-dir www/wasm
```

Depois aponta-se o serviço à pasta, acrescentando `--pagina` ao `ExecStart`:

```
ExecStart=… pombo-relay --dados … --porta 53319 --pagina %h/pombo-pagina
```

Sem `--pagina`, `/d/{id}` responde 404 e o resto do relay funciona na mesma.

**Sem HTTPS, os ficheiros grandes não passam.** Por HTTP simples num IP da LAN a
página não é *contexto seguro*, e o browser não expõe o `showSaveFilePicker` —
sobra juntar tudo em memória, que estoira num vídeo. Não é a internet que exige
TLS aqui, é o próprio browser, dentro de casa. Como se resolve: **HTTPS pelo
Tailscale**, na secção a seguir.

Três coisas que convém saber antes de mandar um link a alguém:

- **Chega tão longe quanto o endereço.** O link leva o endereço do nó que aceitou
  o depósito. Com `192.168.1.17` lá dentro, só abre dentro de casa.
- **Só esse nó o serve.** Os blobs não se replicam; se aquela máquina estiver em
  baixo, o link não abre até ela voltar.
- **Não se gasta ao ser aberto.** Ao contrário da caixa, a página não apaga o
  depósito — quem tiver o endereço busca as vezes que quiser durante os 7 dias.

## App instalável (PWA, S8)

O iPhone não corre Tauri — corre a mesma interface Svelte (`ui/`) servida como
página, instalável no ecrã principal. Duas construções antes de servir:

```sh
cd ui && npm run build                                        # ui/dist
cd pagina && wasm-pack build --target web --out-dir ../ui/public/wasm
cd ui && npm run build                                        # outra vez, para incluir o wasm em dist/
```

A ordem importa: o wasm tem de existir em `ui/public/` **antes** do `npm run
build`, senão o Vite copia a pasta sem ele. Depois aponta-se o relay:

```
ExecStart=… pombo-relay --dados … --porta 53319 --app %h/pombo-app
```

**Só um nó de casa deve servir a app** — ver "Antes do S8" no `PLANO.md`: o
`localStorage` é por origem, e uma PWA instalada a partir de dois sítios
diferentes seria duas apps com identidades diferentes. O Pi (sempre ligado) é
quem serve; o A10 continua só como relay.

Sem `--app`, a raiz do relay responde 404 e o resto continua a funcionar —
mesma opção de sempre, sem obrigar ninguém a instalar nada.

## HTTPS pelo Tailscale

Sem TLS o browser trata a página do link como insegura e fecha-lhe a porta do
disco — os ficheiros grandes ficam de fora, mesmo dentro de casa. O Tailscale
resolve isto **sem expor nada à internet**: emite um certificado verdadeiro (Let's
Encrypt) para o nome `<máquina>.<tailnet>.ts.net`, e o `tailscale serve` termina o
TLS à frente do relay e **renova o certificado sozinho**.

Por isso o relay não fala TLS e não vai falar: seria código nosso, ficheiros de
certificado em disco e um temporizador de renovação, para fazer pior o que a
plataforma já faz.

### Uma vez, na tailnet

Na consola: **[login.tailscale.com/admin/dns](https://login.tailscale.com/admin/dns)**
→ secção **HTTPS Certificates** → **Enable HTTPS**. É um interruptor, vale para a
tailnet toda, e não abre nada ao exterior — só autoriza a emissão de certificados
para os nomes `*.ts.net` que já existem.

Confirmar sem entrar na consola:

```sh
tailscale status --json | grep -A2 CertDomains   # vazio = ainda não está ligado
```

### Em cada máquina que serve o relay

```sh
sudo tailscale set --operator=$USER          # uma vez, para não precisar de sudo a seguir
tailscale serve --bg --https=443 http://127.0.0.1:53319
tailscale serve status                       # confirma
```

A partir daqui a mesma máquina responde nos dois sítios:

| endereço | quem alcança | contexto seguro? |
|---|---|---|
| `http://192.168.1.17:53319` | qualquer um na LAN | ❌ |
| `https://andresantos-m11bb.tail455bb7.ts.net` | quem tem Tailscale | ✅ |

Para desfazer: `tailscale serve reset`.

### Se o certificado não emitir

Sintoma: o `https://` fica pendurado ~20 s e não responde. Em
`sudo journalctl -u tailscaled -f`:

```
cert("…"): starting SetDNS call...
cert("…"): did SetDNS
cert("…"): acme: WaitOrder: OrderError status "invalid"
```

Antes de culpar o Let's Encrypt, **conta os tokens do desafio**:

**e pergunta a cada servidor autoritativo, não só a um** — foi assim que se
apanhou o problema, que contra o `8.8.8.8` parecia não existir:

```sh
for ns in $(dig +short NS ts.net); do
  echo -n "$ns: "
  dig +short TXT _acme-challenge.<máquina>.<tailnet>.ts.net @$ns | wc -l
done
```

Se uns responderem `1` e outros `0`, as réplicas não concordam: o Let's Encrypt
escolhe uma para validar, calha-lhe a que não tem o token, e a autorização falha.
Não há nada a fazer deste lado.

Tenta outra vez e conta de novo. **Se o número não subir**, o `SetDNS` está a
dizer que sim sem escrever: o token daquela encomenda nunca chega ao DNS, o
Let's Encrypt não o encontra, e invalida — sempre, por muitas vezes que se
repita. Não é propagação nem limite do Let's Encrypt, é do lado do Tailscale, e
daqui não se conserta.

Antes de perder tempo, **vê [status.tailscale.com](https://status.tailscale.com)**:
já aconteceu ser um incidente deles (*Certificate Creation Issues*, agosto de
2026, com as duas máquinas de casa a falhar ao mesmo tempo). Nesse caso é só
repetir mais tarde — o `tailscale serve` fica configurado e agarra o certificado
mal ele emita.

### O ficheiro `relays` decide o que sai no link

O link leva o endereço do **primeiro relay que responder ao `/saude`**. Para os
links saírem em `https://`, o endereço do Tailscale tem de vir primeiro:

```sh
cat > ~/.local/share/pombo/relays <<'EOF'
# Primeiro o HTTPS: é o que faz o link do R5 servir para ficheiros grandes.
https://andresantos-m11bb.tail455bb7.ts.net
https://raspberrypi.tail455bb7.ts.net
# LAN como rede de segurança, se o Tailscale estiver em baixo.
http://192.168.1.17:53319
http://192.168.1.232:53319
EOF
```

O preço, dito alto: o depósito passa a subir pelo Tailscale em vez de ir direto
pela LAN. Dentro de casa o Tailscale liga-se diretamente e a diferença é pequena,
mas **não é nenhuma** — e se a ligação cair para o relay DERP, cai para ~2 MB/s.
Só afeta o degrau do relay; os envios diretos entre aparelhos continuam a escolher
a LAN primeiro.

**Isto continua a não chegar a quem não tem Tailscale.** Um link
`…ts.net` só abre de dentro da tailnet. Para o telemóvel de uma visita é preciso
decidir a exposição à internet — domínio próprio ou DDNS —, e aí o `--autorizados`
passa a ser obrigatório.
