import { createSignal, Show } from 'solid-js';
import DOMPurify from 'dompurify';
import { Marked, type Tokens } from 'marked';
import { highlightCode } from '../lib/shikiHighlighter';
import { useAbortableEffect } from '~/lib/useAbortableEffect';
import { openExternal } from '../utils';
import CodeViewer from './CodeViewer';

export interface MarkdownViewerProps {
  source: string;
  /** Called when the user clicks a navigable link. */
  onNavigate?: (workspace_path: string) => void;
  /** Set of all workspace-relative file paths (for resolving links). */
  workspacePaths?: Set<string>;
  /** Workspace-relative path of the current file. */
  currentFilePath?: string;
  /** Theme family ('dark' | 'light') for syntax highlighting. */
  themeFamily?: 'dark' | 'light';
}

const MarkdownViewer = (props: MarkdownViewerProps) => {
  const [mode, setMode] = createSignal<'rendered' | 'source'>('rendered');
  const [renderedHtml, setRenderedHtml] = createSignal('');
  const [loading, setLoading] = createSignal(true);

  // Render markdown with syntax-highlighted code fences
  useAbortableEffect(async (signal) => {
    const source = props.source;
    setLoading(true);

    try {
      const html = await renderMarkdownWithHighlighting(source, props.themeFamily);
      if (signal.aborted) return;
      setRenderedHtml(html);
    } catch {
      if (signal.aborted) return;
      // Fallback: basic marked rendering
      const fallback = new Marked({ breaks: true, gfm: true });
      const basicHtml = DOMPurify.sanitize(await fallback.parse(source));
      if (signal.aborted) return;
      setRenderedHtml(basicHtml);
    } finally {
      if (!signal.aborted) setLoading(false);
    }
  });

  // Handle clicks on links within the rendered markdown
  const handleClick = (e: MouseEvent) => {
    const target = e.target as HTMLElement;
    const anchor = target.closest('a');
    if (!anchor) return;

    const href = anchor.getAttribute('href');
    if (!href) return;

    // Open external links in the user's default browser
    if (href.startsWith('http://') || href.startsWith('https://') || href.startsWith('//')) {
      e.preventDefault();
      e.stopPropagation();
      void openExternal(href.startsWith('//') ? `https:${href}` : href);
      return;
    }

    // Try to resolve as a workspace path
    if (props.workspacePaths && props.currentFilePath) {
      const currentDir = props.currentFilePath.includes('/')
        ? props.currentFilePath.slice(0, props.currentFilePath.lastIndexOf('/'))
        : '';
      const resolved = normalizePath(currentDir ? `${currentDir}/${href}` : href);

      if (props.workspacePaths.has(resolved)) {
        e.preventDefault();
        e.stopPropagation();
        props.onNavigate?.(resolved);
        return;
      }
    }

    // Prevent navigation for unresolvable relative links
    if (!href.startsWith('http') && !href.startsWith('#')) {
      e.preventDefault();
    }
  };

  return (
    <div class="markdown-viewer">
      <div class="markdown-viewer-toggle">
        <button
          class={`markdown-toggle-btn ${mode() === 'rendered' ? 'active' : ''}`}
          onClick={() => setMode('rendered')}
        >
          Rendered
        </button>
        <button
          class={`markdown-toggle-btn ${mode() === 'source' ? 'active' : ''}`}
          onClick={() => setMode('source')}
        >
          Source
        </button>
      </div>

      <Show when={mode() === 'rendered'}>
        <Show
          when={!loading()}
          fallback={
            <div class="code-viewer-loading" style="padding: 16px;">
              <div class="code-viewer-skeleton" />
              <div class="code-viewer-skeleton" style="width: 60%;" />
              <div class="code-viewer-skeleton" style="width: 80%;" />
            </div>
          }
        >
          <div
            class="markdown-viewer-content markdown-body"
            innerHTML={renderedHtml()}
            onClick={handleClick}
          />
        </Show>
      </Show>

      <Show when={mode() === 'source'}>
        <CodeViewer
          code={props.source}
          language="markdown"
          onNavigate={props.onNavigate}
          workspacePaths={props.workspacePaths}
          currentFilePath={props.currentFilePath}
          themeFamily={props.themeFamily}
        />
      </Show>
    </div>
  );
};

/**
 * Render markdown with shiki-highlighted code fences.
 *
 * Strategy: pre-collect all fenced code blocks from the source via regex,
 * highlight them with shiki, then use a custom marked renderer that emits
 * the pre-highlighted HTML for each code block in order.
 */
async function renderMarkdownWithHighlighting(source: string, themeFamily?: 'dark' | 'light'): Promise<string> {
  // Pre-highlight every fenced code block
  const codeBlockHighlights: string[] = [];
  const codeRegex = /```(\w*)\r?\n([\s\S]*?)```/g;
  let match: RegExpExecArray | null;

  while ((match = codeRegex.exec(source)) !== null) {
    const lang = match[1] || undefined;
    const code = match[2];
    try {
      const result = await highlightCode(code, lang, themeFamily);
      codeBlockHighlights.push(`<div class="markdown-code-block">${result.html}</div>`);
    } catch {
      codeBlockHighlights.push(`<pre><code>${escapeHtml(code)}</code></pre>`);
    }
  }

  // Render with marked, replacing code blocks with pre-highlighted HTML
  let highlightIdx = 0;
  const instance = new Marked({
    breaks: true,
    gfm: true,
    renderer: {
      code({ text }: Tokens.Code) {
        if (highlightIdx < codeBlockHighlights.length) {
          return codeBlockHighlights[highlightIdx++];
        }
        return `<pre><code>${escapeHtml(text)}</code></pre>`;
      },
    },
  });

  const html = await instance.parse(source);
  return DOMPurify.sanitize(html, {
    ADD_TAGS: ['span'],
    ADD_ATTR: ['class', 'style'],
  });
}

function normalizePath(path: string): string {
  const parts = path.split('/');
  const result: string[] = [];
  for (const part of parts) {
    if (part === '.' || part === '') continue;
    if (part === '..') {
      if (result.length === 0) return '';
      result.pop();
    } else {
      result.push(part);
    }
  }
  return result.join('/');
}

function escapeHtml(text: string): string {
  return text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

export default MarkdownViewer;
