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
# LAN primeiro (rápida), Tailscale a seguir (funciona de fora, e safa se o
# DHCP mudar o IP). `#` faz comentário.
http://192.168.1.17:53319
http://192.168.1.232:53319
http://andresantos-m11bb:53319
http://raspberrypi:53319
EOF
```

Ao **depositar**, fica-se com o primeiro que responder ao `/saude` — é para
isso que são dois. Ao **buscar**, passa-se por todos: um nó em baixo não pode
impedir o outro de entregar, e um depósito preso num nó que caiu fica lá à
espera de ele voltar (os blobs não se replicam, de propósito). **Sem este ficheiro não há relay nenhum**, e a app comporta-se
exatamente como antes.

Os depósitos duram 7 dias e são limpos de hora a hora. Um ficheiro entregue é
apagado do relay assim que está em disco do outro lado.

## Link sem instalação (R5)

Para quem não tem a app — o browser decifra sozinho, porque a chave vai depois
do `#` e essa parte nunca chega ao servidor.

```sh
pombo ligacao ~/Imagens/ferias.jpg
# → http://192.168.1.17:53319/d/a1b2…#3f4e…   (dura 7 dias)
```

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

Três coisas que convém saber antes de mandar um link a alguém:

- **Chega tão longe quanto o endereço.** O link leva o endereço do nó que aceitou
  o depósito. Com `192.168.1.17` lá dentro, só abre dentro de casa.
- **Só esse nó o serve.** Os blobs não se replicam; se aquela máquina estiver em
  baixo, o link não abre até ela voltar.
- **Não se gasta ao ser aberto.** Ao contrário da caixa, a página não apaga o
  depósito — quem tiver o endereço busca as vezes que quiser durante os 7 dias.
