import { For, createSignal, createMemo } from 'solid-js';
import { Popover, PopoverAnchor, PopoverContent } from '~/ui/popover';

export interface TopicInfo {
  topic: string;
  description: string;
  payload_keys?: string[];
}

export interface TopicSelectorProps {
  value: string;
  onChange: (topic: string) => void;
  topics: TopicInfo[];
  disabled?: boolean;
  placeholder?: string;
}

export default function TopicSelector(props: TopicSelectorProps) {
  const [search, setSearch] = createSignal('');
  const [open, setOpen] = createSignal(false);

  const filtered = createMemo(() => {
    const q = search().toLowerCase();
    if (!q) return props.topics;
    return props.topics.filter(t => t.topic.toLowerCase().includes(q) || t.description.toLowerCase().includes(q));
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
          onInput={(e) => { setSearch(e.currentTarget.value); if (!open()) setOpen(true); }}
          onFocus={() => { setSearch(props.value); setOpen(true); }}
          onBlur={() => setTimeout(() => setOpen(false), 200)}
          placeholder={props.placeholder ?? 'Select or type event topic'}
          disabled={props.disabled}
          style="width:100%;padding:6px 10px;border-radius:4px;border:1px solid hsl(var(--border));background:hsl(var(--background));color:hsl(var(--foreground));font-size:0.85em;box-sizing:border-box;"
        />
      </PopoverAnchor>
      <PopoverContent class="w-auto p-0" style={{
        'z-index': '10000',
        'max-height': '200px',
        'overflow-y': 'auto',
        background: 'hsl(var(--card))',
        border: '1px solid hsl(var(--border))',
        'border-radius': '6px',
        'box-shadow': '0 4px 12px hsl(var(--foreground) / 0.12)',
      }}>
      <For each={filtered()}>
        {(t) => (
          <div
            onMouseDown={(e) => { e.preventDefault(); props.onChange(t.topic); setSearch(''); setOpen(false); }}
            style="padding:6px 10px;cursor:pointer;font-size:0.82em;color:hsl(var(--foreground));"
            class="tool-dropdown-item"
          >
            <div style="font-weight:500;">{t.topic}</div>
            <div style="font-size:0.85em;color:hsl(var(--muted-foreground));">{t.description}</div>
          </div>
        )}
      </For>
      </PopoverContent>
    </Popover>
  );
}

/** Extract payload keys for a topic, including wildcard matching */
export function payloadKeysForTopic(topic: string, allTopics: TopicInfo[]): string[] {
  if (!topic) return [];
  const exact = allTopics.find(t => t.topic === topic);
  if (exact?.payload_keys?.length) return exact.payload_keys;
  const keys = new Set<string>();
  for (const t of allTopics) {
    if (!t.payload_keys?.length) continue;
    const topicParts = topic.replace(/\.\*$/, '').split('.');
    const entryParts = t.topic.split('.');
    if (entryParts.length >= topicParts.length && topicParts.every((p, i) => p === '*' || p === entryParts[i])) {
      t.payload_keys.forEach(k => keys.add(k));
    }
  }
  return [...keys];
}
