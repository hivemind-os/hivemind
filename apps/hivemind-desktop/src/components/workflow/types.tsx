import type { JSX } from 'solid-js';
import { For, Show } from 'solid-js';
import { Play, Bell, Inbox, Wrench, Bot, Hand, Timer, Radio, RotateCcw, Calendar, PenLine, GitBranch, Repeat, RotateCw, Flag, ScrollText } from 'lucide-solid';

// ── Shared Types ───────────────────────────────────────────────────────

export interface ToolDefinitionProp {
  id: string;
  name: string;
  description: string;
  input_schema: Record<string, unknown>;
  output_schema: Record<string, unknown> | null;
}

export interface PromptTemplateProp {
  id: string;
  name: string;
  description?: string;
  template: string;
  input_schema?: Record<string, any>;
}

export interface PersonaProp {
  id: string;
  name: string;
  description?: string;
  allowed_tools?: string[];
  prompts?: PromptTemplateProp[];
}

export interface ChannelProp {
  id: string;
  name: string;
  provider?: string;
  hasComms?: boolean;
  connector_id?: string;
}

export interface DesignerNode {
  id: string;
  type: 'trigger' | 'task' | 'control_flow';
  subtype: string;
  x: number;
  y: number;
  config: Record<string, any>;
  outputs: Record<string, string>;
  onError: { strategy: string; max_retries?: number; delay_secs?: number; fallback_step?: string } | null;
  riskLevel?: 'safe' | 'caution' | 'danger' | 'unknown';
}

export interface DesignerEdge {
  id: string;
  source: string;
  target: string;
  label?: string;
  edgeType?: 'then' | 'else' | 'body' | 'default';
}

export interface WorkflowVariable {
  name: string;
  varType: 'string' | 'number' | 'boolean' | 'object' | 'array';
  description: string;
  required: boolean;
  defaultValue: string;
  enumValues: string[];
  minLength?: number;
  maxLength?: number;
  pattern?: string;
  minimum?: number;
  maximum?: number;
  itemsType?: string;
  itemProperties?: WorkflowVariable[];
  properties?: WorkflowVariable[];
  xUi?: { widget?: string; [key: string]: any };
}

export interface WorkflowAttachment {
  id: string;
  filename: string;
  description: string;
  media_type?: string;
  size_bytes?: number;
}

// ── Shared Constants ───────────────────────────────────────────────────

export const NODE_CATEGORY_COLORS: Record<string, { bg: string; border: string }> = {
  trigger: { bg: 'hsl(var(--muted))', border: 'hsl(var(--chart-1, 217 91% 60%))' },
  task: { bg: 'hsl(var(--muted))', border: 'hsl(var(--chart-2, 142 71% 45%))' },
  control_flow: { bg: 'hsl(var(--muted))', border: 'hsl(var(--chart-4, 38 92% 50%))' },
};

// ── Shared Styles ──────────────────────────────────────────────────────

export const inputStyle = {
  background: 'hsl(var(--background))',
  color: 'hsl(var(--foreground))',
  border: '1px solid hsl(var(--border))',
  'border-radius': '4px',
  padding: '4px 8px',
  'font-size': '0.85em',
  width: '100%',
  'box-sizing': 'border-box' as const,
  'font-family': 'inherit',
};

export const labelStyle = {
  'font-size': '0.82em',
  'font-weight': '500',
  color: 'hsl(var(--muted-foreground))',
  'margin-bottom': '2px',
  'margin-top': '6px',
};

// ── Shared Components ──────────────────────────────────────────────────

export function SubtypeIcon(props: { subtype: string; size?: number }): JSX.Element {
  const s = () => props.size ?? 14;
  switch (props.subtype) {
    case 'manual': return <Play size={s()} />;
    case 'event': return <Bell size={s()} />;
    case 'incoming_message': return <Inbox size={s()} />;
    case 'schedule': return <Calendar size={s()} />;
    case 'call_tool': return <Wrench size={s()} />;
    case 'invoke_agent': return <Bot size={s()} />;
    case 'invoke_prompt': return <ScrollText size={s()} />;
    case 'feedback_gate': return <Hand size={s()} />;
    case 'delay': return <Timer size={s()} />;
    case 'signal_agent': return <Radio size={s()} />;
    case 'launch_workflow': return <RotateCcw size={s()} />;
    case 'schedule_task': return <Calendar size={s()} />;
    case 'event_gate': return <Bell size={s()} />;
    case 'set_variable': return <PenLine size={s()} />;
    case 'branch': return <GitBranch size={s()} />;
    case 'for_each': return <Repeat size={s()} />;
    case 'while': return <RotateCw size={s()} />;
    case 'end_workflow': return <Flag size={s()} />;
    default: return <Wrench size={s()} />;
  }
}

export function EnumEditor(props: {
  values: string[];
  onUpdate: (vals: string[]) => void;
  disabled?: boolean;
}) {
  let inputRef: HTMLInputElement | undefined;

  function addValue() {
    const val = inputRef?.value?.trim();
    if (val && !props.values.includes(val)) {
      props.onUpdate([...props.values, val]);
      if (inputRef) inputRef.value = '';
    }
  }

  return (
    <div class="wf-enum-editor">
      <div class="wf-enum-tags">
        <For each={props.values}>
          {(val, i) => (
            <span class="wf-enum-tag">
              {val}
              <Show when={!props.disabled}>
                <button
                  class="wf-enum-tag-remove"
                  onClick={() => props.onUpdate(props.values.filter((_, idx) => idx !== i()))}
                >✕</button>
              </Show>
            </span>
          )}
        </For>
      </div>
      <Show when={!props.disabled}>
        <div class="wf-enum-add">
          <input
            ref={inputRef}
            class="wf-launch-input"
            placeholder="Add value…"
            onKeyDown={(e) => { if (e.key === 'Enter') { e.preventDefault(); addValue(); } }}
          />
          <button class="wf-btn-secondary" style="padding:4px 10px;font-size:0.8em;" onClick={addValue}>Add</button>
        </div>
      </Show>
    </div>
  );
}
