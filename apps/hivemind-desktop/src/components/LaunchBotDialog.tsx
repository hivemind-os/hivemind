import { createSignal, createEffect, For, Show, Accessor } from 'solid-js';
import { Rocket, XCircle, Bot } from 'lucide-solid';
import type { Persona, ToolDefinition } from '../types';
import PermissionRulesEditor, { type PermissionRule } from './PermissionRulesEditor';
import { Dialog, DialogContent } from '~/ui/dialog';
import { Button } from '~/ui/button';
import { Switch, SwitchControl, SwitchThumb, SwitchLabel } from '~/ui/switch';
import { buildNamespaceTree, flattenNamespaceTree } from '~/lib/workflowGrouping';

export interface ModelOption {
  id: string;
  label: string;
}

interface LaunchBotDialogProps {
  open: Accessor<boolean>;
  onClose: () => void;
  onLaunch: (config: LaunchConfig) => Promise<void>;
  personas: Accessor<Persona[]>;
  availableTools: Accessor<string[]>;
  toolDefinitions?: ToolDefinition[];
}

export interface LaunchConfig {
  friendlyName: string;
  description: string;
  persona_id: string | null;
  launchPrompt: string;
  mode: 'idle_after_task' | 'continuous' | 'one_shot';
  timeoutSecs: number | null;
  allowed_tools: string[];
  data_class: string;
  avatar: string;
  permissionRules: PermissionRule[];
}

type WizardStep = 'basics' | 'behavior' | 'access' | 'review';

const STEPS: WizardStep[] = ['basics', 'behavior', 'access', 'review'];
const STEP_LABELS: Record<WizardStep, string> = {
  basics: 'Basics',
  behavior: 'Behavior',
  access: 'Access & Permissions',
  review: 'Review & Launch',
};

export default function LaunchBotDialog(props: LaunchBotDialogProps) {
  const [step, setStep] = createSignal<WizardStep>('basics');
  const [name, setName] = createSignal('');
  const [persona_id, setPersonaId] = createSignal(props.personas()?.[0]?.id ?? '');
  const [launchPrompt, setLaunchPrompt] = createSignal('');
  const [mode, setMode] = createSignal<'idle_after_task' | 'continuous' | 'one_shot'>('one_shot');
  const [overrideTools, setOverrideTools] = createSignal(false);
  const [selectedTools, setSelectedTools] = createSignal<string[]>([]);
  const [data_class, setDataClass] = createSignal('INTERNAL');
  const [launching, setLaunching] = createSignal(false);
  const [permRules, setPermRules] = createSignal<PermissionRule[]>([]);
  const [timeoutEnabled, setTimeoutEnabled] = createSignal(false);
  const [timeoutHours, setTimeoutHours] = createSignal(0);
  const [timeoutMinutes, setTimeoutMinutes] = createSignal(5);
  const [error, setError] = createSignal<string | null>(null);

  const selectedPersona = () => props.personas().find(p => p.id === persona_id()) ?? null;

  // When personas load for the first time, set default selection
  createEffect(() => {
    const list = props.personas();
    if (list.length > 0 && !list.some(p => p.id === persona_id())) {
      setPersonaId(list[0].id);
    } else if (list.length === 0) {
      setPersonaId('');
    }
  });
  const currentIdx = () => STEPS.indexOf(step());

  const computedTimeoutSecs = () => {
    if (!timeoutEnabled()) return null;
    const total = timeoutHours() * 3600 + timeoutMinutes() * 60;
    return total > 0 ? total : null;
  };

  const reset = () => {
    setStep('basics');
    setName('');
    setPersonaId(props.personas()?.[0]?.id ?? '');
    setLaunchPrompt('');
    setMode('one_shot');
    setOverrideTools(false);
    setSelectedTools([]);
    setDataClass('INTERNAL');
    setPermRules([]);
    setTimeoutEnabled(false);
    setTimeoutHours(0);
    setTimeoutMinutes(5);
    setError(null);
  };

  const canAdvance = (): boolean => {
    switch (step()) {
      case 'basics':
        return name().trim().length > 0 && launchPrompt().trim().length > 0;
      default:
        return true;
    }
  };

  const next = () => {
    const idx = currentIdx();
    if (idx < STEPS.length - 1 && canAdvance()) setStep(STEPS[idx + 1]);
  };

  const back = () => {
    const idx = currentIdx();
    if (idx > 0) setStep(STEPS[idx - 1]);
  };

  const handleLaunch = async () => {
    if (!name().trim() || !launchPrompt().trim()) return;
    setLaunching(true);
    setError(null);
    try {
      const persona = selectedPersona();
      let allowed_tools: string[];
      if (overrideTools()) {
        allowed_tools = selectedTools();
      } else {
        allowed_tools = persona?.allowed_tools ?? ['*'];
      }
      await props.onLaunch({
        friendlyName: name().trim(),
        description: persona?.description ?? '',
        persona_id: persona_id() || null,
        launchPrompt: launchPrompt().trim(),
        mode: mode(),
        timeoutSecs: mode() === 'one_shot' ? computedTimeoutSecs() : null,
        allowed_tools,
        data_class: data_class(),
        avatar: persona?.avatar ?? '',
        permissionRules: permRules(),
      });
      reset();
      props.onClose();
    } catch (err: any) {
      const message = typeof err === 'string' ? err : err?.message ?? 'Unknown error';
      setError(message);
      console.error('Failed to launch bot:', err);
    } finally {
      setLaunching(false);
    }
  };

  const toggleTool = (tool: string) => {
    setSelectedTools(prev =>
      prev.includes(tool) ? prev.filter(t => t !== tool) : [...prev, tool]
    );
  };

  // ── Step renderers ────────────────────────────────────────────────────

  const renderBasicsStep = () => (
    <>
      <div class="space-y-1.5 mb-4">
        <label class="font-medium">Name *</label>
        <input
          type="text"
          value={name()}
          onInput={(e) => setName(e.currentTarget.value)}
          placeholder="e.g. Code Reviewer"
          class="w-full"
        />
      </div>

      <div class="space-y-1.5 mb-4">
        <label class="font-medium">Persona</label>
        <select
          value={persona_id()}
          onChange={(e) => setPersonaId(e.currentTarget.value)}
          class="w-full"
        >
          <Show when={props.personas().length === 0}>
            <option disabled value="">Loading personas…</option>
          </Show>
          <For each={flattenNamespaceTree(buildNamespaceTree(props.personas(), (p) => p.id, (p) => p.name))}>
            {([ns, personas]) => (
              <optgroup label={ns}>
                <For each={personas}>
                  {(p) => <option value={p.id}>{p.avatar ? `${p.avatar} ` : ''}{p.name}</option>}
                </For>
              </optgroup>
            )}
          </For>
        </select>
        <Show when={selectedPersona()}>
          {(_p) => {
            const p = selectedPersona()!;
            return (
              <div class="text-xs text-muted-foreground mt-1">
                {p.description}
                <Show when={p.preferred_models?.length}>
                  <div class="mt-0.5">Models: {p.preferred_models!.join(', ')}</div>
                </Show>
              </div>
            );
          }}
        </Show>
      </div>

      <div class="space-y-1.5 mb-4">
        <label class="font-medium">Launch Prompt *</label>
        <textarea
          value={launchPrompt()}
          onInput={(e) => setLaunchPrompt(e.currentTarget.value)}
          placeholder="Initial task or standing orders for the bot"
          rows={5}
          class="w-full"
        />
      </div>
    </>
  );

  const renderBehaviorStep = () => (
    <>
      <div class="space-y-1.5 mb-4">
        <label class="font-medium">Mode</label>
        <div class="flex gap-3 flex-wrap mt-1">
          <label class="cursor-pointer flex items-center gap-1.5">
            <input type="radio" name="bot-mode" checked={mode() === 'one_shot'} onChange={() => setMode('one_shot')} />
            Complete task &amp; terminate
          </label>
          <label class="cursor-pointer flex items-center gap-1.5">
            <input type="radio" name="bot-mode" checked={mode() === 'idle_after_task'} onChange={() => setMode('idle_after_task')} />
            Complete task then wait
          </label>
          <label class="cursor-pointer flex items-center gap-1.5">
            <input type="radio" name="bot-mode" checked={mode() === 'continuous'} onChange={() => setMode('continuous')} />
            Run continuously
          </label>
        </div>
      </div>

      <Show when={mode() === 'one_shot'}>
        <div class="space-y-1.5 mb-4">
          <Switch checked={timeoutEnabled()} onChange={(checked) => setTimeoutEnabled(checked)} class="flex items-center gap-2">
            <SwitchControl><SwitchThumb /></SwitchControl>
            <SwitchLabel class="font-medium">Timeout</SwitchLabel>
          </Switch>
          <Show when={timeoutEnabled()}>
            <div class="flex items-center gap-2 mt-1">
              <input
                type="number"
                value={timeoutHours()}
                onInput={(e) => setTimeoutHours(Math.max(0, parseInt(e.currentTarget.value) || 0))}
                min="0"
                max="72"
                style="width:70px;"
              />
              <span class="text-sm text-muted-foreground">hr</span>
              <input
                type="number"
                value={timeoutMinutes()}
                onInput={(e) => setTimeoutMinutes(Math.max(0, Math.min(59, parseInt(e.currentTarget.value) || 0)))}
                min="0"
                max="59"
                style="width:70px;"
              />
              <span class="text-sm text-muted-foreground">min</span>
            </div>
            <div class="text-xs text-muted-foreground mt-1">
              Maximum time the agent can run before being terminated.
            </div>
          </Show>
          <Show when={!timeoutEnabled()}>
            <div class="text-xs text-muted-foreground mt-0.5">
              No timeout — the bot will run until the task is complete.
            </div>
          </Show>
        </div>
      </Show>

      <div class="space-y-1.5 mb-4">
        <label class="font-medium">Data Classification</label>
        <select value={data_class()} onChange={(e) => setDataClass(e.currentTarget.value)} class="w-full">
          <option value="PUBLIC">Public</option>
          <option value="INTERNAL">Internal</option>
          <option value="CONFIDENTIAL">Confidential</option>
          <option value="RESTRICTED">Restricted</option>
        </select>
      </div>
    </>
  );

  const renderAccessStep = () => (
    <>
      {/* Tool overrides */}
      <div class="space-y-1.5 mb-4">
        <Switch checked={overrideTools()} onChange={(checked) => {
          setOverrideTools(checked);
          if (checked && selectedTools().length === 0) {
            const persona = selectedPersona();
            if (persona) {
              const personaTools = persona.allowed_tools ?? [];
              if (personaTools.includes('*')) {
                setSelectedTools([...props.availableTools()]);
              } else {
                setSelectedTools(personaTools.filter(t => props.availableTools().includes(t)));
              }
            }
          }
        }} class="flex items-center gap-2">
          <SwitchControl><SwitchThumb /></SwitchControl>
          <SwitchLabel>Override persona tools</SwitchLabel>
        </Switch>
        <Show when={overrideTools()}>
          <div class="max-h-[180px] overflow-y-auto border border-border rounded-md p-1.5 mt-1">
            {(() => {
              const grouped = () => {
                const groups: Record<string, { id: string; label: string }[]> = {};
                const defs = props.toolDefinitions ?? [];
                for (const toolName of props.availableTools()) {
                  const def = defs.find(d => d.name === toolName);
                  const tool_id = def?.id ?? toolName;
                  const dot = tool_id.indexOf('.');
                  const ns = dot > 0 ? tool_id.slice(0, dot) : 'other';
                  const label = dot > 0 ? tool_id.slice(dot + 1) : toolName;
                  (groups[ns] ??= []).push({ id: toolName, label });
                }
                return Object.entries(groups).sort(([a], [b]) => a.localeCompare(b));
              };
              return (
                <For each={grouped()}>
                  {([ns, tools]) => {
                    const [expanded, setExpanded] = createSignal(true);
                    const toolIds = () => tools.map(t => t.id);
                    const allSelected = () => toolIds().every(t => selectedTools().includes(t));
                    const someSelected = () => toolIds().some(t => selectedTools().includes(t));
                    const toggleGroup = () => {
                      const ids = toolIds();
                      if (allSelected()) {
                        setSelectedTools(prev => prev.filter(t => !ids.includes(t)));
                      } else {
                        setSelectedTools(prev => [...new Set([...prev, ...ids])]);
                      }
                    };
                    return (
                      <div class="mb-1">
                        <div class="flex items-center gap-1 py-0.5 cursor-pointer select-none" style="font-size:0.8em;">
                          <button onClick={() => setExpanded(!expanded())} class="border-none bg-transparent cursor-pointer text-muted-foreground" style="padding:0;width:14px;font-size:10px;">
                            {expanded() ? '▾' : '▸'}
                          </button>
                          <input
                            type="checkbox"
                            checked={allSelected()}
                            ref={(el) => { el.indeterminate = someSelected() && !allSelected(); }}
                            onChange={toggleGroup}
                          />
                          <span class="font-semibold text-muted-foreground" onClick={() => setExpanded(!expanded())} style="text-transform:uppercase;letter-spacing:0.5px;">
                            {ns} ({tools.length})
                          </span>
                        </div>
                        <Show when={expanded()}>
                          <div style="padding-left:28px;">
                            <For each={tools}>
                              {(tool) => (
                                <label class="block cursor-pointer py-0.5" style="font-size:0.85em;">
                                  <input type="checkbox" checked={selectedTools().includes(tool.id)} onChange={() => toggleTool(tool.id)} />{' '}
                                  {tool.label}
                                </label>
                              )}
                            </For>
                          </div>
                        </Show>
                      </div>
                    );
                  }}
                </For>
              );
            })()}
          </div>
        </Show>
      </div>

      {/* Permission rules */}
      <div class="mb-3">
        <label class="font-medium">Permission Rules</label>
        <p class="text-xs text-muted-foreground" style="margin:4px 0 8px;">
          Control tool approval: Auto (allow), Ask (prompt), or Deny.
        </p>
        <PermissionRulesEditor
          rules={permRules}
          setRules={(rules) => setPermRules(rules)}
          toolDefinitions={props.toolDefinitions}
        />
      </div>
    </>
  );

  const renderReviewStep = () => {
    const persona = selectedPersona();
    const modeLabel = () => {
      switch (mode()) {
        case 'one_shot': return 'Complete task & terminate';
        case 'idle_after_task': return 'Complete task then wait';
        case 'continuous': return 'Run continuously';
      }
    };
    return (
      <div class="space-y-3">
        <div class="rounded-md border border-border p-3 space-y-2 text-sm">
          <div class="flex justify-between">
            <span class="text-muted-foreground">Name</span>
            <span class="font-medium">{name()}</span>
          </div>
          <div class="flex justify-between">
            <span class="text-muted-foreground">Persona</span>
            <span class="font-medium">{persona?.avatar ?? ''} {persona?.name ?? persona_id()}</span>
          </div>
          <div class="flex justify-between">
            <span class="text-muted-foreground">Mode</span>
            <span class="font-medium">{modeLabel()}</span>
          </div>
          <Show when={mode() === 'one_shot' && computedTimeoutSecs()}>
            <div class="flex justify-between">
              <span class="text-muted-foreground">Timeout</span>
              <span class="font-medium">
                {timeoutHours() > 0 ? `${timeoutHours()}h ` : ''}{timeoutMinutes()}m
              </span>
            </div>
          </Show>
          <div class="flex justify-between">
            <span class="text-muted-foreground">Data Classification</span>
            <span class="font-medium">{data_class()}</span>
          </div>
          <Show when={overrideTools()}>
            <div class="flex justify-between">
              <span class="text-muted-foreground">Tools Override</span>
              <span class="font-medium">{selectedTools().length} tool{selectedTools().length !== 1 ? 's' : ''}</span>
            </div>
          </Show>
          <Show when={permRules().length > 0}>
            <div class="flex justify-between">
              <span class="text-muted-foreground">Permission Rules</span>
              <span class="font-medium">{permRules().length} rule{permRules().length !== 1 ? 's' : ''}</span>
            </div>
          </Show>
        </div>

        <div class="rounded-md border border-border p-3">
          <div class="text-xs text-muted-foreground mb-1">Launch Prompt</div>
          <div class="text-sm whitespace-pre-wrap" style="max-height:120px;overflow-y:auto;">
            {launchPrompt()}
          </div>
        </div>
      </div>
    );
  };

  // ── Main render ───────────────────────────────────────────────────────

  return (
    <Dialog open={props.open()} onOpenChange={(open) => { if (!open) { reset(); props.onClose(); } }}>
      <DialogContent class="max-w-[650px] w-[90vw] max-h-[85vh] flex flex-col overflow-hidden p-0" onInteractOutside={(e) => e.preventDefault()}>
        {/* Header with step indicators */}
        <div class="channel-wizard-header">
          <h2><Bot size={16} style="display:inline;vertical-align:-2px;margin-right:6px;" />Launch Bot</h2>
          <div class="wizard-steps">
            <For each={STEPS}>
              {(s, i) => (
                <>
                  <Show when={i() > 0}>
                    <div class="wizard-step-line" classList={{ completed: i() <= currentIdx() }} />
                  </Show>
                  <div
                    class="wizard-step"
                    classList={{
                      active: i() === currentIdx(),
                      completed: i() < currentIdx(),
                    }}
                  >
                    <div class="wizard-step-num">
                      {i() < currentIdx() ? '✓' : i() + 1}
                    </div>
                    <span class="wizard-step-label">{STEP_LABELS[s]}</span>
                  </div>
                </>
              )}
            </For>
          </div>
        </div>

        {/* Body */}
        <div class="channel-wizard-body">
          <Show when={step() === 'basics'}>{(_: any) => renderBasicsStep()}</Show>
          <Show when={step() === 'behavior'}>{(_: any) => renderBehaviorStep()}</Show>
          <Show when={step() === 'access'}>{(_: any) => renderAccessStep()}</Show>
          <Show when={step() === 'review'}>{(_: any) => renderReviewStep()}</Show>
        </div>

        {/* Footer */}
        <div class="channel-wizard-footer">
          <Show when={step() !== 'basics'} fallback={
            <Button variant="outline" onClick={() => { reset(); props.onClose(); }}>Cancel</Button>
          }>
            <Button variant="outline" onClick={back}>← Back</Button>
          </Show>

          <Show when={step() !== 'review'} fallback={
            <Button onClick={handleLaunch} disabled={launching()}>
              {launching() ? 'Launching…' : <><Rocket size={14} /> Launch Bot</>}
            </Button>
          }>
            <Button onClick={next} disabled={!canAdvance()}>
              Next →
            </Button>
          </Show>
        </div>

        {/* Error overlay */}
        <Dialog open={!!error()} onOpenChange={(open) => { if (!open) setError(null); }}>
          <DialogContent class="max-w-[480px] text-center" style={{ "z-index": "1100" }}>
            <div style="font-size:2rem;margin-bottom:12px;"><XCircle size={28} /></div>
            <h3 style="margin:0 0 8px;">Launch Failed</h3>
            <p style="color:hsl(var(--muted-foreground));word-break:break-word;white-space:pre-wrap;background:rgba(0,0,0,0.2);padding:0.75rem 1rem;border-radius:6px;font-size:0.85em;margin:12px 0 16px;text-align:left;">
              {error()}
            </p>
            <Button variant="outline" onClick={() => setError(null)}>Close</Button>
          </DialogContent>
        </Dialog>
      </DialogContent>
    </Dialog>
  );
}
