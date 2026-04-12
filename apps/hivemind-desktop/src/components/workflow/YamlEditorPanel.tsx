import { Show } from 'solid-js';

// ── Types ──────────────────────────────────────────────────────────────

export interface YamlEditorPanelProps {
  yamlOutput: string;
  sectionHeaderStyle: Record<string, string>;
}

// ── Component ──────────────────────────────────────────────────────────

export function YamlEditorPanel(props: YamlEditorPanelProps) {
  return (
    <div style="width:280px;min-width:280px;max-width:280px;background:hsl(var(--card));border-left:1px solid hsl(var(--border));display:flex;flex-direction:column;overflow:hidden;">
      <div style={props.sectionHeaderStyle}>YAML Preview</div>
      <pre style={{
        flex: '1', 'overflow-y': 'auto', margin: '0', padding: '8px',
        'font-size': '0.72em', 'font-family': 'monospace',
        color: 'hsl(var(--foreground))',
        'white-space': 'pre-wrap', 'word-break': 'break-all',
      }}>{props.yamlOutput}</pre>
    </div>
  );
}
