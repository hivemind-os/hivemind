import { For, Show, Index, createSignal, createMemo } from 'solid-js';
import { ClipboardList } from 'lucide-solid';
import { Popover, PopoverTrigger, PopoverContent } from '~/ui/popover';

export interface ToolDefinitionInfo {
  id: string;
  name: string;
  description: string;
  input_schema: Record<string, unknown>;
}

export interface ChannelInfo {
  id: string;
  name: string;
  provider: string;
}

export interface SchemaField {
  key: string;
  label?: string;
  type: string;
  description: string;
  required: boolean;
  enumValues?: string[];
}

export interface ToolCallBuilderProps {
  tool_id: string;
  arguments: Record<string, any>;
  onToolChange: (tool_id: string) => void;
  onArgsChange: (args: Record<string, any>) => void;
  tools: ToolDefinitionInfo[];
  channels?: ChannelInfo[];
  disabled?: boolean;
}

export function getSchemaProperties(tool: ToolDefinitionInfo | undefined): SchemaField[] {
  if (!tool?.input_schema) return [];
  const schema = tool.input_schema as any;
  const props = schema.properties || {};
  const req = new Set<string>(schema.required || []);
  return Object.entries(props).map(([key, prop]: [string, any]) => ({
    key,
    label: prop['x-ui']?.label,
    type: prop.type || 'string',
    description: prop.description || '',
    required: req.has(key),
    enumValues: prop.enum,
  }));
}

export default function ToolCallBuilder(props: ToolCallBuilderProps) {
  const [showDropdown, setShowDropdown] = createSignal(false);
  const [search, setSearch] = createSignal('');
  const [jsonMode, setJsonMode] = createSignal(false);
  const [jsonText, setJsonText] = createSignal('{}');
  const ro = () => props.disabled ?? false;
  const selectedTool = createMemo(() => props.tools.find(t => t.id === props.tool_id));
  const schemaFields = createMemo(() => getSchemaProperties(selectedTool()));

  const filtered = createMemo(() => {
    const q = search().toLowerCase();
    if (!q) return props.tools;
    return props.tools.filter(t =>
      t.name.toLowerCase().includes(q) || t.id.toLowerCase().includes(q) || (t.description || '').toLowerCase().includes(q)
    );
  });

  function selectTool(tool_id: string) {
    props.onToolChange(tool_id);
    props.onArgsChange({});
    setShowDropdown(false);
    setSearch('');
    setJsonText('{}');
  }

  function getArgValue(key: string): string {
    const val = props.arguments?.[key];
    if (val === undefined || val === null) return '';
    return typeof val === 'object' ? JSON.stringify(val) : String(val);
  }

  function setArgField(key: string, value: string, type: string) {
    const args = { ...props.arguments };
    if (value === '') { delete args[key]; }
    else if (type === 'number' || type === 'integer') { const n = value.includes('.') ? parseFloat(value) : parseInt(value, 10); args[key] = isNaN(n) ? value : n; }
    else if (type === 'boolean') { args[key] = value === 'true'; }
    else if (type === 'object' || type === 'array') { try { args[key] = JSON.parse(value); } catch { args[key] = value; } }
    else { args[key] = value; }
    props.onArgsChange(args);
  }

  const inputSt = "padding:4px 8px;border-radius:4px;border:1px solid hsl(var(--border));background:hsl(var(--background));color:hsl(var(--foreground));font-size:0.82em;width:100%;box-sizing:border-box;";

  return (
    <div style="display:flex;flex-direction:column;gap:8px;">
      {/* Tool picker */}
      <Popover
        open={showDropdown()}
        onOpenChange={(isOpen) => {
          if (ro() && isOpen) return;
          setShowDropdown(isOpen);
          if (!isOpen) setSearch('');
        }}
        placement="bottom"
        sameWidth
      >
        <PopoverTrigger as="div" style={{ display: 'block' }}>
          <div
            style="padding:6px 10px;border-radius:4px;border:1px solid hsl(var(--border));background:hsl(var(--background));color:hsl(var(--foreground));font-size:0.85em;cursor:pointer;display:flex;align-items:center;justify-content:space-between;"
          >
            <span style="overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">
              {selectedTool() ? <><span style="font-weight:600;">{selectedTool()!.name}</span> <span style="color:hsl(var(--muted-foreground));font-size:0.9em;">{selectedTool()!.id}</span></> : 'Select tool…'}
            </span>
            <span style="font-size:0.7em;color:hsl(var(--muted-foreground));margin-left:8px;">{showDropdown() ? '▲' : '▼'}</span>
          </div>
        </PopoverTrigger>
        <PopoverContent class="w-auto p-0" style={{
          'z-index': '10000',
          'max-height': '220px',
          'overflow-y': 'auto',
          background: 'hsl(var(--card))',
          border: '1px solid hsl(var(--border))',
          'border-radius': '0 0 6px 6px',
          'box-shadow': '0 6px 16px hsl(0 0% 0% / 0.5)',
        }}>
        <div style="padding:4px;border-bottom:1px solid hsl(var(--border));">
          <input
            ref={(el) => requestAnimationFrame(() => el?.focus())}
            style={inputSt}
            placeholder="Search tools…"
            value={search()}
            onInput={(e) => setSearch(e.currentTarget.value)}
            onKeyDown={(e) => { if (e.key === 'Escape') setShowDropdown(false); }}
          />
        </div>
        <For each={filtered()}>
          {(tool) => (
            <div
              onMouseDown={(e) => { e.preventDefault(); selectTool(tool.id); }}
              style={{
                padding: '6px 8px', cursor: 'pointer', 'font-size': '0.8em',
                color: 'hsl(var(--foreground))',
                background: tool.id === props.tool_id ? 'hsl(var(--primary) / 0.12)' : 'none',
              }}
              class="tool-dropdown-item"
            >
              <div style="font-weight:500;">{tool.name}</div>
              <div style="font-size:0.85em;color:hsl(var(--muted-foreground));white-space:nowrap;overflow:hidden;text-overflow:ellipsis;">{tool.description || tool.id}</div>
            </div>
          )}
        </For>
        <Show when={filtered().length === 0}>
          <div style="padding:10px;font-size:0.8em;color:hsl(var(--muted-foreground));text-align:center;">No tools found</div>
        </Show>
        </PopoverContent>
      </Popover>

      {/* Tool description */}
      <Show when={selectedTool()?.description}>
        {(desc) => (
          <div style="font-size:0.72em;color:hsl(var(--muted-foreground));font-style:italic;">
            {desc()}
          </div>
        )}
      </Show>

      {/* Arguments section */}
      <Show when={props.tool_id}>
        <div style="display:flex;align-items:center;gap:8px;">
          <span style="font-size:0.78em;color:hsl(var(--muted-foreground));">Arguments</span>
          <Show when={schemaFields().length > 0}>
            <button
              onClick={() => {
                const next = !jsonMode();
                setJsonMode(next);
                if (next) setJsonText(JSON.stringify(props.arguments ?? {}, null, 2));
              }}
              style="padding:1px 6px;border-radius:3px;border:1px solid hsl(var(--border));background:transparent;color:hsl(var(--primary));cursor:pointer;font-size:0.7em;"
            >{jsonMode() ? <><ClipboardList size={12} /> Form</> : '{ } JSON'}</button>
          </Show>
        </div>

        {/* JSON mode */}
        <Show when={jsonMode() || schemaFields().length === 0}>
          <textarea
            value={jsonMode() ? jsonText() : JSON.stringify(props.arguments ?? {}, null, 2)}
            onInput={(e) => {
              setJsonText(e.currentTarget.value);
              try { props.onArgsChange(JSON.parse(e.currentTarget.value)); } catch { /* only update on valid JSON */ }
            }}
            placeholder='{"key": "value"}'
            rows={4}
            style="padding:6px 10px;border-radius:4px;border:1px solid hsl(var(--border));background:hsl(var(--background));color:hsl(var(--foreground));font-size:0.82em;resize:vertical;font-family:monospace;"
            disabled={ro()}
          />
        </Show>

        {/* Form mode */}
        <Show when={!jsonMode() && schemaFields().length > 0}>
          <div style="display:flex;flex-direction:column;gap:6px;">
            <Index each={schemaFields()}>
              {(field) => {
                const channelList = () => (field().key === 'connector_id' || field().key === 'channel_id') ? (props.channels ?? []) : [];
                return (
                  <div>
                    <div style="display:flex;align-items:center;gap:4px;margin-bottom:2px;">
                      <label style="font-size:0.76em;color:hsl(var(--foreground));font-weight:500;">
                        {field().label || field().key}
                        {field().required ? <span class="text-amber-500 ml-0.5">*</span> : null}
                      </label>
                      <span style="font-size:0.65em;color:hsl(var(--muted-foreground));">
                        {field().type === 'integer' ? 'number' : field().type}
                      </span>
                    </div>
                    <Show when={field().description}>
                      <div style="font-size:0.65em;color:hsl(var(--muted-foreground));margin-bottom:2px;">{field().description}</div>
                    </Show>
                    {/* Channel selector */}
                    <Show when={channelList().length > 0} fallback={
                      /* Enum selector */
                      <Show when={field().enumValues && field().enumValues!.length > 0} fallback={
                        /* Boolean selector */
                        <Show when={field().type === 'boolean'} fallback={
                          /* Text/number input */
                          <input
                            style={inputSt}
                            type={field().type === 'number' || field().type === 'integer' ? 'number' : 'text'}
                            value={getArgValue(field().key)}
                            onInput={(e) => setArgField(field().key, e.currentTarget.value, field().type)}
                            disabled={ro()}
                            placeholder={field().type === 'object' || field().type === 'array' ? 'JSON value' : ''}
                          />
                        }>
                          {(_) => (
                            <select style={inputSt} value={getArgValue(field().key)} onChange={(e) => setArgField(field().key, e.currentTarget.value, 'boolean')} disabled={ro()}>
                              <option value="">—</option>
                              <option value="true">true</option>
                              <option value="false">false</option>
                            </select>
                          )}
                        </Show>
                      }>
                        {(_) => (
                          <select style={inputSt} value={getArgValue(field().key)} onChange={(e) => setArgField(field().key, e.currentTarget.value, field().type)} disabled={ro()}>
                            <option value="">—</option>
                            <For each={field().enumValues!}>{(v) => <option value={v}>{v}</option>}</For>
                          </select>
                        )}
                      </Show>
                    }>
                      {(_) => (
                        <select style={inputSt} value={getArgValue(field().key)} onChange={(e) => setArgField(field().key, e.currentTarget.value, field().type)} disabled={ro()}>
                          <option value="">Select a channel…</option>
                          <For each={channelList()}>{(ch) => <option value={ch.id}>{ch.name}{ch.provider ? ` (${ch.provider})` : ''}</option>}</For>
                        </select>
                      )}
                    </Show>
                  </div>
                );
              }}
            </Index>
          </div>
        </Show>
      </Show>
    </div>
  );
}
