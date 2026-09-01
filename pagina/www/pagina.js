// A página do link, do lado do browser.
//
// Este ficheiro não decide nada sobre legitimidade: não conhece a chave, não vê
// os hashes, e não sabe abrir um bloco. Pergunta ao WASM de quantos bytes é o
// bloco seguinte, entrega-os, e recebe conteúdo já verificado. Tudo o que possa
// dizer "isto está bem" vive do outro lado da fronteira, em Rust — o mesmo
// código que corre no Linux e no Android.

import init, { Descarga } from './wasm/pombo_correio_pagina.js';

const id = location.pathname.split('/').filter(Boolean).pop();
const chave = decodeURIComponent(location.hash.slice(1));
const conteudo = document.getElementById('conteudo');

/** Acima disto, juntar tudo em memória é pedir para o separador morrer. */
const LIMITE_MEMORIA = 500 * 1024 * 1024;

/** De quantos em quantos milissegundos se pergunta se a subida já acabou. */
const ESPERA = 3000;

const html = (s) => { conteudo.innerHTML = s; };
const escapar = (s) => s.replace(/[<>&"]/g, (c) =>
  ({ '<': '&lt;', '>': '&gt;', '&': '&amp;', '"': '&quot;' })[c]);

function tamanhoLegivel(bytes) {
  const unidades = ['B', 'KB', 'MB', 'GB'];
  let n = bytes, i = 0;
  while (n >= 1000 && i < unidades.length - 1) { n /= 1000; i++; }
  return `${n < 10 && i > 0 ? n.toFixed(1) : Math.round(n)} ${unidades[i]}`;
}

function falhar(mensagem, sugestao = '') {
  html(`<p class="ficheiro erro">${escapar(mensagem)}</p>
        ${sugestao ? `<p class="pormenor">${escapar(sugestao)}</p>` : ''}`);
}

/** Junta duas fatias. O buffer nunca passa de um bloco, por isso isto é barato. */
function juntar(a, b) {
  const fora = new Uint8Array(a.length + b.length);
  fora.set(a);
  fora.set(b, a.length);
  return fora;
}

/**
 * Onde escrever o que se vai decifrando.
 *
 * Com o `showSaveFilePicker` os bytes vão para disco à medida que chegam, e o
 * tamanho do ficheiro deixa de importar. Sem ele — Safari e Firefox — junta-se
 * tudo em memória, que é o que obriga ao aviso lá em cima. **Tem de ser chamado
 * ainda dentro do clique**: um `await` antes disto quebra o gesto do utilizador
 * e o browser recusa abrir o seletor.
 */
async function abrirDestino(nome) {
  if (window.showSaveFilePicker) {
    const puxador = await window.showSaveFilePicker({ suggestedName: nome });
    const escrita = await puxador.createWritable();
    return {
      escrever: (b) => escrita.write(b),
      fechar: async () => { await escrita.close(); return null; },
      // Deixar meio ficheiro em disco com o nome final seria pior do que não o
      // deixar: parece completo e não é.
      desistir: () => escrita.abort().catch(() => {}),
    };
  }

  const pedacos = [];
  return {
    escrever: (b) => { pedacos.push(b); },
    fechar: async () => URL.createObjectURL(new Blob(pedacos)),
    desistir: () => { pedacos.length = 0; },
  };
}

/** Estado do depósito, ou `null` se já não existe. */
async function estado() {
  const r = await fetch(`/deposito/${id}`);
  if (r.status === 404) return null;
  if (!r.ok) throw new Error(`o servidor respondeu ${r.status}`);
  return r.json();
}

/** Põe o botão outra vez utilizável, com o motivo por baixo. */
function correuMal(botao, e) {
  botao.disabled = false;
  botao.textContent = 'Tentar outra vez';
  const aviso = document.createElement('p');
  aviso.className = 'pormenor erro';
  aviso.textContent = String(e.message ?? e);
  botao.after(aviso);
}

async function descarregar(d, botao) {
  // Antes de qualquer `await`, senão perde-se o gesto do clique.
  let destino;
  try {
    destino = await abrirDestino(d.nome());
  } catch (e) {
    // Fechar a caixa de gravação não é uma falha — é mudar de ideias. Sem este
    // ramo, desistir deixava a página muda e uma rejeição por tratar.
    if (e.name !== 'AbortError') correuMal(botao, e);
    return;
  }

  const total = d.total();
  botao.disabled = true;
  botao.textContent = 'A descarregar…';
  const barra = document.createElement('progress');
  barra.max = total;
  botao.after(barra);

  try {
    const r = await fetch(`/deposito/${id}/blocos`);
    if (!r.ok) throw new Error(`o servidor respondeu ${r.status}`);

    const leitor = r.body.getReader();
    let buffer = new Uint8Array(0);

    for (let feitos = 0; ; feitos++) {
      const preciso = d.proximo_tamanho();
      if (preciso === 0) break;

      while (buffer.length < preciso) {
        const { done, value } = await leitor.read();
        if (done) {
          throw new Error('a ligação acabou antes do ficheiro — volta a abrir o link');
        }
        buffer = juntar(buffer, value);
      }

      // Se isto rebentar, não se escreve nada: o bloco não passou nas contas.
      await destino.escrever(d.bloco(buffer.subarray(0, preciso)));
      buffer = buffer.slice(preciso);
      barra.value = feitos + 1;
    }

    const url = await destino.fechar();
    barra.remove();

    if (url) {
      // Caminho da memória: só agora existe um ficheiro, e é preciso um clique
      // do browser para ele ir para as transferências.
      const a = document.createElement('a');
      a.href = url;
      a.download = d.nome();
      a.click();
      setTimeout(() => URL.revokeObjectURL(url), 60_000);
    }

    botao.textContent = 'Guardado ✓';
  } catch (e) {
    await destino.desistir();
    barra.remove();
    correuMal(botao, e);
  }
}

function mostrar(d) {
  const grande = !window.showSaveFilePicker && d.tamanho() > LIMITE_MEMORIA;

  html(`
    <p class="ficheiro">${escapar(d.nome())}</p>
    <p class="pormenor">${tamanhoLegivel(d.tamanho())} · de ${escapar(d.de())}</p>
    ${grande ? `<p class="aviso">Este ficheiro é grande e este browser tem de o
      juntar todo em memória. Num computador com o Chrome ou o Edge corre
      melhor.</p>` : ''}
    <button id="guardar">Guardar ficheiro</button>
  `);

  const botao = document.getElementById('guardar');
  botao.addEventListener('click', () => descarregar(d, botao));
}

/**
 * Espera que a subida acabe.
 *
 * Descarregar um depósito a meio ficaria sem bytes no meio de um bloco, o que é
 * indistinguível de corrupção para quem está a ver. Mais vale dizer que ainda
 * está a chegar.
 */
async function esperarQueEsteja(e) {
  while (!e.completo) {
    const feito = Math.floor((e.bytes / e.tamanho) * 100);
    html(`<p class="ficheiro">Ainda a ser enviado</p>
          <p class="pormenor">${feito}% — esta página atualiza-se sozinha.</p>`);
    await new Promise((r) => setTimeout(r, ESPERA));
    e = await estado();
    if (!e) throw new Error('quem enviou cancelou este link');
  }
  return e;
}

async function principal() {
  if (!chave) {
    return falhar(
      'Este link está incompleto.',
      'Falta a parte depois do #. Alguns chats cortam-na — pede o link outra vez, colado inteiro.',
    );
  }

  await init();

  let e;
  try {
    e = await estado();
  } catch (erro) {
    return falhar('Não consegui falar com o servidor.', String(erro.message ?? erro));
  }

  if (!e) {
    return falhar('Este link já não existe.', 'Os links duram 7 dias. Pede outro a quem to enviou.');
  }

  try {
    e = await esperarQueEsteja(e);
    const d = new Descarga(chave, e.oferta);
    mostrar(d);
    aprenderRemetente(d);
  } catch (erro) {
    // Mensagens do WASM: chave errada, assinatura que não bate, manifesto
    // adulterado. Todas dizem o que é em português.
    falhar(String(erro.message ?? erro));
  }
}

/**
 * Grava a identidade pública de quem criou este link em `localStorage`
 * (S8 P5) — é assim que a PWA (`ui/`, servida pela mesma origem) aprende o
 * primeiro dispositivo de casa a que pode depositar de volta, sem mDNS e
 * sem uma segunda cerimónia de confiança: só abrir um link já existente.
 *
 * Mesma chave e formato que `ui/src/lib/wasm-backend.ts` usa — os dois lados
 * não partilham módulo (são dois bundles servidos à parte), só o contrato.
 * Falhar aqui nunca impede a descarga: é só menos um dispositivo lembrado.
 */
function aprenderRemetente(d) {
  try {
    const remetente = JSON.parse(d.remetenteJson());
    const lista = JSON.parse(localStorage.getItem('pombo-conhecidos') ?? '{}');
    lista[hex(remetente.verifying)] = {
      nome: remetente.label,
      verifying: hex(remetente.verifying),
      encrypting: hex(remetente.encrypting),
    };
    localStorage.setItem('pombo-conhecidos', JSON.stringify(lista));
  } catch {
    // localStorage indisponível, ou remetenteJson por alguma razão não deu —
    // não é motivo para a descarga, que já mostrou o botão, falhar também.
  }
}

function hex(bytes) {
  return Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
}

principal();
