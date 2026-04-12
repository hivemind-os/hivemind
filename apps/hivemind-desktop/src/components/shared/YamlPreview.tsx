import { Show, createSignal } from 'solid-js';
import { Copy, Check } from 'lucide-solid';
import { highlightYaml, YamlBlock } from '../YamlHighlight';

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export interface YamlPreviewProps {
  /** The value to display — object, array, or already-serialised JSON/YAML string. */
  data: unknown;
  /** Extra CSS class names applied to the outer wrapper. */
  class?: string;
  /** Maximum height for the preview (CSS value). Defaults to "200px". */
  maxHeight?: string;
  /** Show a small copy-to-clipboard button in the top-right corner. */
  copyButton?: boolean;
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

/**
 * Enhanced YAML preview block.
 *
 * Wraps the existing `YamlBlock` from `YamlHighlight.tsx` with optional
 * max-height scrolling and a copy-to-clipboard button so that every call-site
 * (ToolApprovalDialog, AgentApprovalToast, InteractionTriageDialog, …)
 * gets a consistent look.
 */
export default function YamlPreview(props: YamlPreviewProps) {
  const [copied, setCopied] = createSignal(false);

  const maxH = () => props.maxHeight ?? '200px';

  const handleCopy = async () => {
    try {
      // Build a plain-text version by stripping HTML from the highlighted output
      const el = document.createElement('div');
      el.innerHTML = highlightYaml(props.data);
      await navigator.clipboard.writeText(el.textContent ?? '');
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      /* clipboard may be unavailable in some contexts — fail silently */
    }
  };

  return (
    <div class={`relative ${props.class ?? ''}`}>
      <Show when={props.copyButton}>
        <button
          class="absolute right-2 top-2 rounded p-1 text-muted-foreground hover:text-foreground hover:bg-accent/40 transition-colors border-none bg-transparent cursor-pointer z-10"
          onClick={handleCopy}
          title="Copy to clipboard"
          type="button"
        >
          {copied() ? <Check size={14} /> : <Copy size={14} />}
        </button>
      </Show>
      <YamlBlock
        data={props.data}
        class="overflow-auto whitespace-pre-wrap break-all rounded-md bg-black/30 p-2 font-mono text-xs text-muted-foreground"
        style={`max-height:${maxH()};`}
      />
    </div>
  );
}
