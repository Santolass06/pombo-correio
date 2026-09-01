// Fronteira com o backend — Rust por Tauri no desktop/Android, wasm+fetch na
// PWA de iOS (S8). Nenhuma função de envio direto move bytes de ficheiros por
// aqui: no Tauri, o IPC serializa tudo para JSON, por isso o conteúdo fica do
// lado do Rust e só passam nomes, tamanhos e contagens; no wasm, os bytes
// já vão diretos do browser para o `fetch()`, sem passar por este ficheiro.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import * as wasm from "./wasm-backend";

/// `window.__TAURI_INTERNALS__` é o mesmo sinal que o `main.ts` usa para
/// decidir se regista o service worker — dentro do Tauri não há nada disso a
/// fazer, e aqui é o inverso: dentro do Tauri há sempre `invoke`.
///
/// Exportada: a interface usa-a para escolher entre o diálogo nativo e um
/// `<input type=file>` (P5) — a mesma pergunta que aqui decide `invoke` vs.
/// `wasm`.
export const eTauri = () => "__TAURI_INTERNALS__" in window;

export type Eu = { nome: string; numero_seguranca: string };

export type Vizinho = {
  nome: string;
  endereco: string;
  impressao: string | null;
};

export type Evento =
  | { tipo: "a_escutar"; porta: number; anunciado: boolean }
  | {
      tipo: "oferta";
      id: string;
      de: string;
      numero_seguranca: string;
      ficheiro: string;
      tamanho: number;
      novo: boolean;
      nome_anterior: string | null;
      retoma: number;
      total: number;
    }
  | { tipo: "destino"; nome: string; numero_seguranca: string }
  | {
      tipo: "progresso";
      id: string;
      feitos: number;
      total: number;
      bytes: number;
      bytes_por_seg: number;
    }
  | { tipo: "recebido"; id: string; caminho: string }
  | { tipo: "enviado"; id: string }
  | { tipo: "depositado"; id: string; para: string }
  | { tipo: "cancelado"; id: string }
  | { tipo: "falhou"; id: string; erro: string };

export const identidade = () => (eTauri() ? invoke<Eu>("identidade") : wasm.obterIdentidade());

/// Na PWA nunca há vizinhos por descoberta — sem mDNS possível no Safari, a
/// lista fica vazia até o P5 escolher destinos por endereço ou por conhecido.
export const dispositivos = () => (eTauri() ? invoke<Vizinho[]>("dispositivos") : wasm.listarDispositivos());

/// O destino vai inteiro, e não só o nome: o endereço que já veio da lista
/// poupa ao Rust a busca na rede antes de cada envio.
///
/// Só existe no Tauri: no browser um "ficheiro" não é um caminho, é um
/// `File` do seletor — o P5 é que traz o envio a partir do iPhone.
export const enviarFicheiro = (ficheiro: string, destino: Vizinho) =>
  invoke<void>("enviar_ficheiro", { ficheiro, destino });

/// No Android o seletor de ficheiros abre-se do lado do Rust (devolve um URI
/// content://, não um caminho), por isso o envio é um comando próprio.
export const enviarAndroid = (destino: Vizinho) =>
  invoke<void>("enviar_android", { destino });

/// Um ficheiro que outra app partilhou para cá pela folha de partilha.
export type Partilhado = { uri: string; nome: string };

/// O que outra app partilhou para cá desde a última pergunta. Consome: a
/// resposta só vem uma vez, porque o URI não vale fora desta sessão da app.
///
/// Sem folha de partilha possível no browser — vazia na PWA, tal como já era
/// vazia no desktop Tauri; não é um caminho por implementar, é sempre assim.
export const partilhaPendente = () =>
  eTauri() ? invoke<Partilhado[]>("partilha_pendente") : Promise.resolve([] as Partilhado[]);

export const enviarPartilhados = (destino: Vizinho, ficheiros: Partilhado[]) =>
  invoke<void>("enviar_partilhados", { destino, ficheiros });

/// O WebView do Android identifica-se no user agent; chega para escolher o
/// caminho de envio certo sem mais um plugin de plataforma.
export const eAndroid = () => /Android/i.test(navigator.userAgent);

export const cancelar = (id: string) => (eTauri() ? invoke<void>("cancelar", { id }) : wasm.cancelar(id));

/// Vai ver ao relay de casa se chegou coisa enquanto o aparelho esteve
/// desligado. Devolve zero — sem falhar — se não houver relay configurado.
export const buscarNoRelay = () => (eTauri() ? invoke<number>("buscar_no_relay") : wasm.buscarNoRelay());

/// Deixa um ficheiro no relay de casa e devolve um link, para quem não tem a
/// app. `null` significa que o seletor do Android foi cancelado.
///
/// No desktop passa-se o caminho já escolhido; no Android passa-se `null` e o
/// seletor abre-se do lado do Rust, que é onde se sabe lidar com `content://`.
/// Só existe no Tauri por agora — o P5 traz o equivalente para o browser.
export const criarLigacao = (ficheiro: string | null) =>
  invoke<string | null>("criar_ligacao", { ficheiro });

export const aoAcontecer = (f: (e: Evento) => void) =>
  eTauri() ? listen<Evento>("pombo", (m) => f(m.payload)) : wasm.aoAcontecer(f);

/// A impressão digital de 16 dígitos hexadecimais desta identidade, ou
/// `null` no Tauri (não há comando equivalente — o desktop/Android ainda
/// não precisam de a mostrar). É a que entra em `--autorizados`; sem ela lá,
/// o primeiro depósito da PWA leva 403.
export const impressaoDigital = () => (eTauri() ? Promise.resolve(null) : wasm.impressaoDigital());

/// Envia para um dispositivo de casa já conhecido — aprendido ao abrir um
/// link criado nesse dispositivo (ver `PLANO.md`, desenho do P5). Só existe
/// na PWA: no Tauri o envio segue por `enviarFicheiro`/`enviarAndroid`.
export const enviarParaConhecido = (ficheiro: File, destino: Vizinho) =>
  wasm.enviarParaConhecido(ficheiro, destino);

/// Cria um link a partir de um `File` do browser — o equivalente PWA de
/// `criarLigacao`.
export const criarLigacaoWeb = (ficheiro: File) => wasm.criarLigacaoWeb(ficheiro);

/// Pede notificações push e regista a subscrição no relay (P4). Só existe na
/// PWA — o desktop/Android já ficam abertos e reparam sozinhos.
export const ativarPush = () => wasm.ativarPush();
export type { ResultadoPush } from "./wasm-backend";

/// Tamanhos em unidades que uma pessoa lê sem contar zeros.
export function tamanhoLegivel(bytes: number): string {
  if (bytes < 1000) return `${bytes} B`;
  const u = ["kB", "MB", "GB", "TB"];
  let i = -1;
  let n = bytes;
  while (n >= 1000 && i < u.length - 1) {
    n /= 1000;
    i++;
  }
  return `${n.toFixed(n < 10 ? 1 : 0)} ${u[i]}`;
}
