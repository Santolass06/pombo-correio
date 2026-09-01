import { mount } from 'svelte'
import './app.css'
import App from './App.svelte'

const app = mount(App, {
  target: document.getElementById('app')!,
})

// Só fora do Tauri: dentro da app nativa não há nada a instalar nem a servir
// offline, e um service worker ali só seria uma camada de cache a mais para
// se enganar. `window.__TAURI_INTERNALS__` é o mesmo sinal que o `api.ts`
// usa para escolher o backend.
if (!('__TAURI_INTERNALS__' in window) && 'serviceWorker' in navigator) {
  navigator.serviceWorker.register('/sw.js')
}

export default app
