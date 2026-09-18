// O backend de quando a `ui/` corre sem Tauri — a PWA de iOS (S8).
//
// Mesmo protocolo do `core/`, mas por trás de `fetch()` em vez de sockets
// Rust, e sem mDNS (o Safari não descobre nada na LAN) nem sistema de
// ficheiros (por isso a identidade persiste em `localStorage`, não em disco).
//
// P2 trouxe a identidade. P5 traz o envio — sempre pelo relay (mixed content
// torna o caminho direto impossível a partir de HTTPS) e sempre para um
// destinatário aprendido ao abrir um link (sem mDNS, não há descoberta).
// Receber pelo relay ao abrir a app é o P3.

import type { Eu, Evento, Vizinho } from "./api";

// O módulo wasm-pack não passa pelo bundler do Vite: é servido como ficheiro
// estático (`ui/public/wasm/`), o mesmo padrão já provado pelo `pagina/www/`
// para a página do link. `@vite-ignore` desliga a análise estática do Vite
// para este import — sem isto, o build tenta (e falha) resolver o caminho.
type ModuloWasm = typeof import("../../public/wasm/pombo_correio_pagina.js");
type IdentidadeWasm = ReturnType<ModuloWasm["IdentidadeWasm"]["gerar"]>;

let modulo: ModuloWasm | null = null;

async function carregarWasm(): Promise<ModuloWasm> {
  if (!modulo) {
    // Um caminho por variável, não um literal: é o que impede o `tsc` de
    // tentar resolver "/wasm/..." como módulo do projeto (não é — é servido
    // à parte) e falhar a compilação por um ficheiro que só existe depois do
    // `wasm-pack build`.
    const caminho = "/wasm/pombo_correio_pagina.js";
    modulo = (await import(/* @vite-ignore */ caminho)) as ModuloWasm;
    await modulo.default();
  }
  return modulo;
}

const CHAVE_IDENTIDADE = "pombo-identidade";

interface IdentidadeGuardada {
  signing: string;
  secret: string;
  nome: string;
}

/**
 * A identidade carregada, para o P3/P4/P5 assinarem pedidos ao relay com ela.
 * Não exportada diretamente — sai por `impressaoDigital()` e pelo que as
 * fases seguintes vierem a expor.
 */
let identidadeAtual: IdentidadeWasm | null = null;

function nomePorOmissao(): string {
  // Sem API de sistema para o nome do aparelho no browser — ao contrário do
  // Android (Settings) ou do desktop (hostname). Um nome genérico chega; a
  // impressão digital, não o nome, é o que identifica o dispositivo a sério.
  return /iPhone|iPad/i.test(navigator.userAgent) ? "iPhone" : "Navegador";
}

/** Carrega a identidade guardada, ou gera e guarda uma nova na primeira visita. */
export async function obterIdentidade(): Promise<Eu> {
  const m = await carregarWasm();
  if (identidadeAtual) {
    return { nome: identidadeAtual.nome(), numero_seguranca: identidadeAtual.numeroSeguranca() };
  }

  let guardada: IdentidadeGuardada | null = null;
  try {
    guardada = JSON.parse(localStorage.getItem(CHAVE_IDENTIDADE) ?? "null");
  } catch {
    // `localStorage` corrompido ou indisponível (modo privado): trata-se como
    // primeira visita, gera-se outra abaixo.
  }

  identidadeAtual = guardada
    ? m.IdentidadeWasm.deSegredo(guardada.signing, guardada.secret, guardada.nome)
    : m.IdentidadeWasm.gerar(nomePorOmissao());

  if (!guardada) {
    try {
      localStorage.setItem(
        CHAVE_IDENTIDADE,
        JSON.stringify({
          signing: identidadeAtual.segredoSigning(),
          secret: identidadeAtual.segredoSecret(),
          nome: identidadeAtual.nome(),
        } satisfies IdentidadeGuardada),
      );
    } catch {
      // Sem `localStorage` a identidade não sobrevive a um recarregar — pior
      // do que perder um clique, mas não impede a app de correr agora.
    }
  }

  // Sem `await`: não faz sentido atrasar a primeira pintura da interface por
  // uma chamada de rede que, na pior das hipóteses, deixa o dispositivo tal
  // e qual como estava antes desta fase (a pedir autorização manual).
  void resgatarConvites();

  return { nome: identidadeAtual.nome(), numero_seguranca: identidadeAtual.numeroSeguranca() };
}

/**
 * A impressão digital de 16 dígitos hexadecimais desta identidade — a que
 * entra em `--autorizados` nos dois nós de casa. O P5 vai precisar de a
 * mostrar em destaque: sem isto no ficheiro, o primeiro depósito do iPhone
 * leva 403 e ninguém sabe porquê.
 */
export async function impressaoDigital(): Promise<string> {
  await obterIdentidade();
  return identidadeAtual!.impressaoCurta();
}

// --- Conhecidos: quem esta identidade já viu, aprendido a abrir links -----
//
// Sem mDNS possível no Safari, o único jeito de a PWA aprender a identidade
// pública de um dispositivo de casa é alguém lho mostrar. O link sem
// instalação (R5) já faz exatamente isso: o `pagina.js`, ao abrir um link,
// vê a `PublicIdentity` completa de quem o criou (`Descarga.remetenteJson`) e
// grava-a aqui — mesma origem (o Pi serve os dois papéis), mesmo
// `localStorage`. **Por confirmar no iPhone real**: se uma PWA instalada
// partilha mesmo o `localStorage` com o Safari normal para a mesma origem —
// a mesma incerteza do ITP já registada no P2.

const CHAVE_CONHECIDOS = "pombo-conhecidos";

export interface Conhecido {
  nome: string;
  verifying: string;
  encrypting: string;
  /** S10 T3: o `id` do depósito/link que ensinou este conhecido, por
   * resgatar em `POST /convite` — `undefined` depois de resgatado (ou para
   * um conhecido aprendido por reciprocidade, não por link). */
  convite?: string;
}

function lerConhecidos(): Record<string, Conhecido> {
  try {
    return JSON.parse(localStorage.getItem(CHAVE_CONHECIDOS) ?? "{}");
  } catch {
    return {};
  }
}

/** Chamado pela página do link (`pagina.js`) quando abre um link a sério. */
export function aprenderConhecido(c: Conhecido): void {
  try {
    const lista = lerConhecidos();
    lista[c.verifying] = c;
    localStorage.setItem(CHAVE_CONHECIDOS, JSON.stringify(lista));
  } catch {
    // Sem `localStorage`, este dispositivo simplesmente não fica lembrado —
    // o próximo link tem de se abrir outra vez.
  }
}

/**
 * Resgata convites pendentes (S10 T3): para cada conhecido aprendido por um
 * link ainda não resgatado, pede ao relay para se autorizar a si próprio
 * com o `id` desse link como prova. Chamado uma vez por arranque, depois de
 * a identidade estar pronta para assinar — nunca bloqueia nada à volta:
 * falhar aqui só significa continuar sem autorização automática, o mesmo
 * estado de antes desta fase.
 *
 * O campo `convite` só se limpa a seguir a uma resposta que não seja um
 * erro transitório de servidor — sucesso (204), ou uma recusa definitiva
 * (403/404) que repetir nunca ia resultar. **Um `5xx`/`429` preserva o
 * convite** (S11 T1, Review4 F-16): `fetch()` só rejeita a Promise por falha
 * de rede/DNS, nunca por um código de erro HTTP — esse chega como sucesso
 * da Promise, e apagar o convite aí (a versão de antes desta fase) perdia-o
 * para sempre sem nunca ter sido resgatado a sério, mesmo com o relay só
 * momentaneamente em baixo (a meio de um reinício, disco cheio). Seguro por
 * construção repetir: `convidar()` já responde 204 a quem já está
 * autorizado, por isso um pedido que chegou a ser processado no relay mas se
 * perdeu em trânsito não duplica nada ao repetir-se.
 */
async function resgatarConvites(): Promise<void> {
  const lista = lerConhecidos();
  const pendentes = Object.values(lista).filter((c) => c.convite);
  if (pendentes.length === 0) return;

  const base = relayDeCasa();
  for (const c of pendentes) {
    let resposta: Response;
    try {
      resposta = await fetch(`${base}/convite`, {
        method: "POST",
        headers: { ...(await cabecalhosAssinados("convite", c.convite!)), "content-type": "application/json" },
        body: JSON.stringify({ id: c.convite }),
      });
    } catch {
      // Falha de rede: fica por resgatar até ao próximo arranque, ver acima.
      continue;
    }
    if (resposta.status >= 500 || resposta.status === 429) continue;
    delete c.convite;
  }
  try {
    localStorage.setItem(CHAVE_CONHECIDOS, JSON.stringify(lista));
  } catch {
    // Sem `localStorage`, o próximo arranque tenta outra vez — inofensivo.
  }
}

/**
 * Os conhecidos como `Vizinho[]`, para a lista de destinos da interface.
 * `endereco` leva as duas chaves em hexadecimal, separadas por dois pontos —
 * não é um endereço de rede (não há um: tudo passa pelo relay), é o único
 * sítio que o tipo `Vizinho` já tinha para as levar até ao envio.
 */
export const listarDispositivos = async (): Promise<Vizinho[]> =>
  Object.values(lerConhecidos()).map((c) => ({
    nome: c.nome,
    endereco: `${c.verifying}:${c.encrypting}`,
    impressao: null,
  }));

/** Nada corre ainda em wasm que se possa cancelar — P4 muda isto (a subida do P5 já corre até ao fim). */
export const cancelar = async (_id: string): Promise<void> => {};

type Ouvinte = (e: Evento) => void;
const ouvintes = new Set<Ouvinte>();

/** Onde o P3/P4 vão publicar os seus acontecimentos também. */
export function emitir(e: Evento): void {
  for (const f of ouvintes) f(e);
}

export async function aoAcontecer(f: Ouvinte): Promise<() => void> {
  ouvintes.add(f);
  return () => ouvintes.delete(f);
}

// --- P5: enviar, sempre pelo relay ------------------------------------

/**
 * O relay a usar do lado do browser. Ao contrário do Tauri (lê
 * `<dados>/relays` e tenta cada um), a PWA não tem sistema de ficheiros — e
 * como a app só é servida pelo próprio Pi (decisão do S8), a sua origem já
 * *é* um relay.
 *
 * `ponytail:` sem HA de vários relays do lado do emissor — se o Pi cair a
 * meio de uma sessão já aberta (a app em si pode continuar viva pelo cache
 * do service worker), isto falha em vez de tentar o A10. Não há UI para
 * configurar um segundo relay ainda; adicionar quando isso for pedido a
 * sério, não antes.
 */
function relayDeCasa(): string {
  return location.origin;
}

/** A palavra que marca um depósito por link — tem de bater com `CAIXA_LIGACAO` em `rede/src/relay.rs`. */
const CAIXA_LIGACAO = "ligacao";

function hexDaRaiz(manifestoJson: string): string {
  const raiz: number[] = JSON.parse(manifestoJson).root;
  return raiz.map((b) => b.toString(16).padStart(2, "0")).join("");
}

interface CabecalhosAssinados {
  identidade: string;
  instante: string;
  assinatura: string;
}

/** Overhead de selar um bloco: 24 de nonce + 16 de autenticação — `SEAL_OVERHEAD` em `core/src/crypto.rs`. */
const SEAL_OVERHEAD = 24 + 16;

/** De quantos em quantos bytes selados se faz um `PUT` (S12 T1) — bastante para poucos pedidos, pouco para não voltar a esgotar a RAM do Safari num ficheiro de vários GB. */
const TAM_PEDACO_PUT = 16 * 1024 * 1024;

/**
 * Sela e sobe um ficheiro para o relay, endereçado a `destinatario` (ou à
 * caixa de link, se `caixa` for `CAIXA_LIGACAO`). Duas passagens pelo
 * `File` — ver o desenho do P5 no PLANO.md: a primeira só resume (fecha o
 * manifesto, que o `POST /deposito` exige completo), a segunda sela e sobe.
 *
 * A segunda passagem sobe em pedaços moderados (`TAM_PEDACO_PUT`), lidos do
 * `File` sob demanda e largados da memória logo depois de cada `PUT` — sem
 * isto, um ficheiro de vídeo de vários GB juntava-se todo num único `Blob`
 * em RAM antes do envio, o que rebenta o limite Jetsam do WebKit no iOS
 * (S12 T1, Review3 F-13).
 */
async function depositarEm(
  ficheiro: File,
  destinatarioHex: { verifying: string; encrypting: string },
  caixa: string,
  /** Só para o evento `depositado` — "chega a X" na interface. `null` no
      modo link, onde não há destinatário a nomear. */
  nomeDestinatario: string | null,
): Promise<{ idRelay: string; chaveHex: string }> {
  const m = await carregarWasm();
  await obterIdentidade();
  const remetente = identidadeAtual!;
  const base = relayDeCasa();

  const chaveHex = m.gerarChaveTransferencia();
  const tamBloco = m.tamanhoBloco();

  // 1ª passagem: só resumir — o manifesto tem de fechar antes do `POST`.
  const builder = new m.ManifestoBuilderWasm();
  for (let inicio = 0; inicio < ficheiro.size; inicio += tamBloco) {
    const bloco = new Uint8Array(await ficheiro.slice(inicio, inicio + tamBloco).arrayBuffer());
    builder.push(bloco);
  }
  const manifestoJson = builder.finalizar(ficheiro.name);
  const id = hexDaRaiz(manifestoJson);

  const ofertaJson = m.construirOferta(
    remetente,
    chaveHex,
    manifestoJson,
    destinatarioHex.verifying,
    destinatarioHex.encrypting,
  );

  const numBlocos = Math.max(1, Math.ceil(ficheiro.size / tamBloco));
  const tamanhoSelado = ficheiro.size + numBlocos * SEAL_OVERHEAD;

  const agora = BigInt(Math.floor(Date.now() / 1000));
  const cab: CabecalhosAssinados = JSON.parse(
    remetente.assinarPedido("deposito", `${caixa}|${tamanhoSelado}`, agora),
  );

  emitir({
    tipo: "oferta",
    id,
    de: remetente.nome(),
    numero_seguranca: remetente.numeroSeguranca(),
    ficheiro: ficheiro.name,
    tamanho: ficheiro.size,
    novo: false,
    nome_anterior: null,
    retoma: 0,
    total: numBlocos,
  });

  try {
    const respCriar = await fetch(`${base}/deposito?para=${caixa}&tamanho=${tamanhoSelado}`, {
      method: "POST",
      headers: {
        "x-pombo-identidade": cab.identidade,
        "x-pombo-instante": cab.instante,
        "x-pombo-assinatura": cab.assinatura,
      },
      body: ofertaJson,
    });
    if (!respCriar.ok) throw new Error(`o relay recusou o depósito: ${await respCriar.text()}`);
    const { id: idRelay } = (await respCriar.json()) as { id: string };

    // 2ª passagem: selar e subir, em pedaços — nunca o ficheiro inteiro de
    // uma vez. Um `Blob` por pedaço (não um `ReadableStream` em upload) de
    // propósito — o suporte do Safari para corpos de `fetch` em fluxo é
    // incerto, e um `Blob` funciona em todo o lado.
    const tamBlocoSelado = tamBloco + SEAL_OVERHEAD;
    let proximoBloco = 0;

    for (let tentativa = 0; ; tentativa++) {
      try {
        while (proximoBloco * tamBloco < ficheiro.size || (proximoBloco === 0 && ficheiro.size === 0)) {
          const primeiroBlocoDoPedaco = proximoBloco;
          const partes: Uint8Array[] = [];
          let bytesNoPedaco = 0;

          while (
            bytesNoPedaco < TAM_PEDACO_PUT &&
            (proximoBloco * tamBloco < ficheiro.size || (proximoBloco === 0 && ficheiro.size === 0))
          ) {
            const inicio = proximoBloco * tamBloco;
            const bloco = new Uint8Array(await ficheiro.slice(inicio, inicio + tamBloco).arrayBuffer());
            if (bloco.length === 0) break;
            const selado = m.selarBloco(chaveHex, proximoBloco, bloco);
            partes.push(selado);
            bytesNoPedaco += selado.length;
            proximoBloco++;
          }
          if (partes.length === 0) break;

          const respSubir = await fetch(`${base}/deposito/${idRelay}?de=${primeiroBlocoDoPedaco * tamBlocoSelado}`, {
            method: "PUT",
            // `as BlobPart[]`: o TS pede uma `ArrayBuffer` garantidamente não
            // partilhada; o `Uint8Array` que o wasm-bindgen devolve é sempre
            // uma (nunca `SharedArrayBuffer`), só o tipo genérico não o garante.
            body: new Blob(partes as BlobPart[]),
          });
          if (!respSubir.ok) throw new Error(`a subida ao relay falhou: ${respSubir.status}`);

          emitir({ tipo: "progresso", id, feitos: proximoBloco, total: numBlocos, bytes: 0, bytes_por_seg: 0 });
        }
        break; // subida completa
      } catch (erroSubida) {
        if (tentativa + 1 >= TENTATIVAS) throw erroSubida;
        // Falhou a meio de um pedaço: o relay pode já ter recebido parte
        // dele (a escrita é em fluxo, não atómica por pedido). Perguntar-lhe
        // quantos bytes selados tem a sério, e retomar na fronteira de bloco
        // mais próxima — nunca mais atrás do que ele confirma, nunca mais à
        // frente (S12 T1, Review3 F-13; contrato em `deposito.rs::abrir_para_subir`).
        const bytesConfirmados = await (async () => {
          try {
            const e: EstadoDeposito = await (await fetch(`${base}/deposito/${idRelay}`)).json();
            return e.bytes;
          } catch {
            return 0;
          }
        })();
        proximoBloco = Math.floor(bytesConfirmados / tamBlocoSelado);
      }
    }

    emitir({ tipo: "depositado", id, para: nomeDestinatario ?? "" });
    return { idRelay, chaveHex };
  } catch (erro) {
    emitir({ tipo: "falhou", id, erro: String((erro as Error).message ?? erro) });
    throw erro;
  }
}

/**
 * Envia para um dispositivo de casa já conhecido (aprendido por um link).
 * `destino.endereco` leva `"<verifying>:<encrypting>"` — ver `listarDispositivos`.
 */
export async function enviarParaConhecido(ficheiro: File, destino: Vizinho): Promise<void> {
  const [verifying, encrypting] = destino.endereco.split(":");
  const m = await carregarWasm();
  const caixa = m.impressaoCurtaDe(verifying, encrypting);
  await depositarEm(ficheiro, { verifying, encrypting }, caixa, destino.nome);
}

function hexDe(bytes: number[]): string {
  return bytes.map((b) => b.toString(16).padStart(2, "0")).join("");
}

/**
 * Cria um link para quem não tem a app — o equivalente de `criarLigacao` no
 * Tauri. Sela para uma identidade gerada e deitada fora aqui mesmo (o mesmo
 * truque do `depositar_ligacao` nativo): fica válido, e ninguém a reabre.
 */
export async function criarLigacaoWeb(ficheiro: File): Promise<string> {
  const m = await carregarWasm();
  const ninguem = m.IdentidadeWasm.gerar("ligação");
  const publica = JSON.parse(ninguem.publicaJson()) as { verifying: number[]; encrypting: number[] };

  const { idRelay, chaveHex } = await depositarEm(
    ficheiro,
    { verifying: hexDe(publica.verifying), encrypting: hexDe(publica.encrypting) },
    CAIXA_LIGACAO,
    null,
  );

  return `${relayDeCasa()}/d/${idRelay}#${chaveHex}`;
}

// --- P3: receber pelo relay ao abrir a app -----------------------------

/**
 * Acima disto, juntar tudo em memória arrisca a aba (sem `showSaveFilePicker`
 * fiável — mesma limitação que o R5 já tinha). Mesmo número do `pagina.js`,
 * por consistência; **não medido num iPhone real**, ver o desenho do P3 no
 * PLANO.md.
 */
const LIMITE_MEMORIA = 500 * 1024 * 1024;

interface EstadoDeposito {
  oferta: string;
  bytes: number;
  tamanho: number;
  completo: boolean;
}

/** Dispara a gravação de um `Blob` sem depender de um clique — chamado ao abrir a app, não a um gesto do utilizador. */
function descarregarBlob(blob: Blob, nome: string) {
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = nome;
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 60_000);
}

async function cabecalhosAssinados(acao: string, extra: string): Promise<HeadersInit> {
  const agora = BigInt(Math.floor(Date.now() / 1000));
  const cab: CabecalhosAssinados = JSON.parse(identidadeAtual!.assinarPedido(acao, extra, agora));
  return {
    "x-pombo-identidade": cab.identidade,
    "x-pombo-instante": cab.instante,
    "x-pombo-assinatura": cab.assinatura,
  };
}

/** Quantas vezes se tenta uma descida antes de desistir — mesmo número e mesma razão do cliente nativo (`rede/src/relay.rs::TENTATIVAS`). */
const TENTATIVAS = 3;

/**
 * `idRelay` cujo `trazer()` está em curso. O temporizador de 4 s do
 * `App.svelte` não espera o `buscarNoRelay()` anterior acabar — com retoma a
 * alongar quanto tempo uma descida fica em curso num Wi-Fi mau, duas
 * chamadas concorrentes ao mesmo depósito deixam de ser só teóricas (ver o
 * desenho do Q2 no PLANO.md). Sem isto, cada uma criava o seu `Descarga`,
 * descarregava o ficheiro a dobrar e tentava apagar o depósito duas vezes.
 */
const emCurso = new Set<string>();

/** Traz um depósito completo da caixa. Devolve `true` se o ficheiro chegou. */
async function trazer(base: string, idRelay: string): Promise<boolean> {
  if (emCurso.has(idRelay)) return false;
  emCurso.add(idRelay);
  try {
    return await trazerUmaVez(base, idRelay);
  } finally {
    emCurso.delete(idRelay);
  }
}

async function trazerUmaVez(base: string, idRelay: string): Promise<boolean> {
  const m = await carregarWasm();
  const remetente = identidadeAtual!;

  const e: EstadoDeposito = await (await fetch(`${base}/deposito/${idRelay}`)).json();
  if (!e.completo) return false; // ainda a subir — fica para a próxima

  const d = m.Descarga.daCaixa(remetente, e.oferta);
  const total = d.total();
  const tamBloco = m.tamanhoBloco();

  if (d.tamanho() > LIMITE_MEMORIA) {
    emitir({
      tipo: "falhou",
      id: "",
      erro: `${d.nome()} é grande demais para este browser guardar (mais de ${Math.round(LIMITE_MEMORIA / 1e6)} MB) — abre-o num computador.`,
    });
    return false;
  }

  const lista = lerConhecidos();
  const remetenteJson = JSON.parse(d.remetenteJson()) as { verifying: number[]; encrypting: number[]; label: string };
  const impressaoRemetente = hexDe(remetenteJson.verifying);
  const novo = !(impressaoRemetente in lista);
  // Reciprocidade: quem nos depositou algo já nos ensinou a sua identidade —
  // não é preciso esperar por um link para o conhecer também neste sentido.
  aprenderConhecido({ nome: remetenteJson.label, verifying: impressaoRemetente, encrypting: hexDe(remetenteJson.encrypting) });

  const id = crypto.randomUUID();
  emitir({
    tipo: "oferta",
    id,
    de: d.de(),
    numero_seguranca: d.numeroSegurancaDe(),
    ficheiro: d.nome(),
    tamanho: d.tamanho(),
    novo,
    nome_anterior: null,
    retoma: 0,
    total,
  });

  // `d` e `partes` sobrevivem às tentativas de propósito — é o que retém o
  // progresso sem precisar de guardar nada em disco/IndexedDB: `d` já sabe
  // quantos blocos decifrou, e `feitos * (tamBloco + SEAL_OVERHEAD)` dá o
  // byte por onde retomar, o mesmo cálculo do `byte_do_bloco` nativo.
  const partes: Uint8Array[] = [];
  let feitos = 0;
  let ultimoErro = "";

  for (let tentativa = 0; tentativa < TENTATIVAS; tentativa++) {
    try {
      const offset = feitos * (tamBloco + SEAL_OVERHEAD);
      const resposta = await fetch(
        `${base}/deposito/${idRelay}/blocos`,
        offset > 0 ? { headers: { Range: `bytes=${offset}-` } } : undefined,
      );
      if (!resposta.ok || !resposta.body) throw new Error(`o relay respondeu ${resposta.status}`);

      const leitor = resposta.body.getReader();
      let buffer = new Uint8Array(0);

      for (;;) {
        const preciso = d.proximo_tamanho();
        if (preciso === 0) break;

        while (buffer.length < preciso) {
          const { done, value } = await leitor.read();
          if (done) throw new Error("a ligação ao relay caiu");
          const junto = new Uint8Array(buffer.length + value.length);
          junto.set(buffer);
          junto.set(value, buffer.length);
          buffer = junto;
        }

        // Só entra em `partes`/`feitos` depois de um bloco inteiro decifrado
        // — um bloco a meio nunca fica pendurado para a tentativa seguinte,
        // a retoma parte sempre de uma fronteira, tal como o cliente nativo.
        partes.push(d.bloco(buffer.subarray(0, preciso)));
        buffer = buffer.slice(preciso);
        feitos++;
        emitir({ tipo: "progresso", id, feitos, total, bytes: 0, bytes_por_seg: 0 });
      }

      descarregarBlob(new Blob(partes as BlobPart[]), d.nome());
      emitir({ tipo: "recebido", id, caminho: d.nome() });

      // Só se apaga depois de o ficheiro estar gravado — apagar antes trocaria
      // uma falha recuperável (voltar a descarregar) por perda, mesma razão do
      // `trazer()` nativo.
      await fetch(`${base}/deposito/${idRelay}`, { method: "DELETE" });
      return true;
    } catch (erro) {
      ultimoErro = String((erro as Error).message ?? erro);
    }
  }

  emitir({ tipo: "falhou", id, erro: ultimoErro });
  return false;
}

/** Vai ver ao relay de casa se chegou coisa. Devolve quantos ficheiros chegaram. */
export async function buscarNoRelay(): Promise<number> {
  await obterIdentidade();
  const base = relayDeCasa();

  const respCaixa = await fetch(`${base}/caixa`, { headers: await cabecalhosAssinados("caixa", "") });
  if (!respCaixa.ok) throw new Error(`não consegui falar com o relay: ${respCaixa.status}`);
  const ids: string[] = await respCaixa.json();

  let chegaram = 0;
  for (const idRelay of ids) {
    // Um depósito estragado não pode impedir os outros de chegar — mesma
    // regra do `buscar()` nativo.
    if (await trazer(base, idRelay).catch(() => false)) chegaram++;
  }
  return chegaram;
}

/** O que `ativarPush()` pode devolver — a interface decide o texto a mostrar. */
export type ResultadoPush = "ativado" | "sem_suporte" | "recusado";

function base64UrlParaBytes(base64url: string): Uint8Array {
  const preenchimento = "=".repeat((4 - (base64url.length % 4)) % 4);
  const base64 = (base64url + preenchimento).replace(/-/g, "+").replace(/_/g, "/");
  const bruto = atob(base64);
  return Uint8Array.from(bruto, (c) => c.charCodeAt(0));
}

/**
 * Pede permissão, subscreve push e regista a subscrição no relay (P4).
 *
 * A ordem importa: `Notification.requestPermission()` tem de ser a primeira
 * chamada, sem `await` nenhum antes dela no manipulador do clique — o Safari
 * só mostra o pedido dentro do mesmo gesto que carregou no botão (a mesma
 * armadilha do seletor de ficheiros em `escolherFicheiroWeb`). Depois de
 * concedida, a permissão já vale para o resto da função — `subscribe()` não
 * precisa de gesto próprio.
 */
export async function ativarPush(): Promise<ResultadoPush> {
  if (!("serviceWorker" in navigator) || !("PushManager" in window)) return "sem_suporte";

  const permissao = await Notification.requestPermission();
  if (permissao !== "granted") return "recusado";

  const base = relayDeCasa();
  const { chave }: { chave: string } = await (await fetch(`${base}/vapid`)).json();
  const registo = await navigator.serviceWorker.ready;
  const subscricao = await registo.pushManager.subscribe({
    userVisibleOnly: true,
    // O buffer nunca é partilhado a sério — só o tipo genérico do
    // `Uint8Array` não o garante ao TypeScript.
    applicationServerKey: base64UrlParaBytes(chave) as BufferSource,
  });
  const json = subscricao.toJSON() as { endpoint: string; keys: { p256dh: string; auth: string } };
  // "tipo" distingue do Q5 (FCM, Android nativo) no mesmo mapa do relay —
  // ver `relay/src/subscricoes.rs`.
  const corpo = { tipo: "web_push", ...json };

  await obterIdentidade();
  await fetch(`${base}/subscricao`, {
    method: "POST",
    headers: { ...(await cabecalhosAssinados("subscricao", json.endpoint)), "content-type": "application/json" },
    body: JSON.stringify(corpo),
  });

  return "ativado";
}
