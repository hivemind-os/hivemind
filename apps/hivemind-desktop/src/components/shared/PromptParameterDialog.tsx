import { For, Show, Index, createSignal, createEffect, createMemo } from 'solid-js';
import Handlebars from 'handlebars';
import type { PromptTemplate } from '../../types';
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter } from '~/ui/dialog';
import { Switch, SwitchControl, SwitchThumb, SwitchLabel } from '~/ui/switch';
import { Button } from '~/ui/button';
import { evaluateFieldCondition } from '~/lib/formConditions';

export interface PromptParameterDialogProps {
  template: PromptTemplate;
  /** Called with the rendered prompt text and raw parameter values when the user submits. */
  onSubmit: (renderedText: string, params: Record<string, any>) => void;
  onCancel: () => void;
  /** Label for the submit button (default: "Send"). */
  submitLabel?: string;
  /** Controls dialog visibility. Defaults to true for mount/unmount usage. */
  open?: boolean;
}

interface SchemaFieldInfo {
  name: string;
  label?: string;
  type: string;
  required: boolean;
  description: string;
  defaultValue?: any;
  enumValues?: string[];
  widget?: string;
  widgetMeta?: Record<string, any>;
  minimum?: number;
  maximum?: number;
  maxLength?: number;
}

function extractFields(schema?: Record<string, any>): SchemaFieldInfo[] {
  if (!schema?.properties) return [];
  const required: string[] = schema.required ?? [];
  return Object.entries(schema.properties as Record<string, any>).map(([name, prop]) => {
    const p = prop as any;
    return {
      name,
      label: p['x-ui']?.label,
      type: p.type ?? 'string',
      required: required.includes(name),
      description: p.description ?? '',
      defaultValue: p.default,
      enumValues: Array.isArray(p.enum) ? p.enum : undefined,
      widget: p['x-ui']?.widget,
      widgetMeta: p['x-ui'],
      minimum: p.minimum,
      maximum: p.maximum,
      maxLength: p.maxLength,
    };
  });
}

function buildDefaults(fields: SchemaFieldInfo[]): Record<string, any> {
  const defaults: Record<string, any> = {};
  for (const f of fields) {
    if (f.defaultValue !== undefined) {
      defaults[f.name] = f.defaultValue;
    } else if (f.type === 'boolean') {
      defaults[f.name] = false;
    }
  }
  return defaults;
}

/** Render a Handlebars template with the given values, matching server-side behavior. */
function renderWithHandlebars(template: string, values: Record<string, any>): string {
  try {
    const compiled = Handlebars.compile(template, { strict: true });
    return compiled(values);
  } catch (e: any) {
    return `(template error: ${e.message})`;
  }
}

const PromptParameterDialog = (props: PromptParameterDialogProps) => {
  const fields = createMemo(() => extractFields(props.template.input_schema));
  const [values, setValues] = createSignal<Record<string, any>>(buildDefaults(fields()));
  const [jsonMode, setJsonMode] = createSignal(false);
  const [jsonText, setJsonText] = createSignal('{}');

  createEffect(() => {
    setValues(buildDefaults(fields()));
    setJsonText(JSON.stringify(buildDefaults(fields()), null, 2));
  });

  const updateValue = (name: string, value: any) => {
    setValues((prev) => ({ ...prev, [name]: value }));
  };

  const preview = createMemo(() => {
    return renderWithHandlebars(props.template.template, values());
  });

  const canSubmit = createMemo(() => {
    for (const f of fields()) {
      if (f.required) {
        const v = values()[f.name];
        if (v === undefined || v === null || v === '') return false;
      }
    }
    return true;
  });

  const handleSubmit = () => {
    const rendered = renderWithHandlebars(props.template.template, values());
    props.onSubmit(rendered, { ...values() });
  };

  const inputSt = "padding:4px 8px;border-radius:4px;border:1px solid hsl(var(--border));background:hsl(var(--background));color:hsl(var(--foreground));font-size:0.85em;width:100%;box-sizing:border-box;";

  const renderField = (field: SchemaFieldInfo) => {
    const widget = field.widget;

    if (widget === 'textarea' || widget === 'code-editor') {
      return (
        <textarea
          value={String(values()[field.name] ?? '')}
          placeholder={field.description || field.name}
          maxLength={field.maxLength}
          rows={field.widgetMeta?.rows ?? 4}
          onInput={(e) => updateValue(field.name, e.currentTarget.value)}
          style={inputSt + "resize:vertical;" + (widget === 'code-editor' ? "font-family:monospace;" : "")}
        />
      );
    }
    if (widget === 'password') {
      return (
        <input
          type="password"
          value={String(values()[field.name] ?? '')}
          placeholder={field.description || field.name}
          maxLength={field.maxLength}
          onInput={(e) => updateValue(field.name, e.currentTarget.value)}
          style={inputSt}
        />
      );
    }
    if (widget === 'date') {
      return (
        <input
          type="date"
          value={String(values()[field.name] ?? '')}
          onInput={(e) => updateValue(field.name, e.currentTarget.value)}
          style={inputSt}
        />
      );
    }
    if (widget === 'color-picker') {
      return (
        <input
          type="color"
          value={String(values()[field.name] ?? '#000000')}
          onInput={(e) => updateValue(field.name, e.currentTarget.value)}
          style="border:1px solid hsl(var(--border));background:hsl(var(--background));border-radius:4px;width:48px;height:28px;padding:2px;cursor:pointer;"
        />
      );
    }
    if (widget === 'slider' && (field.type === 'number' || field.type === 'integer')) {
      return (
        <div style="display:flex;align-items:center;gap:6px;">
          <input
            type="range"
            value={values()[field.name] ?? field.minimum ?? 0}
            min={field.minimum ?? 0}
            max={field.maximum ?? 100}
            step={field.widgetMeta?.step ?? 1}
            onInput={(e) => updateValue(field.name, Number(e.currentTarget.value))}
            style="flex:1;"
          />
          <span style="font-size:0.82em;color:hsl(var(--foreground));min-width:28px;text-align:right;">
            {values()[field.name] ?? field.minimum ?? 0}
          </span>
        </div>
      );
    }

    // Enum → select
    if (field.enumValues && field.enumValues.length > 0) {
      return (
        <select
          value={String(values()[field.name] ?? '')}
          onChange={(e) => {
            const raw = e.currentTarget.value;
            const coerced = (field.type === 'number' || field.type === 'integer') ? Number(raw) : raw;
            updateValue(field.name, coerced);
          }}
          style={inputSt}
        >
          <option value="">—</option>
          <For each={field.enumValues}>{(v) => <option value={v}>{v}</option>}</For>
        </select>
      );
    }
    if (field.type === 'boolean') {
      return (
        <Switch checked={!!values()[field.name]} onChange={(checked) => updateValue(field.name, checked)} class="flex items-center gap-2">
          <SwitchControl><SwitchThumb /></SwitchControl>
          <SwitchLabel>{field.name}</SwitchLabel>
        </Switch>
      );
    }
    if (field.type === 'number' || field.type === 'integer') {
      return (
        <input
          type="number"
          value={values()[field.name] ?? ''}
          min={field.minimum}
          max={field.maximum}
          onInput={(e) => {
            const raw = e.currentTarget.value;
            updateValue(field.name, raw === '' ? '' : Number(raw));
          }}
          style={inputSt}
        />
      );
    }
    return (
      <input
        type="text"
        value={String(values()[field.name] ?? '')}
        placeholder={field.description || field.name}
        maxLength={field.maxLength}
        onInput={(e) => updateValue(field.name, e.currentTarget.value)}
        style={inputSt}
      />
    );
  };

  return (
    <Dialog open={props.open ?? true} onOpenChange={(open) => { if (!open) props.onCancel(); }}>
      <DialogContent class="max-w-[500px] max-h-[80vh] overflow-y-auto">
        <DialogHeader>
          <DialogTitle>{props.template.name || 'Prompt Template'}</DialogTitle>
          <Show when={props.template.description}>
            <p class="text-xs text-muted-foreground mt-0.5">
              {props.template.description}
            </p>
          </Show>
        </DialogHeader>

          <div class="space-y-3">
            <Show when={fields().length > 0}>
              <div class="flex items-center justify-between mb-1.5">
                <span class="text-xs font-medium">Parameters</span>
                <button
                  onClick={() => {
                    if (!jsonMode()) setJsonText(JSON.stringify(values(), null, 2));
                    setJsonMode(!jsonMode());
                  }}
                  class="px-1.5 py-0.5 rounded border border-border bg-transparent text-primary cursor-pointer text-xs"
                >{jsonMode() ? 'Form' : 'JSON'}</button>
              </div>

              <Show when={jsonMode()}>
                <textarea
                  value={jsonText()}
                  onInput={(e) => {
                    setJsonText(e.currentTarget.value);
                    try {
                      const parsed = JSON.parse(e.currentTarget.value);
                      setValues(parsed);
                    } catch { /* wait for valid JSON */ }
                  }}
                  rows={6}
                  style={inputSt + "resize:vertical;font-family:monospace;"}
                />
              </Show>

              <Show when={!jsonMode()}>
                <div class="flex flex-col gap-2">
                  <For each={fields()}>
                    {(field) => (
                      <Show when={evaluateFieldCondition(field.widgetMeta?.condition, values())}>
                      <div class="flex flex-col gap-0.5">
                        <label class="text-xs text-muted-foreground">
                          {field.label || field.name}
                          {field.required ? <span class="text-amber-500 ml-0.5">*</span> : null}
                          {field.description ? <span style="margin-left:6px;font-size:0.9em;opacity:0.7;">— {field.description.slice(0, 80)}</span> : null}
                        </label>
                        {renderField(field)}
                      </div>
                      </Show>
                    )}
                  </For>
                </div>
              </Show>
            </Show>

            {/* Preview */}
            <div style="margin-top:10px;">
              <span class="text-xs font-medium text-muted-foreground">Preview</span>
              <pre class="text-xs bg-secondary border border-border rounded-md p-2 overflow-x-auto whitespace-pre-wrap break-words max-h-[200px] font-mono">{preview()}</pre>
            </div>
          </div>

      <DialogFooter class="flex-row gap-2">
        <Button variant="outline" onClick={() => props.onCancel()}>Cancel</Button>
        <Button disabled={!canSubmit()} onClick={handleSubmit}>
          {props.submitLabel ?? 'Send'}
        </Button>
      </DialogFooter>
    </DialogContent></Dialog>
  );
};

export default PromptParameterDialog;
