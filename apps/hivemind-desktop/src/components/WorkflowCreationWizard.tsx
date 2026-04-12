import { createSignal, createEffect, Show, For, Index, untrack } from 'solid-js';
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter } from '~/ui/dialog';
import { Button } from '~/ui/button';
import { Switch, SwitchControl, SwitchThumb, SwitchLabel } from '~/ui/switch';
import { Zap, Bot, ChevronDown, Hand, Calendar, Radio, Bell, ArrowLeft, Sparkles, Copy, Plus, Trash2, Play } from 'lucide-solid';
import { invoke } from '@tauri-apps/api/core';
import yaml from 'js-yaml';
import CronBuilder from './shared/CronBuilder';
import TopicSelector, { payloadKeysForTopic } from './shared/TopicSelector';
import type { TopicInfo } from './shared/TopicSelector';
import { buildNamespaceTree, flattenNamespaceTree } from '~/lib/workflowGrouping';

// ── Types ──

interface ChannelProp {
  id: string;
  name: string;
  provider?: string;
  hasComms?: boolean;
}

interface WorkflowCreationWizardProps {
  open: boolean;
  onClose: () => void;
  /** Called when wizard completes. Returns initial YAML, whether AI assist should auto-open, and optional AI prompt. */
  onComplete: (yaml: string, openAiAssist: boolean, aiPrompt?: string) => void;
  /** Existing definitions for copy-from-template. */
  definitions: { name: string; version: string; description?: string | null; mode: string }[];
  /** Called to copy an existing definition. */
  onCopy: (source_name: string, sourceVersion: string, newName: string) => Promise<boolean>;
  /** Available connectors (for incoming_message trigger). */
  channels?: ChannelProp[];
  /** Known event topics (for event_pattern trigger). */
  eventTopics?: TopicInfo[];
}

// ── WorkflowVariable (matches WorkflowDesigner) ──

interface WorkflowVariable {
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
  itemProperties?: WorkflowVariable[];
  properties?: WorkflowVariable[];
  xUi?: { widget?: string; [key: string]: any };
}

const TYPE_LABELS: Record<string, string> = {
  'string': 'Text',
  'number': 'Number',
  'boolean': 'Boolean',
  'object': 'Object',
  'array': 'List',
};
const TYPE_VALUES = ['string', 'number', 'boolean', 'object', 'array'] as const;

interface TriggerConfig {
  type: string;
  // manual
  input_schema?: WorkflowVariable[];
  // schedule
  cron?: string;
  // event_pattern
  topic?: string;
  filter?: string;
  // incoming_message
  connector_id?: string;
  listen_channel_id?: string;
  from_filter?: string;
  subject_filter?: string;
  body_filter?: string;
  mark_as_read?: boolean;
  ignore_replies?: boolean;
}

// ── Schema builders (matching WorkflowDesigner) ──

function buildSubPropertySchema(p: WorkflowVariable): Record<string, any> {
  const prop: Record<string, any> = { type: p.varType };
  if (p.description) prop.description = p.description;
  return prop;
}

function buildVariableSchema(v: WorkflowVariable): Record<string, any> {
  const prop: Record<string, any> = { type: v.varType };
  if (v.description) prop.description = v.description;
  if (v.defaultValue) {
    if (v.varType === 'number') prop.default = Number(v.defaultValue);
    else if (v.varType === 'boolean') prop.default = v.defaultValue === 'true';
    else prop.default = v.defaultValue;
  }
  if (v.enumValues && v.enumValues.length > 0) prop.enum = v.enumValues;
  if (v.varType === 'string') {
    if (v.minLength != null) prop.minLength = v.minLength;
    if (v.maxLength != null) prop.maxLength = v.maxLength;
    if (v.pattern) prop.pattern = v.pattern;
  }
  if (v.varType === 'number') {
    if (v.minimum != null) prop.minimum = v.minimum;
    if (v.maximum != null) prop.maximum = v.maximum;
  }
  if (v.varType === 'array' && v.itemsType) {
    const items: Record<string, any> = { type: v.itemsType };
    if (v.itemsType === 'object' && v.itemProperties && v.itemProperties.length > 0) {
      items.properties = Object.fromEntries(
        v.itemProperties.map(p => [p.name, buildSubPropertySchema(p)])
      );
    }
    prop.items = items;
  }
  if (v.varType === 'object' && v.properties && v.properties.length > 0) {
    prop.properties = Object.fromEntries(
      v.properties.map(p => [p.name, buildSubPropertySchema(p)])
    );
  }
  if (v.xUi && Object.values(v.xUi).some(val => val !== undefined)) prop['x-ui'] = v.xUi;
  return prop;
}

// ── YAML generation ──

function generateSkeletonYaml(
  name: string,
  version: string,
  mode: string,
  trigger: TriggerConfig | null,
): string {
  const triggerStep: Record<string, any> = {
    id: 'trigger_1',
    type: 'trigger',
    trigger: {} as Record<string, any>,
    next: [],
  };

  if (trigger) {
    const t: Record<string, any> = { type: trigger.type };

    if (trigger.type === 'manual') {
      const vars = trigger.input_schema ?? [];
      if (vars.length > 0) {
        const schema: Record<string, any> = { type: 'object' };
        const requiredInputs = vars.filter(v => v.required).map(v => v.name);
        if (requiredInputs.length > 0) schema.required = requiredInputs;
        schema.properties = Object.fromEntries(
          vars.map(v => [v.name, buildVariableSchema(v)])
        );
        t.input_schema = schema;
      }
      t.inputs = [];
    } else if (trigger.type === 'schedule' && trigger.cron) {
      t.cron = trigger.cron;
    } else if (trigger.type === 'event_pattern') {
      if (trigger.topic) t.topic = trigger.topic;
      if (trigger.filter) t.filter = trigger.filter;
    } else if (trigger.type === 'incoming_message') {
      if (trigger.connector_id) t.channel_id = trigger.connector_id;
      if (trigger.listen_channel_id) t.listen_channel_id = trigger.listen_channel_id;
      if (trigger.from_filter) t.from_filter = trigger.from_filter;
      if (trigger.subject_filter) t.subject_filter = trigger.subject_filter;
      if (trigger.body_filter) t.body_filter = trigger.body_filter;
      if (trigger.mark_as_read) t.mark_as_read = true;
      if (trigger.ignore_replies) t.ignore_replies = true;
    }
    triggerStep.trigger = t;
  } else {
    triggerStep.trigger = { type: 'manual' };
  }

  const doc: Record<string, any> = {
    name,
    version,
    mode,
    steps: [triggerStep],
  };

  return yaml.dump(doc, { lineWidth: -1, noRefs: true, quotingType: '"', forceQuotes: false });
}

// ── Name validation ──

function isValidWorkflowName(name: string): boolean {
  const parts = name.split('/');
  if (parts.length < 2) return false;
  const segmentPattern = /^[a-zA-Z0-9][a-zA-Z0-9\-_]*$/;
  return parts.every((p) => p.length > 0 && segmentPattern.test(p));
}

// ── Shared styles ──

const cardStyle = (selected: boolean): string =>
  `border: 1px solid ${selected ? 'var(--accent, #3b82f6)' : 'var(--border, #333)'}; ` +
  `border-radius: 8px; padding: 20px; cursor: pointer; flex: 1; ` +
  `background: ${selected ? 'var(--accent-bg, rgba(59,130,246,0.08))' : 'var(--card, transparent)'}; ` +
  `transition: border-color 0.15s, background 0.15s;`;

const labelStyle = 'display: block; font-size: 0.82em; color: var(--muted-foreground, #888); margin-bottom: 6px;';
const inputStyle =
  'width: 100%; padding: 8px 10px; border-radius: 6px; border: 1px solid var(--border, #333); ' +
  'background: var(--input, transparent); color: var(--foreground, #fff); font-size: 0.88em; outline: none;';

// ── Component ──

export default function WorkflowCreationWizard(props: WorkflowCreationWizardProps) {
  const [step, setStep] = createSignal(1);
  const [mode, setMode] = createSignal<'background' | 'chat'>('background');
  const [workflowName, setWorkflowName] = createSignal('user/');
  const [triggerType, setTriggerType] = createSignal<string | null>(null);

  // Trigger config — manual
  const [input_schema, setInputSchema] = createSignal<WorkflowVariable[]>([]);
  // Trigger config — schedule
  const [cronExpr, setCronExpr] = createSignal('');
  // Trigger config — event_pattern
  const [eventTopic, setEventTopic] = createSignal('');
  const [eventFilter, setEventFilter] = createSignal('');
  // Trigger config — incoming_message
  const [connectorId, setConnectorId] = createSignal('');
  const [listenChannelId, setListenChannelId] = createSignal('');
  const [fromFilter, setFromFilter] = createSignal('');
  const [subjectFilter, setSubjectFilter] = createSignal('');
  const [bodyFilter, setBodyFilter] = createSignal('');
  const [markAsRead, setMarkAsRead] = createSignal(false);
  const [ignoreReplies, setIgnoreReplies] = createSignal(false);
  // Incoming message — dynamic channel list for chat connectors
  const [connectorChannels, setConnectorChannels] = createSignal<{ id: string; name: string; channel_type?: string; group_name?: string }[]>([]);
  const [channelsLoading, setChannelsLoading] = createSignal(false);
  const [channelsError, setChannelsError] = createSignal<string | null>(null);
   // Input schema dialog
  const [showInputSchemaDialog, setShowInputSchemaDialog] = createSignal(false);
  const [localInputVars, setLocalInputVars] = createSignal<WorkflowVariable[]>([]);

  // AI prompt — structured fields
  const [aiPrompt, setAiPrompt] = createSignal('');
  const [aiIncludeApproval, setAiIncludeApproval] = createSignal<string>('not-sure');
  const [aiErrorHandling, setAiErrorHandling] = createSignal<string>('retry');

  /**
   * Build a structured prompt from the wizard's AI fields and trigger context.
   */
  const buildAiPrompt = (): string => {
    const parts: string[] = [];
    const desc = aiPrompt().trim();
    if (desc) parts.push(`Goal: ${desc}`);

    // Include trigger context so the AI doesn't re-ask
    const tt = triggerType();
    if (tt) {
      parts.push(`Trigger type: ${tt}`);
      if (tt === 'schedule' && cronExpr()) parts.push(`Cron: ${cronExpr()}`);
      if (tt === 'event_pattern' && eventTopic()) parts.push(`Event topic: ${eventTopic()}`);
      if (tt === 'incoming_message' && connectorId()) parts.push(`Connector: ${connectorId()}`);
    }

    parts.push(`Mode: ${mode()}`);

    const approval = aiIncludeApproval();
    if (approval === 'yes') parts.push('Include human approval / feedback gates for important actions.');
    else if (approval === 'no') parts.push('Fully automated, no human approval steps needed.');

    const errh = aiErrorHandling();
    if (errh === 'retry') parts.push('Error handling: retry external calls, skip non-critical failures.');
    else if (errh === 'stop') parts.push('Error handling: stop the workflow on any error.');

    return parts.join('\n');
  };

  // Copy sub-flow
  const [showCopy, setShowCopy] = createSignal(false);
  const [copySource, setCopySource] = createSignal<string>('');
  const [copyNewName, setCopyNewName] = createSignal('user/');
  const [copyError, setCopyError] = createSignal('');
  const [copying, setCopying] = createSignal(false);

  // Template sub-flow
  const [showTemplate, setShowTemplate] = createSignal(false);
  const [selectedTemplate, setSelectedTemplate] = createSignal<string>('');
  const [templateNewName, setTemplateNewName] = createSignal('user/');

  const templateOptions = [
    { id: 'system/email-triage', label: 'Email Triage', desc: 'Classify, draft response, human review, send' },
    { id: 'system/scheduled-report', label: 'Scheduled Report', desc: 'Fetch data on schedule, AI analysis, deliver' },
    { id: 'system/approval-workflow', label: 'Approval Workflow', desc: 'Submit request, AI analysis, human approval' },
    { id: 'system/data-sync-pipeline', label: 'Data Sync Pipeline', desc: 'Fetch, transform, push with retry' },
    { id: 'system/event-monitor', label: 'Event Monitor', desc: 'Watch events, AI analysis, notify on action needed' },
  ];

  const handleTemplateSelect = async () => {
    const tmpl = selectedTemplate();
    if (!tmpl || !isValidWorkflowName(templateNewName())) return;
    setCopying(true);
    setCopyError('');
    try {
      const ok = await props.onCopy(tmpl, '1.0', templateNewName());
      if (ok) props.onClose();
      else setCopyError('Failed to create from template. The template may not be available yet — try restarting the application.');
    } catch {
      setCopyError('Failed to create from template. The template may not be available yet — try restarting the application.');
    } finally {
      setCopying(false);
    }
  };

  // Reset state when dialog opens/closes
  createEffect(() => {
    if (props.open) {
      setStep(1);
      setMode('background');
      setWorkflowName('user/');
      setTriggerType(null);
      setInputSchema([]);
      setCronExpr('');
      setEventTopic('');
      setEventFilter('');
      setConnectorId('');
      setListenChannelId('');
      setFromFilter('');
      setSubjectFilter('');
      setBodyFilter('');
      setMarkAsRead(false);
      setIgnoreReplies(false);
      setConnectorChannels([]);
      setChannelsLoading(false);
      setChannelsError(null);
      setShowInputSchemaDialog(false);
      setLocalInputVars([]);
      setAiPrompt('');
      setAiIncludeApproval('not-sure');
      setAiErrorHandling('retry');
      setShowCopy(false);
      setCopySource('');
      setCopyNewName('user/');
      setCopyError('');
      setCopying(false);
      setShowTemplate(false);
      setSelectedTemplate('');
      setTemplateNewName('user/');
    }
  });

  const nameValid = () => isValidWorkflowName(workflowName());

  const buildTrigger = (): TriggerConfig | null => {
    const tt = triggerType();
    if (!tt) return null;
    if (tt === 'manual') return { type: 'manual', input_schema: input_schema() };
    if (tt === 'schedule') return { type: 'schedule', cron: cronExpr() };
    if (tt === 'event_pattern') return { type: 'event_pattern', topic: eventTopic(), filter: eventFilter() };
    if (tt === 'incoming_message') return {
      type: 'incoming_message',
      connector_id: connectorId(),
      listen_channel_id: listenChannelId(),
      from_filter: fromFilter(),
      subject_filter: subjectFilter(),
      body_filter: bodyFilter(),
      mark_as_read: markAsRead(),
      ignore_replies: ignoreReplies(),
    };
    return { type: tt };
  };

  const handleCreate = () => {
    const yaml = generateSkeletonYaml(workflowName(), '1.0', mode(), buildTrigger());
    props.onComplete(yaml, false);
  };

  const handleAiYes = () => {
    const yaml = generateSkeletonYaml(workflowName(), '1.0', mode(), buildTrigger());
    const structuredPrompt = buildAiPrompt();
    props.onComplete(yaml, true, structuredPrompt || undefined);
  };

  const handleCopy = async () => {
    const src = copySource();
    if (!src) { setCopyError('Select a source definition'); return; }
    if (!isValidWorkflowName(copyNewName())) { setCopyError('Invalid name (use namespace/name format)'); return; }
    const [srcName, srcVersion] = src.split('::');
    setCopying(true);
    setCopyError('');
    try {
      const ok = await props.onCopy(srcName, srcVersion, copyNewName());
      if (ok) props.onClose();
      else setCopyError('Copy failed');
    } catch {
      setCopyError('Copy failed');
    } finally {
      setCopying(false);
    }
  };

  const triggerOptions = () =>
    mode() === 'chat'
      ? [{ id: 'manual', label: 'Manual', icon: Hand, desc: 'Triggered manually with optional inputs' }]
      : [
          { id: 'manual', label: 'Manual', icon: Hand, desc: 'Triggered manually with optional inputs' },
          { id: 'schedule', label: 'Schedule', icon: Calendar, desc: 'Runs on a cron schedule' },
          { id: 'event_pattern', label: 'Event Pattern', icon: Radio, desc: 'Reacts to event topics' },
          { id: 'incoming_message', label: 'Incoming Message', icon: Bell, desc: 'Triggered by channel messages' },
        ];

  // ── Input schema helpers (matching WorkflowDesigner) ──

  function inputSchemaAddVar() {
    setLocalInputVars(prev => [...prev, {
      name: `input_${prev.length + 1}`,
      varType: 'string' as const,
      description: '',
      required: false,
      defaultValue: '',
      enumValues: [],
    }]);
  }

  function inputSchemaRemoveVar(idx: number) {
    setLocalInputVars(prev => prev.filter((_, i) => i !== idx));
  }

  function inputSchemaUpdateVar(idx: number, field: keyof WorkflowVariable, value: any) {
    setLocalInputVars(prev => prev.map((v, i) => i === idx ? { ...v, [field]: value } : v));
  }

  function inputSchemaAddNestedProp(varIdx: number) {
    setLocalInputVars(prev => prev.map((v, i) => {
      if (i !== varIdx) return v;
      const props = v.properties ? [...v.properties] : [];
      props.push({ name: `prop_${props.length + 1}`, varType: 'string' as const, description: '', required: false, defaultValue: '', enumValues: [] });
      return { ...v, properties: props };
    }));
  }

  function inputSchemaUpdateNestedProp(varIdx: number, propIdx: number, field: keyof WorkflowVariable, value: any) {
    setLocalInputVars(prev => prev.map((v, i) => {
      if (i !== varIdx || !v.properties) return v;
      const props = v.properties.map((p, pi) => pi === propIdx ? { ...p, [field]: value } : p);
      return { ...v, properties: props };
    }));
  }

  function inputSchemaRemoveNestedProp(varIdx: number, propIdx: number) {
    setLocalInputVars(prev => prev.map((v, i) => {
      if (i !== varIdx || !v.properties) return v;
      return { ...v, properties: v.properties.filter((_, pi) => pi !== propIdx) };
    }));
  }

  function inputSchemaAddItemProp(varIdx: number) {
    setLocalInputVars(prev => prev.map((v, i) => {
      if (i !== varIdx) return v;
      const props = v.itemProperties ? [...v.itemProperties] : [];
      props.push({ name: `prop_${props.length + 1}`, varType: 'string' as const, description: '', required: false, defaultValue: '', enumValues: [] });
      return { ...v, itemProperties: props };
    }));
  }

  function inputSchemaUpdateItemProp(varIdx: number, propIdx: number, field: keyof WorkflowVariable, value: any) {
    setLocalInputVars(prev => prev.map((v, i) => {
      if (i !== varIdx || !v.itemProperties) return v;
      const props = v.itemProperties.map((p, pi) => pi === propIdx ? { ...p, [field]: value } : p);
      return { ...v, itemProperties: props };
    }));
  }

  function inputSchemaRemoveItemProp(varIdx: number, propIdx: number) {
    setLocalInputVars(prev => prev.map((v, i) => {
      if (i !== varIdx || !v.itemProperties) return v;
      return { ...v, itemProperties: v.itemProperties.filter((_, pi) => pi !== propIdx) };
    }));
  }

  function handleInputSchemaOk() {
    setInputSchema(localInputVars());
    setShowInputSchemaDialog(false);
  }

  // ── Enum editor (matching WorkflowDesigner) ──

  function renderEnumEditor(values: string[], onUpdate: (vals: string[]) => void) {
    let inputRef: HTMLInputElement | undefined;
    function addValue() {
      const val = inputRef?.value?.trim();
      if (val && !values.includes(val)) {
        onUpdate([...values, val]);
        if (inputRef) inputRef.value = '';
      }
    }
    return (
      <div style={{ display: 'flex', 'flex-direction': 'column', gap: '4px' }}>
        <div style={{ display: 'flex', 'flex-wrap': 'wrap', gap: '4px' }}>
          <For each={values}>
            {(val, i) => (
              <span style={{ display: 'inline-flex', 'align-items': 'center', gap: '2px', padding: '2px 8px', background: 'var(--accent-bg, rgba(59,130,246,0.1))', 'border-radius': '4px', 'font-size': '0.8em' }}>
                {val}
                <button
                  style={{ background: 'none', border: 'none', cursor: 'pointer', color: 'var(--muted-foreground, #888)', 'font-size': '0.9em', padding: '0 2px' }}
                  onClick={() => onUpdate(values.filter((_, idx) => idx !== i()))}
                >✕</button>
              </span>
            )}
          </For>
        </div>
        <div style={{ display: 'flex', gap: '4px' }}>
          <input
            ref={inputRef}
            style={`${inputStyle} flex: 1;`}
            placeholder="Add value…"
            onKeyDown={(e) => { if (e.key === 'Enter') { e.preventDefault(); addValue(); } }}
          />
          <Button variant="outline" style="padding:4px 10px;font-size:0.8em;" onClick={addValue}>Add</Button>
        </div>
      </div>
    );
  }

  // ── Connector channel fetch (for incoming_message) ──

  const commsChannels = () => (props.channels ?? []).filter(c => c.hasComms !== false);
  const selectedConnector = () => {
    const cid = connectorId();
    return cid ? commsChannels().find(c => c.id === cid) : undefined;
  };
  const providerType = () => selectedConnector()?.provider ?? '';
  const isChat = () => ['discord', 'slack'].includes(providerType());
  const isEmail = () => ['microsoft', 'gmail', 'imap'].includes(providerType());

  const fetchChannels = async (cid: string) => {
    if (!cid) { setConnectorChannels([]); setChannelsError(null); return; }
    setChannelsLoading(true);
    setChannelsError(null);
    try {
      const chs = await invoke<any[]>('list_connector_channels', { connector_id: cid });
      setConnectorChannels(chs ?? []);
    } catch (e: any) {
      console.error('Failed to load connector channels:', e);
      setConnectorChannels([]);
      setChannelsError(typeof e === 'string' ? e : e?.message ?? 'Failed to load channels');
    } finally {
      setChannelsLoading(false);
    }
  };

  createEffect(() => {
    const conn = selectedConnector();
    if (conn && ['discord', 'slack'].includes(conn.provider ?? '')) {
      fetchChannels(conn.id);
    } else {
      setConnectorChannels([]);
    }
  });

  // Step indicator
  const stepLabels = () => {
    const base = ['Start', 'Type', 'Name', 'Attachments', 'AI'];
    if (step() === 6) return [...base, 'Trigger'];
    return base;
  };

  return (
    <Dialog open={props.open} onOpenChange={(open) => { if (!open) props.onClose(); }}>
      <DialogContent class="w-[600px] max-w-[95vw]">
        {/* Step indicator */}
        <Show when={step() > 1 && !showCopy()}>
          <div style="display: flex; align-items: center; gap: 6px; margin-bottom: 4px;">
            <button
              style="background: none; border: none; cursor: pointer; color: var(--muted-foreground, #888); padding: 2px;"
              onClick={() => setStep((s) => Math.max(1, s - 1))}
              aria-label="Back"
            >
              <ArrowLeft size={14} />
            </button>
            <div style="display: flex; gap: 4px; align-items: center;">
              <For each={stepLabels()}>
                {(label, idx) => (
                  <div style="display: flex; align-items: center; gap: 4px;">
                    <Show when={idx() > 0}>
                      <div style="width: 16px; height: 1px; background: var(--border, #333);" />
                    </Show>
                    <span
                      style={`font-size: 0.72em; padding: 2px 8px; border-radius: 10px; ${
                        idx() + 1 === step()
                          ? 'background: var(--accent, #3b82f6); color: white;'
                          : idx() + 1 < step()
                            ? 'color: var(--accent, #3b82f6);'
                            : 'color: var(--muted-foreground, #555);'
                      }`}
                    >
                      {label}
                    </span>
                  </div>
                )}
              </For>
            </div>
          </div>
        </Show>

        {/* Step 1: Start */}
        <Show when={step() === 1 && !showCopy() && !showTemplate()}>
          <DialogHeader>
            <DialogTitle>Create New Workflow</DialogTitle>
          </DialogHeader>
          <div style="display: flex; flex-direction: column; gap: 12px; margin-top: 8px;">
            <button
              style={`${cardStyle(false)} display: flex; align-items: center; gap: 12px; text-align: left;`}
              onClick={() => setStep(2)}
            >
              <Sparkles size={22} style="color: var(--accent, #3b82f6); flex-shrink: 0;" />
              <div>
                <div style="font-weight: 600; font-size: 0.95em;">Start from scratch</div>
                <div style="font-size: 0.78em; color: var(--muted-foreground, #888); margin-top: 2px;">
                  Choose type, name, and initial trigger step by step
                </div>
              </div>
            </button>
            <button
              style={`${cardStyle(false)} display: flex; align-items: center; gap: 12px; text-align: left;`}
              onClick={() => setShowCopy(true)}
            >
              <Copy size={22} style="color: var(--accent, #3b82f6); flex-shrink: 0;" />
              <div>
                <div style="font-weight: 600; font-size: 0.95em;">Copy from existing</div>
                <div style="font-size: 0.78em; color: var(--muted-foreground, #888); margin-top: 2px;">
                  Duplicate an existing workflow as a starting point
                </div>
              </div>
            </button>
            <button
              style={`${cardStyle(false)} display: flex; align-items: center; gap: 12px; text-align: left;`}
              onClick={() => setShowTemplate(true)}
            >
              <Zap size={22} style="color: var(--accent, #3b82f6); flex-shrink: 0;" />
              <div>
                <div style="font-weight: 600; font-size: 0.95em;">Start from a template</div>
                <div style="font-size: 0.78em; color: var(--muted-foreground, #888); margin-top: 2px;">
                  Use a pre-built workflow pattern as your foundation
                </div>
              </div>
            </button>
          </div>
        </Show>

        {/* Copy sub-flow */}
        <Show when={showCopy()}>
          <DialogHeader>
            <DialogTitle>Copy Existing Workflow</DialogTitle>
          </DialogHeader>
          <div style="display: flex; flex-direction: column; gap: 14px; margin-top: 8px;">
            <div>
              <label style={labelStyle}>Source definition</label>
              <select
                style={inputStyle}
                value={copySource()}
                onChange={(e) => setCopySource(e.currentTarget.value)}
              >
                <option value="">— Select —</option>
                <For each={flattenNamespaceTree(buildNamespaceTree(props.definitions))}>
                  {([ns, defs]) => (
                    <optgroup label={ns}>
                      <For each={defs}>
                        {(def) => (
                          <option value={`${def.name}::${def.version}`}>
                            {def.name} v{def.version}
                          </option>
                        )}
                      </For>
                    </optgroup>
                  )}
                </For>
              </select>
            </div>
            <div>
              <label style={labelStyle}>New workflow name</label>
              <div style="display: flex; align-items: stretch;">
                <span style="display: inline-flex; align-items: center; padding: 0 10px; background: var(--muted, rgba(255,255,255,0.06)); border: 1px solid var(--border, #333); border-right: none; border-radius: 4px 0 0 4px; font-size: 0.88em; color: var(--muted-foreground, #888); white-space: nowrap; user-select: none;">user/</span>
                <input
                  type="text"
                  style={inputStyle + ' border-radius: 0 4px 4px 0; flex: 1;'}
                  placeholder="my-workflow-copy"
                  value={copyNewName().startsWith('user/') ? copyNewName().slice(5) : copyNewName()}
                  onInput={(e) => setCopyNewName('user/' + e.currentTarget.value)}
                  autocomplete="off"
                  autocorrect="off"
                  spellcheck={false}
                />
              </div>
            </div>
            <Show when={copyError()}>
              <div style="font-size: 0.82em; color: var(--destructive, #f87171);">{copyError()}</div>
            </Show>
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setShowCopy(false)}>Back</Button>
            <Button disabled={copying()} onClick={() => void handleCopy()}>
              {copying() ? 'Copying…' : 'Copy'}
            </Button>
          </DialogFooter>
        </Show>

        {/* Template sub-flow */}
        <Show when={showTemplate()}>
          <DialogHeader>
            <DialogTitle>Start from Template</DialogTitle>
          </DialogHeader>
          <div style="display: flex; flex-direction: column; gap: 10px; margin-top: 8px;">
            <div style="font-size: 0.85em; color: var(--muted-foreground, #888); line-height: 1.5;">
              Choose a pre-built workflow pattern. You can customize it in the visual designer afterward.
            </div>
            <div style="display: flex; flex-direction: column; gap: 8px;">
              <For each={templateOptions}>
                {(tmpl) => (
                  <button
                    style={cardStyle(selectedTemplate() === tmpl.id) + ' text-align: left; padding: 12px 16px;'}
                    onClick={() => setSelectedTemplate(tmpl.id)}
                  >
                    <div style="font-weight: 600; font-size: 0.88em;">{tmpl.label}</div>
                    <div style="font-size: 0.74em; color: var(--muted-foreground, #888); margin-top: 2px;">{tmpl.desc}</div>
                  </button>
                )}
              </For>
            </div>
            <Show when={selectedTemplate()}>
              <div>
                <label style={labelStyle}>New workflow name</label>
                <div style="display: flex; align-items: stretch;">
                  <span style="display: inline-flex; align-items: center; padding: 0 10px; background: var(--muted, rgba(255,255,255,0.06)); border: 1px solid var(--border, #333); border-right: none; border-radius: 4px 0 0 4px; font-size: 0.88em; color: var(--muted-foreground, #888); white-space: nowrap; user-select: none;">user/</span>
                  <input
                    type="text"
                    style={inputStyle + ' border-radius: 0 4px 4px 0; flex: 1;'}
                    placeholder="my-workflow"
                    value={templateNewName().startsWith('user/') ? templateNewName().slice(5) : templateNewName()}
                    onInput={(e) => setTemplateNewName('user/' + e.currentTarget.value)}
                    autocomplete="off"
                    autocorrect="off"
                    spellcheck={false}
                  />
                </div>
              </div>
            </Show>
          </div>
          <Show when={copyError()}>
            <div style="font-size: 0.82em; color: var(--destructive, #f87171); padding: 0 1.5rem;">{copyError()}</div>
          </Show>
          <DialogFooter>
            <Button variant="outline" onClick={() => setShowTemplate(false)}>Back</Button>
            <Button
              disabled={!selectedTemplate() || !isValidWorkflowName(templateNewName()) || copying()}
              onClick={() => void handleTemplateSelect()}
            >
              {copying() ? 'Creating…' : 'Create from Template'}
            </Button>
          </DialogFooter>
        </Show>

        {/* Step 2: Choose Type */}
        <Show when={step() === 2}>
          <DialogHeader>
            <DialogTitle>Choose Workflow Type</DialogTitle>
          </DialogHeader>
          <div style="display: flex; gap: 14px; margin-top: 8px;">
            <button style={cardStyle(mode() === 'background')} onClick={() => { setMode('background'); setStep(3); }}>
              <Zap size={28} style="color: var(--accent, #3b82f6); margin-bottom: 8px;" />
              <div style="font-weight: 600; margin-bottom: 4px;">Background</div>
              <div style="font-size: 0.78em; color: var(--muted-foreground, #888); line-height: 1.4;">
                Runs independently in the background. Managed from the Workflows page. Supports all trigger types including scheduled, event-driven, and message-driven.
              </div>
            </button>
            <button style={cardStyle(mode() === 'chat')} onClick={() => { setMode('chat'); setStep(3); }}>
              <Bot size={28} style="color: var(--accent, #3b82f6); margin-bottom: 8px;" />
              <div style="font-weight: 600; margin-bottom: 4px;">Chat</div>
              <div style="font-size: 0.78em; color: var(--muted-foreground, #888); line-height: 1.4;">
                Attached to a chat session. Shares the session workspace, surfaces interactions in the chat thread, and shows a result widget. Only manual triggers are supported.
              </div>
            </button>
          </div>
        </Show>

        {/* Step 3: Name */}
        <Show when={step() === 3}>
          <DialogHeader>
            <DialogTitle>Name Your Workflow</DialogTitle>
          </DialogHeader>
          <div style="margin-top: 8px;">
            <label style={labelStyle}>Workflow name</label>
            <div style="display: flex; align-items: stretch;">
              <span style="display: inline-flex; align-items: center; padding: 0 10px; background: var(--muted, rgba(255,255,255,0.06)); border: 1px solid var(--border, #333); border-right: none; border-radius: 4px 0 0 4px; font-size: 0.88em; color: var(--muted-foreground, #888); white-space: nowrap; user-select: none;">user/</span>
              <input
                type="text"
                style={inputStyle + ' border-radius: 0 4px 4px 0; flex: 1;'}
                placeholder="my-workflow"
                value={workflowName().startsWith('user/') ? workflowName().slice(5) : workflowName()}
                onInput={(e) => setWorkflowName('user/' + e.currentTarget.value)}
                autocomplete="off"
                autocorrect="off"
                spellcheck={false}
              />
            </div>
            <Show when={workflowName().length > 5 && !nameValid()}>
              <div style="font-size: 0.78em; color: var(--destructive, #f87171); margin-top: 6px;">
                Name can contain letters, numbers, hyphens, and underscores.
              </div>
            </Show>
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setStep(2)}>Back</Button>
            <Button disabled={!nameValid()} onClick={() => setStep(4)}>Next</Button>
          </DialogFooter>
        </Show>

        {/* Step 4: Attachments */}
        <Show when={step() === 4}>
          <DialogHeader>
            <DialogTitle>Attachments (Optional)</DialogTitle>
          </DialogHeader>
          <div style="font-size: 0.85em; color: var(--muted-foreground, #888); line-height: 1.5; margin-top: 8px;">
            Attach reference files that AI agents can read during workflow execution — for example, templates, schemas, or style guides. Each attachment gets a description so agents know when to use it.
          </div>
          <div style="font-size: 0.78em; color: var(--muted-foreground, #666); margin-top: 4px;">
            You can add attachments later in the visual designer.
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setStep(3)}>Back</Button>
            <Button variant="outline" onClick={() => setStep(5)}>Skip</Button>
            <Button onClick={() => setStep(5)}>Next</Button>
          </DialogFooter>
        </Show>

        {/* Step 5: AI Generation */}
        <Show when={step() === 5}>
          <DialogHeader>
            <DialogTitle>Generate with AI?</DialogTitle>
          </DialogHeader>
          <div style="font-size: 0.85em; color: var(--muted-foreground, #888); line-height: 1.5; margin-top: 8px;">
            AI can generate a complete workflow based on your description. You can always refine it in the visual designer afterward.
          </div>
          <div style="margin-top: 12px; display: flex; flex-direction: column; gap: 12px;">
            <div>
              <label style={labelStyle}>What should this workflow do? *</label>
              <textarea
                style={inputStyle + ' resize: vertical; min-height: 80px;'}
                placeholder="e.g. When I receive an email from a customer, use AI to draft a helpful reply, let me review it, then send it"
                value={aiPrompt()}
                onInput={(e) => setAiPrompt(e.currentTarget.value)}
                rows={4}
                autocomplete="off"
                autocorrect="off"
                spellcheck={false}
              />
            </div>
            <div>
              <label style={labelStyle}>Should it include human approval steps?</label>
              <div style="display: flex; gap: 8px;">
                <button
                  style={`padding: 6px 14px; border-radius: 6px; font-size: 0.82em; cursor: pointer; border: 1px solid var(--border, #333); ${aiIncludeApproval() === 'yes' ? 'background: var(--accent-bg, rgba(59,130,246,0.15)); border-color: var(--accent, #3b82f6); color: var(--accent, #3b82f6);' : 'background: transparent; color: var(--foreground, #fff);'}`}
                  onClick={() => setAiIncludeApproval('yes')}
                >Yes</button>
                <button
                  style={`padding: 6px 14px; border-radius: 6px; font-size: 0.82em; cursor: pointer; border: 1px solid var(--border, #333); ${aiIncludeApproval() === 'no' ? 'background: var(--accent-bg, rgba(59,130,246,0.15)); border-color: var(--accent, #3b82f6); color: var(--accent, #3b82f6);' : 'background: transparent; color: var(--foreground, #fff);'}`}
                  onClick={() => setAiIncludeApproval('no')}
                >No</button>
                <button
                  style={`padding: 6px 14px; border-radius: 6px; font-size: 0.82em; cursor: pointer; border: 1px solid var(--border, #333); ${aiIncludeApproval() === 'not-sure' ? 'background: var(--accent-bg, rgba(59,130,246,0.15)); border-color: var(--accent, #3b82f6); color: var(--accent, #3b82f6);' : 'background: transparent; color: var(--foreground, #fff);'}`}
                  onClick={() => setAiIncludeApproval('not-sure')}
                >Let AI decide</button>
              </div>
            </div>
            <div>
              <label style={labelStyle}>How should errors be handled?</label>
              <div style="display: flex; gap: 8px;">
                <button
                  style={`padding: 6px 14px; border-radius: 6px; font-size: 0.82em; cursor: pointer; border: 1px solid var(--border, #333); ${aiErrorHandling() === 'retry' ? 'background: var(--accent-bg, rgba(59,130,246,0.15)); border-color: var(--accent, #3b82f6); color: var(--accent, #3b82f6);' : 'background: transparent; color: var(--foreground, #fff);'}`}
                  onClick={() => setAiErrorHandling('retry')}
                >Retry &amp; continue</button>
                <button
                  style={`padding: 6px 14px; border-radius: 6px; font-size: 0.82em; cursor: pointer; border: 1px solid var(--border, #333); ${aiErrorHandling() === 'stop' ? 'background: var(--accent-bg, rgba(59,130,246,0.15)); border-color: var(--accent, #3b82f6); color: var(--accent, #3b82f6);' : 'background: transparent; color: var(--foreground, #fff);'}`}
                  onClick={() => setAiErrorHandling('stop')}
                >Stop on failure</button>
              </div>
            </div>
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setStep(4)}>Back</Button>
            <Button variant="outline" onClick={() => setStep(6)}>No, I'll build it manually</Button>
            <Button disabled={!aiPrompt().trim()} onClick={handleAiYes}>
              <Sparkles size={14} style="margin-right: 6px;" /> Generate with AI
            </Button>
          </DialogFooter>
        </Show>

        {/* Step 6: Initial Trigger */}
        <Show when={step() === 6}>
          <DialogHeader>
            <DialogTitle>Choose Initial Trigger</DialogTitle>
          </DialogHeader>
          <div style="display: flex; flex-direction: column; gap: 10px; margin-top: 8px;">
            {/* Trigger type cards */}
            <div style="display: grid; grid-template-columns: repeat(2, 1fr); gap: 10px;">
              <For each={triggerOptions()}>
                {(opt) => (
                  <button
                    style={cardStyle(triggerType() === opt.id)}
                    onClick={() => setTriggerType(opt.id)}
                  >
                    <opt.icon size={20} style="color: var(--accent, #3b82f6); margin-bottom: 6px;" />
                    <div style="font-weight: 600; font-size: 0.88em;">{opt.label}</div>
                    <div style="font-size: 0.74em; color: var(--muted-foreground, #888); margin-top: 2px;">{opt.desc}</div>
                  </button>
                )}
              </For>
            </div>

            {/* ── Manual Trigger ── */}
            <Show when={triggerType() === 'manual'}>
              <div style="border: 1px solid var(--border, #333); border-radius: 8px; padding: 14px;">
                <div style="font-size: 0.8em; color: var(--muted-foreground, #888); margin-bottom: 8px;">
                  <Play size={14} style="display: inline; vertical-align: middle;" /> Manual trigger — workflow starts when launched by a user or API call.
                </div>
                <div>
                  <span style="font-size: 0.85em; font-weight: 600;">Input Schema ({input_schema().length} field{input_schema().length !== 1 ? 's' : ''})</span>
                  <Show when={input_schema().length > 0}>
                    <div style="font-size: 0.8em; color: var(--muted-foreground, #888); margin-top: 4px; margin-bottom: 4px;">
                      <For each={input_schema()}>
                        {(v) => <div>• {v.name}: {TYPE_LABELS[v.varType] ?? v.varType}{v.required ? ' (required)' : ''}</div>}
                      </For>
                    </div>
                  </Show>
                  <button
                    style="background: none; border: 1px solid var(--border, #333); border-radius: 6px; cursor: pointer; color: var(--accent, #3b82f6); font-size: 0.82em; padding: 4px 12px; margin-top: 4px;"
                    onClick={() => {
                      setLocalInputVars(JSON.parse(JSON.stringify(input_schema())));
                      setShowInputSchemaDialog(true);
                    }}
                  >Edit Input Schema</button>
                </div>
              </div>
            </Show>

            {/* ── Schedule Trigger ── */}
            <Show when={triggerType() === 'schedule'}>
              <div style="border: 1px solid var(--border, #333); border-radius: 8px; padding: 14px;">
                <label style={labelStyle}>Cron expression</label>
                <CronBuilder value={cronExpr()} onChange={(v) => setCronExpr(v)} />
              </div>
            </Show>

            {/* ── Event Pattern Trigger ── */}
            <Show when={triggerType() === 'event_pattern'}>
              <div style="border: 1px solid var(--border, #333); border-radius: 8px; padding: 14px;">
                <label style={labelStyle}>Event topic</label>
                <TopicSelector
                  value={eventTopic()}
                  onChange={(v) => setEventTopic(v)}
                  topics={props.eventTopics ?? []}
                  placeholder="e.g. builds.completed"
                />
                <div style="margin-top: 8px;">
                  <label style={labelStyle}>Filter expression <span style="font-size: 0.85em; color: var(--muted-foreground, #666);">(optional)</span></label>
                  <input
                    type="text"
                    style={inputStyle}
                    placeholder='e.g. event.status == "failed"'
                    value={eventFilter()}
                    onInput={(e) => setEventFilter(e.currentTarget.value)}
                  />
                </div>
              </div>
            </Show>

            {/* ── Incoming Message Trigger ── */}
            <Show when={triggerType() === 'incoming_message'}>
              <div style="border: 1px solid var(--border, #333); border-radius: 8px; padding: 14px;">
                <div style="margin-bottom: 8px;">
                  <label style={labelStyle}>Connector</label>
                  <Show when={commsChannels().length > 0} fallback={
                    <div style="font-size: 0.8em; color: hsl(40, 90%, 84%); padding: 8px; background: hsla(40, 90%, 84%, 0.1); border-radius: 4px;">
                      No connectors with communication enabled. Add or enable one in Settings → Connectors.
                    </div>
                  }>
                    <select
                      style={inputStyle}
                      value={connectorId()}
                      onChange={(e) => {
                        setConnectorId(e.currentTarget.value);
                        setListenChannelId('');
                      }}
                    >
                      <option value="">— Select connector —</option>
                      <For each={commsChannels()}>
                        {(ch) => <option value={ch.id}>{ch.name}{ch.provider ? ` (${ch.provider})` : ''}</option>}
                      </For>
                    </select>
                  </Show>
                </div>
                <Show when={selectedConnector()}>
                  {/* Channel dropdown for chat connectors */}
                  <Show when={isChat()}>
                    <div style="margin-bottom: 8px;">
                      <label style={labelStyle}>Channel <span style="font-size: 0.85em; color: var(--muted-foreground, #666);">(optional — blank listens to all)</span></label>
                      <Show when={!channelsLoading()} fallback={
                        <div style="font-size: 0.8em; color: var(--muted-foreground, #888); padding: 6px;">Loading channels…</div>
                      }>
                        <Show when={channelsError()} fallback={
                          <select style={inputStyle} value={listenChannelId()} onChange={(e) => setListenChannelId(e.currentTarget.value)}>
                            <option value="">— All channels —</option>
                            <For each={connectorChannels()}>
                              {(ch) => <option value={ch.id}>{ch.group_name ? `${ch.group_name} / ` : ''}{ch.name}{ch.channel_type ? ` (${ch.channel_type})` : ''}</option>}
                            </For>
                          </select>
                        }>
                          <div style="font-size: 0.8em; color: hsl(0, 70%, 70%); padding: 8px; background: hsla(0, 70%, 70%, 0.1); border-radius: 4px;">
                            Failed to load channels: {channelsError()}
                            <button style="margin-left: 8px; font-size: 0.9em; text-decoration: underline; cursor: pointer; background: none; border: none; color: inherit;"
                              onClick={() => { const conn = selectedConnector(); if (conn) fetchChannels(conn.id); }}>
                              Retry
                            </button>
                          </div>
                        </Show>
                      </Show>
                    </div>
                  </Show>
                  <div style="font-size: 0.8em; color: var(--muted-foreground, #888); padding: 6px 8px; background: var(--card, rgba(0,0,0,0.2)); border-radius: 4px; margin-bottom: 8px;">
                    {isChat()
                      ? `Messages from ${providerType()} will trigger this workflow. Use the filters below to narrow which messages trigger it.`
                      : `Incoming email on this connector will trigger this workflow. Use the filters below to narrow which messages trigger it.`}
                  </div>
                  <div style="font-size: 0.85em; font-weight: bold; margin-bottom: 6px;">
                    Filters <span style="font-weight: normal; color: var(--muted-foreground, #888); font-size: 0.9em;">(optional — leave blank to match all)</span>
                  </div>
                  <div style="margin-bottom: 6px;">
                    <label style={labelStyle}>From <span style="font-size: 0.85em; color: var(--muted-foreground, #666);">(contains)</span></label>
                    <input style={inputStyle} type="text" value={fromFilter()} onInput={(e) => setFromFilter(e.currentTarget.value)}
                      placeholder={isEmail() ? 'e.g. alice@example.com' : 'e.g. username'} />
                  </div>
                  <Show when={isEmail()}>
                    <div style="margin-bottom: 6px;">
                      <label style={labelStyle}>Subject <span style="font-size: 0.85em; color: var(--muted-foreground, #666);">(contains)</span></label>
                      <input style={inputStyle} type="text" value={subjectFilter()} onInput={(e) => setSubjectFilter(e.currentTarget.value)}
                        placeholder="e.g. Invoice" />
                    </div>
                  </Show>
                  <div style="margin-bottom: 8px;">
                    <label style={labelStyle}>Body <span style="font-size: 0.85em; color: var(--muted-foreground, #666);">(contains)</span></label>
                    <input style={inputStyle} type="text" value={bodyFilter()} onInput={(e) => setBodyFilter(e.currentTarget.value)}
                      placeholder="e.g. keyword or phrase" />
                  </div>
                  <div style="display: flex; flex-direction: column; gap: 6px;">
                    <Switch
                      checked={markAsRead()}
                      onChange={(checked: boolean) => setMarkAsRead(checked)}
                      class="flex items-center gap-2"
                    >
                      <SwitchControl><SwitchThumb /></SwitchControl>
                      <SwitchLabel>Mark message as read after triggering</SwitchLabel>
                    </Switch>
                    <Switch
                      checked={ignoreReplies()}
                      onChange={(checked: boolean) => setIgnoreReplies(checked)}
                      class="flex items-center gap-2"
                    >
                      <SwitchControl><SwitchThumb /></SwitchControl>
                      <SwitchLabel>Ignore replies (only trigger on new messages)</SwitchLabel>
                    </Switch>
                  </div>
                </Show>
              </div>
            </Show>
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setStep(5)}>Back</Button>
            <Button disabled={!triggerType()} onClick={handleCreate}>Create</Button>
          </DialogFooter>
        </Show>

        {/* ── Input Schema Editor (inline overlay within dialog) ── */}
        <Show when={showInputSchemaDialog()}>
          <div style="position: fixed; inset: 0; z-index: 9999; display: flex; align-items: center; justify-content: center; pointer-events: auto;">
            {/* Backdrop */}
            <div
              style="position: absolute; inset: 0; background: rgba(0,0,0,0.5);"
              onClick={() => setShowInputSchemaDialog(false)}
            />
            {/* Content */}
            <div
              style="position: relative; z-index: 1; max-width: 32rem; width: calc(100% - 32px); max-height: 80vh; overflow-y: auto; border: 1px solid var(--popover-border, #333); background: var(--popover, #1e1e2e); padding: 24px; border-radius: 8px; box-shadow: 0 25px 50px -12px rgba(0,0,0,0.5);"
            >
              <DialogHeader>
                <DialogTitle>Trigger Input Schema</DialogTitle>
              </DialogHeader>

            <div style={{ display: 'flex', 'flex-direction': 'column', gap: '12px' }}>
              <Index each={localInputVars()}>
                {(v, idx) => (
                  <div style="border: 1px solid var(--border, #333); border-radius: 8px; padding: 12px;">
                    <div style="display: flex; gap: 8px; align-items: center; margin-bottom: 8px;">
                      <input
                        style={`${inputStyle} flex: 1;`}
                        value={v().name}
                        onInput={(e) => inputSchemaUpdateVar(idx, 'name', e.currentTarget.value)}
                        placeholder="Input name"
                      />
                      <button
                        onClick={() => inputSchemaRemoveVar(idx)}
                        style="background: none; border: none; color: var(--muted-foreground, #888); cursor: pointer; font-size: 1em; padding: 0 4px;"
                        title="Delete input"
                      >✕</button>
                    </div>

                    <div style="display: grid; grid-template-columns: 1fr 1fr; gap: 8px;">
                      <div>
                        <label style={labelStyle}>Type</label>
                        <select
                          style={inputStyle}
                          value={v().varType}
                          onChange={(e) => inputSchemaUpdateVar(idx, 'varType', e.currentTarget.value)}
                        >
                          <For each={TYPE_VALUES}>
                            {(t) => <option value={t}>{TYPE_LABELS[t]}</option>}
                          </For>
                        </select>
                      </div>
                      <div style="display: flex; align-items: flex-end; padding-bottom: 4px;">
                        <label style="display: flex; align-items: center; gap: 6px; cursor: pointer; font-size: 0.85em;">
                          <input
                            type="checkbox"
                            checked={v().required}
                            onChange={(e) => inputSchemaUpdateVar(idx, 'required', e.currentTarget.checked)}
                          />
                          Required
                        </label>
                      </div>

                      <div style="grid-column: 1 / -1;">
                        <label style={labelStyle}>Label</label>
                        <input
                          style={inputStyle}
                          type="text"
                          value={v().xUi?.label ?? ''}
                          onInput={(e) => {
                            const label = e.currentTarget.value || undefined;
                            inputSchemaUpdateVar(idx, 'xUi', label ? { ...v().xUi, label } : (() => { const { label: _, ...rest } = v().xUi ?? {}; return Object.keys(rest).length > 0 ? rest : undefined; })());
                          }}
                          placeholder="Human-readable label (optional)"
                        />
                      </div>

                      <div style="grid-column: 1 / -1;">
                        <label style={labelStyle}>Description</label>
                        <textarea
                          style={`${inputStyle} resize: vertical;`}
                          value={v().description}
                          onInput={(e) => inputSchemaUpdateVar(idx, 'description', e.currentTarget.value)}
                          placeholder="Input description"
                          rows={1}
                        />
                      </div>

                      <div style="grid-column: 1 / -1;">
                        <label style={labelStyle}>Default value</label>
                        {v().varType === 'boolean' ? (
                          <label style="display: flex; align-items: center; gap: 6px; cursor: pointer; font-size: 0.9em; padding: 4px 0;">
                            <input
                              type="checkbox"
                              checked={v().defaultValue === 'true'}
                              onChange={(e) => inputSchemaUpdateVar(idx, 'defaultValue', e.currentTarget.checked ? 'true' : 'false')}
                            />
                            {v().defaultValue === 'true' ? 'true' : 'false'}
                          </label>
                        ) : v().varType === 'number' ? (
                          <input
                            style={inputStyle}
                            type="number"
                            value={v().defaultValue}
                            onInput={(e) => inputSchemaUpdateVar(idx, 'defaultValue', e.currentTarget.value)}
                            placeholder="0"
                          />
                        ) : (
                          <input
                            style={inputStyle}
                            type="text"
                            value={v().defaultValue}
                            onInput={(e) => inputSchemaUpdateVar(idx, 'defaultValue', e.currentTarget.value)}
                            placeholder="default"
                          />
                        )}
                      </div>
                    </div>

                    {/* Widget override */}
                    <div style="margin-top: 8px;">
                      <label style={labelStyle}>Widget</label>
                      <select
                        style={inputStyle}
                        value={v().xUi?.widget ?? ''}
                        onChange={(e) => {
                          const w = e.currentTarget.value;
                          inputSchemaUpdateVar(idx, 'xUi', w ? { ...v().xUi, widget: w } : undefined);
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
                      <div style="margin-top: 6px;">
                        <label style={labelStyle}>Rows</label>
                        <input
                          style={inputStyle}
                          type="number"
                          value={v().xUi?.rows ?? ''}
                          min={1}
                          max={50}
                          onInput={(e) => {
                            const rows = e.currentTarget.value ? Number(e.currentTarget.value) : undefined;
                            inputSchemaUpdateVar(idx, 'xUi', { ...v().xUi, rows });
                          }}
                          placeholder="4"
                        />
                      </div>
                    </Show>

                    <Show when={v().xUi?.widget === 'slider'}>
                      <div style="margin-top: 6px;">
                        <label style={labelStyle}>Step</label>
                        <input
                          style={inputStyle}
                          type="number"
                          value={v().xUi?.step ?? ''}
                          min={0}
                          onInput={(e) => {
                            const s = e.currentTarget.value ? Number(e.currentTarget.value) : undefined;
                            inputSchemaUpdateVar(idx, 'xUi', { ...v().xUi, step: s });
                          }}
                          placeholder="1"
                        />
                      </div>
                    </Show>

                    {/* Conditional visibility */}
                    <div style="display: grid; grid-template-columns: 1fr 1fr; gap: 8px; margin-top: 6px;">
                      <div>
                        <label style={labelStyle}>Visible when</label>
                        <select
                          style={inputStyle}
                          value={v().xUi?.condition?.field ?? ''}
                          onChange={(e) => {
                            const field = e.currentTarget.value;
                            if (!field) {
                              const { condition, ...rest } = v().xUi ?? {};
                              inputSchemaUpdateVar(idx, 'xUi', Object.keys(rest).length > 0 ? rest : undefined);
                            } else {
                              inputSchemaUpdateVar(idx, 'xUi', { ...v().xUi, condition: { field, eq: true } });
                            }
                          }}
                        >
                          <option value="">(always visible)</option>
                          <For each={localInputVars().filter((_, i) => i !== idx)}>
                            {(other) => <option value={other.name}>{other.name}</option>}
                          </For>
                        </select>
                      </div>
                      <Show when={v().xUi?.condition?.field}>
                        <div>
                          <label style={labelStyle}>equals</label>
                          <input
                            style={inputStyle}
                            value={v().xUi?.condition?.eq != null ? String(v().xUi!.condition!.eq) : ''}
                            onInput={(e) => {
                              const raw = e.currentTarget.value;
                              let val: any = raw;
                              if (raw === 'true') val = true;
                              else if (raw === 'false') val = false;
                              else if (raw !== '' && !isNaN(Number(raw))) val = Number(raw);
                              inputSchemaUpdateVar(idx, 'xUi', { ...v().xUi, condition: { ...v().xUi?.condition, eq: val } });
                            }}
                            placeholder="true"
                          />
                        </div>
                      </Show>
                    </div>

                    {/* String constraints */}
                    <Show when={v().varType === 'string'}>
                      <div style="margin-top: 8px;">
                        <label style={labelStyle}>Allowed values</label>
                        {renderEnumEditor(v().enumValues, (vals) => inputSchemaUpdateVar(idx, 'enumValues', vals))}
                      </div>
                      <div style="display: grid; grid-template-columns: 1fr 1fr; gap: 8px; margin-top: 6px;">
                        <div>
                          <label style={labelStyle}>Min length</label>
                          <input style={inputStyle} type="number" value={v().minLength ?? ''} onInput={(e) => inputSchemaUpdateVar(idx, 'minLength', e.currentTarget.value ? Number(e.currentTarget.value) : undefined)} placeholder="—" />
                        </div>
                        <div>
                          <label style={labelStyle}>Max length</label>
                          <input style={inputStyle} type="number" value={v().maxLength ?? ''} onInput={(e) => inputSchemaUpdateVar(idx, 'maxLength', e.currentTarget.value ? Number(e.currentTarget.value) : undefined)} placeholder="—" />
                        </div>
                      </div>
                      <div style="margin-top: 6px;">
                        <label style={labelStyle}>Pattern (regex)</label>
                        <input style={inputStyle} type="text" value={v().pattern ?? ''} onInput={(e) => inputSchemaUpdateVar(idx, 'pattern', e.currentTarget.value || undefined)} placeholder="^[a-z]+$" />
                      </div>
                    </Show>

                    {/* Number constraints */}
                    <Show when={v().varType === 'number'}>
                      <div style="margin-top: 8px;">
                        <label style={labelStyle}>Allowed values</label>
                        {renderEnumEditor(v().enumValues, (vals) => inputSchemaUpdateVar(idx, 'enumValues', vals))}
                      </div>
                      <div style="display: grid; grid-template-columns: 1fr 1fr; gap: 8px; margin-top: 6px;">
                        <div>
                          <label style={labelStyle}>Minimum</label>
                          <input style={inputStyle} type="number" value={v().minimum ?? ''} onInput={(e) => inputSchemaUpdateVar(idx, 'minimum', e.currentTarget.value ? Number(e.currentTarget.value) : undefined)} placeholder="—" />
                        </div>
                        <div>
                          <label style={labelStyle}>Maximum</label>
                          <input style={inputStyle} type="number" value={v().maximum ?? ''} onInput={(e) => inputSchemaUpdateVar(idx, 'maximum', e.currentTarget.value ? Number(e.currentTarget.value) : undefined)} placeholder="—" />
                        </div>
                      </div>
                    </Show>

                    {/* Array items type */}
                    <Show when={v().varType === 'array'}>
                      <div style="margin-top: 8px;">
                        <label style={labelStyle}>Item type</label>
                        <select style={inputStyle} value={v().itemsType ?? 'string'} onChange={(e) => inputSchemaUpdateVar(idx, 'itemsType', e.currentTarget.value)}>
                          <For each={TYPE_VALUES.filter(t => t !== 'array')}>
                            {(t) => <option value={t}>{TYPE_LABELS[t]}</option>}
                          </For>
                        </select>
                      </div>

                      <Show when={v().itemsType === 'object'}>
                        <div style="font-size: 0.82em; font-weight: 600; margin-top: 8px; margin-bottom: 4px;">Item Properties</div>
                        <div style="display: flex; flex-direction: column; gap: 6px;">
                          <Index each={v().itemProperties ?? []}>
                            {(p, pIdx) => (
                              <div style="border: 1px solid var(--border, #333); border-radius: 6px; padding: 8px;">
                                <div style="display: flex; gap: 6px; align-items: center;">
                                  <input style={`${inputStyle} flex: 1;`} value={p().name} onInput={(e) => inputSchemaUpdateItemProp(idx, pIdx, 'name', e.currentTarget.value)} placeholder="Property name" />
                                  <select style={`${inputStyle} width: 100px; flex: none;`} value={p().varType} onChange={(e) => inputSchemaUpdateItemProp(idx, pIdx, 'varType', e.currentTarget.value)}>
                                    <option value="string">{TYPE_LABELS['string']}</option>
                                    <option value="number">{TYPE_LABELS['number']}</option>
                                    <option value="boolean">{TYPE_LABELS['boolean']}</option>
                                  </select>
                                  <button onClick={() => inputSchemaRemoveItemProp(idx, pIdx)} style="background: none; border: none; color: var(--muted-foreground, #888); cursor: pointer; font-size: 0.9em; padding: 0 4px;" title="Remove">✕</button>
                                </div>
                                <div style="margin-top: 4px;">
                                  <label style={labelStyle}>Description</label>
                                  <input style={inputStyle} value={p().description} onInput={(e) => inputSchemaUpdateItemProp(idx, pIdx, 'description', e.currentTarget.value)} placeholder="Property description" />
                                </div>
                              </div>
                            )}
                          </Index>
                          <button style="background: none; border: 1px solid var(--border, #333); border-radius: 6px; cursor: pointer; color: var(--accent, #3b82f6); font-size: 0.82em; padding: 4px 12px; align-self: flex-start;"
                            onClick={() => inputSchemaAddItemProp(idx)}>+ Add property</button>
                        </div>
                      </Show>
                    </Show>

                    {/* Object nested properties */}
                    <Show when={v().varType === 'object'}>
                      <div style="font-size: 0.82em; font-weight: 600; margin-top: 8px; margin-bottom: 4px;">Properties</div>
                      <div style="display: flex; flex-direction: column; gap: 6px;">
                        <Index each={v().properties ?? []}>
                          {(p, pIdx) => (
                            <div style="border: 1px solid var(--border, #333); border-radius: 6px; padding: 8px;">
                              <div style="display: flex; gap: 6px; align-items: center;">
                                <input style={`${inputStyle} flex: 1;`} value={p().name} onInput={(e) => inputSchemaUpdateNestedProp(idx, pIdx, 'name', e.currentTarget.value)} placeholder="Property name" />
                                <select style={`${inputStyle} width: 100px; flex: none;`} value={p().varType} onChange={(e) => inputSchemaUpdateNestedProp(idx, pIdx, 'varType', e.currentTarget.value)}>
                                  <option value="string">{TYPE_LABELS['string']}</option>
                                  <option value="number">{TYPE_LABELS['number']}</option>
                                  <option value="boolean">{TYPE_LABELS['boolean']}</option>
                                </select>
                                <button onClick={() => inputSchemaRemoveNestedProp(idx, pIdx)} style="background: none; border: none; color: var(--muted-foreground, #888); cursor: pointer; font-size: 0.9em; padding: 0 4px;" title="Remove">✕</button>
                              </div>
                              <div style="margin-top: 4px;">
                                <label style={labelStyle}>Description</label>
                                <input style={inputStyle} value={p().description} onInput={(e) => inputSchemaUpdateNestedProp(idx, pIdx, 'description', e.currentTarget.value)} placeholder="Property description" />
                              </div>
                            </div>
                          )}
                        </Index>
                        <button style="background: none; border: 1px solid var(--border, #333); border-radius: 6px; cursor: pointer; color: var(--accent, #3b82f6); font-size: 0.82em; padding: 4px 12px; align-self: flex-start;"
                          onClick={() => inputSchemaAddNestedProp(idx)}>+ Add property</button>
                      </div>
                    </Show>
                  </div>
                )}
              </Index>
            </div>

            <button
              style="background: none; border: 1px solid var(--border, #333); border-radius: 6px; cursor: pointer; color: var(--accent, #3b82f6); font-size: 0.85em; padding: 6px 14px; align-self: flex-start; margin-top: 4px;"
              onClick={inputSchemaAddVar}
            >+ Add input</button>

            <DialogFooter>
              <Button variant="outline" onClick={() => setShowInputSchemaDialog(false)}>Cancel</Button>
              <Button onClick={handleInputSchemaOk}>OK</Button>
            </DialogFooter>
            </div>
          </div>
        </Show>
      </DialogContent>
    </Dialog>
  );
}
