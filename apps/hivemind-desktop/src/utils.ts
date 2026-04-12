import DOMPurify from 'dompurify';
import { marked } from 'marked';
import { invoke } from '@tauri-apps/api/core';
import type { ChatRunState, DataClass, McpConnectionStatus } from './types';

marked.setOptions({
  breaks: true,
  gfm: true,
});

export const renderMarkdown = (content: string) => {
  const result = marked.parse(content);
  if (typeof result !== 'string') return '';
  return DOMPurify.sanitize(result);
};

export const workspaceNameFromPath = (path: string) => {
  const segments = path.split(/[\\/]/).filter(Boolean);
  return segments[segments.length - 1] ?? path;
};

export const fileToBase64 = async (file: File) =>
  await new Promise<string>((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      const result = reader.result;
      if (typeof result !== 'string') {
        reject(new Error(`Failed to read ${file.name}.`));
        return;
      }
      resolve(result.split(',', 2)[1] ?? '');
    };
    reader.onerror = () => reject(reader.error ?? new Error(`Failed to read ${file.name}.`));
    reader.readAsDataURL(file);
  });

export const formatTime = (timestamp_ms: number) => {
  if (timestamp_ms == null || !Number.isFinite(timestamp_ms)) return '--';
  return new Date(timestamp_ms).toLocaleTimeString();
};

export const statusClass = (state: ChatRunState | null | undefined) => {
  switch (state) {
    case 'running':
      return 'online';
    case 'paused':
      return 'paused';
    case 'interrupted':
      return 'offline';
    default:
      return 'neutral';
  }
};

export const mcpStatusClass = (state: McpConnectionStatus) => {
  switch (state) {
    case 'connected':
      return 'complete';
    case 'error':
      return 'failed';
    case 'connecting':
      return 'processing';
    default:
      return 'neutral';
  }
};

export const formatPayload = (payload: unknown) => {
  try {
    const text = JSON.stringify(payload);
    return text.length > 140 ? `${text.slice(0, 140)}…` : text;
  } catch {
    return String(payload);
  }
};

export const riskClass = (verdict: 'clean' | 'suspicious' | 'detected') => {
  switch (verdict) {
    case 'clean':
      return 'complete';
    case 'suspicious':
      return 'processing';
    case 'detected':
      return 'failed';
    default:
      return '';
  }
};

export const formatBytes = (bytes: number): string => {
  if (!Number.isFinite(bytes) || bytes < 0) return '—';
  if (bytes === 0) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  const i = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  const value = bytes / Math.pow(1024, i);
  return `${value.toFixed(i > 0 ? 1 : 0)} ${units[i]}`;
};

export const dataClassBadge = (dc: DataClass) => {
  switch (dc) {
    case 'PUBLIC':
      return 'complete';
    case 'INTERNAL':
      return 'neutral';
    case 'CONFIDENTIAL':
      return 'processing';
    case 'RESTRICTED':
      return 'failed';
    default:
      return '';
  }
};

// ── Error helpers ────────────────────────────────────────────────

export const isNoTokenError = (msg: string) => msg.includes('[HF_NO_TOKEN]');
export const isLicenseError = (msg: string) => msg.includes('[HF_LICENSE]');
export const isAuthError = (msg: string) => isNoTokenError(msg) || isLicenseError(msg);

/**
 * Returns true if the error originates from the Tauri IPC layer being
 * unavailable (e.g. running in a plain browser during development).
 * These errors are not user-actionable and should be suppressed.
 */
export const isTauriInternalError = (error: unknown): boolean => {
  const msg = String(error);
  return (
    msg.includes('transformCallback') ||
    msg.includes('__TAURI_INTERNALS__') ||
    (msg.includes('Cannot read properties of undefined') && msg.includes("'invoke'"))
  );
};

export const extractRepoFromError = (msg: string): string | null => {
  const m = msg.match(/repo=([^\s]+)/);
  return m ? m[1] : null;
};

export const openExternal = async (url: string) => {
  try { await invoke('open_url', { url }); } catch { window.open(url, '_blank', 'noopener'); }
};

/**
 * Join path segments with forward-slashes, collapsing duplicate separators.
 * Handles a mix of `/` and `\` in the input segments.
 */
export const joinPath = (...segments: string[]): string => {
  const joined = segments.join('/');
  const isUNC = /^[\\/]{2}[^\\/]/.test(joined);
  const normalized = joined.replace(/[\\/]+/g, '/').replace(/\/$/, '') || '/';
  return isUNC ? '/' + normalized : normalized;
};

export const scrollToHfToken = () => {
  const el = document.getElementById('hf-token-input');
  if (el) { el.focus(); el.scrollIntoView({ behavior: 'smooth', block: 'center' }); }
};

/**
 * Convert a human-readable name into a kebab-case slug ID.
 * If `existingIds` contains a collision, appends `-2`, `-3`, etc.
 */
export const slugifyId = (name: string, existingIds: string[] = []): string => {
  const base = name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
    || 'untitled';

  if (!existingIds.includes(base)) return base;

  let counter = 2;
  while (existingIds.includes(`${base}-${counter}`)) counter++;
  return `${base}-${counter}`;
};
