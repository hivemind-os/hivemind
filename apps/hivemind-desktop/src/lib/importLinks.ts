/**
 * Parses import / require / use / include statements from source code and
 * markdown links. Returns token ranges that can be turned into clickable
 * navigation targets.
 */

export interface ImportLink {
  /** 0-based line index. */
  line: number;
  /** 0-based column start of the path token (inclusive). */
  colStart: number;
  /** 0-based column end of the path token (exclusive). */
  colEnd: number;
  /** The raw path string as written in the source. */
  rawPath: string;
}

// ---------- per-language patterns ----------

// JS / TS: import ... from 'path'  |  require('path')  |  import('path')
const JS_IMPORT_FROM = /(?:from|import)\s+['"]([^'"]+)['"]/g;
const JS_REQUIRE = /require\s*\(\s*['"]([^'"]+)['"]\s*\)/g;

// Python: import path  |  from path import ...
const PY_IMPORT = /^\s*(?:from\s+(\S+)|import\s+(\S+))/gm;

// Rust: use crate::path  |  mod path;
const RUST_USE = /\buse\s+(crate::[\w:]+)/g;
const RUST_MOD = /\bmod\s+(\w+)\s*;/g;

// C / C++: #include "path"
const C_INCLUDE = /#include\s+"([^"]+)"/g;

// Go: import "path"  |  "path" inside import block
const GO_IMPORT = /(?:import\s+)?"([^"]+)"/g;

// Markdown: [text](path)
const MD_LINK = /\[(?:[^\]]*)\]\(([^)]+)\)/g;

type PatternSet = RegExp[];

function patternsForLanguage(lang: string): PatternSet {
  switch (lang) {
    case 'javascript':
    case 'typescript':
    case 'jsx':
    case 'tsx':
    case 'vue':
    case 'svelte':
      return [JS_IMPORT_FROM, JS_REQUIRE];
    case 'python':
      return [PY_IMPORT];
    case 'rust':
      return [RUST_USE, RUST_MOD];
    case 'c':
    case 'cpp':
      return [C_INCLUDE];
    case 'go':
      return [GO_IMPORT];
    case 'markdown':
    case 'mdx':
      return [MD_LINK];
    default:
      return [JS_IMPORT_FROM, JS_REQUIRE, C_INCLUDE, MD_LINK];
  }
}

/**
 * Parse all import-like references from source code.
 */
export function parseImportLinks(source: string, language: string): ImportLink[] {
  const patterns = patternsForLanguage(language);
  const lines = source.split('\n');
  const results: ImportLink[] = [];

  for (let lineIdx = 0; lineIdx < lines.length; lineIdx++) {
    const line = lines[lineIdx];
    for (const pattern of patterns) {
      // Reset lastIndex because we reuse the regex across lines
      const re = new RegExp(pattern.source, pattern.flags.replace('g', '') + 'g');
      let m: RegExpExecArray | null;
      while ((m = re.exec(line)) !== null) {
        // Pick the first captured group
        const rawPath = m[1] ?? m[2];
        if (!rawPath) continue;

        // Compute column positions of the captured path within the full match
        const pathStartInMatch = m[0].indexOf(rawPath);
        if (pathStartInMatch < 0) continue;

        const colStart = m.index + pathStartInMatch;
        const colEnd = colStart + rawPath.length;

        results.push({ line: lineIdx, colStart, colEnd, rawPath });
      }
    }
  }

  return results;
}

// ---------- path resolution ----------

/**
 * Attempt to resolve a raw import path to a workspace-relative file path.
 *
 * @param rawPath - The path as written in source (e.g., `./utils`, `../lib/foo`).
 * @param currentFilePath - Workspace-relative path of the file containing the import.
 * @param workspacePaths - Set of all known workspace-relative file paths.
 * @returns The resolved workspace-relative path, or `null` if not resolvable.
 */
export function resolveImportPath(
  rawPath: string,
  currentFilePath: string,
  workspacePaths: Set<string>,
): string | null {
  // Skip absolute paths, URLs, and package references
  if (rawPath.startsWith('http://') || rawPath.startsWith('https://')) return null;
  if (rawPath.startsWith('//')) return null;

  // For non-relative paths that aren't file-like, skip (e.g., npm packages)
  const isRelative = rawPath.startsWith('./') || rawPath.startsWith('../');
  if (!isRelative && !rawPath.includes('/') && !rawPath.includes('.')) return null;

  // Rust crate:: paths → convert to file path
  if (rawPath.startsWith('crate::')) {
    return resolveRustPath(rawPath, workspacePaths);
  }

  // Python dotted imports → convert to file path
  if (!rawPath.includes('/') && rawPath.includes('.') && !rawPath.startsWith('.')) {
    return resolvePythonPath(rawPath, workspacePaths);
  }

  // Resolve relative paths
  const currentDir = currentFilePath.includes('/')
    ? currentFilePath.slice(0, currentFilePath.lastIndexOf('/'))
    : '';

  const resolved = normalizePath(currentDir ? `${currentDir}/${rawPath}` : rawPath);

  // Try exact match, then with common extensions
  if (workspacePaths.has(resolved)) return resolved;

  const extensions = ['.ts', '.tsx', '.js', '.jsx', '.rs', '.py', '.go', '.md', '.json'];
  for (const ext of extensions) {
    if (workspacePaths.has(resolved + ext)) return resolved + ext;
  }

  // Try index files (e.g., ./utils → ./utils/index.ts)
  const indexNames = ['index.ts', 'index.tsx', 'index.js', 'index.jsx', 'mod.rs', '__init__.py'];
  for (const idx of indexNames) {
    const candidate = `${resolved}/${idx}`;
    if (workspacePaths.has(candidate)) return candidate;
  }

  return null;
}

function resolveRustPath(rawPath: string, workspacePaths: Set<string>): string | null {
  // crate::foo::bar → src/foo/bar.rs or src/foo/bar/mod.rs
  const parts = rawPath.replace('crate::', '').split('::');
  const filePath = `src/${parts.join('/')}.rs`;
  if (workspacePaths.has(filePath)) return filePath;

  const modPath = `src/${parts.join('/')}/mod.rs`;
  if (workspacePaths.has(modPath)) return modPath;

  return null;
}

function resolvePythonPath(rawPath: string, workspacePaths: Set<string>): string | null {
  const parts = rawPath.split('.');
  const filePath = `${parts.join('/')}.py`;
  if (workspacePaths.has(filePath)) return filePath;

  const initPath = `${parts.join('/')}/__init__.py`;
  if (workspacePaths.has(initPath)) return initPath;

  return null;
}

/**
 * Normalize a path by resolving `.` and `..` segments.
 */
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

// ---------- workspace path set builder ----------

/**
 * Build a set of all file paths from a recursive workspace entry tree.
 */
export function buildWorkspacePathSet(entries: any[]): Set<string> {
  const paths = new Set<string>();
  const walk = (items: any[]) => {
    for (const entry of items) {
      if (!entry.is_dir) {
        paths.add(entry.path);
      }
      if (entry.children?.length) {
        walk(entry.children);
      }
    }
  };
  walk(entries);
  return paths;
}
