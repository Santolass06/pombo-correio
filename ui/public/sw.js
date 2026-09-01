// Service worker mínimo: torna a app instalável e dá-lhe uma casca offline.
//
// Estratégia de rede-primeiro-cai-para-cache: cada pedido tenta a rede, e só
// se ela falhar (aparelho sem ligação) é que se serve o que ficou em cache da
// última vez que correu. O contrário — cache primeiro — serviria sempre a
// versão antiga da app até um "recarregar forçado", que ninguém da família
// sabe fazer.
//
// O `CACHE` muda de nome a cada instalação nova: é o que faz a `activate`
// deitar fora o cache da versão anterior em vez de o ir acumulando.
const CACHE = "pombo-v1";

self.addEventListener("install", (e) => {
  // Não pré-carrega nada: os nomes dos ficheiros do build (`assets/App.a1b2.js`)
  // mudam a cada versão, e adivinhá-los aqui duplicava o que o Vite já sabe.
  // A casca offline enche-se sozinha à medida que a app é usada.
  self.skipWaiting();
});

self.addEventListener("activate", (e) => {
  e.waitUntil(
    caches
      .keys()
      .then((chaves) => Promise.all(chaves.filter((c) => c !== CACHE).map((c) => caches.delete(c))))
      .then(() => self.clients.claim()),
  );
});

self.addEventListener("fetch", (e) => {
  // Só pedidos GET da mesma origem: o relay tem rotas com efeitos (`/deposito`
  // por POST) e falar com outro relay pela HA vai para outra origem — nenhum
  // dos dois deve passar por um cache de casca de app.
  const url = new URL(e.request.url);
  if (e.request.method !== "GET" || url.origin !== location.origin) return;

  e.respondWith(
    fetch(e.request)
      .then((resposta) => {
        const copia = resposta.clone();
        caches.open(CACHE).then((c) => c.put(e.request, copia));
        return resposta;
      })
      .catch(() => caches.match(e.request).then((c) => c || caches.match("/"))),
  );
});
