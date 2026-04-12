import { For, Show, Index, createSignal, createMemo, type JSX } from 'solid-js';
import { Popover, PopoverTrigger, PopoverContent } from '~/ui/popover';
import { Switch, SwitchControl, SwitchThumb, SwitchLabel } from '~/ui/switch';
import { Button } from '~/ui/button';
import { evaluateFieldCondition } from '~/lib/formConditions';
import { buildNamespaceTree, type NamespaceNode } from '~/lib/workflowGrouping';

export interface TriggerInput {
  name: string;
  input_type: string;
  required: boolean;
  default?: any;
  description?: string;
  enum?: string[];
  minLength?: number;
  maxLength?: number;
  minimum?: number;
  maximum?: number;
  pattern?: string;
  xUi?: { widget?: string; [key: string]: any };
}

export interface ManualTriggerOption {
  stepId: string;
  label: string;
  schema: TriggerInput[];
}

export interface WorkflowDefSummary {
  name: string;
  version: string;
  description: string | null;
}

export interface WorkflowLaunchValue {
  definition: string;
  version?: string;
  inputs: Record<string, any>;
  trigger_step_id?: string;
}

export interface WorkflowLauncherProps {
  definitions: WorkflowDefSummary[];
  /** Called when the user picks a definition — should return the parsed YAML definition object. */
  fetchParsedDefinition: (name: string) => Promise<{ definition: any } | null>;
  value: WorkflowLaunchValue | null;
  onChange: (value: WorkflowLaunchValue) => void;
  disabled?: boolean;
}

/** Extract manual trigger options from a parsed workflow definition. */
export function extractManualTriggers(def: any): ManualTriggerOption[] {
  const steps: any[] = def.steps || [];
  const options: ManualTriggerOption[] = [];

  for (const step of steps) {
    if (step.type !== 'trigger') continue;
    const trigDef = step.trigger;
    if (!trigDef || trigDef.type !== 'manual') continue;

    let inputFields: TriggerInput[] = [];
    if (trigDef.input_schema && typeof trigDef.input_schema === 'object' && trigDef.input_schema.properties) {
      const schemaProps = trigDef.input_schema.properties;
      const schemaRequired: string[] = trigDef.input_schema.required || [];
      for (const [pName, pDef] of Object.entries(schemaProps as Record<string, any>)) {
        inputFields.push({
          name: pName,
          input_type: pDef?.type || 'string',
          required: schemaRequired.includes(pName),
          default: pDef?.default,
          description: pDef?.description,
          enum: Array.isArray(pDef?.enum) ? pDef.enum : undefined,
          minLength: pDef?.minLength,
          maxLength: pDef?.maxLength,
          minimum: pDef?.minimum,
          maximum: pDef?.maximum,
          pattern: pDef?.pattern,
          xUi: pDef?.['x-ui'] && typeof pDef['x-ui'] === 'object' ? pDef['x-ui'] : undefined,
        });
      }
    } else {
      inputFields = (trigDef.inputs || []).map((inp: any) => ({
        name: inp.name,
        input_type: inp.input_type || 'string',
        required: inp.required || false,
        default: inp.default,
        description: inp.description,
        enum: inp.enum,
        minLength: inp.minLength,
        maxLength: inp.maxLength,
        minimum: inp.minimum,
        maximum: inp.maximum,
        pattern: inp.pattern,
      }));
      const vars = def.variables;
      if (vars?.properties) {
        for (const inp of inputFields) {
          const varSchema = vars.properties[inp.name];
          if (varSchema) {
            if (!inp.description && varSchema.description) inp.description = varSchema.description;
            if (!inp.enum && varSchema.enum) inp.enum = varSchema.enum;
            if (inp.minimum == null && varSchema.minimum != null) inp.minimum = varSchema.minimum;
            if (inp.maximum == null && varSchema.maximum != null) inp.maximum = varSchema.maximum;
            if (inp.minLength == null && varSchema.minLength != null) inp.minLength = varSchema.minLength;
            if (inp.maxLength == null && varSchema.maxLength != null) inp.maxLength = varSchema.maxLength;
            if (inp.pattern == null && varSchema.pattern) inp.pattern = varSchema.pattern;
          }
        }
      }
    }

    options.push({ stepId: step.id, label: step.id, schema: inputFields });
  }

  return options;
}

export default function WorkflowLauncher(props: WorkflowLauncherProps) {
  const [open, setOpen] = createSignal(false);
  const [search, setSearch] = createSignal('');
  const [loading, setLoading] = createSignal(false);
  const [triggers, setTriggers] = createSignal<ManualTriggerOption[]>([]);
  const [selectedTrigger, setSelectedTrigger] = createSignal<ManualTriggerOption | null>(null);
  const [inputValues, setInputValues] = createSignal<Record<string, any>>({});
  const [jsonMode, setJsonMode] = createSignal(false);
  const [jsonText, setJsonText] = createSignal('{}');

  const ro = () => props.disabled ?? false;

  const selectedDef = () => props.definitions.find(d => d.name === props.value?.definition);

  const filtered = () => {
    const q = search().toLowerCase();
    if (!q) return props.definitions;
    return props.definitions.filter(d =>
      d.name.toLowerCase().includes(q) || (d.description || '').toLowerCase().includes(q)
    );
  };

  const filteredTree = () => buildNamespaceTree(filtered());
  const [collapsedNs, setCollapsedNs] = createSignal<Set<string>>(new Set());
  const toggleNs = (ns: string) => {
    setCollapsedNs(prev => {
      const next = new Set(prev);
      next.has(ns) ? next.delete(ns) : next.add(ns);
      return next;
    });
  };

  function emitChange(overrides?: Partial<WorkflowLaunchValue>) {
    const current = props.value ?? { definition: '', inputs: {} };
    const merged = { ...current, ...overrides };
    if (merged.definition) props.onChange(merged);
  }

  async function selectDefinition(name: string) {
    setOpen(false);
    setSearch('');
    setLoading(true);
    setTriggers([]);
    setSelectedTrigger(null);
    setInputValues({});

    emitChange({ definition: name, inputs: {}, trigger_step_id: undefined });

    try {
      const result = await props.fetchParsedDefinition(name);
      if (!result) { setLoading(false); return; }
      const options = extractManualTriggers(result.definition);
      setTriggers(options);
      if (options.length > 0) {
        selectTriggerOption(options[0], name);
      }
    } catch {
      // ignore
    }
    setLoading(false);
  }

  function selectTriggerOption(opt: ManualTriggerOption, defName?: string) {
    setSelectedTrigger(opt);
    const defaults: Record<string, any> = {};
    for (const inp of opt.schema) {
      if (inp.default != null) defaults[inp.name] = inp.default;
      else if (inp.input_type === 'boolean') defaults[inp.name] = false;
      else if (inp.input_type === 'number') defaults[inp.name] = 0;
      else defaults[inp.name] = '';
    }
    setInputValues(defaults);
    emitChange({
      definition: defName ?? props.value?.definition ?? '',
      inputs: defaults,
      trigger_step_id: opt.stepId,
    });
  }

  function updateInput(key: string, value: any) {
    const updated = { ...inputValues(), [key]: value };
    setInputValues(updated);
    emitChange({ inputs: updated });
  }

  const schema = () => selectedTrigger()?.schema ?? [];

  function renderNsNode(node: NamespaceNode<WorkflowDefSummary>, depth: number): JSX.Element {
    const countAll = (n: NamespaceNode<WorkflowDefSummary>): number =>
      n.items.length + n.children.reduce((sum, c) => sum + countAll(c), 0);
    const paddingLeft = `${10 + depth * 12}px`;
    const itemPaddingLeft = `${20 + depth * 12}px`;
    return (
      <div>
        <div
          onMouseDown={(e) => { e.preventDefault(); toggleNs(node.fullPath); }}
          style={`padding:5px 10px 5px ${paddingLeft};cursor:pointer;font-size:0.78em;font-weight:600;color:hsl(var(--muted-foreground));display:flex;align-items:center;gap:6px;user-select:none;text-transform:uppercase;letter-spacing:0.04em;border-bottom:1px solid hsl(var(--border) / 0.3);`}
        >
          <span style={`display:inline-block;transition:transform 0.15s;transform:${collapsedNs().has(node.fullPath) ? 'rotate(-90deg)' : 'rotate(0deg)'};font-size:0.9em;`}>▾</span>
          {node.segment}
          <span style="font-size:0.85em;opacity:0.7;">({countAll(node)})</span>
        </div>
        <Show when={!collapsedNs().has(node.fullPath)}>
          <For each={node.items}>
            {(d) => (
              <div
                onMouseDown={(e) => { e.preventDefault(); void selectDefinition(d.name); }}
                style={`padding:6px 10px 6px ${itemPaddingLeft};cursor:pointer;font-size:0.82em;color:hsl(var(--foreground));`}
                class="tool-dropdown-item"
              >
                <div style="font-weight:500;">{d.name}</div>
                <Show when={d.description}>
                  <div style="font-size:0.85em;color:hsl(var(--muted-foreground));overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">{d.description}</div>
                </Show>
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

  return (
    <div class="flex flex-col gap-2">
      {/* Definition picker */}
      <Popover
        open={open()}
        onOpenChange={(isOpen) => {
          if (ro() && isOpen) return;
          setOpen(isOpen);
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
              {selectedDef() ? `${selectedDef()!.name} — ${selectedDef()!.description?.slice(0, 60) ?? ''}` : 'Select workflow…'}
            </span>
            <span style="font-size:0.7em;color:hsl(var(--muted-foreground));margin-left:8px;">{open() ? '▲' : '▼'}</span>
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
            type="text"
            value={search()}
            onInput={(e) => setSearch(e.currentTarget.value)}
            onKeyDown={(e) => { if (e.key === 'Escape') setOpen(false); }}
            placeholder="Search workflows…"
            style="width:100%;padding:4px 8px;border-radius:4px;border:none;background:hsl(var(--background));color:hsl(var(--foreground));font-size:0.82em;box-sizing:border-box;"
          />
        </div>
        <For each={filteredTree()}>
          {(node) => renderNsNode(node, 0)}
        </For>
        <Show when={filtered().length === 0}>
          <div style="padding:8px 10px;font-size:0.8em;color:hsl(var(--muted-foreground));text-align:center;">No workflows found</div>
        </Show>
        </PopoverContent>
      </Popover>

      <Show when={loading()}>
        <div class="text-xs text-muted-foreground">Loading workflow schema…</div>
      </Show>

      <Show when={props.value?.definition && !loading()}>
        {/* Trigger selection when multiple manual triggers */}
        <Show when={triggers().length > 1}>
          <label class="text-xs text-muted-foreground">Trigger Step</label>
          <select
            value={selectedTrigger()?.stepId ?? ''}
            onChange={(e) => {
              const opt = triggers().find(t => t.stepId === e.currentTarget.value);
              if (opt) selectTriggerOption(opt);
            }}
            disabled={ro()}
            style="padding:4px 8px;border-radius:4px;border:1px solid hsl(var(--border));background:hsl(var(--background));color:hsl(var(--foreground));font-size:0.82em;"
          >
            <For each={triggers()}>
              {(opt) => <option value={opt.stepId}>{opt.label}</option>}
            </For>
          </select>
        </Show>

        <Show when={triggers().length === 0 && !loading()}>
          <div class="text-xs text-muted-foreground italic">
            No manual triggers found in this workflow.
          </div>
        </Show>

        {/* Input form */}
        <Show when={schema().length > 0}>
          <div class="flex items-center gap-2">
            <span class="text-xs text-muted-foreground">Inputs</span>
            <Button variant="outline" size="sm" class="h-5 px-1.5 text-[0.7em]"
              onClick={() => {
                if (!jsonMode()) setJsonText(JSON.stringify(inputValues(), null, 2));
                setJsonMode(!jsonMode());
              }}
            >{jsonMode() ? 'Form' : 'JSON'}</Button>
          </div>

          <Show when={jsonMode()}>
            <textarea
              value={jsonText()}
              onInput={(e) => {
                setJsonText(e.currentTarget.value);
                try { const parsed = JSON.parse(e.currentTarget.value); emitChange({ inputs: parsed }); } catch { /* wait for valid JSON */ }
              }}
              rows={4}
              disabled={ro()}
              style="padding:6px 10px;border-radius:4px;border:1px solid hsl(var(--border));background:hsl(var(--background));color:hsl(var(--foreground));font-size:0.85em;resize:vertical;font-family:monospace;"
            />
          </Show>

          <Show when={!jsonMode()}>
            <div class="flex flex-col gap-1.5">
              <Index each={schema()}>
                {(input) => (
                  <Show when={evaluateFieldCondition(input().xUi?.condition, inputValues())}>
                  <div class="flex flex-col gap-0.5">
                    <label class="text-xs text-muted-foreground">
                      {input().xUi?.label || input().name}
                      {input().required ? <span class="text-amber-500 ml-0.5">*</span> : null}
                      {input().description ? <span style="margin-left:6px;font-size:0.9em;opacity:0.7;">— {input().description?.slice(0, 60)}</span> : null}
                    </label>
                    {(() => {
                      const widget = input().xUi?.widget;
                      const inputSt = "padding:4px 8px;border-radius:4px;border:1px solid hsl(var(--border));background:hsl(var(--background));color:hsl(var(--foreground));font-size:0.82em;";
                      if (widget === 'textarea' || widget === 'code-editor') {
                        return (
                          <textarea
                            value={String(inputValues()[input().name] ?? '')}
                            placeholder={input().description || input().name}
                            maxLength={input().maxLength}
                            rows={input().xUi?.rows ?? 4}
                            onInput={(e) => updateInput(input().name, e.currentTarget.value)}
                            disabled={ro()}
                            style={inputSt + "resize:vertical;" + (widget === 'code-editor' ? "font-family:monospace;" : "")}
                          />
                        );
                      }
                      if (widget === 'password') {
                        return (
                          <input
                            type="password"
                            value={String(inputValues()[input().name] ?? '')}
                            placeholder={input().description || input().name}
                            maxLength={input().maxLength}
                            onInput={(e) => updateInput(input().name, e.currentTarget.value)}
                            disabled={ro()}
                            style={inputSt}
                          />
                        );
                      }
                      if (widget === 'date') {
                        return (
                          <input
                            type="date"
                            value={String(inputValues()[input().name] ?? '')}
                            onInput={(e) => updateInput(input().name, e.currentTarget.value)}
                            disabled={ro()}
                            style={inputSt}
                          />
                        );
                      }
                      if (widget === 'color-picker') {
                        return (
                          <input
                            type="color"
                            value={String(inputValues()[input().name] ?? '#000000')}
                            onInput={(e) => updateInput(input().name, e.currentTarget.value)}
                            disabled={ro()}
                            style="border:1px solid hsl(var(--border));background:hsl(var(--background));border-radius:4px;width:48px;height:28px;padding:2px;cursor:pointer;"
                          />
                        );
                      }
                      if (widget === 'slider' && input().input_type === 'number') {
                        return (
                          <div style="display:flex;align-items:center;gap:6px;">
                            <input
                              type="range"
                              value={inputValues()[input().name] ?? input().minimum ?? 0}
                              min={input().minimum ?? 0}
                              max={input().maximum ?? 100}
                              step={input().xUi?.step ?? 1}
                              onInput={(e) => updateInput(input().name, Number(e.currentTarget.value))}
                              disabled={ro()}
                              style="flex:1;"
                            />
                            <span style="font-size:0.82em;color:hsl(var(--foreground));min-width:28px;text-align:right;">
                              {inputValues()[input().name] ?? input().minimum ?? 0}
                            </span>
                          </div>
                        );
                      }
                      // Default type-based dispatch
                      if (input().enum && input().enum!.length > 0) {
                        return (
                          <select
                            value={String(inputValues()[input().name] ?? '')}
                            onChange={(e) => updateInput(input().name, e.currentTarget.value)}
                            disabled={ro()}
                            style={inputSt}
                          >
                            <option value="">—</option>
                            <For each={input().enum!}>{(v) => <option value={v}>{v}</option>}</For>
                          </select>
                        );
                      }
                      if (input().input_type === 'boolean') {
                        return (
                          <Switch checked={!!inputValues()[input().name]} onChange={(checked) => updateInput(input().name, checked)} disabled={ro()} class="flex items-center gap-2">
                            <SwitchControl><SwitchThumb /></SwitchControl>
                            <SwitchLabel>{input().name}</SwitchLabel>
                          </Switch>
                        );
                      }
                      if (input().input_type === 'number') {
                        return (
                          <input
                            type="number"
                            value={inputValues()[input().name] ?? ''}
                            min={input().minimum}
                            max={input().maximum}
                            onInput={(e) => updateInput(input().name, Number(e.currentTarget.value))}
                            disabled={ro()}
                            style={inputSt}
                          />
                        );
                      }
                      return (
                        <input
                          type="text"
                          value={String(inputValues()[input().name] ?? '')}
                          placeholder={input().description || input().name}
                          maxLength={input().maxLength}
                          onInput={(e) => updateInput(input().name, e.currentTarget.value)}
                          disabled={ro()}
                          style={inputSt}
                        />
                      );
                    })()}
                  </div>
                  </Show>
                )}
              </Index>
            </div>
          </Show>
        </Show>

        {/* Raw JSON fallback when no schema */}
        <Show when={schema().length === 0 && triggers().length > 0}>
          <div class="text-xs text-muted-foreground">
            No input schema defined. Enter raw JSON inputs:
          </div>
          <textarea
            value={jsonText()}
            onInput={(e) => {
              setJsonText(e.currentTarget.value);
              try { const parsed = JSON.parse(e.currentTarget.value); emitChange({ inputs: parsed }); } catch { /* wait */ }
            }}
            rows={3}
            disabled={ro()}
            placeholder='{}'
            style="padding:6px 10px;border-radius:4px;border:1px solid hsl(var(--border));background:hsl(var(--background));color:hsl(var(--foreground));font-size:0.85em;resize:vertical;font-family:monospace;"
          />
        </Show>
      </Show>
    </div>
  );
}
