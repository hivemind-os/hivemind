import { onMount, onCleanup, createEffect, on } from 'solid-js';
import { EditorView, keymap, lineNumbers, highlightActiveLine, highlightActiveLineGutter, drawSelection, rectangularSelection, placeholder as cmPlaceholder } from '@codemirror/view';
import { EditorState, Compartment } from '@codemirror/state';
import { markdown, markdownLanguage } from '@codemirror/lang-markdown';
import { languages } from '@codemirror/language-data';
import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands';
import { syntaxHighlighting, defaultHighlightStyle, foldGutter, foldKeymap, bracketMatching, indentOnInput, HighlightStyle } from '@codemirror/language';
import { searchKeymap, highlightSelectionMatches, openSearchPanel } from '@codemirror/search';
import { closeBrackets, closeBracketsKeymap } from '@codemirror/autocomplete';
import { tags } from '@lezer/highlight';

export interface CodeMirrorEditorProps {
  value: string;
  onChange: (value: string) => void;
  class?: string;
  placeholder?: string;
}

// Custom dark theme matching the app's CSS variables
const hivemindDarkTheme = EditorView.theme({
  '&': {
    backgroundColor: 'hsl(215 21% 7%)',
    color: 'hsl(210 13% 81%)',
    fontSize: '13px',
    height: '100%',
  },
  '.cm-content': {
    caretColor: 'hsl(210 13% 81%)',
    fontFamily: "'SF Mono', 'Fira Code', 'Fira Mono', Menlo, Consolas, 'DejaVu Sans Mono', monospace",
    padding: '8px 0',
  },
  '.cm-cursor, .cm-dropCursor': {
    borderLeftColor: 'hsl(210 13% 81%)',
  },
  '&.cm-focused .cm-selectionBackground, .cm-selectionBackground, .cm-content ::selection': {
    backgroundColor: 'hsla(215 50% 40% / 0.3)',
  },
  '.cm-panels': {
    backgroundColor: 'hsl(215 19% 11%)',
    color: 'hsl(210 13% 81%)',
    borderBottom: '1px solid hsl(214 14% 22%)',
  },
  '.cm-panels.cm-panels-top': {
    borderBottom: '1px solid hsl(214 14% 22%)',
  },
  '.cm-searchMatch': {
    backgroundColor: 'hsla(50 80% 50% / 0.25)',
    outline: '1px solid hsla(50 80% 50% / 0.4)',
  },
  '.cm-searchMatch.cm-searchMatch-selected': {
    backgroundColor: 'hsla(50 80% 50% / 0.45)',
  },
  '.cm-activeLine': {
    backgroundColor: 'hsla(215 50% 50% / 0.07)',
  },
  '.cm-gutters': {
    backgroundColor: 'hsl(215 19% 9%)',
    color: 'hsl(212 10% 53%)',
    border: 'none',
    borderRight: '1px solid hsl(214 14% 16%)',
  },
  '.cm-activeLineGutter': {
    backgroundColor: 'hsla(215 50% 50% / 0.07)',
    color: 'hsl(210 13% 81%)',
  },
  '.cm-foldPlaceholder': {
    backgroundColor: 'hsl(214 14% 16%)',
    color: 'hsl(212 10% 53%)',
    border: '1px solid hsl(214 14% 22%)',
    borderRadius: '3px',
    padding: '0 4px',
  },
  '.cm-tooltip': {
    backgroundColor: 'hsl(215 19% 11%)',
    color: 'hsl(210 13% 81%)',
    border: '1px solid hsl(214 14% 22%)',
  },
  '.cm-panel.cm-search': {
    backgroundColor: 'hsl(215 19% 11%)',
  },
  '.cm-panel.cm-search input, .cm-panel.cm-search button': {
    color: 'hsl(210 13% 81%)',
    backgroundColor: 'hsl(215 21% 7%)',
    border: '1px solid hsl(214 14% 22%)',
    borderRadius: '4px',
    padding: '2px 6px',
  },
  '.cm-panel.cm-search button:hover': {
    backgroundColor: 'hsl(214 14% 16%)',
  },
  '.cm-panel.cm-search label': {
    color: 'hsl(212 10% 53%)',
  },
  '.cm-scroller': {
    overflow: 'auto',
  },
}, { dark: true });

const hivemindHighlightStyle = HighlightStyle.define([
  { tag: tags.heading1, color: '#79c0ff', fontWeight: 'bold', fontSize: '1.4em' },
  { tag: tags.heading2, color: '#79c0ff', fontWeight: 'bold', fontSize: '1.2em' },
  { tag: tags.heading3, color: '#79c0ff', fontWeight: 'bold', fontSize: '1.1em' },
  { tag: [tags.heading4, tags.heading5, tags.heading6], color: '#79c0ff', fontWeight: 'bold' },
  { tag: tags.emphasis, fontStyle: 'italic', color: '#d2a8ff' },
  { tag: tags.strong, fontWeight: 'bold', color: '#ff7b72' },
  { tag: tags.strikethrough, textDecoration: 'line-through' },
  { tag: tags.link, color: '#58a6ff', textDecoration: 'underline' },
  { tag: tags.url, color: '#58a6ff' },
  { tag: tags.monospace, color: '#a5d6ff', backgroundColor: 'hsla(215 50% 50% / 0.1)', borderRadius: '3px', padding: '1px 3px' },
  { tag: tags.quote, color: '#8b949e', fontStyle: 'italic' },
  { tag: tags.list, color: '#ff7b72' },
  { tag: tags.keyword, color: '#ff7b72' },
  { tag: tags.string, color: '#a5d6ff' },
  { tag: tags.comment, color: '#8b949e' },
  { tag: tags.processingInstruction, color: '#8b949e' },
  { tag: tags.meta, color: '#8b949e' },
  { tag: tags.contentSeparator, color: 'hsl(214 14% 22%)' },
]);

/**
 * CodeMirror 6 editor wrapped for SolidJS.
 * Provides markdown editing with syntax highlighting, find/replace, code folding,
 * line numbers, and a dark theme matching the app's design.
 */
const CodeMirrorEditor = (props: CodeMirrorEditorProps) => {
  let containerRef: HTMLDivElement | undefined;
  let view: EditorView | undefined;
  let isExternalUpdate = false;
  const readOnlyComp = new Compartment();

  onMount(() => {
    if (!containerRef) return;

    const updateListener = EditorView.updateListener.of((update) => {
      if (update.docChanged && !isExternalUpdate) {
        props.onChange(update.state.doc.toString());
      }
    });

    const state = EditorState.create({
      doc: props.value,
      extensions: [
        lineNumbers(),
        highlightActiveLine(),
        highlightActiveLineGutter(),
        drawSelection(),
        rectangularSelection(),
        indentOnInput(),
        bracketMatching(),
        closeBrackets(),
        history(),
        foldGutter(),
        highlightSelectionMatches(),
        EditorView.lineWrapping,
        markdown({ base: markdownLanguage, codeLanguages: languages }),
        hivemindDarkTheme,
        syntaxHighlighting(hivemindHighlightStyle),
        syntaxHighlighting(defaultHighlightStyle, { fallback: true }),
        keymap.of([
          ...defaultKeymap,
          ...historyKeymap,
          ...foldKeymap,
          ...searchKeymap,
          ...closeBracketsKeymap,
          indentWithTab,
        ]),
        updateListener,
        ...(props.placeholder ? [cmPlaceholder(props.placeholder)] : []),
      ],
    });

    view = new EditorView({
      state,
      parent: containerRef,
    });
  });

  // Sync external value changes into the editor
  createEffect(on(() => props.value, (newValue) => {
    if (!view) return;
    const currentValue = view.state.doc.toString();
    if (newValue !== currentValue) {
      isExternalUpdate = true;
      view.dispatch({
        changes: { from: 0, to: currentValue.length, insert: newValue },
      });
      isExternalUpdate = false;
    }
  }));

  onCleanup(() => {
    view?.destroy();
  });

  return (
    <div
      ref={containerRef}
      class={props.class ?? ''}
      style="height:100%;overflow:hidden"
    />
  );
};

export default CodeMirrorEditor;
