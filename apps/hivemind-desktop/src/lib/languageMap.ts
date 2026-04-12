/** Maps file extensions to shiki language identifiers. */

const EXT_TO_LANG: Record<string, string> = {
  // Rust
  rs: 'rust',
  toml: 'toml',

  // Web
  js: 'javascript',
  jsx: 'jsx',
  ts: 'typescript',
  tsx: 'tsx',
  html: 'html',
  htm: 'html',
  css: 'css',
  scss: 'scss',
  less: 'less',
  vue: 'vue',
  svelte: 'svelte',

  // Data / Config
  json: 'json',
  jsonc: 'jsonc',
  yaml: 'yaml',
  yml: 'yaml',
  xml: 'xml',
  csv: 'csv',
  ini: 'ini',
  env: 'dotenv',

  // Scripting
  py: 'python',
  rb: 'ruby',
  sh: 'shellscript',
  bash: 'shellscript',
  zsh: 'shellscript',
  fish: 'fish',
  bat: 'bat',
  ps1: 'powershell',
  lua: 'lua',
  perl: 'perl',
  pl: 'perl',

  // Systems
  c: 'c',
  h: 'c',
  cpp: 'cpp',
  cxx: 'cpp',
  cc: 'cpp',
  hpp: 'cpp',
  go: 'go',
  java: 'java',
  kt: 'kotlin',
  swift: 'swift',
  cs: 'csharp',
  fs: 'fsharp',
  zig: 'zig',

  // Query / Schema
  sql: 'sql',
  graphql: 'graphql',
  gql: 'graphql',
  proto: 'proto',

  // Markup / Docs
  md: 'markdown',
  mdx: 'mdx',
  tex: 'latex',
  rst: 'rst',

  // DevOps / Config
  dockerfile: 'dockerfile',
  tf: 'hcl',
  hcl: 'hcl',
  nix: 'nix',

  // Misc
  r: 'r',
  ex: 'elixir',
  exs: 'elixir',
  erl: 'erlang',
  hs: 'haskell',
  clj: 'clojure',
  scala: 'scala',
  dart: 'dart',
  vim: 'viml',
  log: 'log',
  txt: 'text',
  cfg: 'ini',
};

/** Well-known extensionless filenames → language. */
const FILENAME_TO_LANG: Record<string, string> = {
  Dockerfile: 'dockerfile',
  Makefile: 'makefile',
  Justfile: 'makefile',
  Rakefile: 'ruby',
  Gemfile: 'ruby',
  Vagrantfile: 'ruby',
  Brewfile: 'ruby',
  CMakeLists: 'cmake',
  '.gitignore': 'gitignore',
  '.dockerignore': 'gitignore',
  '.env': 'dotenv',
  '.editorconfig': 'ini',
};

/**
 * Derive a shiki language ID from a file path.
 * Returns `undefined` when no mapping is found (shiki will fall back to plain text).
 */
export function languageFromPath(filePath: string): string | undefined {
  const segments = filePath.split(/[\\/]/);
  const filename = segments[segments.length - 1] ?? '';

  // Try exact filename match first
  const byName = FILENAME_TO_LANG[filename];
  if (byName) return byName;

  // Try extension
  const dotIdx = filename.lastIndexOf('.');
  if (dotIdx >= 0) {
    const ext = filename.slice(dotIdx + 1).toLowerCase();
    return EXT_TO_LANG[ext];
  }

  return undefined;
}

/**
 * Return the file extension (without dot) from a path, or empty string.
 */
export function extensionFromPath(filePath: string): string {
  if (!filePath) return '';
  const segments = filePath.split(/[\\/]/);
  const filename = segments[segments.length - 1] ?? '';
  const dotIdx = filename.lastIndexOf('.');
  return dotIdx >= 0 ? filename.slice(dotIdx + 1).toLowerCase() : '';
}
