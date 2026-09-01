// O backend de quando a `ui/` corre sem Tauri — a PWA de iOS (S8).
//
// Mesmo protocolo do `core/`, mas por trás de `fetch()` em vez de sockets
// Rust, e sem mDNS (o Safari não descobre nada na LAN) nem sistema de
// ficheiros (por isso a identidade persiste em `localStorage`, não em disco).
//
// P2 só traz a identidade — é o que todo o resto precisa para assinar. Enviar
// e receber pelo relay entram no P5 e no P3.

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

/** Sem mDNS possível no Safari (decisão do S8): nunca há vizinhos por descoberta. */
export const listarDispositivos = async (): Promise<Vizinho[]> => [];

/** Nada corre ainda em wasm que se possa cancelar — P3/P4/P5 mudam isto. */
export const cancelar = async (_id: string): Promise<void> => {};

export const buscarNoRelay = async (): Promise<number> => {
  throw new Error("a caixa do relay ainda não está ligada nesta versão — chega no P3");
};

type Ouvinte = (e: Evento) => void;
const ouvintes = new Set<Ouvinte>();

/** Onde o P3/P4/P5 vão publicar os seus acontecimentos, quando existirem. */
export function emitir(e: Evento): void {
  for (const f of ouvintes) f(e);
}

export async function aoAcontecer(f: Ouvinte): Promise<() => void> {
  ouvintes.add(f);
  return () => ouvintes.delete(f);
}
