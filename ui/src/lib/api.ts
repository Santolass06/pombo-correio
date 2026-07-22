// Fronteira com o Rust.
//
// Nenhuma destas funções move bytes de ficheiros. O IPC do Tauri serializa tudo
// para JSON, por isso o conteúdo fica sempre do lado do Rust e daqui só passam
// nomes, tamanhos e contagens.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

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
  | { tipo: "progresso"; id: string; feitos: number; total: number; bytes: number }
  | { tipo: "recebido"; id: string; caminho: string }
  | { tipo: "enviado"; id: string }
  | { tipo: "cancelado"; id: string }
  | { tipo: "falhou"; id: string; erro: string };

export const identidade = () => invoke<Eu>("identidade");
export const dispositivos = () => invoke<Vizinho[]>("dispositivos");

export const enviarFicheiro = (ficheiro: string, destino: string) =>
  invoke<void>("enviar_ficheiro", { ficheiro, destino });

export const cancelar = (id: string) => invoke<void>("cancelar", { id });

export const aoAcontecer = (f: (e: Evento) => void) =>
  listen<Evento>("pombo", (m) => f(m.payload));

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
