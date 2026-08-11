# CLAUDE.md — como trabalhar neste projeto

Guia operacional, carregado a cada sessão. O plano completo, a arquitetura e o
histórico estão em **PLANO.md** (fonte de verdade do plano). Como correr, em
**COMO_CORRER.md**.

Pombo-Correio: partilha de ficheiros entre os dispositivos de casa, byte-exata,
sem serviços de terceiros. Android, iOS (PWA), macOS, Windows, Linux x64+arm64.

## Estrutura

```
core/          Rust puro. Protocolo, identidade, cifra, blocos Merkle. Compila nativo E wasm32.
rede/          Descoberta (mDNS), envio, receção. NÃO escreve no terminal — comunica por Evento.
cli/           Binário `pombo`. Um ouvinte que imprime os eventos.
relay/         Binário `pombo-relay`. Guarda bytes opacos; serve também a página do link.
pagina/        O `core/` em wasm + a página do link sem instalação (`www/`). Sem framework.
ui/            Tauri v2 + Svelte 5. A app. src-tauri/ é a ponte para a rede.
```

O CLI e a app só diferem no que fazem aos `Evento`s — a lógica é a mesma, em `rede/`.

## Comandos

```sh
# App — compila-se pela CLI do Tauri, NUNCA por cargo (senão a janela não carrega).
cd ui && npm run app          # dev (debug — cripto lenta, ver abaixo)
cd ui && npm run app:build    # release, o que se usa a sério

# CLI
cargo build --release -p pombo-correio     # -p, senão arrasta a árvore do Tauri

# Testes
cargo test -p pombo-correio-core -p pombo-correio-rede -p pombo-correio
cargo clippy --workspace --all-targets     # tem de dar zero
```

## Workflow

Sprints; commits por unidade coerente; tag `sprint-N` no fim. PLANO.md atualizado
à medida. Implementação **local** (o remoto não tem rota para a LAN/Pi/Tailscale).
Cada lógica não-trivial deixa **um teste que falha se ela partir**.

**As três regras que custaram caro (segue-as sempre):**

1. **Desenhar antes de implementar** o que envolve duas partes, protocolo, estado
   ou timing. Escreve em PLANO.md, por extenso, o caminho feliz **e os de falha**,
   e responde a: **o que é que cada lado observa, e quando?**. Foi saltar isto que
   fez o cancelamento nascer partido três vezes.
2. **Pré-mortem, uma linha, para mudanças de protocolo:** *se esta ligação morrer
   agora, o que conclui cada lado?*.
3. **Honestidade sobre onde algo foi provado.** Loopback, build debug e uma-só-
   máquina **não** provam comportamento distribuído nem desempenho real. Distingue
   sempre "verificado em loopback" de "validado entre máquinas". Uma ponta
   distribuída só fecha com validação em máquinas reais.

## Armadilhas já pagas (não repetir)

- **Velocidade a 2 MB/s = build debug.** A cripto sem otimizações é dezenas de
  vezes mais lenta. Para uso real, release nos dois lados. (O perfil dev já
  otimiza as dependências, mas não iguala.)
- **Verificar a app = provar que a interface carregou**, não só que o processo
  não rebentou. O `identidade` só é chamado pelo frontend — serve de prova.
- **Os bytes dos ficheiros nunca atravessam o IPC do Tauri** (serializa para
  JSON e colapsa). O Rust é dono do ficheiro; à WebView só vão eventos.
- **Escolha de IP: LAN antes do Tailscale.** A gama CGNAT (100.64/10) fica abaixo
  de qualquer LAN, senão o tráfego passa pelo relay do Tailscale a ~2 MB/s.
- **Cancelamento vai por canal à parte** (`POST /cancelar/{id}`), não pela ligação
  da transferência, que está a ser cortada.
- **`/tmp` é tmpfs (RAM).** Testes com ficheiros grandes vão para `/var/tmp`.
- **Testes com timing usam `#[tokio::test(flavor = "multi_thread")]`**, senão a
  tarefa que dispara o cancelamento fica esfomeada e o teste fica flaky.
- **Android: `screencap` mostra a WebView a preto** (camada de hardware não
  capturável). Para ver o que está renderizado, inspeciona o DOM pelo DevTools
  (`adb forward` ao socket `webview_devtools_remote_<pid>`, depois DevTools
  Protocol por WebSocket — o Node 22 tem `WebSocket` nativo).
- **Android bloqueia ligações de entrada para apps sob restrição de bateria**
  (OxygenOS e afins): ping passa, porta da app dá timeout. Precisa de foreground
  service. Para testar à mão: `adb shell dumpsys deviceidle whitelist +<pkg>`.
- **Android `app_data_dir()` é a raiz `/data/data/<pkg>/`**, não `files/`. Os
  recebidos ficam em `<raiz>/recebidos/`. Inspeciona com `adb shell run-as <pkg>`.
- **mDNS no Android precisa de `MulticastLock`**; sem ele não há descoberta, só
  IP direto.
- **A lista de dispositivos já traz o endereço — não o redescubras.** A janela
  do mDNS é de 3 s fixos (não sai mais cedo ao encontrar), e pagá-la antes de
  cada envio adiava tudo, no Android até o seletor de ficheiros abrir. Usa
  `resolver_com_dica`. Guardar endereços obriga a confirmar a impressão digital
  de quem lá está: um IP pode ter mudado de dono desde a listagem.
- **Lentidão sentida: mede de que lado está antes de mexer.** No Android o
  logcat dá o corte exato — o toque (`input_interaction`), o disparo do
  `Intent` (`START u0`) e o desenho (`wm_activity_launch_time`). Duas vezes
  neste projeto a causa "óbvia" era a errada.
- **Um intent de partilha fabricado por `adb` não prova nada.** O `shell` não
  consegue conceder um URI do MediaStore a outra app, por isso
  `am start -a SEND --grant-read-uri-permission` chega sem permissão e a
  abertura falha — igualzinho a um bug nosso. Confirma a concessão em
  `dumpsys activity permissions | grep -A3 <pkg>`; se não aparecer lá, o teste é
  que está partido. Para testar a sério, conduz a app **Ficheiros** por
  `uiautomator` (selecionar → Partilhar → Pombo-Correio).
- **`adb shell input tap` só acerta com a app em primeiro plano.** Em segundo
  plano o DevTools continua a responder (a WebView sobrevive), o que dá a
  ilusão de que a app está à frente, mas o toque vai para o ecrã inicial.
  Confirma com `dumpsys activity activities | grep topResumedActivity`. E uma
  caixa de diálogo esquecida por cima (um "Abrir com") come o toque sem a
  `uiautomator` a mostrar. **Só o seletor SAF exige gesto real**; para o resto,
  carrega no botão pelo DOM via DevTools, que é determinista.

## Idioma

Tudo em português de Portugal — código, comentários, mensagens, interface.
