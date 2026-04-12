import { JSX, mergeProps } from 'solid-js';
import yaml from 'js-yaml';

/**
 * Serialize a value to a pretty YAML string.
 * If `raw` is already a string, try to parse it as JSON first so we get
 * a nicely-formatted YAML representation of the structured data.
 */
function toYaml(raw: unknown): string {
  let value: unknown = raw;
  if (typeof raw === 'string') {
    try {
      value = JSON.parse(raw);
    } catch {
      return raw; // not JSON – return the raw string as-is
    }
  }
  if (value == null) return '';
  try {
    return yaml.dump(value, {
      indent: 2,
      lineWidth: 120,
      noRefs: true,
      sortKeys: false,
    }).trimEnd();
  } catch {
    return String(value);
  }
}

/**
 * Syntax-highlight a YAML string, returning HTML with <span> tokens.
 * Handles keys, strings, numbers, booleans, null, and list markers.
 */
export function highlightYaml(raw: unknown): string {
  const text = toYaml(raw);
  if (!text) return '';

  // Escape HTML entities first
  const escaped = text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;');

  // Process line-by-line for reliable YAML tokenization
  return escaped
    .split('\n')
    .map((line) => {
      // Comment lines
      if (/^\s*#/.test(line)) {
        return `<span class="yaml-comment">${line}</span>`;
      }

      // Lines with a key: value pattern
      const kvMatch = line.match(/^(\s*)(- )?([\w][\w.\-/]*)(:\s*)(.*)/);
      if (kvMatch) {
        const [, indent, dash, key, colon, val] = kvMatch;
        const prefix = (dash ? `${indent}<span class="yaml-marker">${dash}</span>` : indent);
        const highlightedKey = `<span class="yaml-key">${key}</span>${colon}`;
        const highlightedVal = val ? highlightValue(val) : '';
        return `${prefix}${highlightedKey}${highlightedVal}`;
      }

      // Bare list items (- value)
      const listMatch = line.match(/^(\s*)(- )(.*)/);
      if (listMatch) {
        const [, indent, dash, val] = listMatch;
        return `${indent}<span class="yaml-marker">${dash}</span>${highlightValue(val)}`;
      }

      return line;
    })
    .join('\n');
}

/** Highlight a scalar value fragment. */
function highlightValue(val: string): string {
  const trimmed = val.trim();
  if (!trimmed) return val;

  // Quoted strings (single or double)
  if (/^".*"$/.test(trimmed) || /^'.*'$/.test(trimmed)) {
    return val.replace(trimmed, `<span class="yaml-string">${trimmed}</span>`);
  }
  // Booleans
  if (/^(true|false)$/i.test(trimmed)) {
    return val.replace(trimmed, `<span class="yaml-boolean">${trimmed}</span>`);
  }
  // Null
  if (/^(null|~)$/i.test(trimmed)) {
    return val.replace(trimmed, `<span class="yaml-null">${trimmed}</span>`);
  }
  // Numbers (int, float, scientific)
  if (/^-?\d+(\.\d+)?([eE][+-]?\d+)?$/.test(trimmed)) {
    return val.replace(trimmed, `<span class="yaml-number">${trimmed}</span>`);
  }
  // Unquoted strings – highlight as string
  return `<span class="yaml-string">${val}</span>`;
}

export interface YamlBlockProps {
  /** The value to display — object, array, or already-serialised JSON/YAML string */
  data: unknown;
  /** Extra CSS class names */
  class?: string;
  /** Inline style overrides */
  style?: JSX.CSSProperties | string;
}

/**
 * Pretty-printed, syntax-highlighted YAML display block.
 */
export function YamlBlock(inProps: YamlBlockProps) {
  const props = mergeProps({ class: '' }, inProps);
  const html = () => highlightYaml(props.data);

  return (
    <pre
      class={`yaml-block ${props.class}`}
      style={props.style}
      innerHTML={html()}
    />
  );
}
