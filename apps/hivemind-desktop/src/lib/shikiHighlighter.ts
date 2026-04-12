/**
 * Main-thread API for shiki highlighting. All actual tokenization happens
 * in a Web Worker so the UI never freezes.
 */

let worker: Worker | null = null;
let nextId = 0;
const pending = new Map<number, {
  resolve: (v: HighlightResult) => void;
  reject: (e: Error) => void;
}>();

/** Reject all pending highlight requests and tear down the worker. */
function rejectAllPending(reason: string) {
  const err = new Error(reason);
  for (const [id, p] of pending) {
    p.reject(err);
  }
  pending.clear();
}

function destroyWorker() {
  if (worker) {
    try { worker.terminate(); } catch { /* best-effort */ }
    worker = null;
  }
}

function createWorker(): Worker {
  const w = new Worker(new URL('./shikiWorker.ts', import.meta.url), { type: 'module' });

  w.onmessage = (e: MessageEvent) => {
    const { id, html, language, error } = e.data;
    const p = pending.get(id);
    if (!p) return;
    pending.delete(id);
    if (error) {
      p.reject(new Error(error));
    } else {
      p.resolve({ html, language });
    }
  };

  // Handle worker-level errors (e.g. WASM init failure, script load error).
  // Without this the error propagates unhandled and can crash the WebView.
  w.onerror = (event: ErrorEvent) => {
    event.preventDefault(); // prevent the error from propagating to the window
    console.error('[shiki] worker error:', event.message);
    rejectAllPending('shiki worker error: ' + (event.message ?? 'unknown'));
    destroyWorker();
  };

  return w;
}

function getWorker(): Worker {
  if (!worker) {
    worker = createWorker();
  }
  return worker;
}

export interface HighlightResult {
  /** The highlighted HTML string (shiki wraps in <pre><code>). */
  html: string;
  /** The language that was actually used for highlighting. */
  language: string;
}

/**
 * Highlight source code in a Web Worker. Returns a promise that resolves
 * once the worker finishes tokenising.
 */
export function highlightCode(
  code: string,
  lang: string | undefined,
  themeFamily?: 'dark' | 'light',
): Promise<HighlightResult> {
  return new Promise((resolve, reject) => {
    const id = nextId++;
    pending.set(id, { resolve, reject });
    try {
      getWorker().postMessage({ id, code, lang, themeFamily });
    } catch (err) {
      // Worker may have been terminated or failed to create
      pending.delete(id);
      destroyWorker();
      reject(err instanceof Error ? err : new Error(String(err)));
    }
  });
}

/**
 * Pre-initialize the shiki Web Worker so WASM compilation happens early,
 * before the user opens a file. This reduces resource contention when
 * the first highlight request arrives.
 */
export function warmUpHighlighter(): void {
  try {
    getWorker();
  } catch {
    // Silently ignore — the worker will be created on first use
  }
}

export const THEME_DARK = 'github-dark';
export const THEME_LIGHT = 'github-light';
