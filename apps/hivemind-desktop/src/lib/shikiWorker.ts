/**
 * Web Worker that runs shiki highlighting off the main thread.
 *
 * Communicates via postMessage:
 *   Request:  { id: number, code: string, lang?: string }
 *   Response: { id: number, html: string, language: string }
 *       or    { id: number, error: string }
 */
import { createHighlighter, type Highlighter, type BundledLanguage } from 'shiki';

let highlighterPromise: Promise<Highlighter> | null = null;
let highlighter: Highlighter | null = null;

const THEME_DARK = 'github-dark';
const THEME_LIGHT = 'github-light';

const PRELOAD_LANGS: BundledLanguage[] = [
  'javascript',
  'typescript',
  'json',
  'markdown',
  'python',
  'rust',
  'html',
  'css',
];

async function getHighlighter(): Promise<Highlighter> {
  if (highlighter) return highlighter;
  if (!highlighterPromise) {
    highlighterPromise = createHighlighter({
      themes: [THEME_DARK, THEME_LIGHT],
      langs: PRELOAD_LANGS,
    }).then((h) => {
      highlighter = h;
      return h;
    });
  }
  return highlighterPromise;
}

async function ensureLanguage(h: Highlighter, lang: string): Promise<boolean> {
  const loaded = h.getLoadedLanguages();
  if (loaded.includes(lang as BundledLanguage)) return true;
  try {
    await h.loadLanguage(lang as BundledLanguage);
    return true;
  } catch {
    return false;
  }
}

self.onmessage = async (e: MessageEvent) => {
  const { id, code, lang, themeFamily } = e.data as { id: number; code: string; lang?: string; themeFamily?: 'dark' | 'light' };
  try {
    const h = await getHighlighter();
    const effectiveLang = lang ?? 'text';
    const loaded = await ensureLanguage(h, effectiveLang);
    const useLang = loaded ? effectiveLang : 'text';
    if (useLang === 'text') await ensureLanguage(h, 'text');

    const theme = themeFamily === 'light' ? THEME_LIGHT : THEME_DARK;
    const html = h.codeToHtml(code, { lang: useLang, theme });
    self.postMessage({ id, html, language: useLang });
  } catch (err: any) {
    self.postMessage({ id, error: err?.message ?? 'highlight failed' });
  }
};
