import { render } from 'solid-js/web';
import { installTauriBridge } from './mocks/tauri-api-bridge';

// Read daemon connection info from query params
const params = new URLSearchParams(window.location.search);
const daemon_url = params.get('daemon_url') || 'http://localhost:9876';
const authToken = params.get('authToken') || 'test-token';

// Install the Tauri API bridge BEFORE importing the App
installTauriBridge(daemon_url, authToken);

// Dynamic import so the bridge is in place first
import('../src/App').then(({ default: App }) => {
  render(() => <App />, document.getElementById('root')!);
});

// Heartbeat for freeze detection (same pattern as app-harness.tsx)
const el = document.createElement('span');
el.id = 'heartbeat';
el.dataset.alive = 'true';
el.style.cssText = 'position:fixed;bottom:2px;right:2px;font-size:10px;color:#585b70;z-index:99999;';
el.textContent = '♥';
document.body.appendChild(el);

setInterval(() => {
  el.dataset.ts = String(Date.now());
}, 500);
