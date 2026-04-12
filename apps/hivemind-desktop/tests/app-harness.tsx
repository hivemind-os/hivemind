import { render } from 'solid-js/web';
import { installTauriMocks } from './mocks/tauri-mock';

// Install Tauri mocks BEFORE importing the App (which calls invoke on import)
installTauriMocks();

// Dynamic import so mocks are in place first
import('../src/App').then(({ default: App }) => {
  render(() => <App />, document.getElementById('root')!);
});

// Heartbeat for freeze detection
const el = document.createElement('span');
el.id = 'heartbeat';
el.dataset.alive = 'true';
el.style.cssText = 'position:fixed;bottom:2px;right:2px;font-size:10px;color:#585b70;z-index:99999;';
el.textContent = '♥';
document.body.appendChild(el);

setInterval(() => {
  el.dataset.ts = String(Date.now());
}, 500);
