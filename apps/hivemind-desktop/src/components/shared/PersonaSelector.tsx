import { For, Show, createSignal, createMemo, JSX } from 'solid-js';
import { ChevronUp, ChevronDown } from 'lucide-solid';
import { Popover, PopoverTrigger, PopoverContent } from '~/ui/popover';
import { buildNamespaceTree, NamespaceNode } from '~/lib/workflowGrouping';

export interface PersonaInfo {
  id: string;
  name: string;
  description?: string;
  archived?: boolean;
}

export interface PersonaSelectorProps {
  value: string;
  onChange: (persona_id: string) => void;
  personas: PersonaInfo[];
  disabled?: boolean;
}

export default function PersonaSelector(props: PersonaSelectorProps) {
  const [open, setOpen] = createSignal(false);
  const [search, setSearch] = createSignal('');

  const selected = () => props.personas.find(p => p.id === props.value);

  const filtered = createMemo(() => {
    const q = search().toLowerCase();
    const active = props.personas.filter(p => !p.archived);
    if (!q) return active;
    return active.filter(p => p.name.toLowerCase().includes(q) || p.id.toLowerCase().includes(q));
  });

  const filteredTree = createMemo(() => buildNamespaceTree(filtered(), (p) => p.id, (p) => p.name));
  const [collapsedNs, setCollapsedNs] = createSignal<Set<string>>(new Set());
  const toggleNs = (ns: string) => {
    setCollapsedNs(prev => {
      const next = new Set(prev);
      next.has(ns) ? next.delete(ns) : next.add(ns);
      return next;
    });
  };

  function toggle() {
    if (props.disabled) return;
    const next = !open();
    setOpen(next);
    if (next) setSearch('');
  }

  function select(id: string) {
    props.onChange(id);
    setOpen(false);
    setSearch('');
  }

  function countItems(node: NamespaceNode<PersonaInfo>): number {
    let c = node.items.length;
    for (const child of node.children) c += countItems(child);
    return c;
  }

  function renderNsNode(node: NamespaceNode<PersonaInfo>, depth: number): JSX.Element {
    return (
      <div>
        <div
          onMouseDown={(e) => { e.preventDefault(); toggleNs(node.fullPath); }}
          style={{
            padding: `5px 10px 5px ${5 + depth * 12}px`,
            cursor: 'pointer',
            'font-size': '0.78em',
            'font-weight': '600',
            color: 'hsl(var(--muted-foreground))',
            display: 'flex',
            'align-items': 'center',
            gap: '6px',
            'user-select': 'none',
            'text-transform': 'uppercase',
            'letter-spacing': '0.04em',
            'border-bottom': '1px solid hsl(var(--border) / 0.3)',
          }}
        >
          <span style={`display:inline-block;transition:transform 0.15s;transform:${collapsedNs().has(node.fullPath) ? 'rotate(-90deg)' : 'rotate(0deg)'};font-size:0.9em;`}>▾</span>
          {node.segment}
          <span style="font-size:0.85em;opacity:0.7;">({countItems(node)})</span>
        </div>
        <Show when={!collapsedNs().has(node.fullPath)}>
          <For each={node.items}>
            {(p) => (
              <div
                onMouseDown={(e) => { e.preventDefault(); select(p.id); }}
                style={{
                  padding: `6px 10px 6px ${20 + depth * 12}px`,
                  cursor: 'pointer',
                  'font-size': '0.82em',
                  color: 'hsl(var(--foreground))',
                  background: p.id === props.value ? 'hsl(var(--primary) / 0.12)' : 'none',
                }}
                class="tool-dropdown-item"
              >
                <div style="font-weight:500;">{p.name}</div>
                <div style="font-size:0.85em;color:hsl(var(--muted-foreground));">{p.id}</div>
              </div>
            )}
          </For>
          <For each={node.children}>
            {(child) => renderNsNode(child, depth + 1)}
          </For>
        </Show>
      </div>
    );
  }

  const dropdownStyle: Record<string, string> = {
    'max-height': '220px',
    'overflow-y': 'auto',
    background: 'hsl(var(--card))',
    border: '1px solid hsl(var(--border))',
    'border-radius': '6px',
    'box-shadow': '0 6px 16px hsl(var(--foreground) / 0.2)',
  };

  return (
    <Popover
      open={open()}
      onOpenChange={(o) => { setOpen(o); if (!o) setSearch(''); }}
      placement="bottom-start"
      gutter={2}
      sameWidth
    >
      <PopoverTrigger as="div"
        style="padding:6px 10px;border-radius:4px;border:1px solid hsl(var(--border));background:hsl(var(--background));color:hsl(var(--foreground));font-size:0.85em;cursor:pointer;display:flex;align-items:center;justify-content:space-between;"
      >
          <span style="overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">{selected() ? `${selected()!.name} (${selected()!.id})` : 'Select persona…'}</span>
          <span style="color:hsl(var(--muted-foreground));margin-left:8px;display:flex;align-items:center;">{open() ? <ChevronUp size={12} /> : <ChevronDown size={12} />}</span>
      </PopoverTrigger>
      <PopoverContent class="w-auto p-0" style={{ 'z-index': '10000', ...dropdownStyle }}>
      <div style="padding:4px;border-bottom:1px solid hsl(var(--border));">
        <input
          ref={(el) => requestAnimationFrame(() => el?.focus())}
          type="text"
          value={search()}
          onInput={(e) => setSearch(e.currentTarget.value)}
          placeholder="Search personas…"
          style="width:100%;padding:4px 8px;border-radius:4px;border:1px solid hsl(var(--border));background:hsl(var(--background));color:hsl(var(--foreground));font-size:0.82em;box-sizing:border-box;"
          onKeyDown={(e) => { if (e.key === 'Escape') { setOpen(false); setSearch(''); } }}
        />
      </div>
      <For each={filteredTree()}>
        {(node) => renderNsNode(node, 0)}
      </For>
      <Show when={filtered().length === 0}>
        <div style="padding:8px 10px;font-size:0.8em;color:hsl(var(--muted-foreground));text-align:center;">No personas found</div>
      </Show>
      </PopoverContent>
    </Popover>
  );
}
