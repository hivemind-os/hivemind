/**
 * Wrapper around daemon API calls that routes through Tauri IPC.
 *
 * In production Tauri builds the webview origin is https://tauri.localhost,
 * so direct fetch() to the HTTP daemon (http://127.0.0.1:*) is blocked as
 * mixed content.  This module proxies all daemon requests through a generic
 * `daemon_fetch` Tauri command which makes the HTTP call from the Rust
 * backend, bypassing browser security restrictions.
 *
 * The returned Response-like object mirrors the subset of the fetch Response
 * API used by callers (ok, status, json(), text()).
 */

import { invoke } from '@tauri-apps/api/core';

export interface DaemonResponse {
  ok: boolean;
  status: number;
  statusText: string;
  json(): Promise<any>;
  text(): Promise<string>;
}

function extractPathFromUrl(input: RequestInfo | URL): string {
  const url = typeof input === 'string' ? input : input instanceof URL ? input.href : input.url;
  // If already a path (starts with /), use as-is.
  if (url.startsWith('/')) return url;
  // Extract path + query from full URL.
  try {
    const parsed = new URL(url);
    return parsed.pathname + parsed.search;
  } catch {
    return url;
  }
}

export async function authFetch(input: RequestInfo | URL, init?: RequestInit): Promise<DaemonResponse> {
  const path = extractPathFromUrl(input);
  const method = (init?.method ?? 'GET').toUpperCase();

  let body: any = null;
  if (init?.body) {
    if (typeof init.body === 'string') {
      try { body = JSON.parse(init.body); } catch { body = null; }
    }
  }

  try {
    const data = await invoke<any>('daemon_fetch', { method, path, body });
    return {
      ok: true,
      status: 200,
      statusText: 'OK',
      json: async () => data,
      text: async () => (data != null ? JSON.stringify(data) : ''),
    };
  } catch (e: any) {
    const errStr = String(e);
    // Try to extract HTTP status code from error strings like "404 Not Found: ..."
    const statusMatch = errStr.match(/^(\d{3})\s/);
    const status = statusMatch ? parseInt(statusMatch[1], 10) : 500;
    return {
      ok: false,
      status,
      statusText: errStr,
      json: async () => { try { return JSON.parse(errStr); } catch { return { error: errStr }; } },
      text: async () => errStr,
    };
  }
}
