import Handlebars from 'handlebars';
import { For, Index, Show, createEffect, createMemo, createSignal, onMount } from 'solid-js';
import type { Accessor, JSX } from 'solid-js';
import { invoke } from '@tauri-apps/api/core';
import type { Persona, PromptTemplate, ToolDefinition, McpServerConfig, InstalledSkill } from '../types';
import { Pencil, RotateCcw, EyeOff, Archive, Undo2, Bot, Plus, Trash2, ChevronDown, ChevronRight, Copy, HelpCircle, Package } from 'lucide-solid';
import { Popover, PopoverTrigger, PopoverContent } from '~/ui/popover';
import { Switch, SwitchControl, SwitchThumb, SwitchLabel } from '~/ui/switch';
import { Collapsible, CollapsibleTrigger, CollapsibleContent } from '~/ui/collapsible';
import { Button } from '~/ui/button';
import { ConfirmDialog } from '~/ui/confirm-dialog';
import { McpServerWizard } from './McpServerWizard';
import SkillsTab from './SkillsTab';
import PromptSchemaEditor, { type PromptSchemaField } from './PromptSchemaEditor';
import SystemPromptEditorDialog from './SystemPromptEditorDialog';
import GroupedToolSelector from './GroupedToolSelector';
import { buildNamespaceTree, type NamespaceNode } from '~/lib/workflowGrouping';

interface PersonasTabProps {
  availableModels: { id: string; label: string }[];
  availableTools: Accessor<ToolDefinition[]>;
  daemon_url?: string;
  onPersonasSaved?: () => Promise<void>;
  onExportToKit?: (persona_id: string) => void;
}

const NEW_AGENT_SENTINEL = '__new_agent__';

const createEmptyPersona = (): Persona => ({
  id: 'user/',
  name: '',
  description: '',
  system_prompt: '',
  loop_strategy: 'react',
  preferred_models: null,
  secondary_models: null,
  allowed_tools: ['*'],
  mcp_servers: [],
  avatar: '',
  color: 'hsl(var(--primary))',
  prompts: [],
});

const clonePersona = (persona: Persona): Persona => ({
  ...persona,
  preferred_models: persona.preferred_models ? [...persona.preferred_models] : null,
  secondary_models: persona.secondary_models ? [...persona.secondary_models] : null,
  allowed_tools: [...persona.allowed_tools],
  mcp_servers: persona.mcp_servers ? persona.mcp_servers.map(s => ({...s})) : [],
  prompts: persona.prompts ? persona.prompts.map(p => ({ ...p })) : [],
});

const slugify = (value: string) =>
  value
    .trim()
    .replace(/[^a-zA-Z0-9_-]+/g, '-')
    .replace(/^-+|-+$/g, '');

const buildAgentId = (name: string, existingIds: string[]) => {
  const base = slugify(name) || `agent-${Date.now().toString(36)}`;
  if (!existingIds.includes(base)) {
    return base;
  }

  let suffix = 2;
  while (existingIds.includes(`${base}-${suffix}`)) {
    suffix += 1;
  }
  return `${base}-${suffix}`;
};

const isWildcard = (values: string[]) => values.length === 1 && values[0] === '*';

const isDefaultPersona = (persona: Persona) => persona.id === 'general' || persona.id === 'system/general';

const createEmptyPromptTemplate = (existingIds: string[]): PromptTemplate => {
  let id = 'new-prompt';
  let suffix = 2;
  while (existingIds.includes(id)) {
    id = `new-prompt-${suffix}`;
    suffix++;
  }
  return { id, name: '', description: '', template: '', input_schema: undefined };
};

const loopStrategyLabel = (strategy: Persona['loop_strategy']) => {
  switch (strategy) {
    case 'sequential':
      return 'Sequential';
    case 'plan_then_execute':
      return 'Plan Then Execute';
    case 'react':
    default:
      return 'React';
  }
};

const loopStrategyDescription = (strategy: Persona['loop_strategy']) => {
  switch (strategy) {
    case 'sequential':
      return 'Executes tool calls one at a time in sequence.';
    case 'plan_then_execute':
      return 'Creates a plan first, then executes each step.';
    case 'react':
    default:
      return 'Thinks and acts in alternating steps. Best for general-purpose tasks.';
  }
};

const normalizeOptional = (value: string | null | undefined) => {
  const trimmed = value?.trim();
  return trimmed ? trimmed : null;
};

const displayColor = (value: string | null | undefined) =>
  /^#[0-9a-f]{6}$/i.test(value?.trim() ?? '') ? value!.trim() : '#89b4fa';

const PersonasTab = (props: PersonasTabProps) => {
  const [personas, setPersonas] = createSignal<Persona[]>([]);
  const [loading, setLoading] = createSignal(false);
  const [saving, setSaving] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [editingId, setEditingId] = createSignal<string | null>(null);
  const [draft, setDraft] = createSignal<Persona>(createEmptyPersona());
  const [allTools, setAllTools] = createSignal(true);
  const [selectedTools, setSelectedTools] = createSignal<string[]>([]);
  const [mcpServers, setMcpServers] = createSignal<McpServerConfig[]>([]);
  const [showMcpWizard, setShowMcpWizard] = createSignal(false);
  const [editingMcpServerIdx, setEditingMcpServerIdx] = createSignal<number | null>(null);
  const [expandedPromptIdx, setExpandedPromptIdx] = createSignal<number | null>(null);
  const [openParamHelperId, setOpenParamHelperId] = createSignal<string | null>(null);
  const [showCopyDialog, setShowCopyDialog] = createSignal(false);
  const [copySourceId, setCopySourceId] = createSignal('');
  const [copyNewId, setCopyNewId] = createSignal('user/');
  const [copyError, setCopyError] = createSignal<string | null>(null);
  const [personaSkills, setPersonaSkills] = createSignal<InstalledSkill[]>([]);
  const [showSkillsDialog, setShowSkillsDialog] = createSignal(false);
  const [showPromptEditor, setShowPromptEditor] = createSignal(false);
  const [confirmResetId, setConfirmResetId] = createSignal<string | null>(null);
  let paramHelperCounter = 0;

  const resetEditor = () => {
    setEditingId(null);
    setDraft(createEmptyPersona());
    setAllTools(true);
    setSelectedTools([]);
    setMcpServers([]);
    setShowMcpWizard(false);
    setEditingMcpServerIdx(null);
    setExpandedPromptIdx(null);
    setPersonaSkills([]);
  };

  const activePersonas = createMemo(() => personas().filter((p) => !p.archived));
  const archivedPersonas = createMemo(() => personas().filter((p) => p.archived));

  // Namespace grouping (hierarchical tree, sorted alphabetically at every level)
  const personaTree = createMemo(() => buildNamespaceTree(activePersonas(), (p) => p.id, (p) => p.name));
  const [collapsedNs, setCollapsedNs] = createSignal<Set<string>>(new Set());
  const toggleNs = (ns: string) => {
    setCollapsedNs(prev => {
      const next = new Set(prev);
      next.has(ns) ? next.delete(ns) : next.add(ns);
      return next;
    });
  };

  const loadPersonaSkills = async (persona_id: string) => {
    try {
      const skills = await invoke<InstalledSkill[]>('skills_list_installed_for_persona', { persona_id });
      setPersonaSkills(skills);
    } catch {
      setPersonaSkills([]);
    }
  };

  const addPreferredModel = (input: HTMLInputElement) => {
    const val = input.value.trim();
    if (!val) return;
    const current = draft().preferred_models ?? [];
    if (current.includes(val)) return;
    setDraft((cur) => ({
      ...cur,
      preferred_models: [...(cur.preferred_models ?? []), val],
    }));
    input.value = '';
  };

  const addSecondaryModel = (input: HTMLInputElement) => {
    const val = input.value.trim();
    if (!val) return;
    const current = draft().secondary_models ?? [];
    if (current.includes(val)) return;
    setDraft((cur) => ({
      ...cur,
      secondary_models: [...(cur.secondary_models ?? []), val],
    }));
    input.value = '';
  };

  const loadDefinitions = async () => {
    setLoading(true);
    setError(null);
    try {
      const defs = await invoke<Persona[]>('list_personas', { include_archived: true });
      setPersonas(defs);
      return defs;
    } catch (e: any) {
      setError(e?.toString() ?? 'Failed to load personas.');
      throw e;
    } finally {
      setLoading(false);
    }
  };

  onMount(() => {
    void loadDefinitions();
  });

  const startAdd = () => {
    setError(null);
    setEditingId(NEW_AGENT_SENTINEL);
    setDraft(createEmptyPersona());
    setAllTools(true);
    setSelectedTools([]);
    setMcpServers([]);
    setShowMcpWizard(false);
    setPersonaSkills([]);
  };

  const startEdit = (persona: Persona) => {
    setError(null);
    setEditingId(persona.id);
    setDraft(clonePersona(persona));
    setAllTools(isWildcard(persona.allowed_tools));
    setSelectedTools(isWildcard(persona.allowed_tools) ? [] : [...persona.allowed_tools]);
    setMcpServers(persona.mcp_servers?.map(s => ({...s})) ?? []);
    void loadPersonaSkills(persona.id);
  };

  const savePersonas = async (nextPersonas: Persona[]) => {
    setSaving(true);
    setError(null);
    try {
      await invoke('save_personas', { personas: nextPersonas });
      await loadDefinitions();
      await props.onPersonasSaved?.();
      resetEditor();
    } catch (e: any) {
      setError(e?.toString() ?? 'Failed to save personas.');
    } finally {
      setSaving(false);
    }
  };

  const saveAgent = async () => {
    const current = draft();
    const name = current.name.trim();
    if (!name) {
      setError('Name is required.');
      return;
    }

    let persona_id = current.id;
    if (editingId() === NEW_AGENT_SENTINEL) {
      // For new personas, validate and generate the ID
      if (persona_id && persona_id.startsWith('user/') && persona_id.length > 5) {
        // User provided a full namespaced ID - validate it
        const segment = persona_id.substring(5);
        if (!/^[a-zA-Z0-9][a-zA-Z0-9_-]*$/.test(segment)) {
          setError('Persona ID must use letters, numbers, hyphens, and underscores after "user/".');
          return;
        }
        if (personas().some((p) => p.id === persona_id)) {
          setError(`A persona with ID "${persona_id}" already exists.`);
          return;
        }
      } else {
        // Auto-generate a namespaced ID from the name
        persona_id = 'user/' + buildAgentId(name, personas().map((p) => p.id));
      }
    }

    // Validate Handlebars syntax for all prompt templates before saving
    for (const tpl of current.prompts ?? []) {
      try {
        Handlebars.precompile(tpl.template);
      } catch (e: any) {
        setError(`Template "${tpl.name || tpl.id || '(unnamed)'}" has invalid Handlebars syntax: ${e.message}`);
        return;
      }
    }

    // Validate for duplicate template IDs
    const templateIds = (current.prompts ?? []).map(p => p.id).filter(Boolean);
    const seenIds = new Set<string>();
    for (const tid of templateIds) {
      if (seenIds.has(tid)) {
        setError(`Duplicate template ID "${tid}". Each prompt template must have a unique ID.`);
        return;
      }
      seenIds.add(tid);
    }

    // Validate parameter names within each template
    for (const tpl of current.prompts ?? []) {
      const schema = tpl.input_schema;
      if (schema?.properties) {
        const paramNames = Object.keys(schema.properties);
        const seenParams = new Set<string>();
        for (const pn of paramNames) {
          if (!pn) {
            setError(`Template "${tpl.name || tpl.id || '(unnamed)'}" has a parameter with an empty name.`);
            return;
          }
          if (!/^[a-zA-Z_][a-zA-Z0-9_]*$/.test(pn)) {
            setError(`Template "${tpl.name || tpl.id || '(unnamed)'}" has an invalid parameter name "${pn}". Names must start with a letter or underscore, followed by letters, numbers, or underscores.`);
            return;
          }
          if (seenParams.has(pn)) {
            setError(`Template "${tpl.name || tpl.id || '(unnamed)'}" has duplicate parameter name "${pn}".`);
            return;
          }
          seenParams.add(pn);
        }
      }
    }

    const nextPersona: Persona = {
      ...current,
      id: persona_id,
      name,
      description: current.description.trim(),
      preferred_models: current.preferred_models && current.preferred_models.length > 0
        ? current.preferred_models
        : null,
      secondary_models: current.secondary_models && current.secondary_models.length > 0
        ? current.secondary_models
        : null,
      avatar: normalizeOptional(current.avatar),
      color: normalizeOptional(current.color),
      allowed_tools: allTools() ? ['*'] : selectedTools(),
      mcp_servers: mcpServers(),
    };

    const nextPersonas =
      editingId() === NEW_AGENT_SENTINEL
        ? [...personas(), nextPersona]
        : personas().map((p) => (p.id === editingId() ? nextPersona : p));

    await savePersonas(nextPersonas);
  };

  const [confirmArchiveId, setConfirmArchiveId] = createSignal<string | null>(null);

  const archivePersona = async (persona: Persona) => {
    if (isDefaultPersona(persona)) {
      return;
    }
    await savePersonas(personas().map((item) => item.id === persona.id ? { ...item, archived: true } : item));
  };

  const resetPersona = async (persona: Persona) => {
    if (!persona.bundled) return;
    setSaving(true);
    setError(null);
    try {
      await invoke('reset_persona', { id: persona.id });
      await loadDefinitions();
      await props.onPersonasSaved?.();
      resetEditor();
    } catch (e: any) {
      setError(e?.toString() ?? 'Failed to reset persona.');
    } finally {
      setSaving(false);
    }
  };

  const restorePersona = async (persona: Persona) => {
    await savePersonas(personas().map((p) => p.id === persona.id ? { ...p, archived: false } : p));
  };

  // ── Copy from template ──────────────────────────────────────
  const startCopyFromTemplate = () => {
    setCopySourceId('');
    setCopyNewId('user/');
    setCopyError(null);
    setShowCopyDialog(true);
  };

  const doCopyPersona = async () => {
    const source_id = copySourceId().trim();
    const newId = copyNewId().trim();
    if (!source_id) { setCopyError('Select a source persona.'); return; }
    if (!newId.startsWith('user/') || newId.length <= 5) {
      setCopyError('New ID must start with "user/" followed by a valid name.');
      return;
    }
    const segment = newId.substring(5);
    if (!/^[a-zA-Z0-9][a-zA-Z0-9_-]*$/.test(segment)) {
      setCopyError('ID segment must use letters, numbers, hyphens, and underscores.');
      return;
    }
    if (personas().some((p) => p.id === newId)) {
      setCopyError(`A persona with ID "${newId}" already exists.`);
      return;
    }
    setSaving(true);
    setCopyError(null);
    try {
      await invoke('copy_persona', { source_id, new_id: newId });
      await loadDefinitions();
      await props.onPersonasSaved?.();
      setShowCopyDialog(false);
    } catch (e: any) {
      setCopyError(e?.toString() ?? 'Copy failed.');
    } finally {
      setSaving(false);
    }
  };

  // ── Prompt template helpers ──────────────────────────────────────

  const addPromptTemplate = () => {
    const existing = draft().prompts ?? [];
    const tpl = createEmptyPromptTemplate(existing.map((p) => p.id));
    setDraft((cur) => ({ ...cur, prompts: [...(cur.prompts ?? []), tpl] }));
    setExpandedPromptIdx(existing.length);
  };

  const removePromptTemplate = (idx: number) => {
    setDraft((cur) => ({
      ...cur,
      prompts: (cur.prompts ?? []).filter((_, i) => i !== idx),
    }));
    setExpandedPromptIdx(null);
  };

  const updatePromptTemplate = (idx: number, patch: Partial<PromptTemplate>) => {
    setDraft((cur) => {
      const prompts = [...(cur.prompts ?? [])];
      prompts[idx] = { ...prompts[idx], ...patch };
      // Auto-generate id from name if this is still an untouched slug
      if (patch.name !== undefined && prompts[idx].id.startsWith('new-prompt')) {
        const slug = slugify(patch.name);
        if (slug) prompts[idx].id = slug;
      }
      return { ...cur, prompts };
    });
  };

  type SchemaField = PromptSchemaField;

  const parseSchemaFields = (schema: Record<string, any> | undefined): SchemaField[] => {
    if (!schema?.properties) return [];
    const required: string[] = schema.required ?? [];

    function parseProps(props: Record<string, any>, req: string[]): SchemaField[] {
      return Object.entries(props).map(([name, prop]) => {
        const p = prop as any;
        const fieldType = (p.type ?? 'string') as SchemaField['varType'];
        let defaultValue = '';
        if (p.default !== undefined) {
          defaultValue = fieldType === 'string' ? String(p.default) : JSON.stringify(p.default);
        }
        const field: SchemaField = {
          name,
          varType: fieldType,
          description: p.description ?? '',
          required: req.includes(name),
          defaultValue,
          enumValues: Array.isArray(p.enum) ? p.enum : [],
        };
        if (p.minLength != null) field.minLength = p.minLength;
        if (p.maxLength != null) field.maxLength = p.maxLength;
        if (p.pattern != null) field.pattern = p.pattern;
        if (p.minimum != null) field.minimum = p.minimum;
        if (p.maximum != null) field.maximum = p.maximum;
        if (p['x-ui']) field.xUi = { ...p['x-ui'] };
        if (fieldType === 'object' && p.properties) {
          field.properties = parseProps(p.properties, p.required ?? []);
        }
        if (fieldType === 'array' && p.items) {
          field.itemsType = (p.items.type ?? 'string') as string;
          if (p.items.type === 'object' && p.items.properties) {
            field.itemProperties = parseProps(p.items.properties, p.items.required ?? []);
          }
        }
        return field;
      });
    }

    return parseProps(schema.properties as Record<string, any>, required);
  };

  const buildSchemaFromFields = (fields: SchemaField[]): Record<string, any> | undefined => {
    if (fields.length === 0) return undefined;

    function buildProps(flds: SchemaField[]): { properties: Record<string, any>; required: string[] } {
      const properties: Record<string, any> = {};
      const required: string[] = [];
      for (const f of flds) {
        const prop: Record<string, any> = { type: f.varType };
        if (f.description) prop.description = f.description;
        if (f.defaultValue) {
          if (f.varType === 'string') {
            prop.default = f.defaultValue;
          } else {
            try { prop.default = JSON.parse(f.defaultValue); } catch { prop.default = f.defaultValue; }
          }
        }
        if (f.enumValues && f.enumValues.length > 0) {
          prop.enum = f.enumValues;
        }
        if (f.xUi && Object.values(f.xUi).some(val => val !== undefined)) {
          prop['x-ui'] = { ...f.xUi };
        }
        if (f.minLength != null) prop.minLength = f.minLength;
        if (f.maxLength != null) prop.maxLength = f.maxLength;
        if (f.pattern) prop.pattern = f.pattern;
        if (f.minimum != null) prop.minimum = f.minimum;
        if (f.maximum != null) prop.maximum = f.maximum;
        if (f.varType === 'object' && f.properties && f.properties.length > 0) {
          const nested = buildProps(f.properties);
          prop.properties = nested.properties;
          if (nested.required.length > 0) prop.required = nested.required;
        }
        if (f.varType === 'array') {
          const items: Record<string, any> = { type: f.itemsType ?? 'string' };
          if (f.itemsType === 'object' && f.itemProperties && f.itemProperties.length > 0) {
            const nested = buildProps(f.itemProperties);
            items.properties = nested.properties;
            if (nested.required.length > 0) items.required = nested.required;
          }
          prop.items = items;
        }
        properties[f.name] = prop;
        if (f.required) required.push(f.name);
      }
      return { properties, required };
    }

    const { properties, required } = buildProps(fields);
    const schema: Record<string, any> = { type: 'object', properties };
    if (required.length > 0) schema.required = required;
    return schema;
  };

  function computePreview(templateText: string, fields: SchemaField[]): { text?: string; error?: string } {
    if (!templateText.trim()) return { text: '(empty template)' };
    try {
      const defaults: Record<string, any> = {};
      for (const f of fields) {
        if (f.defaultValue) {
          if (f.varType === 'string') {
            defaults[f.name] = f.defaultValue;
          } else {
            try { defaults[f.name] = JSON.parse(f.defaultValue); } catch { defaults[f.name] = f.defaultValue; }
          }
        } else {
          defaults[f.name] = `<${f.name}>`;
        }
      }
      // Simple substitution for {{var}} placeholders (avoids unsafe-eval from Handlebars.compile).
      const text = templateText.replace(/\{\{\s*([^#/!>][^}]*?)\s*\}\}/g, (_, key) => {
        const trimmed = key.trim();
        return trimmed in defaults ? String(defaults[trimmed]) : `{{${trimmed}}}`;
      });
      return { text };
    } catch (e: any) {
      return { error: e.message };
    }
  }

  function renderParamInsertHelper(
    fields: () => SchemaField[],
    onInsert: (text: string) => void,
    inputEl?: () => HTMLTextAreaElement | HTMLInputElement | undefined,
  ) {
    const myId = `param_${paramHelperCounter++}`;
    const isOpen = () => openParamHelperId() === myId;

    const popupStyle: Record<string, string> = {
      background: 'hsl(var(--card))',
      border: '1px solid hsl(var(--border))',
      'border-radius': '6px',
      'box-shadow': '0 4px 12px rgba(0,0,0,0.4)',
      'min-width': '220px',
      'max-height': '260px',
      'overflow-y': 'auto',
      padding: '4px 0',
    };

    return (
      <Popover
        open={isOpen()}
        onOpenChange={(o) => { if (!o) setOpenParamHelperId(null); }}
        placement="bottom-end"
        gutter={2}
      >
        <PopoverTrigger as="div" style={{ display: 'inline-block' }}>
          <button
            onClick={() => setOpenParamHelperId(isOpen() ? null : myId)}
            style={{
              background: 'none', border: '1px solid hsl(var(--border))',
              color: 'hsl(var(--primary))', cursor: 'pointer', 'border-radius': '3px',
              padding: '1px 5px', 'font-size': '0.7em', 'margin-left': '4px',
            }}
            title="Insert parameter reference"
          >{'{{}}'}</button>
        </PopoverTrigger>
        <PopoverContent class="w-auto p-0" style={{ 'z-index': '10000', ...popupStyle }}>
                <div style={{ 'font-size': '0.65em', color: 'hsl(var(--muted-foreground))', padding: '4px 10px 2px', 'font-weight': '600', 'text-transform': 'uppercase', 'letter-spacing': '0.5px' }}>
                  Parameters
                </div>
                <Show when={fields().length > 0} fallback={
                  <div style={{ padding: '4px 10px 8px', color: 'hsl(var(--muted-foreground))', 'font-size': '0.8em', 'font-style': 'italic' }}>
                    No parameters defined yet
                  </div>
                }>
                  <For each={fields().filter(f => f.name)}>
                    {(f) => (
                      <button
                        onClick={(e) => {
                          e.stopPropagation();
                          const el = inputEl?.();
                          const expr = `{{${f.name}}}`;
                          if (el) {
                            const start = el.selectionStart ?? el.value.length;
                            const end = el.selectionEnd ?? start;
                            const before = el.value.slice(0, start);
                            const after = el.value.slice(end);
                            onInsert(before + expr + after);
                            requestAnimationFrame(() => {
                              el.focus();
                              const pos = start + expr.length;
                              el.setSelectionRange(pos, pos);
                            });
                          } else {
                            onInsert(expr);
                          }
                          setOpenParamHelperId(null);
                        }}
                        style={{
                          display: 'block', width: '100%', 'text-align': 'left',
                          background: 'none', border: 'none', padding: '4px 10px',
                          color: 'hsl(var(--foreground))', cursor: 'pointer',
                          'font-size': '0.85em', 'font-family': 'monospace',
                        }}
                        onMouseEnter={(e) => (e.currentTarget.style.background = 'hsl(var(--primary) / 0.1)')}
                        onMouseLeave={(e) => (e.currentTarget.style.background = 'none')}
                      >
                        {f.name}
                        <span style={{ color: 'hsl(var(--muted-foreground))', 'margin-left': '6px', 'font-size': '0.85em' }}>
                          {`{{${f.name}}}`}
                        </span>
                      </button>
                    )}
                  </For>
                </Show>
                <div style={{ 'font-size': '0.65em', color: 'hsl(var(--muted-foreground))', padding: '4px 10px 2px', 'font-weight': '600', 'text-transform': 'uppercase', 'letter-spacing': '0.5px', 'border-top': '1px solid hsl(var(--border))', 'margin-top': '2px' }}>
                  Helpers
                </div>
                <For each={[
                  { label: '#if', value: '{{#if param}}…{{/if}}' },
                  { label: '#each', value: '{{#each items}}{{this}}{{/each}}' },
                  { label: '#unless', value: '{{#unless param}}…{{/unless}}' },
                ]}>
                  {(h) => (
                    <button
                      onClick={(e) => {
                        e.stopPropagation();
                        const el = inputEl?.();
                        if (el) {
                          const start = el.selectionStart ?? el.value.length;
                          const end = el.selectionEnd ?? start;
                          const before = el.value.slice(0, start);
                          const after = el.value.slice(end);
                          onInsert(before + h.value + after);
                          requestAnimationFrame(() => {
                            el.focus();
                            const pos = start + h.value.length;
                            el.setSelectionRange(pos, pos);
                          });
                        } else {
                          onInsert(h.value);
                        }
                        setOpenParamHelperId(null);
                      }}
                      style={{
                        display: 'block', width: '100%', 'text-align': 'left',
                        background: 'none', border: 'none', padding: '4px 10px',
                        color: 'hsl(var(--foreground))', cursor: 'pointer',
                        'font-size': '0.85em', 'font-family': 'monospace',
                      }}
                      onMouseEnter={(e) => (e.currentTarget.style.background = 'hsl(var(--primary) / 0.1)')}
                      onMouseLeave={(e) => (e.currentTarget.style.background = 'none')}
                    >
                      {h.label}
                      <span style={{ color: 'hsl(var(--muted-foreground))', 'margin-left': '6px', 'font-size': '0.85em' }}>
                        {h.value}
                      </span>
                    </button>
                  )}
                </For>
      </PopoverContent>
    </Popover>
    );
  }

  return (
    <div class="settings-section">
      <div class="settings-section-header">
        <div>
          <h3>Personas</h3>
          <p class="muted agents-subtitle">Create custom personas with tailored prompts, models, and access rules.</p>
        </div>
        <Show when={!editingId()}>
          <div class="flex gap-2">
            <Button size="sm" variant="outline" disabled={saving()} onClick={startCopyFromTemplate}>
              <Copy size={14} /> Copy from Template
            </Button>
            <Button size="sm" disabled={saving()} onClick={startAdd}>+ Add Persona</Button>
          </div>
        </Show>
      </div>

      <Show when={error()}>
        <div class="error-banner agents-error-banner">{error()}</div>
      </Show>

      <Show
        when={editingId()}
        fallback={
          <Show when={!loading()} fallback={<p class="muted">Loading personas…</p>}>
            {/* Render a persona card */}
            {(() => {
              const renderPersonaCard = (persona: Persona) => (
                <article class="memory-card agent-card">
                  <header class="agent-card-header">
                    <div class="agent-card-main">
                      <div
                        class="agent-avatar"
                        style={{
                          'border-color': displayColor(persona.color),
                          'background-color': `${displayColor(persona.color)}22`,
                        }}
                      >
                        {persona.avatar || <Bot size={14} />}
                      </div>
                      <div class="agent-card-copy">
                        <div class="agent-title-row">
                          <strong>{persona.name}</strong>
                          <Show when={persona.bundled}>
                            <span class="pill neutral">Built-in</span>
                          </Show>
                        </div>
                        <code>
                          <Show when={persona.id.includes('/')}>
                            <span style="opacity: 0.5">{persona.id.substring(0, persona.id.indexOf('/') + 1)}</span>
                            {persona.id.substring(persona.id.indexOf('/') + 1)}
                          </Show>
                          <Show when={!persona.id.includes('/')}>
                            {persona.id}
                          </Show>
                        </code>
                      </div>
                    </div>
                    <div class="agent-card-actions">
                      <button
                        class="agent-icon-button"
                        title={`Edit ${persona.name}`}
                        onClick={() => startEdit(persona)}
                      >
                        <Pencil size={14} />
                      </button>
                      <Show when={persona.bundled}>
                        <button
                          class="agent-icon-button"
                          disabled={saving()}
                          title={`Reset ${persona.name} to factory defaults`}
                          onClick={() => setConfirmResetId(persona.id)}
                        >
                          <RotateCcw size={14} />
                        </button>
                      </Show>
                      <Show when={props.onExportToKit && !persona.id.startsWith('system/')}>
                        <button
                          class="agent-icon-button"
                          title={`Export ${persona.name} as Agent Kit`}
                          onClick={() => props.onExportToKit?.(persona.id)}
                        >
                          <Package size={14} />
                        </button>
                      </Show>
                      <button
                        class="agent-icon-button danger-outline"
                        disabled={saving() || (isDefaultPersona(persona) && !persona.bundled)}
                        title={persona.bundled ? `Hide ${persona.name}` : isDefaultPersona(persona) ? 'The default persona cannot be archived.' : `Archive ${persona.name}`}
                        onClick={() => setConfirmArchiveId(persona.id)}
                      >
                        {persona.bundled ? <EyeOff size={14} /> : <Archive size={14} />}
                      </button>
                    </div>
                  </header>
                  <p>{persona.description || 'No description provided.'}</p>
                  <div class="agent-meta">
                    <span class="badge">{loopStrategyLabel(persona.loop_strategy)}</span>
                    <Show when={persona.preferred_models && persona.preferred_models.length > 0}>
                      <For each={persona.preferred_models ?? []}>
                        {(model) => <span class="badge">{model}</span>}
                      </For>
                    </Show>
                    <Show when={persona.secondary_models && persona.secondary_models.length > 0}>
                      <For each={persona.secondary_models ?? []}>
                        {(model) => <span class="badge" title="Secondary model">{model} (2°)</span>}
                      </For>
                    </Show>
                    <span class="pill neutral">{isWildcard(persona.allowed_tools) ? 'All tools' : `${persona.allowed_tools.length} tool${persona.allowed_tools.length === 1 ? '' : 's'}`}</span>
                    <span class="pill neutral">{persona.mcp_servers?.length ? `${persona.mcp_servers.length} MCP server${persona.mcp_servers.length === 1 ? '' : 's'}` : 'No MCP servers'}</span>
                  </div>
                </article>
              );
              function countItems(node: NamespaceNode<Persona>): number {
                let n = node.items.length;
                for (const child of node.children) n += countItems(child);
                return n;
              }

              function renderNsNode(node: NamespaceNode<Persona>, depth: number): JSX.Element {
                const collapsed = () => collapsedNs().has(node.fullPath);
                return (
                  <div style={`margin-bottom: 0.5rem; padding-left: ${depth * 16}px;`}>
                    <div
                      class="settings-section-header"
                      style="margin-top: 0.5rem; cursor: pointer; user-select: none;"
                      onClick={() => toggleNs(node.fullPath)}
                    >
                      <div style="display: flex; align-items: center; gap: 6px;">
                        {collapsed()
                          ? <ChevronRight size={14} style="opacity: 0.6;" />
                          : <ChevronDown size={14} style="opacity: 0.6;" />
                        }
                        <h4 style="margin: 0; opacity: 0.7; text-transform: capitalize;">{node.segment}</h4>
                      </div>
                      <span class="pill neutral">{countItems(node)}</span>
                    </div>
                    <Show when={!collapsed()}>
                      <Show when={node.items.length > 0}>
                        <div class="agents-grid">
                          <For each={node.items}>
                            {(persona) => renderPersonaCard(persona)}
                          </For>
                        </div>
                      </Show>
                      <For each={node.children}>
                        {(child) => renderNsNode(child, depth + 1)}
                      </For>
                    </Show>
                  </div>
                );
              }

              return (
                <>
                  <For each={personaTree()}>
                    {(node) => renderNsNode(node, 0)}
                  </For>
                </>
              );
            })()}

            <Show when={archivedPersonas().length > 0}>
              <div class="settings-section-header" style="margin-top: 1.5rem;">
                <h3>Hidden / Archived</h3>
                <span class="pill neutral">{archivedPersonas().length}</span>
              </div>
              <div class="agents-grid">
                <For each={archivedPersonas()}>
                  {(persona) => (
                    <article class="memory-card agent-card" style="opacity: 0.6;">
                      <header class="agent-card-header">
                        <div class="agent-card-main">
                          <div
                            class="agent-avatar"
                            style={{
                              'border-color': 'hsl(var(--muted-foreground))',
                              'background-color': 'hsl(var(--muted-foreground) / 0.1)',
                            }}
                          >
                            {persona.avatar || <Bot size={14} />}
                          </div>
                          <div class="agent-card-copy">
                            <div class="agent-title-row">
                              <strong>{persona.name}</strong>
                              <span class="pill neutral">{persona.bundled ? 'Hidden' : 'Archived'}</span>
                              <Show when={persona.bundled}>
                                <span class="pill neutral">Built-in</span>
                              </Show>
                            </div>
                            <code>{persona.id}</code>
                          </div>
                        </div>
                        <div class="agent-card-actions">
                          <Show when={persona.bundled}>
                            <button
                              class="agent-icon-button"
                              disabled={saving()}
                              title={`Reset ${persona.name} to factory defaults and restore`}
                              onClick={() => setConfirmResetId(persona.id)}
                            >
                              <RotateCcw size={14} />
                            </button>
                          </Show>
                          <button
                            class="agent-icon-button"
                            disabled={saving()}
                            title={`Restore ${persona.name}`}
                            onClick={() => void restorePersona(persona)}
                          >
                            <Undo2 size={14} />
                          </button>
                        </div>
                      </header>
                      <p>{persona.description || 'No description provided.'}</p>
                    </article>
                  )}
                </For>
              </div>
            </Show>
          </Show>
        }
      >
        <div class="memory-card agents-editor">
          <div class="settings-section-header">
            <h3>{editingId() === NEW_AGENT_SENTINEL ? 'Add Persona' : `Edit ${draft().name || 'Persona'}`}</h3>
            <Show when={draft().bundled}>
              <span class="pill neutral">Built-in</span>
            </Show>
          </div>

          <Show when={draft().bundled}>
            <p class="muted agents-help">This is a built-in persona. You can customize it, and use the Reset button to restore factory defaults.</p>
          </Show>

          <div class="settings-form">
            <label>
              <span>Name</span>
              <input
                type="text"
                value={draft().name}
                onInput={(e) => setDraft((current) => ({ ...current, name: e.currentTarget.value }))}
                placeholder="Research Persona"
              />
            </label>

            <Show when={editingId() === NEW_AGENT_SENTINEL}>
              <label>
                <span>Persona ID</span>
                <input
                  type="text"
                  value={draft().id}
                  onInput={(e) => setDraft((current) => ({ ...current, id: e.currentTarget.value }))}
                  placeholder="user/my-persona"
                />
                <span class="muted" style="font-size: 0.8rem;">Must start with "user/" — leave as "user/" to auto-generate from name</span>
              </label>
            </Show>

            <label>
              <span>Description</span>
              <input
                type="text"
                value={draft().description}
                onInput={(e) => setDraft((current) => ({ ...current, description: e.currentTarget.value }))}
                placeholder="Describe what this persona is optimized for"
              />
            </label>

            <label class="agents-form-field">
              <span>System Prompt</span>
              <div class="agents-form-control">
                <div
                  style="position:relative;border:1px solid hsl(214 14% 22%);border-radius:6px;background:hsl(215 21% 7%);cursor:pointer;overflow:hidden"
                  onClick={() => setShowPromptEditor(true)}
                >
                  <pre
                    style="margin:0;padding:10px 12px;padding-right:48px;font-family:inherit;font-size:0.85em;white-space:pre-wrap;word-break:break-word;color:hsl(210 13% 81%);max-height:4.8em;overflow:hidden;line-height:1.6"
                  >
                    <Show
                      when={draft().system_prompt.trim()}
                      fallback={<span style="color:hsl(212 10% 53%);font-style:italic">Click to add system prompt…</span>}
                    >
                      {draft().system_prompt}
                    </Show>
                  </pre>
                  <button
                    type="button"
                    class="agent-icon-button"
                    title="Edit system prompt"
                    style="position:absolute;top:6px;right:6px;padding:4px"
                    onClick={(e) => { e.stopPropagation(); setShowPromptEditor(true); }}
                  >
                    <Pencil size={14} />
                  </button>
                </div>
                <p class="muted" style="font-size:0.75em;margin:2px 0 0;text-align:right">
                  {draft().system_prompt.length} characters
                </p>
              </div>
            </label>

            <SystemPromptEditorDialog
              open={showPromptEditor()}
              value={draft().system_prompt}
              onSave={(val) => {
                setDraft((current) => ({ ...current, system_prompt: val }));
                setShowPromptEditor(false);
              }}
              onCancel={() => setShowPromptEditor(false)}
            />

            <div>
              <label>
                <span>Loop Strategy</span>
                <select
                  value={draft().loop_strategy}
                  onChange={(e) => setDraft((current) => ({
                    ...current,
                    loop_strategy: e.currentTarget.value as Persona['loop_strategy'],
                  }))}
                >
                  <option value="react">React</option>
                  <option value="sequential">Sequential</option>
                  <option value="plan_then_execute">Plan Then Execute</option>
                </select>
              </label>
              <p class="muted" style="font-size:0.8em;margin:2px 0 0">
                {loopStrategyDescription(draft().loop_strategy)}
              </p>
            </div>

            <label>
              <span class="inline-flex items-center gap-1">
                Preferred Models
                <Popover>
                  <PopoverTrigger class="inline-flex items-center text-muted-foreground hover:text-foreground cursor-help" aria-label="Model pattern help">
                    <HelpCircle size={14} />
                  </PopoverTrigger>
                  <PopoverContent class="w-80 text-xs space-y-2">
                    <p class="font-semibold text-sm">Model Pattern Syntax</p>
                    <p>Patterns are tried in order. The first match becomes the primary model; remaining matches form the fallback chain.</p>
                    <dl class="space-y-1.5">
                      <dt class="font-medium">Wildcards</dt>
                      <dd><code>*</code> matches any characters, <code>?</code> matches exactly one.<br/>
                        <span class="text-muted-foreground">e.g.</span> <code>claude-sonnet-*</code>, <code>gpt-5.?</code></dd>
                      <dt class="font-medium">Provider prefix</dt>
                      <dd>Scope to a provider with <code>provider:pattern</code>.<br/>
                        <span class="text-muted-foreground">e.g.</span> <code>openai:gpt-5.*</code></dd>
                      <dt class="font-medium">Exclusions</dt>
                      <dd>Prefix with <code>!</code> to exclude matching models.<br/>
                        <span class="text-muted-foreground">e.g.</span> <code>!*-mini</code>, <code>!*-nano</code></dd>
                      <dt class="font-medium">Version sorting</dt>
                      <dd>Within each pattern, models are automatically sorted by version (newest first).</dd>
                    </dl>
                    <div class="rounded bg-muted p-1.5 font-mono text-[11px] space-y-0.5">
                      <div>claude-sonnet-*</div>
                      <div>gpt-5.*</div>
                      <div>!*-mini</div>
                      <div>!*-nano</div>
                    </div>
                  </PopoverContent>
                </Popover>
              </span>
              <div class="preferred-models-editor">
                <Show when={(draft().preferred_models ?? []).length > 0}>
                  <ol class="preferred-models-list">
                    <Index each={draft().preferred_models ?? []}>
                      {(model, index) => (
                        <li class="preferred-models-item">
                          <input
                            type="text"
                            list="available-models-list"
                            class="preferred-models-item-input"
                            value={model()}
                            onInput={(e) => {
                              const val = e.currentTarget.value;
                              setDraft((cur) => {
                                const items = [...(cur.preferred_models ?? [])];
                                items[index] = val;
                                return { ...cur, preferred_models: items };
                              });
                            }}
                          />
                          <span class="preferred-models-item-actions">
                            <button type="button" disabled={index === 0}
                              onClick={() => setDraft((cur) => {
                                const items = [...(cur.preferred_models ?? [])];
                                const i = index;
                                [items[i - 1], items[i]] = [items[i], items[i - 1]];
                                return { ...cur, preferred_models: items };
                              })}>▲</button>
                            <button type="button" disabled={index === (draft().preferred_models ?? []).length - 1}
                              onClick={() => setDraft((cur) => {
                                const items = [...(cur.preferred_models ?? [])];
                                const i = index;
                                [items[i], items[i + 1]] = [items[i + 1], items[i]];
                                return { ...cur, preferred_models: items };
                              })}>▼</button>
                            <button type="button" class="tag-remove"
                              onClick={() => setDraft((cur) => {
                                const items = (cur.preferred_models ?? []).filter((_, i) => i !== index);
                                return { ...cur, preferred_models: items.length > 0 ? items : null };
                              })}>×</button>
                          </span>
                        </li>
                      )}
                    </Index>
                  </ol>
                </Show>
                <div class="preferred-models-add">
                  <input
                    type="text"
                    list="available-models-list"
                    placeholder="Type a model name or pattern (e.g. gpt-5.*)"
                    onKeyDown={(e) => {
                      if (e.key === 'Enter') {
                        e.preventDefault();
                        addPreferredModel(e.currentTarget);
                      }
                    }}
                    onChange={(e) => addPreferredModel(e.currentTarget)}
                  />
                  <datalist id="available-models-list">
                    <For each={[...new Set(props.availableModels.map((m) => {
                      const colon = m.id.indexOf(':');
                      return colon >= 0 ? m.id.slice(colon + 1) : m.id;
                    }))]}>
                      {(name) => <option value={name} />}
                    </For>
                  </datalist>
                </div>
              </div>
            </label>

            <label>
              <span class="inline-flex items-center gap-1">
                Secondary Models
                <Popover>
                  <PopoverTrigger class="inline-flex items-center text-muted-foreground hover:text-foreground cursor-help" aria-label="Model pattern help">
                    <HelpCircle size={14} />
                  </PopoverTrigger>
                  <PopoverContent class="w-80 text-xs space-y-2">
                    <p class="font-semibold text-sm">Secondary Models</p>
                    <p>Used as fallbacks when preferred models are unavailable or rate-limited. Same pattern syntax applies (wildcards, exclusions, provider prefix).</p>
                  </PopoverContent>
                </Popover>
              </span>
              <div class="preferred-models-editor">
                <Show when={(draft().secondary_models ?? []).length > 0}>
                  <ol class="preferred-models-list">
                    <Index each={draft().secondary_models ?? []}>
                      {(model, index) => (
                        <li class="preferred-models-item">
                          <input
                            type="text"
                            list="available-models-list"
                            class="preferred-models-item-input"
                            value={model()}
                            onInput={(e) => {
                              const val = e.currentTarget.value;
                              setDraft((cur) => {
                                const items = [...(cur.secondary_models ?? [])];
                                items[index] = val;
                                return { ...cur, secondary_models: items };
                              });
                            }}
                          />
                          <span class="preferred-models-item-actions">
                            <button type="button" disabled={index === 0}
                              onClick={() => setDraft((cur) => {
                                const items = [...(cur.secondary_models ?? [])];
                                const i = index;
                                [items[i - 1], items[i]] = [items[i], items[i - 1]];
                                return { ...cur, secondary_models: items };
                              })}>▲</button>
                            <button type="button" disabled={index === (draft().secondary_models ?? []).length - 1}
                              onClick={() => setDraft((cur) => {
                                const items = [...(cur.secondary_models ?? [])];
                                const i = index;
                                [items[i], items[i + 1]] = [items[i + 1], items[i]];
                                return { ...cur, secondary_models: items };
                              })}>▼</button>
                            <button type="button" class="tag-remove"
                              onClick={() => setDraft((cur) => {
                                const items = (cur.secondary_models ?? []).filter((_, i) => i !== index);
                                return { ...cur, secondary_models: items.length > 0 ? items : null };
                              })}>×</button>
                          </span>
                        </li>
                      )}
                    </Index>
                  </ol>
                </Show>
                <div class="preferred-models-add">
                  <input
                    type="text"
                    list="available-models-list"
                    placeholder="Type a model name or pattern (e.g. gpt-4.1-mini)"
                    onKeyDown={(e) => {
                      if (e.key === 'Enter') {
                        e.preventDefault();
                        addSecondaryModel(e.currentTarget);
                      }
                    }}
                    onChange={(e) => addSecondaryModel(e.currentTarget)}
                  />
                </div>
              </div>
            </label>

            <label>
              <span>Avatar</span>
              <input
                type="text"
                value={draft().avatar ?? ''}
                onInput={(e) => setDraft((current) => ({ ...current, avatar: e.currentTarget.value }))}
                placeholder=""
              />
            </label>

            <label class="agents-form-field">
              <span>Persona Color</span>
              <div class="agents-form-control">
                <div class="agents-color-row">
                  <input
                    type="text"
                    value={draft().color ?? ''}
                    onInput={(e) => setDraft((current) => ({ ...current, color: e.currentTarget.value }))}
                    placeholder="#89b4fa"
                  />
                  <input
                    class="agents-color-swatch"
                    type="color"
                    value={displayColor(draft().color)}
                    onInput={(e) => setDraft((current) => ({ ...current, color: e.currentTarget.value }))}
                  />
                </div>
              </div>
            </label>

            <div class="agents-form-field">
              <span>Allowed Tools</span>
              <div class="agents-form-control">
                <Switch checked={allTools()} onChange={(checked) => setAllTools(checked)} class="flex items-center gap-2">
                  <SwitchControl><SwitchThumb /></SwitchControl>
                  <SwitchLabel>All tools</SwitchLabel>
                </Switch>
                <Show when={!allTools()}>
                  <Show when={props.availableTools().length > 0} fallback={
                    <span class="persona-empty-hint">No tools available — start the daemon first</span>
                  }>
                    <GroupedToolSelector
                      tools={props.availableTools()}
                      selected={selectedTools()}
                      onChange={setSelectedTools}
                      toolKey={(t) => t.name}
                      maxHeight="250px"
                    />
                  </Show>
                </Show>
              </div>
            </div>

            <div class="agents-form-field">
              <div style="display: flex; align-items: center; justify-content: space-between;">
                <span>MCP Servers</span>
                <Show when={props.daemon_url}>
                  <button class="small" onClick={() => { setEditingMcpServerIdx(null); setShowMcpWizard(true); }}>+ Add Server</button>
                </Show>
              </div>
              <div class="agents-form-control">
                <Show when={!props.daemon_url}>
                  <p style="font-size: 0.85rem; color: hsl(var(--muted-foreground));">Start the daemon to manage MCP servers.</p>
                </Show>
                <Show when={props.daemon_url}>
                  <Show when={mcpServers().length === 0}>
                    <p style="font-size: 0.85rem; color: hsl(var(--muted-foreground));">No MCP servers configured for this persona.</p>
                  </Show>
                  <For each={mcpServers()}>
                    {(server, idx) => (
                      <div style="display: flex; align-items: center; justify-content: space-between; padding: 0.5rem 0.75rem; border: 1px solid var(--border); border-radius: 6px; margin-bottom: 0.5rem;">
                        <div>
                          <strong>{server.id}</strong>
                          <span style="margin-left: 0.5rem; font-size: 0.8rem; color: hsl(var(--muted-foreground));">
                            {server.transport === 'stdio' ? `${server.command ?? ''} ${server.args.join(' ')}`.trim() : server.url}
                          </span>
                          <span class="pill neutral" style="margin-left: 0.5rem; font-size: 0.7rem;">{server.transport}</span>
                        </div>
                         <div style="display: flex; gap: 0.5rem; align-items: center;">
                          <label style="font-size: 0.8rem; display: flex; align-items: center; gap: 0.25rem;">
                            <input type="checkbox" checked={server.enabled} onChange={(e) => {
                              const updated = [...mcpServers()];
                              updated[idx()] = { ...updated[idx()], enabled: e.currentTarget.checked };
                              setMcpServers(updated);
                            }} />
                            Enabled
                          </label>
                          <button class="small" onClick={() => {
                            setEditingMcpServerIdx(idx());
                            setShowMcpWizard(true);
                          }}>Edit</button>
                          <button class="small danger" onClick={() => {
                            setMcpServers(mcpServers().filter((_, i) => i !== idx()));
                          }}>Remove</button>
                        </div>
                      </div>
                    )}
                  </For>
                </Show>
                <Show when={showMcpWizard() && props.daemon_url}>
                  <McpServerWizard
                    daemon_url={props.daemon_url!}
                    existingIds={mcpServers()
                      .filter((_, i) => i !== editingMcpServerIdx())
                      .map(s => s.id)}
                    editingConfig={editingMcpServerIdx() !== null ? mcpServers()[editingMcpServerIdx()!] : undefined}
                    onFinish={(config) => {
                      const editIdx = editingMcpServerIdx();
                      if (editIdx !== null) {
                        const updated = [...mcpServers()];
                        updated[editIdx] = config;
                        setMcpServers(updated);
                      } else {
                        setMcpServers([...mcpServers(), config]);
                      }
                      setEditingMcpServerIdx(null);
                      setShowMcpWizard(false);
                    }}
                    onClose={() => {
                      setEditingMcpServerIdx(null);
                      setShowMcpWizard(false);
                    }}
                  />
                </Show>
              </div>
            </div>

            {/* ── Installed Skills ────────────────────────────────── */}
            <Show when={editingId() && editingId() !== NEW_AGENT_SENTINEL}>
              <div class="agents-form-field">
                <div style="display: flex; align-items: center; justify-content: space-between;">
                  <span>Skills</span>
                  <button class="small" onClick={() => setShowSkillsDialog(true)}>Manage Skills</button>
                </div>
                <div class="agents-form-control">
                  <Show when={personaSkills().length === 0}>
                    <p style="font-size: 0.85rem; color: hsl(var(--muted-foreground));">No skills installed for this persona.</p>
                  </Show>
                  <Show when={personaSkills().length > 0}>
                    <div style="display: flex; flex-wrap: wrap; gap: 0.375rem;">
                      <For each={personaSkills()}>
                        {(skill) => (
                          <span
                            class={`pill ${skill.enabled ? 'neutral' : ''}`}
                            style={!skill.enabled ? 'opacity: 0.5; text-decoration: line-through;' : ''}
                            title={`${skill.manifest.description}${!skill.enabled ? ' (disabled)' : ''}`}
                          >
                            {skill.manifest.name}
                          </span>
                        )}
                      </For>
                    </div>
                  </Show>
                </div>
              </div>
            </Show>
          </div>

          {/* ── Prompt Templates ────────────────────────────────── */}
          <div class="agents-form-section">
            <div style="display:flex;align-items:center;justify-content:space-between;margin-bottom:8px">
              <h4 style="margin:0">Prompt Templates</h4>
              <button class="small" onClick={addPromptTemplate} title="Add prompt template">
                <Plus size={14} /> Add
              </button>
            </div>
            <Show when={(draft().prompts ?? []).length === 0}>
              <p class="muted" style="font-size:0.85em;margin:0">
                No prompt templates yet. Add reusable parameterized prompts that can be invoked from chat, the Agent Stage, or Flight Deck.
              </p>
            </Show>
            <Index each={draft().prompts ?? []}>
              {(tpl, idx) => {
                const isExpanded = () => expandedPromptIdx() === idx;
                const fields = () => parseSchemaFields(tpl().input_schema);
                const [localFields, setLocalFields] = createSignal<SchemaField[]>(fields());

                // Sync localFields when switching prompt or fields change externally
                createEffect(() => setLocalFields(fields()));

                const commitFields = (updated: SchemaField[]) => {
                  setLocalFields(updated);
                  updatePromptTemplate(idx, { input_schema: buildSchemaFromFields(updated) });
                };

                const duplicateTemplateId = () => {
                  const currentId = tpl().id;
                  if (!currentId) return false;
                  return (draft().prompts ?? []).filter(p => p.id === currentId).length > 1;
                };

                let templateRef: HTMLTextAreaElement | undefined;

                const preview = createMemo(() => computePreview(tpl().template, localFields()));

                return (
                  <Collapsible open={isExpanded()} onOpenChange={(open) => setExpandedPromptIdx(open ? idx : null)}>
                    <div class="prompt-template-card">
                      <CollapsibleTrigger as="div" class="prompt-template-header">
                        {isExpanded() ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
                        <span class="prompt-template-title">{tpl().name || '(unnamed)'}</span>
                        <button
                          class="icon-btn danger"
                          title="Remove"
                          onClick={(e) => { e.stopPropagation(); removePromptTemplate(idx); }}
                        >
                          <Trash2 size={14} />
                        </button>
                      </CollapsibleTrigger>
                      <CollapsibleContent>
                        <div class="prompt-template-body">
                        <label class="agents-form-field">
                          <span>Name</span>
                          <input
                            type="text"
                            value={tpl().name}
                            onInput={(e) => updatePromptTemplate(idx, { name: e.currentTarget.value })}
                            placeholder="e.g. Review Pull Request"
                          />
                        </label>
                        <div class="agents-form-field">
                          <span>ID</span>
                          <input
                            type="text"
                            value={tpl().id}
                            onInput={(e) => updatePromptTemplate(idx, { id: slugify(e.currentTarget.value) })}
                            placeholder="auto-generated from name"
                          />
                          <Show when={duplicateTemplateId()}>
                            <p style="font-size:0.75em;color:hsl(var(--destructive));margin:2px 0 0">
                              Duplicate template ID — each template must have a unique ID.
                            </p>
                          </Show>
                        </div>
                        <label class="agents-form-field">
                          <span>Description</span>
                          <input
                            type="text"
                            value={tpl().description ?? ''}
                            onInput={(e) => updatePromptTemplate(idx, { description: e.currentTarget.value })}
                            placeholder="Brief description of what this prompt does"
                          />
                        </label>
                        <div class="agents-form-field">
                          <div style={{ display: 'flex', 'align-items': 'center' }}>
                            <span style={{ flex: '1', 'font-weight': '500' }}>Template (Handlebars)</span>
                            {renderParamInsertHelper(
                              localFields,
                              (newVal) => updatePromptTemplate(idx, { template: newVal }),
                              () => templateRef,
                            )}
                          </div>
                          <textarea
                            ref={templateRef}
                            rows={6}
                            value={tpl().template}
                            onInput={(e) => updatePromptTemplate(idx, { template: e.currentTarget.value })}
                            placeholder="Use {{param_name}} for parameters. Supports Handlebars syntax like {{#each items}}."
                            style="font-family:monospace;font-size:0.85em"
                          />
                        </div>

                        {/* ── Parameter schema editor ── */}
                        <PromptSchemaEditor
                          fields={localFields()}
                          onChange={commitFields}
                        />

                        {/* Template Preview */}
                        <div style="margin-top:8px">
                          <span style="font-weight:500;font-size:0.9em">Preview</span>
                          <Show when={preview().error} fallback={
                            <pre style="font-size:0.8em;background:hsl(var(--secondary));border:1px solid hsl(var(--border));border-radius:4px;padding:6px 8px;white-space:pre-wrap;word-break:break-word;max-height:150px;overflow-y:auto;margin-top:4px;font-family:monospace">{preview().text}</pre>
                          }>
                            <p style="font-size:0.8em;color:hsl(var(--destructive));margin-top:4px">{preview().error}</p>
                          </Show>
                        </div>
                      </div>
                      </CollapsibleContent>
                    </div>
                  </Collapsible>
                );
              }}
            </Index>
          </div>

          <div class="button-row agents-form-actions">
            <button class="primary" disabled={saving()} onClick={() => void saveAgent()}>
              {saving() ? 'Saving…' : 'Save Persona'}
            </button>
            <button disabled={saving()} onClick={resetEditor}>Cancel</button>
          </div>
        </div>
      </Show>

      {/* ── Skills Management Dialog ────────────────────────── */}
      <Show when={showSkillsDialog() && editingId() && editingId() !== NEW_AGENT_SENTINEL}>
        <div class="modal-overlay" onClick={(e) => { if (e.target === e.currentTarget) { setShowSkillsDialog(false); void loadPersonaSkills(editingId()!); } }}>
          <div class="modal-content" style="max-width: 720px; max-height: 85vh; overflow-y: auto;" onClick={(e) => e.stopPropagation()}>
            <div style="display: flex; align-items: center; justify-content: space-between; margin-bottom: 0.5rem;">
              <h3 style="margin: 0;">Manage Skills</h3>
              <button class="small" onClick={() => { setShowSkillsDialog(false); void loadPersonaSkills(editingId()!); }}>✕</button>
            </div>
            <SkillsTab availableModels={props.availableModels} persona_id={editingId()!} />
          </div>
        </div>
      </Show>

      {/* ── Copy from Template Dialog ────────────────────────── */}
      <Show when={showCopyDialog()}>
        <div class="modal-overlay" onClick={(e) => { if (e.target === e.currentTarget) setShowCopyDialog(false); }}>
          <div class="modal-content" style="max-width: 480px;" onClick={(e) => e.stopPropagation()}>
            <h3 style="margin-top: 0;">Copy from Template</h3>
            <p class="muted" style="font-size: 0.85rem; margin-bottom: 1rem;">
              Create a new persona by copying an existing one.
            </p>
            <Show when={copyError()}>
              <div class="error-banner" style="margin-bottom: 0.75rem;">{copyError()}</div>
            </Show>
            <label class="agents-form-field">
              <span>Source Persona</span>
              <select
                value={copySourceId()}
                onChange={(e) => setCopySourceId(e.currentTarget.value)}
              >
                <option value="">— Select a persona —</option>
                <For each={personas().filter((p) => !p.archived)}>
                  {(p) => <option value={p.id}>{p.name} ({p.id})</option>}
                </For>
              </select>
            </label>
            <label class="agents-form-field" style="margin-top: 0.75rem;">
              <span>New Persona ID</span>
              <input
                type="text"
                value={copyNewId()}
                onInput={(e) => setCopyNewId(e.currentTarget.value)}
                placeholder="user/my-agent"
              />
              <span class="muted" style="font-size: 0.8rem;">Must start with "user/" — e.g. user/my-assistant</span>
            </label>
            <div class="button-row" style="margin-top: 1rem;">
              <Button disabled={saving()} onClick={() => void doCopyPersona()}>
                {saving() ? 'Copying…' : 'Copy Persona'}
              </Button>
              <Button variant="outline" onClick={() => setShowCopyDialog(false)}>Cancel</Button>
            </div>
          </div>
        </div>
      </Show>

      <ConfirmDialog
        open={!!confirmResetId()}
        onOpenChange={(open) => { if (!open) setConfirmResetId(null); }}
        title="Reset to factory defaults?"
        description="Your customizations to this persona will be lost."
        confirmLabel="Reset"
        variant="destructive"
        onConfirm={() => {
          const p = personas().find((x) => x.id === confirmResetId());
          if (p) void resetPersona(p);
        }}
      />

      <ConfirmDialog
        open={!!confirmArchiveId()}
        onOpenChange={(open) => { if (!open) setConfirmArchiveId(null); }}
        title={(() => {
          const p = personas().find((x) => x.id === confirmArchiveId());
          return p?.bundled ? `Hide "${p.name}"?` : `Archive "${p?.name}"?`;
        })()}
        description="It will be hidden but existing workflows will continue to work."
        confirmLabel={(() => {
          const p = personas().find((x) => x.id === confirmArchiveId());
          return p?.bundled ? 'Hide' : 'Archive';
        })()}
        onConfirm={() => {
          const p = personas().find((x) => x.id === confirmArchiveId());
          if (p) void archivePersona(p);
        }}
      />
    </div>
  );
};

export default PersonasTab;
