import { createSignal, onCleanup, Show, untrack } from 'solid-js';
import { highlightCode } from '../lib/shikiHighlighter';
import { useAbortableEffect } from '~/lib/useAbortableEffect';
import DOMPurify from 'dompurify';

/** Debounce delay (ms) before re-highlighting after a keystroke. */
const HIGHLIGHT_DEBOUNCE = 300;

export interface HighlightedEditorProps {
  value: string;
  language: string | undefined;
  onInput: (value: string) => void;
  /** Extra CSS class applied to the root wrapper */
  class?: string;
  /** Placeholder text shown when value is empty */
  placeholder?: string;
  /** Theme family ('dark' | 'light') for syntax highlighting. */
  themeFamily?: 'dark' | 'light';
}

/**
 * A syntax-highlighted editor built with a transparent `<textarea>` overlaid
 * on shiki-highlighted output. Text is visible immediately; highlighting
 * appears progressively in the background.
 */
const HighlightedEditor = (props: HighlightedEditorProps) => {
  const [highlightedHtml, setHighlightedHtml] = createSignal('');
  const [ready, setReady] = createSignal(false);
  const [highlightTrigger, setHighlightTrigger] = createSignal(0);
  let textareaRef: HTMLTextAreaElement | undefined;
  let preRef: HTMLPreElement | undefined;
  let debounceTimer: ReturnType<typeof setTimeout> | undefined;

  onCleanup(() => clearTimeout(debounceTimer));

  // Sync scroll between textarea and highlighted pre
  const syncScroll = () => {
    if (textareaRef && preRef) {
      preRef.scrollTop = textareaRef.scrollTop;
      preRef.scrollLeft = textareaRef.scrollLeft;
    }
  };

  // Highlight when trigger fires (on mount, language change, or after debounce)
  useAbortableEffect(async (signal) => {
    const _trigger = highlightTrigger(); // track debounced trigger
    const _lang = props.language; // track language dependency
    const _theme = props.themeFamily; // track theme dependency
    const code = untrack(() => props.value); // read without tracking to preserve debounce
    setReady(false);

    try {
      const result = await highlightCode(code, props.language, props.themeFamily);
      if (signal.aborted) return;
      const parser = new DOMParser();
      const doc = parser.parseFromString(result.html, 'text/html');
      const codeEl = doc.querySelector('code');
      setHighlightedHtml(codeEl?.innerHTML ?? escapeHtml(code));
      setReady(true);
    } catch {
      // Highlighting failed — textarea text stays visible as-is
    }
  });

  const handleInput = (e: InputEvent) => {
    const val = (e.currentTarget as HTMLTextAreaElement).value;
    props.onInput(val);

    // Debounce re-highlighting while typing
    clearTimeout(debounceTimer);
    setReady(false);
    debounceTimer = setTimeout(() => setHighlightTrigger((n) => n + 1), HIGHLIGHT_DEBOUNCE);
  };

  return (
    <div class={`highlighted-editor ${props.themeFamily === 'light' ? 'highlighted-editor-light' : ''} ${props.class ?? ''}`}>
      <Show when={!ready()}>
        <div class="code-viewer-progress" />
      </Show>

      {/* Highlighted layer (behind) */}
      <Show when={ready()}>
        <pre
          ref={preRef}
          class="highlighted-editor-pre"
          aria-hidden="true"
          innerHTML={DOMPurify.sanitize(highlightedHtml(), { ALLOWED_TAGS: ['span', 'br'], ALLOWED_ATTR: ['class', 'style'] })}
        />
      </Show>

      {/* Textarea layer (on top, transparent text when highlight is ready) */}
      <textarea
        ref={textareaRef}
        class="highlighted-editor-textarea"
        classList={{ 'highlight-ready': ready() }}
        data-testid="workspace-editor"
        aria-label="File editor"
        value={props.value}
        onInput={handleInput}
        onScroll={syncScroll}
        spellcheck={false}
        placeholder={props.placeholder}
      />
    </div>
  );
};

function escapeHtml(text: string): string {
  return text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

export default HighlightedEditor;
