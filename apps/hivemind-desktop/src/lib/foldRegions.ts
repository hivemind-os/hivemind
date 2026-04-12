/**
 * Detects foldable regions in source code.
 *
 * Strategies:
 * - **Brace-based**: For C-family, Rust, Java, JSON, etc. Folds `{...}` blocks.
 * - **Indent-based**: For Python, YAML, etc. Folds when indentation increases.
 */

export interface FoldRegion {
  /** 0-based start line index (the line with the opening brace / indent increase). */
  startLine: number;
  /** 0-based end line index (the line with the closing brace / last line of block). */
  endLine: number;
}

const BRACE_LANGUAGES = new Set([
  'rust',
  'javascript',
  'typescript',
  'tsx',
  'jsx',
  'json',
  'jsonc',
  'java',
  'c',
  'cpp',
  'csharp',
  'go',
  'kotlin',
  'swift',
  'scala',
  'dart',
  'css',
  'scss',
  'less',
  'html',
  'xml',
  'vue',
  'svelte',
  'zig',
  'fsharp',
  'hcl',
  'proto',
  'graphql',
]);

const INDENT_LANGUAGES = new Set(['python', 'yaml', 'ruby', 'elixir', 'haskell', 'makefile', 'fish', 'nim']);

/**
 * Compute fold regions for the given source lines.
 *
 * @param lines - Array of source lines (already split).
 * @param language - The shiki language id.
 * @returns Sorted array of fold regions.
 */
export function computeFoldRegions(lines: string[], language: string): FoldRegion[] {
  if (BRACE_LANGUAGES.has(language)) {
    return braceFold(lines);
  }
  if (INDENT_LANGUAGES.has(language)) {
    return indentFold(lines);
  }
  // Default: try brace-based, fall back to indent-based if no regions found.
  const braces = braceFold(lines);
  return braces.length > 0 ? braces : indentFold(lines);
}

// Minimum number of lines a region must span to be foldable.
const MIN_FOLD_LINES = 2;

/**
 * Brace-based folding: match `{` to `}` (ignoring those inside strings/comments is
 * impractical without a full parser, so we use a simple stack approach).
 */
function braceFold(lines: string[]): FoldRegion[] {
  const regions: FoldRegion[] = [];
  const stack: number[] = []; // stack of line indices where `{` was found

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    for (const ch of line) {
      if (ch === '{') {
        stack.push(i);
      } else if (ch === '}') {
        const start = stack.pop();
        if (start !== undefined && i - start >= MIN_FOLD_LINES) {
          regions.push({ startLine: start, endLine: i });
        }
      }
    }
  }

  regions.sort((a, b) => a.startLine - b.startLine || a.endLine - b.endLine);
  return regions;
}

/**
 * Indent-based folding: a line starts a foldable region when the next
 * non-empty line has a greater indent level.
 */
function indentFold(lines: string[]): FoldRegion[] {
  const regions: FoldRegion[] = [];
  const indents = lines.map(indentLevel);

  for (let i = 0; i < lines.length; i++) {
    if (lines[i].trim() === '') continue;
    const myIndent = indents[i];

    // Find the next non-blank line
    let next = i + 1;
    while (next < lines.length && lines[next].trim() === '') next++;
    if (next >= lines.length) continue;

    if (indents[next] > myIndent) {
      // Find the end of this indented block
      let end = next;
      for (let j = next + 1; j < lines.length; j++) {
        if (lines[j].trim() === '') continue;
        if (indents[j] <= myIndent) break;
        end = j;
      }
      if (end - i >= MIN_FOLD_LINES) {
        regions.push({ startLine: i, endLine: end });
      }
    }
  }

  return regions;
}

function indentLevel(line: string): number {
  let count = 0;
  for (const ch of line) {
    if (ch === ' ') count++;
    else if (ch === '\t') count += 4;
    else break;
  }
  return count;
}
