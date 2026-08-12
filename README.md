# Pombo-Correio

Partilha de ficheiros entre os aparelhos de casa. **Byte-exata** — a foto que
chega é o mesmo ficheiro que saiu, sem recompressão — e **sem serviços de
terceiros**: os bytes vão diretos entre os dois aparelhos, e quando não podem ir
diretos passam por um servidor de casa que só vê texto cifrado.

Existe porque não havia forma nenhuma de mandar uma foto do Android para o PC
Windows sem a estragar: o Quick Share precisa de Bluetooth que o PC não tem, e o
WhatsApp recomprime tudo.

## Estado

| | |
|---|---|
| Linux x86-64 e ARM64 | ✅ app e CLI, validado entre o A10 e o Pi 5 |
| Android | ✅ app, partilha pela folha do sistema |
| Windows | ✅ app e CLI (122 testes; a app recebeu 40 MB byte a byte) · ⚠️ o ecrã por confirmar a olho |
| macOS | ⚠️ nunca compilado |
| iOS / iPadOS | ⬜ PWA, por fazer (S8) |
| Relay de casa | ✅ dois nós, ativo-ativo, serviço do systemd |
| Link sem instalação | ✅ o browser decifra sozinho |
| Pela internet | ⬜ nada provado — só LAN e Tailscale |

Detalhe fase a fase, com o que está e o que não está provado: **[PLANO.md](PLANO.md)**.

## Depressa

```sh
cd ui && npm install && npm run app:build     # a app
./target/release/pombo-correio-app

cargo build --release -p pombo-correio        # a linha de comandos
./target/release/pombo dispositivos
./target/release/pombo enviar foto.jpg "Pi da sala"
```

Instruções a sério — serviços, relays, métricas, onde ficam os dados — em
**[COMO_CORRER.md](COMO_CORRER.md)**.

## Como funciona

Quem envia tenta por esta ordem, e desce sozinho:

| | Caminho | Quando | Passa por servidor? |
|---|---|---|---|
| 1 | **LAN direta** — descoberta por mDNS, depois HTTP | Os dois na mesma rede | Não |
| 2 | **Tailscale** — mesmo protocolo, endereço `100.x` | Os dois com Tailscale | Não |
| 3 | **Relay de casa** — depósito cifrado, 7 dias | O outro está desligado | Sim, cifrado |
| 4 | **Link** — `…/d/<id>#<chave>` | O outro não tem a app | Sim, cifrado |

O ficheiro é partido em blocos de 1 MiB com uma árvore de Merkle BLAKE3, e cada
bloco é selado com XChaCha20-Poly1305. A chave da transferência é embrulhada para
a chave pública de quem recebe (X25519); a identidade de cada aparelho é uma
chave Ed25519, e o número de segurança que a app mostra é a impressão digital
dela.

Daqui saem três propriedades que interessam:

- **Retoma.** Cortar a rede a meio não recomeça o ficheiro — continua no bloco
  onde ia, nos dois sentidos.
- **Integridade.** Cada bloco é verificado contra o manifesto antes de tocar no
  disco. Um byte trocado não passa.
- **O relay não lê nada.** Guarda bytes opacos com um identificador aleatório.
  Não sabe o nome do ficheiro (vai dentro do manifesto selado), nem o tamanho em
  claro, nem distingue um vídeo de um PDF. Comprometê-lo não expõe ficheiro
  nenhum — é a razão de ele poder ficar exposto.

No link sem instalação a chave viaja depois do `#`, a parte do URL que o browser
**nunca envia ao servidor**. Quem abre descarrega bytes cifrados e decifra-os no
próprio aparelho, com o mesmo código Rust compilado para WebAssembly.

## Estrutura

```
core/     Protocolo, identidade, cifra, blocos Merkle. Rust puro — compila nativo E wasm32.
rede/     Descoberta, envio, receção, cliente do relay. Nunca escreve no terminal.
cli/      Binário `pombo`.
relay/    Binário `pombo-relay`. Guarda bytes opacos; serve também a página do link.
pagina/   O `core/` em wasm + a página do link. Sem framework.
ui/       Tauri v2 + Svelte 5. A app, para desktop e Android.
```

O CLI e a app só diferem no que fazem aos acontecimentos — a lógica está toda em
`rede/`, escrita uma vez.

## O que ainda não é verdade

Escrito aqui de propósito, para não se confundir "compila" com "funciona":

- **Nada foi provado pela internet.** Tudo o que está validado passou-se na rede
  local ou pelo Tailscale.
- **Sem HTTPS, o link não serve para ficheiros grandes.** Por HTTP simples a
  página não é *contexto seguro* e o browser não deixa gravar em disco à medida —
  sobra juntar tudo em memória, que estoira num vídeo. É o primeiro da fila.
- **Em Windows ninguém olhou para o ecrã.** A app corre lá, a WebView2 arranca e
  recebeu 40 MB byte a byte — mas tudo isso se mediu por SSH, que não vê o
  ambiente de trabalho. Que a interface *desenhe*, e que se consiga enviar a
  partir dela, continua por confirmar. **macOS nunca correu de todo.**
- **Houve um bloqueio de entrada pela LAN em Windows que ficou por explicar.**
  Cheguei a atribuí-lo à NordVPN; estava errado. Com a Nord ligada na mesma
  máquina, a entrada pela LAN passou a funcionar e não se reproduz. Fica como
  está: aconteceu, a causa não é conhecida.
- **O Android apaga o GPS das fotos antes de a app as ler**, e o ficheiro fica
  do mesmo tamanho. Enquanto a app não pedir `ACCESS_MEDIA_LOCATION`, "byte-exata"
  não é verdade para fotos com localização — e a app não tem como dar por isso.
- A página do link só foi aberta em Chromium. O Safari do iPhone é onde se espera
  que parta primeiro.

## Idioma

Tudo em português de Portugal — código, comentários, mensagens e interface.

## Licença

MIT.
