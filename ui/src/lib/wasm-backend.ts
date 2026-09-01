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

/** Nada corre ainda em wasm que se possa cancelar — P3/P4 mudam isto. */
export const cancelar = async (_id: string): Promise<void> => {};

export const buscarNoRelay = async (): Promise<number> => {
  throw new Error("a caixa do relay ainda não está ligada nesta versão — chega no P3");
};

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

/**
 * Sela e sobe um ficheiro para o relay, endereçado a `destinatario` (ou à
 * caixa de link, se `caixa` for `CAIXA_LIGACAO`). Duas passagens pelo
 * `File` — ver o desenho do P5 no PLANO.md: a primeira só resume (fecha o
 * manifesto, que o `POST /deposito` exige completo), a segunda sela e sobe.
 *
 * `ponytail:` uma só tentativa, sem retoma — ao contrário do cliente nativo
 * (3 tentativas, `?de=N`). O relay já suporta retomar; falta o lado do
 * browser voltar a tentar quando o `PUT` falhar a meio. Upgrade quando um
 * envio a sério do iPhone mostrar que faz falta.
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

    // 2ª passagem: selar e subir. Um `Blob` (não um `ReadableStream` em
    // upload) de propósito — o suporte do Safari para corpos de `fetch` em
    // fluxo é incerto, e um `Blob` funciona em todo o lado.
    const partes: Uint8Array[] = [];
    for (let i = 0; i * tamBloco < ficheiro.size || (i === 0 && ficheiro.size === 0); i++) {
      const inicio = i * tamBloco;
      const bloco = new Uint8Array(await ficheiro.slice(inicio, inicio + tamBloco).arrayBuffer());
      if (bloco.length === 0) break;
      partes.push(m.selarBloco(chaveHex, i, bloco));
    }

    const respSubir = await fetch(`${base}/deposito/${idRelay}?de=0`, {
      method: "PUT",
      // `as BlobPart[]`: o TS pede uma `ArrayBuffer` garantidamente não
      // partilhada; o `Uint8Array` que o wasm-bindgen devolve é sempre uma
      // (nunca `SharedArrayBuffer`), só o tipo genérico é que não o garante.
      body: new Blob(partes as BlobPart[]),
    });
    if (!respSubir.ok) throw new Error("a subida ao relay falhou");

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
