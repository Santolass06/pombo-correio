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

## Relay (S7, experimental)

```sh
cargo build --release -p pombo-correio-relay
target/release/pombo-relay --porta 53319 --dir /var/lib/pombo-relay   # no Pi / A10
```

Cada dispositivo aponta para ele: na app, um ficheiro `relay` na pasta de dados
(linha 1 `host:porta`, linha 2 token de registo, opcional); no CLI,
`pombo receber --relay H:P` e `pombo enviar F DESTINO --relay H:P --impressao FP`.
O relay só vê bytes cifrados. Sem TLS por agora — **não o exponhas à internet**.
