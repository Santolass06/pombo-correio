<script lang="ts">
  import { onMount } from "svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import {
    aoAcontecer,
    buscarNoRelay,
    cancelar,
    criarLigacao,
    criarLigacaoWeb,
    dispositivos,
    eAndroid,
    eTauri,
    enviarAndroid,
    enviarFicheiro,
    enviarParaConhecido,
    enviarPartilhados,
    identidade,
    impressaoDigital,
    partilhaPendente,
    tamanhoLegivel,
    type Eu,
    type Evento,
    type Partilhado,
    type Vizinho,
  } from "./lib/api";

  type Transferencia = {
    id: string;
    ficheiro: string;
    tamanho: number;
    feitos: number;
    total: number;
    estado: "a_correr" | "pronto" | "no_relay" | "cancelado" | "falhou";
    detalhe?: string;
    /** Velocidade em bytes por segundo, medida pelo Rust e só mostrada aqui. */
    velocidade: number;
    /** Só na primeira vez que se vê um dispositivo. */
    porVerificar?: { de: string; numero: string };
  };

  let eu = $state<Eu | null>(null);
  let vizinhos = $state<Vizinho[]>([]);
  let aProcurar = $state(false);
  let escuta = $state<{ porta: number; anunciado: boolean } | null>(null);
  let transferencias = $state<Transferencia[]>([]);
  let aba = $state<"dispositivos" | "transferencias" | "definicoes">("dispositivos");
  let tema = $state<"auto" | "light" | "dark">("auto");
  let aviso = $state("");
  /** Ficheiros que outra app partilhou para cá e ainda não têm destino. */
  let partilha = $state<Partilhado[]>([]);
  /** O último link criado, à espera de ser copiado. */
  let ligacao = $state("");
  let aCriarLigacao = $state(false);
  let copiado = $state(false);
  /** Só não-nulo na PWA — é lá que o relay pode recusar o primeiro depósito
      com 403 até esta impressão entrar em `--autorizados` (S8 P5). */
  let impressao = $state<string | null>(null);
  let seletorFicheiro: HTMLInputElement;

  const emCurso = $derived(transferencias.filter((t) => t.estado === "a_correr"));

  onMount(() => {
    let vivo = true;
    const parar = { fn: () => {} };

    (async () => {
      eu = await identidade();
      procurar();
      verPartilha();
      buscarNoRelay().catch(() => {});
      impressaoDigital().then((i) => (impressao = i));
      const off = await aoAcontecer(trata);
      if (vivo) parar.fn = off;
      else off();
    })();

    // Uma partilha com a app já aberta não remonta nada: o Android traz a
    // atividade à frente e a WebView volta a ficar visível. É o único aviso que
    // temos de que há coisa nova à espera.
    document.addEventListener("visibilitychange", aoVoltar);

    // Re-procura periódica: a descoberta mDNS não é instantânea, e um
    // dispositivo que só liga a app depois de nós não apareceria até se carregar
    // em "Procurar". Enquanto estamos no ecrã de dispositivos, refresca-se em
    // silêncio para a lista se ir preenchendo sozinha.
    const timer = setInterval(() => {
      if (aba === "dispositivos" && !aProcurar) procurar(true);
    }, 4000);

    return () => {
      vivo = false;
      clearInterval(timer);
      document.removeEventListener("visibilitychange", aoVoltar);
      parar.fn();
    };
  });

  function aoVoltar() {
    if (document.visibilityState === "visible") verPartilha();
  }

  /** Pergunta ao Rust se outra app partilhou ficheiros para cá. */
  async function verPartilha() {
    try {
      const nova = await partilhaPendente();
      if (nova.length === 0) return;
      partilha = nova;
      // Quem partilhou já escolheu o *quê*; o que falta é o *para onde*.
      aba = "dispositivos";
      procurar(true);
    } catch (erro) {
      aviso = String(erro);
    }
  }

  function trata(e: Evento) {
    if (e.tipo === "a_escutar") {
      escuta = { porta: e.porta, anunciado: e.anunciado };
      return;
    }

    if (e.tipo === "destino") {
      aviso = `A falar com ${e.nome}`;
      return;
    }

    if (e.tipo === "oferta") {
      // Um envio e a receção correspondente partilham identificador — é a raiz
      // de Merkle do ficheiro — por isso substitui-se em vez de acumular.
      transferencias = [
        {
          id: e.id,
          ficheiro: e.ficheiro,
          tamanho: e.tamanho,
          feitos: e.retoma,
          total: e.total,
          estado: "a_correr",
          velocidade: 0,
          detalhe: e.retoma > 0 ? "Retomada — o progresso anterior foi mantido" : undefined,
          porVerificar: e.novo ? { de: e.de, numero: e.numero_seguranca } : undefined,
        },
        ...transferencias.filter((t) => t.id !== e.id),
      ];
      // Menos quando o que está a acontecer é criar um link: o link nasce no
      // separador dos dispositivos, e saltar daqui punha a pessoa a olhar para
      // a barra de progresso enquanto o endereço — a única coisa que ela veio
      // buscar — aparecia atrás. Apanhado no tablet, e só lá: no computador o
      // ficheiro é tão rápido que o salto quase não se vê.
      if (!aCriarLigacao) aba = "transferencias";
      return;
    }

    const conhecida = transferencias.some((t) => t.id === e.id);

    transferencias = transferencias.map((t) => {
      if (t.id !== e.id) return t;
      if (e.tipo === "progresso")
        return { ...t, feitos: e.feitos, velocidade: e.bytes_por_seg };
      if (e.tipo === "recebido")
        return { ...t, estado: "pronto" as const, feitos: t.total, detalhe: e.caminho };
      if (e.tipo === "enviado") return { ...t, estado: "pronto" as const, feitos: t.total };
      // "No relay" não é "entregue": o ficheiro está num servidor de casa e a
      // outra pessoa ainda nem sabe dele. Dizer "enviado" aqui faria alguém
      // desligar o telemóvel convencido de que já estava.
      if (e.tipo === "depositado")
        return {
          ...t,
          estado: "no_relay" as const,
          feitos: t.total,
          detalhe: e.para
            ? `Guardado em casa — chega a ${e.para} quando ele abrir a app`
            : "Guardado em casa — abre com o link",
        };
      if (e.tipo === "cancelado")
        return { ...t, estado: "cancelado" as const, detalhe: "Cancelado — o que já passou fica guardado" };
      if (e.tipo === "falhou") return { ...t, estado: "falhou" as const, detalhe: e.erro };
      return t;
    });

    // Uma falha antes de existir transferência — destino inalcançável, por
    // exemplo — não tem a que se colar, e mostra-se à parte.
    if (e.tipo === "falhou" && !conhecida) aviso = e.erro;
  }

  async function procurar(quieto = false) {
    // Uma refrescagem em silêncio não acende o "À procura...", para a lista não
    // piscar de 4 em 4 segundos; só o pedido explícito do utilizador o faz.
    if (!quieto) aProcurar = true;
    try {
      vizinhos = await dispositivos();
    } catch (erro) {
      if (!quieto) aviso = String(erro);
    } finally {
      aProcurar = false;
    }
  }

  /**
   * Abre o seletor de ficheiros do browser e espera pela escolha.
   *
   * Tem de ser chamado ainda dentro do clique — sem `await` antes — senão
   * perde-se o gesto do utilizador e o browser recusa o seletor (a mesma
   * regra que o `showSaveFilePicker` do R5 já respeitava).
   */
  function escolherFicheiroWeb(): Promise<File | null> {
    return new Promise((resolver) => {
      seletorFicheiro.value = "";
      seletorFicheiro.onchange = () => resolver(seletorFicheiro.files?.[0] ?? null);
      seletorFicheiro.click();
    });
  }

  async function enviarPara(destino: Vizinho) {
    try {
      if (partilha.length > 0) {
        // Só se larga a partilha depois de aceite: se o destino não estiver
        // disponível, os ficheiros continuam à espera para se tentar outro.
        await enviarPartilhados(destino, partilha);
        partilha = [];
        return;
      }
      if (eAndroid()) {
        // O seletor abre-se do lado do Rust; devolve logo, o envio segue por eventos.
        await enviarAndroid(destino);
        return;
      }
      if (!eTauri()) {
        const ficheiro = await escolherFicheiroWeb();
        if (!ficheiro) return;
        await enviarParaConhecido(ficheiro, destino);
        return;
      }
      const escolhido = await open({ multiple: false, title: "O que queres enviar?" });
      if (typeof escolhido !== "string") return;
      await enviarFicheiro(escolhido, destino);
    } catch (erro) {
      aviso = String(erro);
    }
  }

  /** Cria um link para quem não tem a app instalada. */
  async function fazerLigacao() {
    aCriarLigacao = true;
    ligacao = "";
    copiado = false;
    try {
      if (!eTauri()) {
        const ficheiro = await escolherFicheiroWeb();
        if (!ficheiro) return;
        ligacao = await criarLigacaoWeb(ficheiro);
        return;
      }
      // No Android o seletor abre-se do lado do Rust, que é onde se sabe lidar
      // com `content://`; aqui passa-se `null` e espera-se.
      let escolhido: string | null = null;
      if (!eAndroid()) {
        const r = await open({ multiple: false, title: "Para que ficheiro queres um link?" });
        if (typeof r !== "string") return;
        escolhido = r;
      }
      ligacao = (await criarLigacao(escolhido)) ?? "";
    } catch (erro) {
      aviso = String(erro);
    } finally {
      aCriarLigacao = false;
    }
  }

  async function copiarLigacao() {
    try {
      await navigator.clipboard.writeText(ligacao);
      copiado = true;
    } catch {
      // Sem permissão de área de transferência sobra selecionar à mão, e é
      // preciso dizê-lo — um botão que não faz nada é pior do que não existir.
      aviso = "Não consegui copiar. Seleciona o endereço e copia à mão.";
    }
  }

  function debito(t: Transferencia): string {
    return t.velocidade > 0 ? `${tamanhoLegivel(t.velocidade)}/s` : "";
  }

  function percentagem(t: Transferencia): number {
    return Math.round((t.feitos / Math.max(t.total, 1)) * 100);
  }

  function mudarTema() {
    tema = tema === "auto" ? "dark" : tema === "dark" ? "light" : "auto";
    const raiz = document.documentElement;
    if (tema === "auto") raiz.removeAttribute("data-theme");
    else raiz.setAttribute("data-theme", tema);
  }
</script>

<div class="app">
  <!-- Sem Tauri (P5): o seletor nativo não existe, isto faz as vezes dele.
       Escondido, não `display:none` — alguns browsers recusam abrir por
       script um seletor que nunca chegou a ganhar layout. -->
  <input bind:this={seletorFicheiro} type="file" style="position:absolute;width:1px;height:1px;opacity:0" />

  <nav class="nav linha">
    <strong class="nav-brand">Pombo-Correio</strong>
    <button class="btn btn-ghost" onclick={mudarTema}>
      Tema: {tema === "auto" ? "automático" : tema === "dark" ? "escuro" : "claro"}
    </button>
  </nav>

  <div class="abas seg" role="tablist">
    <button
      class="seg-opt btn" role="tab" aria-selected={aba === "dispositivos"}
      onclick={() => (aba = "dispositivos")}>Dispositivos</button>
    <button
      class="seg-opt btn" role="tab" aria-selected={aba === "transferencias"}
      onclick={() => (aba = "transferencias")}>
      Transferências{emCurso.length ? ` (${emCurso.length})` : ""}
    </button>
    <button
      class="seg-opt btn" role="tab" aria-selected={aba === "definicoes"}
      onclick={() => (aba = "definicoes")}>Definições</button>
  </div>

  {#if aviso}
    <div class="card elev-sm linha" style="margin-bottom: var(--space-4)">
      <span class="cresce">{aviso}</span>
      <button class="btn btn-ghost" onclick={() => (aviso = "")}>Fechar</button>
    </div>
  {/if}

  {#if aba === "dispositivos"}
    {#if partilha.length > 0}
      <div class="card elev-sm linha" style="margin-bottom: var(--space-4)">
        <div class="cresce">
          <div class="card-title trunca">
            {partilha.length === 1 ? partilha[0].nome : `${partilha.length} ficheiros`}
          </div>
          <div class="card-meta text-muted">Escolhe para onde enviar.</div>
        </div>
        <button class="btn btn-ghost" onclick={() => (partilha = [])}>Cancelar</button>
      </div>
    {/if}

    <div class="linha" style="margin-bottom: var(--space-4)">
      <h2 class="cresce" style="margin:0">Os teus dispositivos</h2>
      <!-- Envolvido de propósito: passado direto, o `onclick` entrega o evento
           do rato como `quieto`, que sendo um objeto conta como verdadeiro — o
           botão nunca acendia "À procura..." e os erros morriam calados. -->
      <button class="btn btn-secondary" onclick={() => procurar()} disabled={aProcurar}>
        {aProcurar ? "À procura..." : "Procurar"}
      </button>
    </div>

    {#if vizinhos.length === 0}
      <div class="card elev-sm vazio">
        <h3>Ainda não há nenhum dispositivo à espera</h3>
        <p class="text-muted">
          Abre o Pombo-Correio noutro aparelho, na mesma rede. Aparece aqui sozinho.
        </p>
      </div>
    {:else}
      <div class="grelha-dispositivos">
        {#each vizinhos as v (v.endereco)}
          <div class="card elev-sm dispositivo">
            <div class="cresce">
              <div class="card-title trunca">{v.nome}</div>
              <!-- Na PWA `endereco` não é um endereço de rede (não há um: tudo
                   passa pelo relay) — leva as chaves públicas em hex, uso
                   interno do envio (ver wasm-backend.ts). Mostrar isso seria
                   ruído; o que interessa dizer é como este dispositivo se
                   tornou conhecido. -->
              <div class="card-meta text-muted trunca">{eTauri() ? v.endereco : "Aprendido por link"}</div>
            </div>
            <button class="btn btn-primary btn-block-mobile" onclick={() => enviarPara(v)}>
              {partilha.length > 0 ? "Enviar aqui" : "Enviar ficheiro"}
            </button>
          </div>
        {/each}
      </div>
    {/if}

    <!-- O degrau para quem está fora deste mundo: sem app, sem conta, sem nada.
         Fica no fim de propósito — é o caminho de exceção, não o normal. -->
    <div class="card elev-sm" style="margin-top: var(--space-6)">
      <div class="linha">
        <div class="cresce">
          <div class="card-title">Para quem não tem a app</div>
          <div class="card-meta text-muted">
            Um endereço que abre o ficheiro em qualquer browser. Dura 7 dias.
          </div>
        </div>
        <button class="btn btn-secondary btn-block-mobile" onclick={fazerLigacao} disabled={aCriarLigacao}>
          {aCriarLigacao ? "A preparar..." : "Criar link"}
        </button>
      </div>

      {#if ligacao}
        <div class="linha" style="margin-top: var(--space-4)">
          <!-- Só de leitura, mas selecionável: se a área de transferência
               falhar, copiar à mão tem de continuar a ser possível. -->
          <input class="cresce" readonly value={ligacao} style="min-width:0" />
          <button class="btn btn-primary" onclick={copiarLigacao}>
            {copiado ? "Copiado ✓" : "Copiar"}
          </button>
        </div>
        <div class="card-meta text-muted" style="margin-top: var(--space-2)">
          Quem tiver este endereço abre o ficheiro. Manda-o por onde quiseres.
        </div>
      {/if}
    </div>

  {:else if aba === "transferencias"}
    {#if transferencias.length === 0}
      <div class="card elev-sm vazio">
        <h3>Nada a acontecer</h3>
        <p class="text-muted">Os envios e as receções aparecem aqui.</p>
      </div>
    {:else}
      <div class="lista">
        {#each transferencias as t (t.id)}
          <div class="card elev-sm">
            <div class="linha">
              <div class="cresce">
                <div class="card-title trunca">{t.ficheiro}</div>
                <div class="card-meta text-muted">{tamanhoLegivel(t.tamanho)}</div>
              </div>
              <span class="tag {t.estado === 'a_correr' || t.estado === 'pronto' || t.estado === 'no_relay' ? 'tag-accent' : 'tag-outline'}">
                {t.estado === "pronto"
                  ? "Concluído"
                  : t.estado === "no_relay"
                    ? "Guardado em casa"
                    : t.estado === "falhou"
                    ? "Falhou"
                    : t.estado === "cancelado"
                      ? "Cancelado"
                      : "A passar"}
              </span>
            </div>

            {#if t.porVerificar}
              <div class="card-body" style="margin-top: var(--space-3)">
                <strong>Dispositivo novo — {t.porVerificar.de}</strong>
                <p class="text-muted" style="margin:var(--space-1) 0">
                  Confirma este número com quem enviou, lendo-o em voz alta:
                </p>
                <div class="numero-seguranca">{t.porVerificar.numero}</div>
              </div>
            {/if}

            {#if t.estado === "a_correr"}
              <div class="barra" style="margin-top: var(--space-3)">
                <div style="width: {percentagem(t)}%"></div>
              </div>
              <div class="linha" style="margin-top: var(--space-2)">
                <div class="card-meta text-muted cresce">
                  {percentagem(t)}% · {t.feitos}/{t.total} blocos{debito(t) ? ` · ${debito(t)}` : ""}
                </div>
                <button class="btn btn-ghost" onclick={() => cancelar(t.id)}>Cancelar</button>
              </div>
            {/if}

            {#if t.detalhe}
              <div class="card-meta text-muted trunca" style="margin-top: var(--space-2)">
                {t.detalhe}
              </div>
            {/if}
          </div>
        {/each}
      </div>
    {/if}

  {:else}
    <h2 style="margin-top:0">Definições</h2>
    <div class="lista">
      <div class="card elev-sm">
        <div class="card-title">Este dispositivo</div>
        <div class="card-meta text-muted">{eu?.nome ?? "..."}</div>
      </div>

      <div class="card elev-sm">
        <div class="card-title">Número de segurança</div>
        <p class="text-muted" style="margin:var(--space-1) 0">
          Serve para confirmar, ao telefone, que é mesmo contigo que estão a falar.
        </p>
        <div class="numero-seguranca">{eu?.numero_seguranca ?? ""}</div>
      </div>

      {#if impressao}
        <div class="card elev-sm">
          <div class="card-title">Impressão digital</div>
          <p class="text-muted" style="margin:var(--space-1) 0">
            Antes de conseguires guardar ou enviar ficheiros, alguém tem de
            acrescentar isto ao <code>--autorizados</code> dos servidores de
            casa — sem isso, o relay recusa este dispositivo.
          </p>
          <div class="numero-seguranca">{impressao}</div>
        </div>
      {/if}

      <div class="card elev-sm">
        <div class="card-title">Receção</div>
        <div class="card-meta text-muted">
          {#if escuta}
            À escuta na porta {escuta.porta}.
            {escuta.anunciado
              ? "Visível na rede local."
              : "Sem anúncio na rede — os outros têm de escrever o endereço."}
          {:else}
            A arrancar...
          {/if}
        </div>
      </div>
    </div>
  {/if}
</div>
