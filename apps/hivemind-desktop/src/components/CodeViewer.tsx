import { createSignal, createEffect, createMemo, For, Show, type JSX } from 'solid-js';
import { ChevronRight, ChevronDown } from 'lucide-solid';
import { highlightCode } from '../lib/shikiHighlighter';
import { computeFoldRegions, type FoldRegion } from '../lib/foldRegions';
import { parseImportLinks, resolveImportPath } from '../lib/importLinks';
import DOMPurify from 'dompurify';

export interface FindMatch {
  lineIdx: number;
  colStart: number;
  colEnd: number;
}

export interface CodeViewerProps {
  code: string;
  language: string | undefined;
  /** Called when the user clicks a navigable import/link. */
  onNavigate?: (workspace_path: string) => void;
  /** Set of all workspace-relative file paths (for resolving imports). */
  workspacePaths?: Set<string>;
  /** Workspace-relative path of the current file (for resolving relative imports). */
  currentFilePath?: string;
  /** Active find query. When set, all occurrences are highlighted. */
  findQuery?: string;
  /** Zero-based index of the currently focused match. */
  currentFindMatch?: number;
  /** Called whenever the total number of matches changes. */
  onFindMatchCount?: (count: number) => void;
  /** Theme family ('dark' | 'light') for syntax highlighting. */
  themeFamily?: 'dark' | 'light';
}

const CodeViewer = (props: CodeViewerProps) => {
  // Plain-text lines are shown immediately; highlighted HTML replaces them progressively.
  const [plainLines, setPlainLines] = createSignal<string[]>([]);
  const [highlightedLines, setHighlightedLines] = createSignal<string[] | null>(null);
  const [highlighting, setHighlighting] = createSignal(false);
  const [foldRegions, setFoldRegions] = createSignal<FoldRegion[]>([]);
  const [collapsedLines, setCollapsedLines] = createSignal<Set<number>>(new Set<number>());
  const [resolvedLinks, setResolvedLinks] = createSignal<Map<number, { colStart: number; colEnd: number; target: string }[]>>(new Map());

  // ── Find / highlight matches ──────────────────────────────────────
  const findMatches = createMemo<FindMatch[]>(() => {
    const q = props.findQuery;
    if (!q) return [];
    const lower = q.toLowerCase();
    const lines = props.code.split('\n');
    const matches: FindMatch[] = [];
    for (let i = 0; i < lines.length; i++) {
      const lineLower = lines[i].toLowerCase();
      let pos = 0;
      while (true) {
        const idx = lineLower.indexOf(lower, pos);
        if (idx === -1) break;
        matches.push({ lineIdx: i, colStart: idx, colEnd: idx + q.length });
        pos = idx + 1;
      }
    }
    return matches;
  });

  // Index find matches by line for efficient rendering
  const findMatchesByLine = createMemo(() => {
    const map = new Map<number, FindMatch[]>();
    for (const m of findMatches()) {
      if (!map.has(m.lineIdx)) map.set(m.lineIdx, []);
      map.get(m.lineIdx)!.push(m);
    }
    return map;
  });

  // Pre-built index from match object to its global index (avoids O(n²) indexOf)
  const findMatchIndexMap = createMemo(() => {
    const map = new Map<FindMatch, number>();
    const matches = findMatches();
    for (let i = 0; i < matches.length; i++) {
      map.set(matches[i], i);
    }
    return map;
  });

  // Notify parent when match count changes
  createEffect(() => {
    props.onFindMatchCount?.(findMatches().length);
  });

  // Scroll the current match into view
  let scrollContainerRef: HTMLDivElement | undefined;
  createEffect(() => {
    const idx = props.currentFindMatch;
    const matches = findMatches();
    if (idx == null || idx < 0 || idx >= matches.length) return;
    const match = matches[idx];
    // Find the row element for this match's line
    const row = scrollContainerRef?.querySelector(`[data-line-idx="${match.lineIdx}"]`);
    row?.scrollIntoView({ block: 'center', behavior: 'smooth' });
  });

  // The lines to render: highlighted if ready, plain otherwise.
  const displayLines = createMemo(() => highlightedLines() ?? plainLines());

  // Sequence counter to prevent stale async results from overwriting current data
  let highlightSeq = 0;

  createEffect(() => {
    const code = props.code;
    const lang = props.language;
    const themeFamily = props.themeFamily;
    const seq = ++highlightSeq;

    // 1. Immediately show plain (escaped) text
    const rawLines = code.split('\n');
    setPlainLines(rawLines.map(escapeHtml));
    setHighlightedLines(null);
    setHighlighting(true);
    setFoldRegions([]);
    setResolvedLinks(new Map());
    setCollapsedLines(new Set<number>());

    // 2. Kick off shiki in the Web Worker
    (async () => {
      try {
        const result = await highlightCode(code, lang, themeFamily);
        if (seq !== highlightSeq) return;

        // Extract per-line highlighted HTML from shiki output
        const parser = new DOMParser();
        const doc = parser.parseFromString(result.html, 'text/html');
        const lineSpans = doc.querySelectorAll('.line');
        const extracted: string[] = [];
        lineSpans.forEach((span) => extracted.push(span.innerHTML));

        if (extracted.length === 0) {
          const codeEl = doc.querySelector('code');
          if (codeEl) {
            extracted.push(...codeEl.innerHTML.split('\n'));
          } else {
            return; // keep plain text
          }
        }

        if (seq !== highlightSeq) return;

        // 3. Swap in highlighted lines — the UI updates in-place
        setHighlightedLines(extracted);

        // 4. Now compute fold regions + import links (lighter work)
        const effectiveLang = result.language;
        setFoldRegions(computeFoldRegions(rawLines, effectiveLang));

        if (props.workspacePaths && props.currentFilePath) {
          const links = parseImportLinks(code, effectiveLang);
          const lineMap = new Map<number, { colStart: number; colEnd: number; target: string }[]>();
          for (const link of links) {
            const resolved = resolveImportPath(link.rawPath, props.currentFilePath, props.workspacePaths);
            if (resolved) {
              if (!lineMap.has(link.line)) lineMap.set(link.line, []);
              lineMap.get(link.line)!.push({
                colStart: link.colStart,
                colEnd: link.colEnd,
                target: resolved,
              });
            }
          }
          setResolvedLinks(lineMap);
        }
      } catch {
        // Highlighting failed — plain text stays visible, no error shown
      } finally {
        if (seq === highlightSeq) setHighlighting(false);
      }
    })();
  });

  // Build a set of lines that start a fold region (for showing fold icons)
  const foldStartLines = () => {
    const starts = new Map<number, FoldRegion>();
    for (const r of foldRegions()) {
      starts.set(r.startLine, r);
    }
    return starts;
  };

  // Determine which lines are hidden (inside a collapsed region)
  const hiddenLines = () => {
    const hidden = new Set<number>();
    const collapsed = collapsedLines();
    for (const region of foldRegions()) {
      if (collapsed.has(region.startLine)) {
        for (let i = region.startLine + 1; i <= region.endLine; i++) {
          hidden.add(i);
        }
      }
    }
    return hidden;
  };

  const toggleFold = (startLine: number) => {
    const next = new Set(collapsedLines());
    if (next.has(startLine)) {
      next.delete(startLine);
    } else {
      next.add(startLine);
    }
    setCollapsedLines(next);
  };

  const handleLinkClick = (e: MouseEvent, target: string) => {
    e.preventDefault();
    e.stopPropagation();
    props.onNavigate?.(target);
  };

  // Render a single line, potentially with clickable import links
  const renderLine = (lineIdx: number, lineHtml: string): JSX.Element => {
    const links = resolvedLinks().get(lineIdx);
    const sanitizedHtml = DOMPurify.sanitize(lineHtml, { ALLOWED_TAGS: ['span', 'br'], ALLOWED_ATTR: ['class', 'style'] });
    if (!links || links.length === 0) {
      return <span class="code-line-text" innerHTML={sanitizedHtml} />;
    }

    return (
      <span class="code-line-text code-line-with-links">
        <span innerHTML={sanitizedHtml} />
        <For each={links}>
          {(link) => (
            <span
              role="link"
              class="code-import-link"
              style={`left: ${link.colStart}ch; width: ${link.colEnd - link.colStart}ch;`}
              title={`Go to ${link.target}`}
              onClick={(e) => handleLinkClick(e, link.target)}
            />
          )}
        </For>
      </span>
    );
  };

  return (
    <div class={`code-viewer ${props.themeFamily === 'light' ? 'code-viewer-light' : ''}`}>
      <Show when={highlighting()}>
        <div class="code-viewer-progress" />
      </Show>
      <div class="code-viewer-scroll" ref={scrollContainerRef}>
        <table class="code-viewer-table">
          <tbody>
            <For each={displayLines()}>
              {(lineHtml, idx) => {
                const lineNum = idx() + 1;
                const lineIdx = idx();
                const isHidden = () => hiddenLines().has(lineIdx);
                const foldStart = () => foldStartLines().get(lineIdx);
                const isCollapsed = () => collapsedLines().has(lineIdx);

                return (
                  <Show when={!isHidden()}>
                    <tr class="code-line" data-line-idx={lineIdx}>
                      <td class="code-gutter">
                        <Show
                          when={foldStart()}
                          fallback={<span class="code-fold-spacer" />}
                        >
                          <button
                            class="code-fold-btn"
                            onClick={() => toggleFold(lineIdx)}
                            title={isCollapsed() ? 'Expand' : 'Collapse'}
                          >
                            <Show when={isCollapsed()} fallback={<ChevronDown size={12} />}>
                              <ChevronRight size={12} />
                            </Show>
                          </button>
                        </Show>
                      </td>
                      <td class="code-line-number">{lineNum}</td>
                      <td class="code-line-content">
                        {renderLine(lineIdx, lineHtml)}
                        <For each={findMatchesByLine().get(lineIdx) ?? []}>
                          {(match) => {
                            const globalIdx = findMatchIndexMap().get(match) ?? -1;
                            const isCurrent = () => globalIdx === props.currentFindMatch;
                            return (
                              <span
                                class={`code-find-match ${isCurrent() ? 'code-find-match-current' : ''}`}
                                style={`left: ${match.colStart}ch; width: ${match.colEnd - match.colStart}ch;`}
                              />
                            );
                          }}
                        </For>
                        <Show when={isCollapsed()}>
                          {(() => {
                            const region = foldStart()!;
                            const hiddenCount = region.endLine - region.startLine;
                            return (
                              <span
                                class="code-fold-placeholder"
                                onClick={() => toggleFold(lineIdx)}
                              >
                                ⋯ {hiddenCount} lines
                              </span>
                            );
                          })()}
                        </Show>
                      </td>
                    </tr>
                  </Show>
                );
              }}
            </For>
          </tbody>
        </table>
      </div>
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

export default CodeViewer;
