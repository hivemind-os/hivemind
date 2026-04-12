import { Component, createSignal, For, Index, Show } from 'solid-js';
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter } from '~/ui/dialog';
import { Button } from '~/ui/button';
import { Tabs, TabsList, TabsTrigger, TabsContent } from '~/ui/tabs';

/* ── Type identical to WorkflowVariable (redefined locally) ── */
export interface PromptSchemaField {
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
  itemProperties?: PromptSchemaField[];
  properties?: PromptSchemaField[];
  xUi?: { widget?: string; [key: string]: any };
}

/* ── Constants ── */
const TYPE_LABELS: Record<string, string> = {
  'string': 'Text',
  'number': 'Number',
  'boolean': 'Boolean',
  'object': 'Object',
  'array': 'List',
};
const TYPE_VALUES = ['string', 'number', 'boolean', 'object', 'array'] as const;

/* ── Props ── */
export interface PromptSchemaEditorProps {
  fields: PromptSchemaField[];
  onChange: (fields: PromptSchemaField[]) => void;
}

/* ── Enum (tag) editor ── */
function renderEnumEditor(
  values: string[],
  onUpdate: (vals: string[]) => void,
) {
  let inputRef: HTMLInputElement | undefined;

  function addValue() {
    const val = inputRef?.value?.trim();
    if (val && !values.includes(val)) {
      onUpdate([...values, val]);
      if (inputRef) inputRef.value = '';
    }
  }

  return (
    <div class="wf-enum-editor">
      <div class="wf-enum-tags">
        <For each={values}>
          {(val, i) => (
            <span class="wf-enum-tag">
              {val}
              <button
                class="wf-enum-tag-remove"
                onClick={() => onUpdate(values.filter((_, idx) => idx !== i()))}
              >✕</button>
            </span>
          )}
        </For>
      </div>
      <div class="wf-enum-add">
        <input
          ref={inputRef}
          class="wf-launch-input"
          placeholder="Add value…"
          onKeyDown={(e) => { if (e.key === 'Enter') { e.preventDefault(); addValue(); } }}
        />
        <button class="wf-btn-secondary" style="padding:4px 10px;font-size:0.8em;" onClick={addValue}>Add</button>
      </div>
    </div>
  );
}

/* ── Main component ── */
const PromptSchemaEditor: Component<PromptSchemaEditorProps> = (props) => {
  const [open, setOpen] = createSignal(false);
  const [localVars, setLocalVars] = createSignal<PromptSchemaField[]>([]);
  const [activeTab, setActiveTab] = createSignal('0');

  /* ── Open dialog ── */
  function openDialog() {
    setLocalVars(JSON.parse(JSON.stringify(props.fields)));
    setActiveTab('0');
    setOpen(true);
  }

  /* ── OK / Cancel ── */
  function handleOk() {
    props.onChange(localVars());
    setOpen(false);
  }

  function handleCancel() {
    setOpen(false);
  }

  /* ── Local helpers ── */
  function addVar() {
    const newIdx = localVars().length;
    setLocalVars((prev) => [
      ...prev,
      {
        name: `param_${prev.length + 1}`,
        varType: 'string' as const,
        description: '',
        required: false,
        defaultValue: '',
        enumValues: [],
      },
    ]);
    setActiveTab(String(newIdx));
  }

  function removeVar(idx: number) {
    setLocalVars((prev) => prev.filter((_, i) => i !== idx));
    const remaining = localVars().length - 1;
    if (remaining <= 0) {
      setActiveTab('0');
    } else {
      const cur = Number(activeTab());
      if (cur >= remaining) setActiveTab(String(remaining - 1));
      else if (idx < cur) setActiveTab(String(cur - 1));
    }
  }

  function updateVar(idx: number, field: keyof PromptSchemaField, value: any) {
    setLocalVars((prev) => prev.map((v, i) => (i === idx ? { ...v, [field]: value } : v)));
  }

  function addNestedProp(varIdx: number) {
    setLocalVars((prev) =>
      prev.map((v, i) => {
        if (i !== varIdx) return v;
        const ps = v.properties ? [...v.properties] : [];
        ps.push({ name: `prop_${ps.length + 1}`, varType: 'string' as const, description: '', required: false, defaultValue: '', enumValues: [] });
        return { ...v, properties: ps };
      }),
    );
  }

  function updateNestedProp(varIdx: number, propIdx: number, field: keyof PromptSchemaField, value: any) {
    setLocalVars((prev) =>
      prev.map((v, i) => {
        if (i !== varIdx || !v.properties) return v;
        const ps = v.properties.map((p, pi) => (pi === propIdx ? { ...p, [field]: value } : p));
        return { ...v, properties: ps };
      }),
    );
  }

  function removeNestedProp(varIdx: number, propIdx: number) {
    setLocalVars((prev) =>
      prev.map((v, i) => {
        if (i !== varIdx || !v.properties) return v;
        return { ...v, properties: v.properties.filter((_, pi) => pi !== propIdx) };
      }),
    );
  }

  function addItemProp(varIdx: number) {
    setLocalVars((prev) =>
      prev.map((v, i) => {
        if (i !== varIdx) return v;
        const ps = v.itemProperties ? [...v.itemProperties] : [];
        ps.push({ name: `prop_${ps.length + 1}`, varType: 'string' as const, description: '', required: false, defaultValue: '', enumValues: [] });
        return { ...v, itemProperties: ps };
      }),
    );
  }

  function updateItemProp(varIdx: number, propIdx: number, field: keyof PromptSchemaField, value: any) {
    setLocalVars((prev) =>
      prev.map((v, i) => {
        if (i !== varIdx || !v.itemProperties) return v;
        const ps = v.itemProperties.map((p, pi) => (pi === propIdx ? { ...p, [field]: value } : p));
        return { ...v, itemProperties: ps };
      }),
    );
  }

  function removeItemProp(varIdx: number, propIdx: number) {
    setLocalVars((prev) =>
      prev.map((v, i) => {
        if (i !== varIdx || !v.itemProperties) return v;
        return { ...v, itemProperties: v.itemProperties.filter((_, pi) => pi !== propIdx) };
      }),
    );
  }

  /* ── Render ── */
  return (
    <div class="prompt-schema-section">
      <div style="font-weight:500;font-size:0.9em;margin-bottom:4px">
        Parameters ({props.fields.length} field{props.fields.length !== 1 ? 's' : ''})
      </div>
      <Show when={props.fields.length > 0}>
        <div style="font-size:0.8em;color:hsl(var(--muted-foreground));margin-bottom:4px">
          <For each={props.fields}>
            {(v) => <div>• {v.name}: {v.varType}{v.required ? ' (required)' : ''}</div>}
          </For>
        </div>
      </Show>
      <button
        class="wf-btn-secondary"
        style="padding:4px 12px;font-size:0.82em;"
        onClick={openDialog}
      >Edit Parameters</button>

      {/* ── Dialog ── */}
      <Dialog open={open()} onOpenChange={(o: boolean) => { if (!o) setOpen(false); }}>
        <DialogContent class="max-w-lg">
          <DialogHeader>
            <DialogTitle>Prompt Template Parameters</DialogTitle>
          </DialogHeader>

          <Show when={localVars().length === 0}>
            <p style="font-size:0.85em;color:hsl(var(--muted-foreground));text-align:center;padding:16px 0">
              No parameters yet.
            </p>
            <button
              class="wf-btn-secondary"
              style="align-self:center;padding:6px 14px;font-size:0.85em;"
              onClick={addVar}
            >+ Add input</button>
          </Show>

          <Show when={localVars().length > 0}>
            <Tabs value={activeTab()} onChange={(v: string) => setActiveTab(v)}>
              <div style="display:flex;align-items:center;gap:0">
                <TabsList style="flex:1;flex-wrap:wrap;justify-content:flex-start">
                  <Index each={localVars()}>
                    {(v, idx) => (
                      <TabsTrigger value={String(idx)} style="font-size:0.82em;padding:4px 10px">
                        {v().name || `param_${idx + 1}`}
                      </TabsTrigger>
                    )}
                  </Index>
                  <button
                    class="wf-btn-secondary"
                    style="padding:2px 10px;font-size:0.82em;margin-left:4px;border-radius:4px"
                    onClick={addVar}
                    title="Add parameter"
                  >+ Add</button>
                </TabsList>
              </div>

              <Index each={localVars()}>
                {(v, idx) => (
                  <TabsContent value={String(idx)}>
                    <div class="wf-var-card">
                  <div class="wf-var-card-header">
                    <input
                      class="wf-launch-input"
                      value={v().name}
                      onInput={(e) => updateVar(idx, 'name', e.currentTarget.value)}
                      placeholder="Parameter name"
                    />
                    <button
                      onClick={() => removeVar(idx)}
                      style="background:none;border:none;color:hsl(var(--muted-foreground));cursor:pointer;font-size:1em;padding:0 4px"
                      title="Delete parameter"
                    >✕</button>
                  </div>

                  <div class="wf-var-grid">
                    <div class="wf-var-field">
                      <label>Type</label>
                      <select
                        class="wf-launch-input"
                        value={v().varType}
                        onChange={(e) => updateVar(idx, 'varType', e.currentTarget.value)}
                      >
                        <For each={TYPE_VALUES}>
                          {(t) => <option value={t}>{TYPE_LABELS[t]}</option>}
                        </For>
                      </select>
                    </div>
                    <div class="wf-var-field" style="justify-content:flex-end">
                      <label style="display:flex;align-items:center;gap:6px;cursor:pointer">
                        <input
                          type="checkbox"
                          checked={v().required}
                          onChange={(e) => updateVar(idx, 'required', e.currentTarget.checked)}
                        />
                        Required
                      </label>
                    </div>
                    <div class="wf-var-field full-width">
                      <label>Label</label>
                      <input
                        class="wf-launch-input"
                        type="text"
                        value={v().xUi?.label ?? ''}
                        onInput={(e) => {
                          const label = e.currentTarget.value || undefined;
                          updateVar(idx, 'xUi', label ? { ...v().xUi, label } : (() => { const { label: _, ...rest } = v().xUi ?? {}; return Object.keys(rest).length > 0 ? rest : undefined; })());
                        }}
                        placeholder="Human-readable label (optional)"
                      />
                    </div>
                    <div class="wf-var-field full-width">
                      <label>Description</label>
                      <textarea
                        class="wf-launch-input"
                        value={v().description}
                        onInput={(e) => updateVar(idx, 'description', e.currentTarget.value)}
                        placeholder="Parameter description"
                        rows={1}
                        style="resize:vertical"
                      />
                    </div>
                    <div class="wf-var-field full-width">
                      <label>Default value</label>
                      {v().varType === 'boolean' ? (
                        <label style="display:flex;align-items:center;gap:6px;cursor:pointer;font-size:0.9em;padding:4px 0">
                          <input
                            type="checkbox"
                            checked={v().defaultValue === 'true'}
                            onChange={(e) => updateVar(idx, 'defaultValue', e.currentTarget.checked ? 'true' : 'false')}
                          />
                          {v().defaultValue === 'true' ? 'true' : 'false'}
                        </label>
                      ) : v().varType === 'number' ? (
                        <input
                          class="wf-launch-input"
                          type="number"
                          value={v().defaultValue}
                          onInput={(e) => updateVar(idx, 'defaultValue', e.currentTarget.value)}
                          placeholder="0"
                        />
                      ) : (
                        <input
                          class="wf-launch-input"
                          type="text"
                          value={v().defaultValue}
                          onInput={(e) => updateVar(idx, 'defaultValue', e.currentTarget.value)}
                          placeholder="default"
                        />
                      )}
                    </div>
                  </div>

                  {/* Widget override */}
                  <div class="wf-var-grid">
                    <div class="wf-var-field full-width">
                      <label>Widget</label>
                      <select
                        class="wf-launch-input"
                        value={v().xUi?.widget ?? ''}
                        onChange={(e) => {
                          const w = e.currentTarget.value;
                          updateVar(idx, 'xUi', w ? { ...v().xUi, widget: w } : undefined);
                        }}
                      >
                        <option value="">(default)</option>
                        <Show when={v().varType === 'string'}>
                          <option value="textarea">Textarea</option>
                          <option value="code-editor">Code Editor</option>
                          <option value="password">Password</option>
                          <option value="date">Date</option>
                          <option value="color-picker">Color Picker</option>
                        </Show>
                        <Show when={v().varType === 'number'}>
                          <option value="slider">Slider</option>
                        </Show>
                      </select>
                    </div>
                    <Show when={v().xUi?.widget === 'textarea' || v().xUi?.widget === 'code-editor'}>
                      <div class="wf-var-field">
                        <label>Rows</label>
                        <input
                          class="wf-launch-input"
                          type="number"
                          value={v().xUi?.rows ?? ''}
                          min={1}
                          max={50}
                          onInput={(e) => {
                            const rows = e.currentTarget.value ? Number(e.currentTarget.value) : undefined;
                            updateVar(idx, 'xUi', { ...v().xUi, rows });
                          }}
                          placeholder="4"
                        />
                      </div>
                    </Show>
                    <Show when={v().xUi?.widget === 'slider'}>
                      <div class="wf-var-field">
                        <label>Step</label>
                        <input
                          class="wf-launch-input"
                          type="number"
                          value={v().xUi?.step ?? ''}
                          min={0}
                          onInput={(e) => {
                            const step = e.currentTarget.value ? Number(e.currentTarget.value) : undefined;
                            updateVar(idx, 'xUi', { ...v().xUi, step });
                          }}
                          placeholder="1"
                        />
                      </div>
                    </Show>
                  </div>

                  {/* Conditional visibility */}
                  <div class="wf-var-grid">
                    <div class="wf-var-field">
                      <label>Visible when</label>
                      <select
                        class="wf-launch-input"
                        value={v().xUi?.condition?.field ?? ''}
                        onChange={(e) => {
                          const field = e.currentTarget.value;
                          if (!field) {
                            const { condition, ...rest } = v().xUi ?? {};
                            updateVar(idx, 'xUi', Object.keys(rest).length > 0 ? rest : undefined);
                          } else {
                            updateVar(idx, 'xUi', { ...v().xUi, condition: { field, eq: true } });
                          }
                        }}
                      >
                        <option value="">(always visible)</option>
                        <For each={localVars().filter((_, i) => i !== idx)}>
                          {(other) => <option value={other.name}>{other.name}</option>}
                        </For>
                      </select>
                    </div>
                    <Show when={v().xUi?.condition?.field}>
                      <div class="wf-var-field">
                        <label>equals</label>
                        <input
                          class="wf-launch-input"
                          value={v().xUi?.condition?.eq != null ? String(v().xUi!.condition!.eq) : ''}
                          onInput={(e) => {
                            const raw = e.currentTarget.value;
                            let val: any = raw;
                            if (raw === 'true') val = true;
                            else if (raw === 'false') val = false;
                            else if (raw !== '' && !isNaN(Number(raw))) val = Number(raw);
                            updateVar(idx, 'xUi', { ...v().xUi, condition: { ...v().xUi?.condition, eq: val } });
                          }}
                          placeholder="true"
                        />
                      </div>
                    </Show>
                  </div>

                  {/* String constraints */}
                  <Show when={v().varType === 'string'}>
                    <div class="wf-var-field">
                      <label>Allowed values</label>
                      {renderEnumEditor(v().enumValues, (vals) => updateVar(idx, 'enumValues', vals))}
                    </div>
                    <div class="wf-var-grid">
                      <div class="wf-var-field">
                        <label>Min length</label>
                        <input class="wf-launch-input" type="number" value={v().minLength ?? ''} onInput={(e) => updateVar(idx, 'minLength', e.currentTarget.value ? Number(e.currentTarget.value) : undefined)} placeholder="—" />
                      </div>
                      <div class="wf-var-field">
                        <label>Max length</label>
                        <input class="wf-launch-input" type="number" value={v().maxLength ?? ''} onInput={(e) => updateVar(idx, 'maxLength', e.currentTarget.value ? Number(e.currentTarget.value) : undefined)} placeholder="—" />
                      </div>
                    </div>
                    <div class="wf-var-field">
                      <label>Pattern (regex)</label>
                      <input class="wf-launch-input" type="text" value={v().pattern ?? ''} onInput={(e) => updateVar(idx, 'pattern', e.currentTarget.value || undefined)} placeholder="^[a-z]+$" />
                    </div>
                  </Show>

                  {/* Number constraints */}
                  <Show when={v().varType === 'number'}>
                    <div class="wf-var-field">
                      <label>Allowed values</label>
                      {renderEnumEditor(v().enumValues, (vals) => updateVar(idx, 'enumValues', vals))}
                    </div>
                    <div class="wf-var-grid">
                      <div class="wf-var-field">
                        <label>Minimum</label>
                        <input class="wf-launch-input" type="number" value={v().minimum ?? ''} onInput={(e) => updateVar(idx, 'minimum', e.currentTarget.value ? Number(e.currentTarget.value) : undefined)} placeholder="—" />
                      </div>
                      <div class="wf-var-field">
                        <label>Maximum</label>
                        <input class="wf-launch-input" type="number" value={v().maximum ?? ''} onInput={(e) => updateVar(idx, 'maximum', e.currentTarget.value ? Number(e.currentTarget.value) : undefined)} placeholder="—" />
                      </div>
                    </div>
                  </Show>

                  {/* Array items type */}
                  <Show when={v().varType === 'array'}>
                    <div class="wf-var-field">
                      <label>Item type</label>
                      <select class="wf-launch-input" value={v().itemsType ?? 'string'} onChange={(e) => updateVar(idx, 'itemsType', e.currentTarget.value)}>
                        <For each={TYPE_VALUES.filter((t) => t !== 'array')}>
                          {(t) => <option value={t}>{TYPE_LABELS[t]}</option>}
                        </For>
                      </select>
                    </div>
                    <Show when={v().itemsType === 'object'}>
                      <div class="wf-var-section-label">Item Properties</div>
                      <div style="display:flex;flex-direction:column;gap:8px">
                        <Index each={v().itemProperties ?? []}>
                          {(p, pIdx) => (
                            <div class="wf-nested-prop">
                              <div class="wf-nested-prop-header">
                                <input class="wf-launch-input" value={p().name} onInput={(e) => updateItemProp(idx, pIdx, 'name', e.currentTarget.value)} placeholder="Property name" />
                                <select class="wf-launch-input" style="width:100px;flex:none" value={p().varType} onChange={(e) => updateItemProp(idx, pIdx, 'varType', e.currentTarget.value)}>
                                  <option value="string">{TYPE_LABELS['string']}</option>
                                  <option value="number">{TYPE_LABELS['number']}</option>
                                  <option value="boolean">{TYPE_LABELS['boolean']}</option>
                                </select>
                                <button onClick={() => removeItemProp(idx, pIdx)} style="background:none;border:none;color:hsl(var(--muted-foreground));cursor:pointer;font-size:0.9em;padding:0 4px" title="Remove property">✕</button>
                              </div>
                              <div class="wf-var-grid">
                                <div class="wf-var-field full-width">
                                  <label>Description</label>
                                  <input class="wf-launch-input" value={p().description} onInput={(e) => updateItemProp(idx, pIdx, 'description', e.currentTarget.value)} placeholder="Property description" />
                                </div>
                              </div>
                            </div>
                          )}
                        </Index>
                        <button class="wf-btn-secondary" style="align-self:flex-start;padding:4px 12px;font-size:0.82em;" onClick={() => addItemProp(idx)}>+ Add property</button>
                      </div>
                    </Show>
                  </Show>

                  {/* Object nested properties */}
                  <Show when={v().varType === 'object'}>
                    <div class="wf-var-section-label">Properties</div>
                    <div style="display:flex;flex-direction:column;gap:8px">
                      <Index each={v().properties ?? []}>
                        {(p, pIdx) => (
                          <div class="wf-nested-prop">
                            <div class="wf-nested-prop-header">
                              <input class="wf-launch-input" value={p().name} onInput={(e) => updateNestedProp(idx, pIdx, 'name', e.currentTarget.value)} placeholder="Property name" />
                              <select class="wf-launch-input" style="width:100px;flex:none" value={p().varType} onChange={(e) => updateNestedProp(idx, pIdx, 'varType', e.currentTarget.value)}>
                                <option value="string">{TYPE_LABELS['string']}</option>
                                <option value="number">{TYPE_LABELS['number']}</option>
                                <option value="boolean">{TYPE_LABELS['boolean']}</option>
                              </select>
                              <button onClick={() => removeNestedProp(idx, pIdx)} style="background:none;border:none;color:hsl(var(--muted-foreground));cursor:pointer;font-size:0.9em;padding:0 4px" title="Remove property">✕</button>
                            </div>
                            <div class="wf-var-grid">
                              <div class="wf-var-field full-width">
                                <label>Description</label>
                                <input class="wf-launch-input" value={p().description} onInput={(e) => updateNestedProp(idx, pIdx, 'description', e.currentTarget.value)} placeholder="Property description" />
                              </div>
                            </div>
                          </div>
                        )}
                      </Index>
                      <button class="wf-btn-secondary" style="align-self:flex-start;padding:4px 12px;font-size:0.82em;" onClick={() => addNestedProp(idx)}>+ Add property</button>
                    </div>
                  </Show>
                </div>
                  </TabsContent>
                )}
              </Index>
            </Tabs>
          </Show>

          <DialogFooter>
            <Button variant="outline" onClick={handleCancel}>Cancel</Button>
            <Button onClick={handleOk}>OK</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
};

export default PromptSchemaEditor;
