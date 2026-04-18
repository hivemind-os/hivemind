import Handlebars from 'handlebars';
import { For, Index, Show, createEffect, createMemo, createSignal, on } from 'solid-js';
import type { Accessor } from 'solid-js';
import { invoke } from '@tauri-apps/api/core';
import type { Persona, PromptTemplate, ToolDefinition, McpServerConfig, InstalledSkill } from '../types';
import { Pencil, Plus, Trash2, ChevronDown, ChevronRight, HelpCircle } from 'lucide-solid';
import { Popover, PopoverTrigger, PopoverContent } from '~/ui/popover';
import { Switch, SwitchControl, SwitchThumb, SwitchLabel } from '~/ui/switch';
import { Collapsible, CollapsibleTrigger, CollapsibleContent } from '~/ui/collapsible';
import { Button } from '~/ui/button';
import { Dialog, DialogContent } from '~/ui/dialog';
import { McpServerWizard } from './McpServerWizard';
import SkillsTab from './SkillsTab';
import PromptSchemaEditor, { type PromptSchemaField } from './PromptSchemaEditor';
import SystemPromptEditorDialog from './SystemPromptEditorDialog';
import GroupedToolSelector from './GroupedToolSelector';
import { parseSchemaFields, buildSchemaFromFields, computePreview } from '~/lib/promptHelpers';

// ── Types ────────────────────────────────────────────────────────

type WizardStep = 'identity' | 'prompt' | 'models' | 'tools' | 'skills' | 'prompts';

const WIZARD_STEPS: WizardStep[] = ['identity', 'prompt', 'models', 'tools', 'skills', 'prompts'];

const STEP_LABELS: Record<WizardStep, string> = {
  identity: 'Identity',
  prompt: 'System Prompt',
  models: 'Models',
  tools: 'Tools & MCP',
  skills: 'Skills',
  prompts: 'Prompts',
};

export interface PersonaWizardProps {
  availableModels: { id: string; label: string }[];
  availableTools: Accessor<ToolDefinition[]>;
  existingPersonaIds: string[];
  daemon_url?: string;
  onFinish: () => Promise<void>;
  onClose: () => void;
}

// ── Helpers ──────────────────────────────────────────────────────

const slugify = (value: string) =>
  value.trim().replace(/[^a-zA-Z0-9_-]+/g, '-').replace(/^-+|-+$/g, '');

const buildAgentId = (name: string, existingIds: string[]) => {
  const base = slugify(name) || `agent-${Date.now().toString(36)}`;
  if (!existingIds.includes(`user/${base}`)) return base;
  let suffix = 2;
  while (existingIds.includes(`user/${base}-${suffix}`)) suffix += 1;
  return `${base}-${suffix}`;
};

const normalizeOptional = (value: string | null | undefined) => {
  const trimmed = value?.trim();
  return trimmed ? trimmed : null;
};

const displayColor = (value: string | null | undefined) =>
  /^#[0-9a-f]{6}$/i.test(value?.trim() ?? '') ? value!.trim() : '#89b4fa';

const loopStrategyDescription = (strategy: Persona['loop_strategy']) => {
  switch (strategy) {
    case 'sequential': return 'Executes tool calls one at a time in sequence.';
    case 'plan_then_execute': return 'Creates a plan first, then executes each step.';
    case 'react': default: return 'Thinks and acts in alternating steps. Best for general-purpose tasks.';
  }
};

const createEmptyPromptTemplate = (existingIds: string[]): PromptTemplate => {
  let id = 'new-prompt';
  let suffix = 2;
  while (existingIds.includes(id)) { id = `new-prompt-${suffix}`; suffix += 1; }
  return { id, name: '', description: '', template: '', input_schema: undefined };
};

// ── Component ────────────────────────────────────────────────────

export function PersonaWizard(props: PersonaWizardProps) {
  // Step state
  const [step, setStep] = createSignal<WizardStep>('identity');
  const currentIdx = () => WIZARD_STEPS.indexOf(step());

  // Draft persona fields
  const [name, setName] = createSignal('');
  const [personaId, setPersonaId] = createSignal('user/');
  const [description, setDescription] = createSignal('');
  const [systemPrompt, setSystemPrompt] = createSignal('');
  const [loopStrategy, setLoopStrategy] = createSignal<Persona['loop_strategy']>('react');
  const [preferredModels, setPreferredModels] = createSignal<string[]>([]);
  const [secondaryModels, setSecondaryModels] = createSignal<string[]>([]);
  const [avatar, setAvatar] = createSignal('');
  const [color, setColor] = createSignal('hsl(var(--primary))');
  const [allTools, setAllTools] = createSignal(true);
  const [selectedTools, setSelectedTools] = createSignal<string[]>([]);
  const [mcpServers, setMcpServers] = createSignal<McpServerConfig[]>([]);
  const [prompts, setPrompts] = createSignal<PromptTemplate[]>([]);

  // MCP wizard state
  const [showMcpWizard, setShowMcpWizard] = createSignal(false);
  const [editingMcpServerIdx, setEditingMcpServerIdx] = createSignal<number | null>(null);

  // System prompt editor state
  const [showPromptEditor, setShowPromptEditor] = createSignal(false);

  // Prompt template state
  const [expandedPromptIdx, setExpandedPromptIdx] = createSignal<number | null>(null);
  const [openParamHelperId, setOpenParamHelperId] = createSignal<string | null>(null);
  let paramHelperCounter = 0;

  // Skills state
  const [personaSkills, setPersonaSkills] = createSignal<InstalledSkill[]>([]);

  // Persistence tracking
  const [persisted, setPersisted] = createSignal(false);
  const [persistedId, setPersistedId] = createSignal<string | null>(null);
  const [saving, setSaving] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  // ── Build persona from draft ──────────────────────────────────

  const buildPersona = (): Persona => {
    const id = persistedId() || resolvedId();
    return {
      id,
      name: name().trim(),
      description: description().trim(),
      system_prompt: systemPrompt(),
      loop_strategy: loopStrategy(),
      preferred_models: preferredModels().length > 0 ? preferredModels() : null,
      secondary_models: secondaryModels().length > 0 ? secondaryModels() : null,
      allowed_tools: allTools() ? ['*'] : selectedTools(),
      mcp_servers: mcpServers(),
      avatar: normalizeOptional(avatar()),
      color: normalizeOptional(color()),
      prompts: prompts(),
    };
  };

  // ── ID resolution ─────────────────────────────────────────────

  const resolvedId = () => {
    const raw = personaId().trim();
    if (raw && raw.startsWith('user/') && raw.length > 5) return raw;
    return 'user/' + buildAgentId(name(), props.existingPersonaIds);
  };

  const idSegment = () => {
    const raw = personaId().trim();
    return raw.startsWith('user/') ? raw.substring(5) : raw;
  };

  const idError = createMemo(() => {
    const seg = idSegment();
    if (!seg) return null; // auto-generate
    if (!/^[a-zA-Z0-9][a-zA-Z0-9_-]*$/.test(seg)) return 'Must use letters, numbers, hyphens, and underscores.';
    if (props.existingPersonaIds.includes(`user/${seg}`) || props.existingPersonaIds.includes(seg))
      return 'A persona with this ID already exists.';
    return null;
  });

  // ── Validation per step ───────────────────────────────────────

  const canAdvance = createMemo(() => {
    const s = step();
    if (s === 'identity') return !!name().trim() && !idError();
    return true; // other steps are optional
  });

  // ── Persistence helpers ───────────────────────────────────────

  const persistPersona = async () => {
    setSaving(true);
    setError(null);
    try {
      // Fetch latest persona list to avoid overwriting
      const current = await invoke<Persona[]>('list_personas', { include_archived: true });
      const persona = buildPersona();
      const finalId = persona.id;

      let next: Persona[];
      if (persisted() && persistedId()) {
        // Update existing
        next = current.map(p => p.id === persistedId() ? persona : p);
      } else {
        // Append new
        next = [...current, persona];
      }

      await invoke('save_personas', { personas: next });
      setPersisted(true);
      setPersistedId(finalId);
      return finalId;
    } catch (e: any) {
      setError(e?.toString() ?? 'Failed to save persona.');
      throw e;
    } finally {
      setSaving(false);
    }
  };

  const deletePersistedPersona = async () => {
    if (!persisted() || !persistedId()) return;
    try {
      const current = await invoke<Persona[]>('list_personas', { include_archived: true });
      const next = current.filter(p => p.id !== persistedId());
      await invoke('save_personas', { personas: next });
    } catch {
      // Best-effort cleanup
    }
  };

  const loadPersonaSkills = async (pid: string) => {
    try {
      const skills = await invoke<InstalledSkill[]>('skills_list_installed_for_persona', { persona_id: pid });
      setPersonaSkills(skills);
    } catch {
      setPersonaSkills([]);
    }
  };

  // ── Navigation ────────────────────────────────────────────────

  const next = async () => {
    const idx = currentIdx();
    if (idx >= WIZARD_STEPS.length - 1) return;

    const nextStep = WIZARD_STEPS[idx + 1];

    // Persist persona before entering skills step (first time or re-save on changes)
    if (nextStep === 'skills') {
      try {
        const pid = await persistPersona();
        await loadPersonaSkills(pid);
      } catch {
        return; // error already set
      }
    }

    setStep(nextStep);
  };

  const back = () => {
    const idx = currentIdx();
    if (idx > 0) setStep(WIZARD_STEPS[idx - 1]);
  };

  const handleCancel = async () => {
    // Clean up persisted persona if we created one
    if (persisted()) {
      await deletePersistedPersona();
    }
    props.onClose();
  };

  const handleFinish = async () => {
    // Validate prompt templates
    for (const tpl of prompts()) {
      try {
        Handlebars.precompile(tpl.template);
      } catch (e: any) {
        setError(`Template "${tpl.name || tpl.id || '(unnamed)'}" has invalid Handlebars syntax: ${e.message}`);
        return;
      }
    }

    // Check duplicate template IDs
    const templateIds = prompts().map(p => p.id).filter(Boolean);
    const seenIds = new Set<string>();
    for (const tid of templateIds) {
      if (seenIds.has(tid)) {
        setError(`Duplicate template ID "${tid}".`);
        return;
      }
      seenIds.add(tid);
    }

    try {
      await persistPersona();
      await props.onFinish();
      props.onClose();
    } catch {
      // error already set by persistPersona
    }
  };

  // ── Model helpers ─────────────────────────────────────────────

  const addPreferredModel = (input: HTMLInputElement) => {
    const val = input.value.trim();
    if (!val || preferredModels().includes(val)) return;
    setPreferredModels(prev => [...prev, val]);
    input.value = '';
  };

  const addSecondaryModel = (input: HTMLInputElement) => {
    const val = input.value.trim();
    if (!val || secondaryModels().includes(val)) return;
    setSecondaryModels(prev => [...prev, val]);
    input.value = '';
  };

  // ── Prompt template helpers ───────────────────────────────────

  const addPromptTemplate = () => {
    const tpl = createEmptyPromptTemplate(prompts().map(p => p.id));
    setPrompts(prev => [...prev, tpl]);
    setExpandedPromptIdx(prompts().length - 1);
  };

  const removePromptTemplate = (idx: number) => {
    setPrompts(prev => prev.filter((_, i) => i !== idx));
    setExpandedPromptIdx(null);
  };

  const updatePromptTemplate = (idx: number, patch: Partial<PromptTemplate>) => {
    setPrompts(prev => {
      const next = [...prev];
      next[idx] = { ...next[idx], ...patch };
      if (patch.name !== undefined && next[idx].id.startsWith('new-prompt')) {
        const slug = slugify(patch.name);
        if (slug) next[idx].id = slug;
      }
      return next;
    });
  };

  function renderParamInsertHelper(
    fields: () => PromptSchemaField[],
    onInsert: (text: string) => void,
    inputEl?: () => HTMLTextAreaElement | HTMLInputElement | undefined,
  ) {
    const myId = `param_${paramHelperCounter++}`;
    const isOpen = () => openParamHelperId() === myId;

    return (
      <Popover open={isOpen()} onOpenChange={(o) => { if (!o) setOpenParamHelperId(null); }} placement="bottom-end" gutter={2}>
        <PopoverTrigger as="div" style={{ display: 'inline-block' }}>
          <button
            onClick={() => setOpenParamHelperId(isOpen() ? null : myId)}
            style={{ background: 'none', border: '1px solid hsl(var(--border))', color: 'hsl(var(--primary))', cursor: 'pointer', 'border-radius': '3px', padding: '1px 5px', 'font-size': '0.7em', 'margin-left': '4px' }}
            title="Insert parameter reference"
          >{'{{}}'}</button>
        </PopoverTrigger>
        <PopoverContent class="w-auto p-0" style={{ 'z-index': '10000', background: 'hsl(var(--card))', border: '1px solid hsl(var(--border))', 'border-radius': '6px', 'box-shadow': '0 4px 12px rgba(0,0,0,0.4)', 'min-width': '220px', 'max-height': '260px', 'overflow-y': 'auto', padding: '4px 0' }}>
          <div style={{ 'font-size': '0.65em', color: 'hsl(var(--muted-foreground))', padding: '4px 10px 2px', 'font-weight': '600', 'text-transform': 'uppercase', 'letter-spacing': '0.5px' }}>Parameters</div>
          <Show when={fields().length > 0} fallback={<div style={{ padding: '4px 10px 8px', color: 'hsl(var(--muted-foreground))', 'font-size': '0.8em', 'font-style': 'italic' }}>No parameters defined yet</div>}>
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
                      onInsert(el.value.slice(0, start) + expr + el.value.slice(end));
                      requestAnimationFrame(() => { el.focus(); el.setSelectionRange(start + expr.length, start + expr.length); });
                    } else {
                      onInsert(expr);
                    }
                    setOpenParamHelperId(null);
                  }}
                  style={{ display: 'block', width: '100%', 'text-align': 'left', background: 'none', border: 'none', padding: '4px 10px', color: 'hsl(var(--foreground))', cursor: 'pointer', 'font-size': '0.85em', 'font-family': 'monospace' }}
                  onMouseEnter={(e) => (e.currentTarget.style.background = 'hsl(var(--primary) / 0.1)')}
                  onMouseLeave={(e) => (e.currentTarget.style.background = 'none')}
                >
                  {f.name}
                  <span style={{ color: 'hsl(var(--muted-foreground))', 'margin-left': '6px', 'font-size': '0.85em' }}>{`{{${f.name}}}`}</span>
                </button>
              )}
            </For>
          </Show>
          <div style={{ 'font-size': '0.65em', color: 'hsl(var(--muted-foreground))', padding: '4px 10px 2px', 'font-weight': '600', 'text-transform': 'uppercase', 'letter-spacing': '0.5px', 'border-top': '1px solid hsl(var(--border))', 'margin-top': '2px' }}>Helpers</div>
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
                    onInsert(el.value.slice(0, start) + h.value + el.value.slice(end));
                    requestAnimationFrame(() => { el.focus(); el.setSelectionRange(start + h.value.length, start + h.value.length); });
                  } else {
                    onInsert(h.value);
                  }
                  setOpenParamHelperId(null);
                }}
                style={{ display: 'block', width: '100%', 'text-align': 'left', background: 'none', border: 'none', padding: '4px 10px', color: 'hsl(var(--foreground))', cursor: 'pointer', 'font-size': '0.85em', 'font-family': 'monospace' }}
                onMouseEnter={(e) => (e.currentTarget.style.background = 'hsl(var(--primary) / 0.1)')}
                onMouseLeave={(e) => (e.currentTarget.style.background = 'none')}
              >{h.label}</button>
            )}
          </For>
        </PopoverContent>
      </Popover>
    );
  }

  // ── Step renderers ────────────────────────────────────────────

  const renderIdentityStep = () => (
    <div class="settings-form" style="gap: 1rem;">
      <label>
        <span>Name *</span>
        <input
          type="text"
          value={name()}
          onInput={(e) => setName(e.currentTarget.value)}
          placeholder="Research Persona"
        />
      </label>

      <label>
        <span>Persona ID</span>
        <input
          type="text"
          value={personaId()}
          onInput={(e) => setPersonaId(e.currentTarget.value)}
          placeholder="user/my-persona"
          disabled={persisted()}
        />
        <Show when={idError()}>
          <span style="font-size: 0.8rem; color: hsl(var(--destructive));">{idError()}</span>
        </Show>
        <Show when={!idError()}>
          <span class="muted" style="font-size: 0.8rem;">
            {persisted() ? 'ID is locked after skills step.' : 'Must start with "user/" — leave as "user/" to auto-generate from name'}
          </span>
        </Show>
      </label>

      <label>
        <span>Description</span>
        <input
          type="text"
          value={description()}
          onInput={(e) => setDescription(e.currentTarget.value)}
          placeholder="Describe what this persona is optimized for"
        />
      </label>

      <label>
        <span>Avatar</span>
        <input
          type="text"
          value={avatar()}
          onInput={(e) => setAvatar(e.currentTarget.value)}
          placeholder=""
        />
      </label>

      <label class="agents-form-field">
        <span>Persona Color</span>
        <div class="agents-form-control">
          <div class="agents-color-row">
            <input
              type="text"
              value={color()}
              onInput={(e) => setColor(e.currentTarget.value)}
              placeholder="#89b4fa"
            />
            <input
              class="agents-color-swatch"
              type="color"
              value={displayColor(color())}
              onInput={(e) => setColor(e.currentTarget.value)}
            />
          </div>
        </div>
      </label>
    </div>
  );

  const renderPromptStep = () => (
    <div class="settings-form" style="gap: 1rem;">
      <label class="agents-form-field">
        <span>System Prompt</span>
        <div class="agents-form-control">
          <div
            style="position:relative;border:1px solid hsl(214 14% 22%);border-radius:6px;background:hsl(215 21% 7%);cursor:pointer;overflow:hidden"
            onClick={() => setShowPromptEditor(true)}
          >
            <pre style="margin:0;padding:10px 12px;padding-right:48px;font-family:inherit;font-size:0.85em;white-space:pre-wrap;word-break:break-word;color:hsl(210 13% 81%);max-height:4.8em;overflow:hidden;line-height:1.6">
              <Show
                when={systemPrompt().trim()}
                fallback={<span style="color:hsl(212 10% 53%);font-style:italic">Click to add system prompt…</span>}
              >
                {systemPrompt()}
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
            {systemPrompt().length} characters
          </p>
        </div>
      </label>

      <SystemPromptEditorDialog
        open={showPromptEditor()}
        value={systemPrompt()}
        onSave={(val) => { setSystemPrompt(val); setShowPromptEditor(false); }}
        onCancel={() => setShowPromptEditor(false)}
      />

      <div>
        <label>
          <span>Loop Strategy</span>
          <select
            value={loopStrategy()}
            onChange={(e) => setLoopStrategy(e.currentTarget.value as Persona['loop_strategy'])}
          >
            <option value="react">React</option>
            <option value="sequential">Sequential</option>
            <option value="plan_then_execute">Plan Then Execute</option>
          </select>
        </label>
        <p class="muted" style="font-size:0.8em;margin:2px 0 0">
          {loopStrategyDescription(loopStrategy())}
        </p>
      </div>
    </div>
  );

  const renderModelsStep = () => (
    <div class="settings-form" style="gap: 1rem;">
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
            </PopoverContent>
          </Popover>
        </span>
        <div class="preferred-models-editor">
          <Show when={preferredModels().length > 0}>
            <ol class="preferred-models-list">
              <Index each={preferredModels()}>
                {(model, index) => (
                  <li class="preferred-models-item">
                    <input
                      type="text"
                      list="wizard-models-list"
                      class="preferred-models-item-input"
                      value={model()}
                      onInput={(e) => {
                        const val = e.currentTarget.value;
                        setPreferredModels(prev => { const items = [...prev]; items[index] = val; return items; });
                      }}
                    />
                    <span class="preferred-models-item-actions">
                      <button type="button" disabled={index === 0}
                        onClick={() => setPreferredModels(prev => { const items = [...prev]; [items[index - 1], items[index]] = [items[index], items[index - 1]]; return items; })}>▲</button>
                      <button type="button" disabled={index === preferredModels().length - 1}
                        onClick={() => setPreferredModels(prev => { const items = [...prev]; [items[index], items[index + 1]] = [items[index + 1], items[index]]; return items; })}>▼</button>
                      <button type="button" class="tag-remove"
                        onClick={() => setPreferredModels(prev => prev.filter((_, i) => i !== index))}>×</button>
                    </span>
                  </li>
                )}
              </Index>
            </ol>
          </Show>
          <div class="preferred-models-add">
            <input
              type="text"
              list="wizard-models-list"
              placeholder="Type a model name or pattern (e.g. gpt-5.*)"
              onKeyDown={(e) => { if (e.key === 'Enter') { e.preventDefault(); addPreferredModel(e.currentTarget); } }}
              onChange={(e) => addPreferredModel(e.currentTarget)}
            />
            <datalist id="wizard-models-list">
              <For each={[...new Set(props.availableModels.map((m) => {
                const colon = m.id.indexOf(':');
                return colon >= 0 ? m.id.slice(colon + 1) : m.id;
              }))]}>
                {(mname) => <option value={mname} />}
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
          <Show when={secondaryModels().length > 0}>
            <ol class="preferred-models-list">
              <Index each={secondaryModels()}>
                {(model, index) => (
                  <li class="preferred-models-item">
                    <input
                      type="text"
                      list="wizard-models-list"
                      class="preferred-models-item-input"
                      value={model()}
                      onInput={(e) => {
                        const val = e.currentTarget.value;
                        setSecondaryModels(prev => { const items = [...prev]; items[index] = val; return items; });
                      }}
                    />
                    <span class="preferred-models-item-actions">
                      <button type="button" disabled={index === 0}
                        onClick={() => setSecondaryModels(prev => { const items = [...prev]; [items[index - 1], items[index]] = [items[index], items[index - 1]]; return items; })}>▲</button>
                      <button type="button" disabled={index === secondaryModels().length - 1}
                        onClick={() => setSecondaryModels(prev => { const items = [...prev]; [items[index], items[index + 1]] = [items[index + 1], items[index]]; return items; })}>▼</button>
                      <button type="button" class="tag-remove"
                        onClick={() => setSecondaryModels(prev => prev.filter((_, i) => i !== index))}>×</button>
                    </span>
                  </li>
                )}
              </Index>
            </ol>
          </Show>
          <div class="preferred-models-add">
            <input
              type="text"
              list="wizard-models-list"
              placeholder="Type a model name or pattern (e.g. gpt-4.1-mini)"
              onKeyDown={(e) => { if (e.key === 'Enter') { e.preventDefault(); addSecondaryModel(e.currentTarget); } }}
              onChange={(e) => addSecondaryModel(e.currentTarget)}
            />
          </div>
        </div>
      </label>
    </div>
  );

  const renderToolsStep = () => (
    <div class="settings-form" style="gap: 1rem;">
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
                    <button class="small" onClick={() => { setEditingMcpServerIdx(idx()); setShowMcpWizard(true); }}>Edit</button>
                    <button class="small danger" onClick={() => setMcpServers(mcpServers().filter((_, i) => i !== idx()))}>Remove</button>
                  </div>
                </div>
              )}
            </For>
          </Show>
          <Show when={showMcpWizard() && props.daemon_url}>
            <McpServerWizard
              daemon_url={props.daemon_url!}
              existingIds={mcpServers().filter((_, i) => i !== editingMcpServerIdx()).map(s => s.id)}
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
              onClose={() => { setEditingMcpServerIdx(null); setShowMcpWizard(false); }}
            />
          </Show>
        </div>
      </div>
    </div>
  );

  const renderSkillsStep = () => (
    <div style="min-height: 300px;">
      <Show when={persisted() && persistedId()} fallback={
        <p style="font-size: 0.9rem; color: hsl(var(--muted-foreground)); text-align: center; padding: 2rem;">
          Persona must be saved before managing skills.
        </p>
      }>
        <SkillsTab availableModels={props.availableModels} persona_id={persistedId()!} />
      </Show>
    </div>
  );

  const renderPromptsStep = () => (
    <div class="settings-form" style="gap: 1rem;">
      <div class="agents-form-section">
        <div style="display:flex;align-items:center;justify-content:space-between;margin-bottom:8px">
          <h4 style="margin:0">Prompt Templates</h4>
          <button class="small" onClick={addPromptTemplate} title="Add prompt template">
            <Plus size={14} /> Add
          </button>
        </div>
        <Show when={prompts().length === 0}>
          <p class="muted" style="font-size:0.85em;margin:0">
            No prompt templates yet. Add reusable parameterized prompts that can be invoked from chat, the Agent Stage, or Flight Deck.
          </p>
        </Show>
        <Index each={prompts()}>
          {(tpl, idx) => {
            const isExpanded = () => expandedPromptIdx() === idx;
            const fields = () => parseSchemaFields(tpl().input_schema);
            const [localFields, setLocalFields] = createSignal<PromptSchemaField[]>(fields());

            createEffect(() => setLocalFields(fields()));

            const commitFields = (updated: PromptSchemaField[]) => {
              setLocalFields(updated);
              updatePromptTemplate(idx, { input_schema: buildSchemaFromFields(updated) });
            };

            const duplicateTemplateId = () => {
              const currentId = tpl().id;
              if (!currentId) return false;
              return prompts().filter(p => p.id === currentId).length > 1;
            };

            let templateRef: HTMLTextAreaElement | undefined;
            const preview = createMemo(() => computePreview(tpl().template, localFields()));

            return (
              <Collapsible open={isExpanded()} onOpenChange={(open) => setExpandedPromptIdx(open ? idx : null)}>
                <div class="prompt-template-card">
                  <CollapsibleTrigger as="div" class="prompt-template-header">
                    {isExpanded() ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
                    <span class="prompt-template-title">{tpl().name || '(unnamed)'}</span>
                    <button class="icon-btn danger" title="Remove" onClick={(e) => { e.stopPropagation(); removePromptTemplate(idx); }}>
                      <Trash2 size={14} />
                    </button>
                  </CollapsibleTrigger>
                  <CollapsibleContent>
                    <div class="prompt-template-body">
                      <label class="agents-form-field">
                        <span>Name</span>
                        <input type="text" value={tpl().name} onInput={(e) => updatePromptTemplate(idx, { name: e.currentTarget.value })} placeholder="e.g. Review Pull Request" />
                      </label>
                      <div class="agents-form-field">
                        <span>ID</span>
                        <input type="text" value={tpl().id} onInput={(e) => updatePromptTemplate(idx, { id: slugify(e.currentTarget.value) })} placeholder="auto-generated from name" />
                        <Show when={duplicateTemplateId()}>
                          <p style="font-size:0.75em;color:hsl(var(--destructive));margin:2px 0 0">Duplicate template ID — each template must have a unique ID.</p>
                        </Show>
                      </div>
                      <label class="agents-form-field">
                        <span>Description</span>
                        <input type="text" value={tpl().description ?? ''} onInput={(e) => updatePromptTemplate(idx, { description: e.currentTarget.value })} placeholder="Brief description of what this prompt does" />
                      </label>
                      <div class="agents-form-field">
                        <div style={{ display: 'flex', 'align-items': 'center' }}>
                          <span style={{ flex: '1', 'font-weight': '500' }}>Template (Handlebars)</span>
                          {renderParamInsertHelper(localFields, (newVal) => updatePromptTemplate(idx, { template: newVal }), () => templateRef)}
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
                      <PromptSchemaEditor fields={localFields()} onChange={commitFields} />
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
    </div>
  );

  // ── Main render ───────────────────────────────────────────────

  return (
    <Dialog open={true} onOpenChange={(open) => { if (!open) void handleCancel(); }}>
      <DialogContent class="max-w-[750px] w-[90vw] max-h-[85vh] overflow-y-auto overflow-x-hidden flex flex-col p-0" onInteractOutside={(e) => e.preventDefault()}>
        <div class="channel-wizard-header">
          <h2>Create Persona</h2>
          <div class="wizard-steps">
            <For each={WIZARD_STEPS}>
              {(s, i) => {
                const stepIdx = () => WIZARD_STEPS.indexOf(s);
                return (
                  <>
                    <Show when={i() > 0}>
                      <div class="wizard-step-line" classList={{ completed: stepIdx() <= currentIdx() }} />
                    </Show>
                    <div
                      class="wizard-step"
                      classList={{
                        active: stepIdx() === currentIdx(),
                        completed: stepIdx() < currentIdx(),
                      }}
                    >
                      <div class="wizard-step-num">
                        {stepIdx() < currentIdx() ? '✓' : i() + 1}
                      </div>
                      <span class="wizard-step-label">{STEP_LABELS[s]}</span>
                    </div>
                  </>
                );
              }}
            </For>
          </div>
        </div>

        <div class="channel-wizard-body">
          <Show when={error()}>
            <div style="padding: 0.5rem 1rem; background: hsl(var(--destructive) / 0.15); border: 1px solid hsl(var(--destructive)); border-radius: 6px; color: hsl(var(--destructive)); font-size: 0.85rem; margin-bottom: 0.75rem;">
              {error()}
            </div>
          </Show>

          <Show when={step() === 'identity'}>{renderIdentityStep()}</Show>
          <Show when={step() === 'prompt'}>{renderPromptStep()}</Show>
          <Show when={step() === 'models'}>{renderModelsStep()}</Show>
          <Show when={step() === 'tools'}>{renderToolsStep()}</Show>
          <Show when={step() === 'skills'}>{renderSkillsStep()}</Show>
          <Show when={step() === 'prompts'}>{renderPromptsStep()}</Show>
        </div>

        <div class="channel-wizard-footer">
          <div style={{ display: 'flex', gap: '0.5rem' }}>
            <Button variant="outline" onClick={() => void handleCancel()}>Cancel</Button>
            <Show when={step() !== 'identity'}>
              <Button variant="outline" onClick={back}>← Back</Button>
            </Show>
          </div>
          <Show when={step() !== 'prompts'} fallback={
            <Button onClick={() => void handleFinish()} disabled={saving()}>
              {saving() ? 'Creating…' : 'Create Persona'}
            </Button>
          }>
            <Button onClick={() => void next()} disabled={!canAdvance() || saving()}>
              {saving() ? 'Saving…' : 'Next →'}
            </Button>
          </Show>
        </div>
      </DialogContent>
    </Dialog>
  );
}
