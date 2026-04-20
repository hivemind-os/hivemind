import { createSignal, For, Show, type Component } from 'solid-js';

/** A serialized config field from a plugin's Zod schema. */
export interface PluginFieldSchema {
  type: string;
  description?: string;
  default?: any;
  enum?: any[];
  minimum?: number;
  maximum?: number;
  items?: PluginFieldSchema;
  hivemind?: {
    label?: string;
    helpText?: string;
    section?: string;
    secret?: boolean;
    radio?: boolean;
    placeholder?: string;
  };
}

export interface PluginConfigSchema {
  type: string;
  properties: Record<string, PluginFieldSchema>;
  required: string[];
}

export interface PluginConfigFormProps {
  schema: PluginConfigSchema;
  values: Record<string, any>;
  onChange: (key: string, value: any) => void;
  disabled?: boolean;
}

/** Group fields by their hivemind.section metadata. */
function groupBySection(schema: PluginConfigSchema): { section: string; fields: [string, PluginFieldSchema][] }[] {
  const groups: Map<string, [string, PluginFieldSchema][]> = new Map();
  for (const [name, field] of Object.entries(schema.properties)) {
    const section = field.hivemind?.section ?? 'General';
    if (!groups.has(section)) groups.set(section, []);
    groups.get(section)!.push([name, field]);
  }
  return Array.from(groups.entries()).map(([section, fields]) => ({ section, fields }));
}

/** Renders a plugin config schema as a form. */
const PluginConfigForm: Component<PluginConfigFormProps> = (props) => {
  const sections = () => groupBySection(props.schema);

  return (
    <div class="space-y-4">
      <For each={sections()}>
        {(group) => (
          <div>
            <h4 class="text-sm font-semibold text-muted-foreground uppercase tracking-wide mb-2">
              {group.section}
            </h4>
            <div class="space-y-3">
              <For each={group.fields}>
                {([name, field]) => (
                  <FieldRenderer
                    name={name}
                    field={field}
                    value={props.values[name] ?? field.default}
                    required={props.schema.required.includes(name)}
                    onChange={(v) => props.onChange(name, v)}
                    disabled={props.disabled}
                  />
                )}
              </For>
            </div>
          </div>
        )}
      </For>
    </div>
  );
};

interface FieldRendererProps {
  name: string;
  field: PluginFieldSchema;
  value: any;
  required: boolean;
  onChange: (v: any) => void;
  disabled?: boolean;
}

const FieldRenderer: Component<FieldRendererProps> = (props) => {
  const label = () => props.field.hivemind?.label ?? props.name;
  const helpText = () => props.field.hivemind?.helpText ?? props.field.description;
  const isSecret = () => props.field.hivemind?.secret ?? false;
  const isRadio = () => props.field.hivemind?.radio ?? false;
  const placeholder = () => props.field.hivemind?.placeholder ?? '';

  // Boolean field
  if (props.field.type === 'boolean') {
    return (
      <div class="flex items-center gap-3">
        <input
          type="checkbox"
          checked={props.value ?? false}
          onChange={(e) => props.onChange(e.currentTarget.checked)}
          disabled={props.disabled}
          class="rounded border-input"
        />
        <div>
          <label class="text-sm font-medium">{label()}</label>
          <Show when={helpText()}>
            <p class="text-xs text-muted-foreground">{helpText()}</p>
          </Show>
        </div>
      </div>
    );
  }

  // Enum field (dropdown or radio)
  if (props.field.enum && props.field.enum.length > 0) {
    if (isRadio()) {
      return (
        <div class="space-y-1">
          <label class="text-sm font-medium">{label()}</label>
          <div class="flex flex-wrap gap-2">
            <For each={props.field.enum}>
              {(opt) => (
                <label class="flex items-center gap-1.5 text-sm cursor-pointer">
                  <input
                    type="radio"
                    name={props.name}
                    checked={props.value === opt}
                    onChange={() => props.onChange(opt)}
                    disabled={props.disabled}
                  />
                  {String(opt)}
                </label>
              )}
            </For>
          </div>
          <Show when={helpText()}>
            <p class="text-xs text-muted-foreground">{helpText()}</p>
          </Show>
        </div>
      );
    }

    return (
      <div class="space-y-1">
        <label class="text-sm font-medium">{label()}</label>
        <select
          value={props.value ?? ''}
          onChange={(e) => props.onChange(e.currentTarget.value)}
          disabled={props.disabled}
          class="w-full rounded-md border border-input bg-background px-3 py-1.5 text-sm"
        >
          <For each={props.field.enum}>
            {(opt) => <option value={opt}>{String(opt)}</option>}
          </For>
        </select>
        <Show when={helpText()}>
          <p class="text-xs text-muted-foreground">{helpText()}</p>
        </Show>
      </div>
    );
  }

  // Number field
  if (props.field.type === 'number' || props.field.type === 'integer') {
    return (
      <div class="space-y-1">
        <label class="text-sm font-medium">
          {label()}
          <Show when={props.required}><span class="text-destructive ml-0.5">*</span></Show>
        </label>
        <input
          type="number"
          value={props.value ?? ''}
          min={props.field.minimum}
          max={props.field.maximum}
          placeholder={placeholder()}
          onInput={(e) => {
            const v = e.currentTarget.value;
            props.onChange(v === '' ? undefined : Number(v));
          }}
          disabled={props.disabled}
          class="w-full rounded-md border border-input bg-background px-3 py-1.5 text-sm"
        />
        <Show when={helpText()}>
          <p class="text-xs text-muted-foreground">{helpText()}</p>
        </Show>
      </div>
    );
  }

  // String field (default)
  return (
    <div class="space-y-1">
      <label class="text-sm font-medium">
        {label()}
        <Show when={props.required}><span class="text-destructive ml-0.5">*</span></Show>
      </label>
      <input
        type={isSecret() ? 'password' : 'text'}
        value={props.value ?? ''}
        placeholder={placeholder()}
        onInput={(e) => props.onChange(e.currentTarget.value)}
        disabled={props.disabled}
        class="w-full rounded-md border border-input bg-background px-3 py-1.5 text-sm"
      />
      <Show when={helpText()}>
        <p class="text-xs text-muted-foreground">{helpText()}</p>
      </Show>
    </div>
  );
};

export default PluginConfigForm;
