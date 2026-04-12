import { Index, For, createSignal, createMemo, type Accessor, type Setter } from 'solid-js';
import { Popover, PopoverAnchor, PopoverContent } from '~/ui/popover';
import { Button } from '~/ui';

export type PermissionRule = { tool_pattern: string; scope: string; decision: string };
export type PermissionRules = { rules: PermissionRule[] };

export type ToolDef = { id: string; name: string; description?: string };

interface PermissionRulesEditorProps {
  rules: Accessor<PermissionRule[]>;
  setRules: (rules: PermissionRule[]) => void;
  toolDefinitions?: ToolDef[];
}

function ToolPatternInput(props: {
  value: string;
  onChange: (v: string) => void;
  toolDefinitions?: ToolDef[];
}) {
  const [open, setOpen] = createSignal(false);
  const [search, setSearch] = createSignal('');

  const filtered = createMemo(() => {
    const defs = props.toolDefinitions;
    if (!defs?.length) return [];
    const q = search().toLowerCase();
    if (!q) return defs.slice(0, 50);
    return defs.filter(t =>
      t.id.toLowerCase().includes(q) ||
      t.name.toLowerCase().includes(q) ||
      (t.description || '').toLowerCase().includes(q)
    ).slice(0, 50);
  });

  return (
    <Popover
      open={open() && filtered().length > 0}
      onOpenChange={(o) => { if (!o) setOpen(false); }}
      placement="bottom-start"
      gutter={2}
      sameWidth
    >
      <PopoverAnchor as="div" style={{ display: 'block' }}>
        <input
          type="text"
          value={open() ? search() : props.value}
          onInput={(e) => {
            const v = e.currentTarget.value;
            props.onChange(v);
            setSearch(v);
            if (!open()) setOpen(true);
          }}
          onFocus={() => { setSearch(props.value); setOpen(true); }}
          onBlur={() => setTimeout(() => setOpen(false), 200)}
          onKeyDown={(e) => { if (e.key === 'Escape') setOpen(false); }}
          class="w-full rounded border border-input bg-secondary px-1.5 py-0.5 text-xs text-foreground"
          placeholder="* or tool id…"
        />
      </PopoverAnchor>
      <PopoverContent
        onOpenAutoFocus={(e: Event) => e.preventDefault()}
        onFocusOutside={(e: Event) => e.preventDefault()}
        class="w-auto p-0" style={{
        'z-index': '10000',
        'max-height': '180px',
        'overflow-y': 'auto',
        background: 'hsl(var(--card))',
        border: '1px solid hsl(var(--border))',
        'border-radius': '0 0 6px 6px',
        'box-shadow': '0 6px 16px rgba(0,0,0,0.5)',
      }}>
        <For each={filtered()}>
          {(tool) => (
            <div
              onMouseDown={(e) => {
                e.preventDefault();
                props.onChange(tool.id);
                setSearch('');
                setOpen(false);
              }}
              class="cursor-pointer px-1.5 py-1 text-xs hover:bg-accent"
            >
              <div class="font-medium">{tool.name}</div>
              <div class="truncate text-[0.7rem] text-muted-foreground">{tool.id}</div>
            </div>
          )}
        </For>
      </PopoverContent>
    </Popover>
  );
}

export default function PermissionRulesEditor(props: PermissionRulesEditorProps) {
  return (
    <div>
      <table class="w-full border-collapse text-xs">
        <thead>
          <tr class="border-b border-input">
            <th class="px-1 py-0.5 text-left">Tool Pattern</th>
            <th class="px-1 py-0.5 text-left">Scope</th>
            <th class="px-1 py-0.5 text-left">Decision</th>
            <th class="w-6" />
          </tr>
        </thead>
        <tbody>
          {/* Index tracks by position — DOM nodes are stable when data changes */}
          <Index each={props.rules()}>
            {(rule, idx) => (
              <tr class="border-b border-input">
                <td class="px-1 py-0.5">
                  <ToolPatternInput
                    value={rule().tool_pattern}
                    onChange={(v) => {
                      const rules = [...props.rules()];
                      rules[idx] = { ...rules[idx], tool_pattern: v };
                      props.setRules(rules);
                    }}
                    toolDefinitions={props.toolDefinitions}
                  />
                </td>
                <td class="px-1 py-0.5">
                  <input
                    type="text"
                    value={rule().scope}
                    onInput={(e) => {
                      const rules = [...props.rules()];
                      rules[idx] = { ...rules[idx], scope: e.currentTarget.value };
                      props.setRules(rules);
                    }}
                    class="w-full rounded border border-input bg-secondary px-1.5 py-0.5 text-xs text-foreground"
                  />
                </td>
                <td class="px-1 py-0.5">
                  <select
                    value={rule().decision}
                    onChange={(e) => {
                      const rules = [...props.rules()];
                      rules[idx] = { ...rules[idx], decision: e.currentTarget.value };
                      props.setRules(rules);
                    }}
                    class="text-xs"
                  >
                    <option value="auto">Auto</option>
                    <option value="ask">Ask</option>
                    <option value="deny">Deny</option>
                  </select>
                </td>
                <td class="px-1 py-0.5">
                  <button
                    onClick={() => props.setRules(props.rules().filter((_, i) => i !== idx))}
                    class="cursor-pointer border-none bg-transparent text-xs text-muted-foreground hover:text-destructive"
                    title="Remove rule"
                  >✕</button>
                </td>
              </tr>
            )}
          </Index>
        </tbody>
      </table>
      <Button
        variant="outline"
        size="sm"
        class="mt-1.5 text-xs"
        onClick={() => props.setRules([...props.rules(), { tool_pattern: '*', scope: '*', decision: 'ask' }])}
      >+ Add Rule</Button>
    </div>
  );
}
