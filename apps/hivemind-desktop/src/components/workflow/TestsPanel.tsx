import { createSignal, createMemo, For, Index, Show } from 'solid-js';
import type { WorkflowTestCase, TestExpectations, WorkflowTestResult, WorkflowTestFailure, ExpectedToolCall } from '~/types';
import { Dialog, DialogContent, DialogHeader, DialogBody, DialogFooter, DialogTitle, Button } from '~/ui';
import TestResultDetailsDialog from './TestResultDetails';

// ── Types ──────────────────────────────────────────────────────────────

export interface TriggerInputField {
  name: string;
  type: string;
  required: boolean;
  description?: string;
  defaultValue?: string;
}

export interface WorkflowTestContext {
  stepIds: string[];
  taskStepIds: string[];
  /** Per-trigger input schema: key = trigger step ID */
  triggerSchemas: Record<string, TriggerInputField[]>;
  triggerStepIds: string[];
  /** Per-trigger subtype: key = trigger step ID, value = 'manual' | 'incoming_message' | 'event' | 'schedule' */
  triggerSubtypes: Record<string, string>;
  /** Per-task subtype: key = task step ID, value = 'invoke_agent' | 'call_tool' | 'invoke_prompt' | etc. */
  taskStepSubtypes: Record<string, string>;
  /** Available tool definitions for tool-call pickers */
  toolDefinitions: { id: string; name: string; description: string; input_schema: Record<string, unknown> }[];
}

export interface TestsPanelProps {
  testCases: WorkflowTestCase[];
  onTestCasesChange: (tests: WorkflowTestCase[]) => void;
  onRunTests: () => Promise<void>;
  onRunTest: (name: string) => Promise<void>;
  onCancelTests?: () => void;
  testResults: WorkflowTestResult[];
  running: boolean;
  /** Per-test progress from live workflow events (test_name → state). */
  testProgress?: Record<string, { state: 'running' | 'passed' | 'failed'; index: number; total: number }>;
  sectionHeaderStyle: Record<string, string>;
  readOnly?: boolean;
  context?: WorkflowTestContext;
  unsavedChanges?: boolean;
}

// ── Panel styles ───────────────────────────────────────────────────────

const panelStyle = 'width:320px;min-width:320px;max-width:320px;background:hsl(var(--card));border-left:1px solid hsl(var(--border));display:flex;flex-direction:column;overflow:hidden;';

const cardBase: Record<string, string> = {
  background: 'hsl(var(--background))',
  border: '1px solid hsl(var(--border))',
  'border-radius': '6px',
  padding: '10px',
  'margin-bottom': '6px',
  'font-size': '0.78em',
};

const btnSm: Record<string, string> = {
  padding: '3px 8px',
  'font-size': '0.75em',
  'border-radius': '4px',
  border: '1px solid hsl(var(--border))',
  background: 'hsl(var(--card))',
  color: 'hsl(var(--foreground))',
  cursor: 'pointer',
};

const btnDanger: Record<string, string> = {
  ...btnSm,
  color: 'hsl(var(--destructive))',
  'border-color': 'hsl(var(--destructive) / 0.3)',
};

const passBadge: Record<string, string> = {
  display: 'inline-block',
  padding: '1px 6px',
  'border-radius': '9999px',
  'font-size': '0.7em',
  'font-weight': '600',
  background: 'hsl(142 76% 36% / 0.15)',
  color: 'hsl(142 76% 36%)',
};

const failBadge: Record<string, string> = {
  ...passBadge,
  background: 'hsl(0 84% 60% / 0.15)',
  color: 'hsl(0 84% 60%)',
};

// Tailwind class string constants
const inputCls = 'w-full rounded-md border border-border bg-background px-3 py-1.5 text-sm text-foreground focus:outline-none focus:ring-1 focus:ring-primary';
const textareaCls = `${inputCls} font-mono resize-y`;
const labelCls = 'block text-xs font-medium text-muted-foreground mb-1';
const chipBase = 'inline-flex items-center px-2 py-0.5 text-xs font-mono rounded border cursor-pointer transition-colors';
const chipOn = 'bg-primary/15 border-primary/50 text-primary';
const chipOff = 'bg-background border-border text-foreground hover:bg-muted';

// ── Helpers ────────────────────────────────────────────────────────────

function safeJsonParse(s: string): unknown {
  try { return JSON.parse(s); } catch { return undefined; }
}

function prettyJson(v: unknown): string {
  if (v === undefined || v === null) return '';
  return JSON.stringify(v, null, 2);
}

function generateInputTemplate(inputs: TriggerInputField[]): string {
  if (inputs.length === 0) return '{}';
  const obj: Record<string, unknown> = {};
  for (const inp of inputs) {
    if (inp.defaultValue) {
      try { obj[inp.name] = JSON.parse(inp.defaultValue); } catch { obj[inp.name] = inp.defaultValue; }
    } else {
      switch (inp.type) {
        case 'string': obj[inp.name] = ''; break;
        case 'number': obj[inp.name] = 0; break;
        case 'boolean': obj[inp.name] = false; break;
        case 'array': obj[inp.name] = []; break;
        case 'object': obj[inp.name] = {}; break;
        default: obj[inp.name] = null;
      }
    }
  }
  return JSON.stringify(obj, null, 2);
}

function generateShadowTemplate(taskStepIds: string[]): string {
  if (taskStepIds.length === 0) return '';
  const obj: Record<string, unknown> = {};
  for (const id of taskStepIds) obj[id] = { result: 'mock_value' };
  return JSON.stringify(obj, null, 2);
}

function toggleStepInList(current: string, stepId: string): string {
  const ids = current.split(',').map(s => s.trim()).filter(Boolean);
  const idx = ids.indexOf(stepId);
  if (idx >= 0) ids.splice(idx, 1); else ids.push(stepId);
  return ids.join(', ');
}

function isStepInList(current: string, stepId: string): boolean {
  return current.split(',').map(s => s.trim()).filter(Boolean).includes(stepId);
}

/** Known fields for incoming_message triggers */
const MESSAGE_FIELDS = [
  { key: 'from', label: 'From', placeholder: 'sender@example.com', multiline: false },
  { key: 'to', label: 'To', placeholder: 'recipient@example.com', multiline: false },
  { key: 'subject', label: 'Subject', placeholder: 'Meeting notes', multiline: false },
  { key: 'body', label: 'Body', placeholder: 'The message content…', multiline: true },
  { key: 'channel_id', label: 'Channel ID', placeholder: 'my_slack_channel', multiline: false },
  { key: 'external_id', label: 'External ID', placeholder: '(optional)', multiline: false },
] as const;

// ── Component ──────────────────────────────────────────────────────────

export function TestsPanel(props: TestsPanelProps) {
  const ctx = createMemo(() => props.context);

  // Dialog state: null = closed, number = editing index, 'new' = creating
  const [dialogMode, setDialogMode] = createSignal<number | 'new' | null>(null);
  const dialogOpen = createMemo(() => dialogMode() !== null);

  // ── Form signals ──
  const [editName, setEditName] = createSignal('');
  const [editDesc, setEditDesc] = createSignal('');
  const [editTriggerId, setEditTriggerId] = createSignal('');
  const [editInputs, setEditInputs] = createSignal('{}');
  const [editShadowOutputs, setEditShadowOutputs] = createSignal('');
  // Per-step mock data: stepId → { enabled, value (JSON for step output), toolCalls (for agent steps) }
  const [editStepMocks, setEditStepMocks] = createSignal<Record<string, { enabled: boolean; value: string; toolCalls?: ExpectedToolCall[] }>>({});
  const [editExpStatus, setEditExpStatus] = createSignal('completed');
  const [editExpOutput, setEditExpOutput] = createSignal('');
  const [editExpStepsCompleted, setEditExpStepsCompleted] = createSignal('');
  const [editExpStepsNotReached, setEditExpStepsNotReached] = createSignal('');
  const [editExpActionCounts, setEditExpActionCounts] = createSignal('');
  const [formError, setFormError] = createSignal('');

  // Message-specific form fields (for incoming_message triggers)
  const [editMsgFrom, setEditMsgFrom] = createSignal('');
  const [editMsgTo, setEditMsgTo] = createSignal('');
  const [editMsgSubject, setEditMsgSubject] = createSignal('');
  const [editMsgBody, setEditMsgBody] = createSignal('');
  const [editMsgChannelId, setEditMsgChannelId] = createSignal('');
  const [editMsgExternalId, setEditMsgExternalId] = createSignal('');

  // Manual-with-schema form fields: Record<fieldName, stringValue>
  const [editSchemaFields, setEditSchemaFields] = createSignal<Record<string, string>>({});

  // Resolved trigger subtype for the active trigger
  const activeTriggerSubtype = createMemo((): string => {
    const subtypes = ctx()?.triggerSubtypes ?? {};
    const tid = editTriggerId();
    if (tid && subtypes[tid]) return subtypes[tid];
    // Default to first trigger's subtype
    const ids = ctx()?.triggerStepIds ?? [];
    if (ids.length > 0 && subtypes[ids[0]]) return subtypes[ids[0]];
    return 'manual';
  });

  // Active trigger's input fields for manual triggers with schema
  const activeTriggerFields = createMemo((): TriggerInputField[] => {
    const schemas = ctx()?.triggerSchemas;
    if (!schemas) return [];
    const tid = editTriggerId();
    if (tid && schemas[tid]) return schemas[tid];
    const all: TriggerInputField[] = [];
    for (const fields of Object.values(schemas)) all.push(...fields);
    return all;
  });

  // ── Hydrate message fields from JSON (when opening an existing test) ──
  function hydrateMessageFields(inputs: Record<string, unknown>) {
    setEditMsgFrom(String(inputs.from ?? ''));
    setEditMsgTo(String(inputs.to ?? ''));
    setEditMsgSubject(String(inputs.subject ?? ''));
    setEditMsgBody(String(inputs.body ?? ''));
    setEditMsgChannelId(String(inputs.channel_id ?? ''));
    setEditMsgExternalId(String(inputs.external_id ?? ''));
  }

  function hydrateSchemaFields(inputs: Record<string, unknown>, fields: TriggerInputField[]) {
    const result: Record<string, string> = {};
    for (const f of fields) {
      const val = inputs[f.name];
      result[f.name] = val !== undefined && val !== null ? (typeof val === 'string' ? val : JSON.stringify(val)) : '';
    }
    setEditSchemaFields(result);
  }

  // ── Build JSON from form fields ──
  function buildMessageInputsJson(): string {
    const obj: Record<string, unknown> = {};
    if (editMsgFrom()) obj.from = editMsgFrom();
    if (editMsgTo()) obj.to = editMsgTo();
    if (editMsgSubject()) obj.subject = editMsgSubject();
    if (editMsgBody()) obj.body = editMsgBody();
    if (editMsgChannelId()) obj.channel_id = editMsgChannelId();
    if (editMsgExternalId()) obj.external_id = editMsgExternalId();
    obj.timestamp_ms = Date.now();
    return JSON.stringify(obj, null, 2);
  }

  function buildSchemaInputsJson(fields: TriggerInputField[]): string {
    const obj: Record<string, unknown> = {};
    const vals = editSchemaFields();
    for (const f of fields) {
      const raw = vals[f.name] ?? '';
      if (!raw && !f.required) continue;
      // Try parsing as JSON for non-string types
      if (f.type !== 'string') {
        const parsed = safeJsonParse(raw);
        if (parsed !== undefined) { obj[f.name] = parsed; continue; }
      }
      obj[f.name] = raw;
    }
    return JSON.stringify(obj, null, 2);
  }

  function resetForm() {
    setEditName('');
    setEditDesc('');
    setEditTriggerId('');
    setEditInputs('{}');
    setEditShadowOutputs('');
    setEditStepMocks({});
    setEditExpStatus('completed');
    setEditExpOutput('');
    setEditExpStepsCompleted('');
    setEditExpStepsNotReached('');
    setEditExpActionCounts('');
    setFormError('');
    setEditMsgFrom(''); setEditMsgTo(''); setEditMsgSubject(''); setEditMsgBody('');
    setEditMsgChannelId(''); setEditMsgExternalId('');
    setEditSchemaFields({});
  }

  function openNew() {
    resetForm();
    setDialogMode('new');
  }

  function openEdit(idx: number) {
    const tc = props.testCases[idx];
    setEditName(tc.name);
    setEditDesc(tc.description ?? '');
    setEditTriggerId(tc.trigger_step_id ?? '');
    setEditInputs(prettyJson(tc.inputs));
    setEditShadowOutputs(tc.shadow_outputs ? prettyJson(tc.shadow_outputs) : '');
    // Hydrate per-step mocks from shadow_outputs + expected_tool_calls
    const mocks: Record<string, { enabled: boolean; value: string; toolCalls?: ExpectedToolCall[] }> = {};
    const subtypes = ctx()?.taskStepSubtypes ?? {};
    if (tc.shadow_outputs) {
      for (const [stepId, val] of Object.entries(tc.shadow_outputs)) {
        const sub = subtypes[stepId] ?? 'call_tool';
        if (sub === 'invoke_agent' || sub === 'invoke_prompt') {
          const v = val as Record<string, unknown>;
          mocks[stepId] = { enabled: true, value: typeof v === 'object' && v !== null ? prettyJson(v) : String(v) };
        } else {
          mocks[stepId] = { enabled: true, value: prettyJson(val) };
        }
      }
    }
    // Hydrate expected_tool_calls for steps that have assertions (cannot overlap with shadow_outputs)
    if (tc.expected_tool_calls) {
      for (const [stepId, calls] of Object.entries(tc.expected_tool_calls)) {
        if (!mocks[stepId]) {
          mocks[stepId] = { enabled: false, value: '', toolCalls: calls };
        } else {
          // shadow_outputs already set — expected_tool_calls not applicable, skip
        }
      }
    }
    setEditStepMocks(mocks);
    setEditExpStatus(tc.expectations.status ?? '');
    setEditExpOutput(tc.expectations.output ? prettyJson(tc.expectations.output) : '');
    setEditExpStepsCompleted(tc.expectations.steps_completed?.join(', ') ?? '');
    setEditExpStepsNotReached(tc.expectations.steps_not_reached?.join(', ') ?? '');
    setEditExpActionCounts(tc.expectations.intercepted_action_counts ? prettyJson(tc.expectations.intercepted_action_counts) : '');
    setFormError('');

    // Hydrate type-specific fields
    const inputs = tc.inputs as Record<string, unknown>;
    hydrateMessageFields(inputs);
    hydrateSchemaFields(inputs, activeTriggerFields());

    setDialogMode(idx);
  }

  function closeDialog() {
    setDialogMode(null);
    setFormError('');
  }

  /** Resolve the actual inputs JSON from the appropriate form */
  function resolveInputsJson(): string {
    const subtype = activeTriggerSubtype();
    if (subtype === 'incoming_message') return buildMessageInputsJson();
    const fields = activeTriggerFields();
    if (subtype === 'manual' && fields.length > 0) return buildSchemaInputsJson(fields);
    return editInputs();
  }

  function saveDialog() {
    const name = editName().trim();
    if (!name) { setFormError('Name is required'); return; }

    const inputsJson = resolveInputsJson();
    const inputs = safeJsonParse(inputsJson);
    if (inputs === undefined) { setFormError('Test data must be valid JSON'); return; }
    if (typeof inputs !== 'object' || Array.isArray(inputs)) { setFormError('Test data must be a JSON object'); return; }

    let shadow_outputs: Record<string, unknown> | undefined;
    let expected_tool_calls: Record<string, ExpectedToolCall[]> | undefined;
    // Build shadow_outputs and expected_tool_calls from per-step mocks
    const mocks = editStepMocks();
    const hasMocks = Object.values(mocks).some(m => m.enabled && (m.value.trim() || (m.toolCalls && m.toolCalls.length > 0)));
    if (hasMocks) {
      shadow_outputs = {};
      for (const [stepId, mock] of Object.entries(mocks)) {
        if (!mock.enabled) continue;
        if (mock.value.trim()) {
          const parsed = safeJsonParse(mock.value);
          if (parsed === undefined) { setFormError(`Mock for step "${stepId}" must be valid JSON`); return; }
          shadow_outputs[stepId] = parsed;
          // Validate: cannot have expected_tool_calls on a mocked step
          if (mock.toolCalls && mock.toolCalls.length > 0) {
            setFormError(`Step "${stepId}": expected tool calls cannot be set on a mocked step (has shadow output)`);
            return;
          }
        }
        if (mock.toolCalls && mock.toolCalls.length > 0) {
          if (!expected_tool_calls) expected_tool_calls = {};
          const valid = mock.toolCalls.filter(c => c.tool_id.trim());
          if (valid.length > 0) expected_tool_calls[stepId] = valid;
        }
      }
      if (Object.keys(shadow_outputs).length === 0) shadow_outputs = undefined;
    } else if (editShadowOutputs().trim()) {
      // Fallback: raw JSON textarea (for advanced users or legacy data)
      const parsed = safeJsonParse(editShadowOutputs());
      if (parsed === undefined) { setFormError('Mocked step results must be valid JSON'); return; }
      if (typeof parsed !== 'object' || Array.isArray(parsed)) { setFormError('Mocked step results must be a JSON object'); return; }
      shadow_outputs = parsed as Record<string, unknown>;
    }

    let output: Record<string, unknown> | undefined;
    if (editExpOutput().trim()) {
      const parsed = safeJsonParse(editExpOutput());
      if (parsed === undefined) { setFormError('Expected output must be valid JSON'); return; }
      if (typeof parsed !== 'object' || Array.isArray(parsed)) { setFormError('Expected output must be a JSON object'); return; }
      output = parsed as Record<string, unknown>;
    }

    let intercepted_action_counts: Record<string, number> | undefined;
    if (editExpActionCounts().trim()) {
      const parsed = safeJsonParse(editExpActionCounts());
      if (parsed === undefined) { setFormError('Action counts must be valid JSON'); return; }
      if (typeof parsed !== 'object' || Array.isArray(parsed)) { setFormError('Action counts must be a JSON object { key: number }'); return; }
      const obj = parsed as Record<string, unknown>;
      for (const [k, v] of Object.entries(obj)) {
        if (typeof v !== 'number') { setFormError(`Action count "${k}" must be a number`); return; }
      }
      intercepted_action_counts = obj as Record<string, number>;
    }

    const expectations: TestExpectations = {};
    if (editExpStatus()) expectations.status = editExpStatus();
    if (output) expectations.output = output;
    if (editExpStepsCompleted().trim()) {
      expectations.steps_completed = editExpStepsCompleted().split(',').map(s => s.trim()).filter(Boolean);
    }
    if (editExpStepsNotReached().trim()) {
      expectations.steps_not_reached = editExpStepsNotReached().split(',').map(s => s.trim()).filter(Boolean);
    }
    if (intercepted_action_counts) expectations.intercepted_action_counts = intercepted_action_counts;

    const tc: WorkflowTestCase = {
      name,
      inputs: inputs as Record<string, unknown>,
      expectations,
    };
    if (editDesc()) tc.description = editDesc();
    if (editTriggerId()) tc.trigger_step_id = editTriggerId();
    if (shadow_outputs) tc.shadow_outputs = shadow_outputs;
    if (expected_tool_calls) tc.expected_tool_calls = expected_tool_calls;

    const mode = dialogMode();
    if (mode === 'new') {
      props.onTestCasesChange([...props.testCases, tc]);
    } else if (typeof mode === 'number') {
      const updated = [...props.testCases];
      updated[mode] = tc;
      props.onTestCasesChange(updated);
    }
    closeDialog();
  }

  function removeTest(idx: number) {
    props.onTestCasesChange(props.testCases.filter((_, i) => i !== idx));
  }

  function duplicateTest(idx: number) {
    const tc = { ...props.testCases[idx], name: props.testCases[idx].name + ' (copy)' };
    props.onTestCasesChange([...props.testCases, tc]);
  }

  function resultFor(name: string): WorkflowTestResult | undefined {
    return props.testResults.find(r => r.test_name === name);
  }

  // Track which test result is shown in the details dialog
  const [detailsResult, setDetailsResult] = createSignal<WorkflowTestResult | null>(null);
  function openDetails(name: string) {
    const r = resultFor(name);
    if (r) setDetailsResult(r);
  }
  function closeDetails() { setDetailsResult(null); }

  // ── Sub-renderers for different trigger input forms ──

  /** Incoming message: labeled form fields */
  function renderMessageForm() {
    const setters: Record<string, (v: string) => void> = {
      from: setEditMsgFrom, to: setEditMsgTo, subject: setEditMsgSubject,
      body: setEditMsgBody, channel_id: setEditMsgChannelId, external_id: setEditMsgExternalId,
    };
    const getters: Record<string, () => string> = {
      from: editMsgFrom, to: editMsgTo, subject: editMsgSubject,
      body: editMsgBody, channel_id: editMsgChannelId, external_id: editMsgExternalId,
    };
    return (
      <div class="space-y-2">
        <For each={MESSAGE_FIELDS as unknown as typeof MESSAGE_FIELDS[number][]}>
          {(field) => (
            <div>
              <label class={labelCls}>{field.label}</label>
              {field.multiline ? (
                <textarea
                  class={textareaCls}
                  style={{ 'min-height': '80px' }}
                  value={getters[field.key]()}
                  onInput={e => setters[field.key](e.currentTarget.value)}
                  placeholder={field.placeholder}
                />
              ) : (
                <input
                  class={inputCls}
                  value={getters[field.key]()}
                  onInput={e => setters[field.key](e.currentTarget.value)}
                  placeholder={field.placeholder}
                />
              )}
            </div>
          )}
        </For>
      </div>
    );
  }

  /** Manual trigger with declared input_schema: auto-generated form fields */
  function renderSchemaForm(fields: TriggerInputField[]) {
    return (
      <div class="space-y-2">
        <For each={fields}>
          {(field) => (
            <div>
              <label class={labelCls}>
                {field.name} <span class="opacity-50">({field.type})</span>
                {field.required && <span class="text-destructive ml-1">*</span>}
              </label>
              <Show when={field.description}>
                <p class="text-[11px] text-muted-foreground -mt-0.5 mb-1 italic">{field.description}</p>
              </Show>
              <Show when={field.type === 'boolean'} fallback={
                <input
                  class={inputCls}
                  value={editSchemaFields()[field.name] ?? ''}
                  onInput={e => setEditSchemaFields(prev => ({ ...prev, [field.name]: e.currentTarget.value }))}
                  placeholder={field.defaultValue ?? (field.type === 'number' ? '0' : '')}
                />
              }>
                <select
                  class={inputCls}
                  value={editSchemaFields()[field.name] ?? ''}
                  onChange={e => setEditSchemaFields(prev => ({ ...prev, [field.name]: e.currentTarget.value }))}
                >
                  <option value="">—</option>
                  <option value="true">true</option>
                  <option value="false">false</option>
                </select>
              </Show>
            </div>
          )}
        </For>
      </div>
    );
  }

  /** Fallback: raw JSON textarea with optional template button */
  function renderJsonForm() {
    const fields = activeTriggerFields();
    return (
      <div>
        <Show when={fields.length > 0}>
          <div class="flex justify-end mb-1">
            <button
              class="text-xs px-2 py-0.5 rounded border border-border bg-background hover:bg-muted cursor-pointer"
              onClick={() => setEditInputs(generateInputTemplate(fields))}
              title="Fill from trigger schema"
            >
              📋 Template
            </button>
          </div>
        </Show>
        <textarea
          class={textareaCls}
          style={{ 'min-height': '120px' }}
          value={editInputs()}
          onInput={e => setEditInputs(e.currentTarget.value)}
          placeholder="{}"
        />
        <Show when={fields.length > 0}>
          <p class="text-[11px] text-muted-foreground mt-1 italic">
            Fields: {fields.map(i => `${i.name}${i.required ? '*' : ''} (${i.type})`).join(', ')}
          </p>
        </Show>
      </div>
    );
  }

  // ── Render ──

  return (
    <div style={panelStyle}>
      <div style={props.sectionHeaderStyle}>
        <span>Tests</span>
        <span style={{ 'margin-left': 'auto', 'font-size': '0.85em', color: 'hsl(var(--muted-foreground))' }}>
          {props.testCases.length} case{props.testCases.length !== 1 ? 's' : ''}
        </span>
      </div>

      {/* ── Scrollable card list ── */}
      <div style={{ flex: '1', 'overflow-y': 'auto', padding: '8px' }}>
        <For each={props.testCases}>
          {(tc, idx) => {
            const result = () => resultFor(tc.name);
            return (
              <div style={{ ...cardBase, ...(result() ? { cursor: 'pointer' } : {}) }} onClick={() => { if (result()) openDetails(tc.name); }}>
                <div style={{ display: 'flex', 'align-items': 'center', gap: '6px', 'margin-bottom': '4px' }}>
                  <span style={{ 'font-weight': '600', flex: '1', overflow: 'hidden', 'text-overflow': 'ellipsis', 'white-space': 'nowrap' }}>{tc.name}</span>
                  <Show when={result()}>
                    {(r) => <span style={r().passed ? passBadge : failBadge}>{r().passed ? '✅ Pass' : '❌ Fail'}</span>}
                  </Show>
                  <Show when={!result() && props.testProgress?.[tc.name]}>
                    {(_p) => {
                      const p = () => props.testProgress?.[tc.name];
                      return <span style={{ ...passBadge, background: 'hsl(210 80% 50% / 0.15)', color: 'hsl(210 80% 50%)' }}>
                        {p()?.state === 'running' ? '⏳ Running…' : p()?.state === 'passed' ? '✅ Pass' : '❌ Fail'}
                      </span>;
                    }}
                  </Show>
                </div>

                <Show when={tc.description}>
                  <div style={{ color: 'hsl(var(--muted-foreground))', 'font-size': '0.85em', 'margin-bottom': '4px' }}>{tc.description}</div>
                </Show>

                <div style={{ color: 'hsl(var(--muted-foreground))', 'font-size': '0.8em' }}>
                  <Show when={tc.expectations.status}><span>Expects: <b>{tc.expectations.status}</b></span></Show>
                  <Show when={tc.expectations.steps_completed?.length}>
                    <span> · {tc.expectations.steps_completed!.length} steps</span>
                  </Show>
                  <Show when={tc.expectations.intercepted_action_counts}>
                    {(counts) => <span>{Object.entries(counts()).map(([k, v]) => ` · ${v} ${k}`).join('')}</span>}
                  </Show>
                </div>

                {/* Brief result summary (steps, actions, duration) */}
                <Show when={result()}>
                  {(r) => (
                    <div style={{ color: 'hsl(var(--muted-foreground))', 'font-size': '0.75em', 'margin-top': '3px' }}>
                      {[
                        r().step_results?.length ? `${r().step_results!.length} steps` : null,
                        (r().intercepted_actions_total ?? r().intercepted_actions?.length) ? `${r().intercepted_actions_total ?? r().intercepted_actions!.length} actions` : null,
                        r().duration_ms != null ? `${r().duration_ms}ms` : null,
                      ].filter(Boolean).join(' · ') || '—'}
                      <span style={{ 'margin-left': '6px', color: 'hsl(var(--primary))' }}>click for details</span>
                    </div>
                  )}
                </Show>

                <Show when={result() && !result()!.passed}>
                  <div style={{ 'margin-top': '6px', 'border-top': '1px solid hsl(var(--border))', 'padding-top': '6px' }}>
                    <For each={result()!.failures}>
                      {(f: WorkflowTestFailure) => (
                        <div style={{ 'font-size': '0.78em', 'margin-bottom': '4px', color: 'hsl(0 84% 60%)' }}>
                          <div style={{ 'font-weight': '500' }}>{f.expectation}</div>
                          <div style={{ 'font-family': 'monospace', 'font-size': '0.9em' }}>Expected: {f.expected}</div>
                          <div style={{ 'font-family': 'monospace', 'font-size': '0.9em' }}>Actual: {f.actual}</div>
                        </div>
                      )}
                    </For>
                  </div>
                </Show>

                <Show when={!props.readOnly}>
                  <div style={{ display: 'flex', gap: '6px', 'margin-top': '6px', 'justify-content': 'flex-end', 'align-items': 'center' }} onClick={(e: MouseEvent) => e.stopPropagation()}>
                    <button
                      style={{ ...btnSm, background: props.running ? 'hsl(var(--muted))' : 'hsl(var(--primary))', color: props.running ? 'hsl(var(--muted-foreground))' : 'hsl(var(--primary-foreground))', 'border-color': 'transparent' }}
                      onClick={() => props.onRunTest(tc.name)}
                      disabled={props.running}
                      title="Run this test"
                    >▶</button>
                    <Show when={result()}>
                      <button style={btnSm} onClick={() => openDetails(tc.name)} title="View details">📋</button>
                    </Show>
                    <div style={{ flex: '1' }} />
                    <button style={btnSm} onClick={() => duplicateTest(idx())} title="Duplicate">⧉</button>
                    <button style={btnSm} onClick={() => openEdit(idx())}>Edit</button>
                    <button style={btnDanger} onClick={() => removeTest(idx())}>Remove</button>
                  </div>
                </Show>
              </div>
            );
          }}
        </For>

        <Show when={props.testCases.length === 0}>
          <div class="px-3 py-5 text-center text-muted-foreground text-sm">
            <p class="mb-2">No test cases yet.</p>
            <Show when={!props.readOnly}>
              <p class="mb-3">Click <b>+ Add Test</b> below to create one.</p>
              <div class="text-left text-xs text-muted-foreground space-y-1">
                <p><b>How it works:</b></p>
                <p>1. <b>Add a test</b> — define inputs and expected outcomes</p>
                <p>2. Click <b>▶ Run All</b> — tests run in shadow mode (no real side effects)</p>
                <p>3. Review results — see which tests passed or failed and why</p>
                <p class="mt-2 opacity-70">Unsaved changes are auto-saved before running.</p>
              </div>
            </Show>
          </div>
        </Show>
      </div>

      {/* ── Bottom actions ── */}
      <div style={{ padding: '8px', 'border-top': '1px solid hsl(var(--border))', display: 'flex', 'flex-direction': 'column', gap: '6px' }}>
        <Show when={props.unsavedChanges && props.testCases.length > 0 && !props.running}>
          <div class="text-xs text-muted-foreground px-1">
            💾 Unsaved changes will be auto-saved before running.
          </div>
        </Show>
        <div style={{ display: 'flex', gap: '6px', 'justify-content': 'space-between' }}>
          <Show when={!props.readOnly}>
            <button style={btnSm} onClick={openNew}>+ Add Test</button>
          </Show>
          <Show when={props.running && props.onCancelTests}>
            <button
              style={{
                ...btnSm,
                background: 'hsl(0 84% 60% / 0.15)',
                color: 'hsl(0 84% 60%)',
                'border-color': 'hsl(0 84% 60% / 0.3)',
                'margin-left': 'auto',
                'margin-right': '4px',
              }}
              onClick={() => props.onCancelTests?.()}
              title="Stop after current test finishes"
            >⏹ Stop</button>
          </Show>
          <button
            style={{
              ...btnSm,
              background: props.running ? 'hsl(var(--muted))' : 'hsl(var(--primary))',
              color: props.running ? 'hsl(var(--muted-foreground))' : 'hsl(var(--primary-foreground))',
              'border-color': 'transparent',
              'margin-left': props.running && props.onCancelTests ? undefined : 'auto',
            }}
            onClick={props.onRunTests}
            disabled={props.running || props.testCases.length === 0}
          >
            {(() => {
              if (props.running) {
                const prog = props.testProgress ?? {};
                const entries = Object.values(prog);
                if (entries.length > 0) {
                  const done = entries.filter(e => e.state !== 'running').length;
                  const total = entries[0]?.total ?? entries.length;
                  return `⏳ ${done}/${total}`;
                }
                return '⏳ Running…';
              }
              return props.unsavedChanges ? '💾 Save & Run All' : '▶ Run All';
            })()}
          </button>
        </div>
      </div>

      {/* ═══════════════════ Edit / New Dialog ═══════════════════ */}
      <Dialog
        open={dialogOpen()}
        onOpenChange={(open) => { if (!open) closeDialog(); }}
      >
        <DialogContent
          class="max-w-[720px] max-h-[85vh] flex flex-col overflow-hidden p-0"
          onInteractOutside={(e: Event) => e.preventDefault()}
        >
          <DialogHeader class="px-6 pt-5 pb-3">
            <DialogTitle class="text-base">
              {dialogMode() === 'new' ? '🧪 New Test Case' : '✏️ Edit Test Case'}
            </DialogTitle>
          </DialogHeader>

          <DialogBody class="px-6 pb-4 space-y-4 text-sm">
            {/* ── Name + Description ── */}
            <div class="grid grid-cols-2 gap-4">
              <div>
                <label class={labelCls}>Name <span class="text-destructive">*</span></label>
                <input class={inputCls} value={editName()} onInput={e => setEditName(e.currentTarget.value)} placeholder="e.g. happy-path" />
              </div>
              <div>
                <label class={labelCls}>Description</label>
                <input class={inputCls} value={editDesc()} onInput={e => setEditDesc(e.currentTarget.value)} placeholder="What this test verifies" />
              </div>
            </div>

            {/* ── Trigger selector (multi-trigger workflows) ── */}
            <Show when={(ctx()?.triggerStepIds.length ?? 0) > 1}>
              <div>
                <label class={labelCls}>Trigger Step</label>
                <select class={inputCls} value={editTriggerId()} onChange={e => setEditTriggerId(e.currentTarget.value)}>
                  <option value="">Auto (first trigger)</option>
                  <For each={ctx()!.triggerStepIds}>
                    {(tid) => {
                      const sub = ctx()!.triggerSubtypes[tid] ?? '';
                      const label = sub ? `${tid} (${sub.replace(/_/g, ' ')})` : tid;
                      return <option value={tid}>{label}</option>;
                    }}
                  </For>
                </select>
              </div>
            </Show>

            {/* ── Two-column layout ── */}
            <div class="grid grid-cols-2 gap-4">
              {/* LEFT: Test Data */}
              <div class="space-y-3">
                <h4 class="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
                  Test Data
                  <span class="normal-case font-normal ml-1 opacity-60">— what your workflow receives</span>
                </h4>

                {/* Render the right form based on trigger type */}
                <Show when={activeTriggerSubtype() === 'incoming_message'}>
                  {renderMessageForm()}
                </Show>
                <Show when={activeTriggerSubtype() === 'manual' && activeTriggerFields().length > 0}>
                  {renderSchemaForm(activeTriggerFields())}
                </Show>
                <Show when={activeTriggerSubtype() !== 'incoming_message' && !(activeTriggerSubtype() === 'manual' && activeTriggerFields().length > 0)}>
                  {renderJsonForm()}
                </Show>

                {/* Mocked Step Results */}
                <div>
                  <div class="flex items-center gap-2 mb-1">
                    <h4 class="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
                      Mocked Step Results
                      <span class="normal-case font-normal ml-1 opacity-60">— define what each step returns</span>
                    </h4>
                  </div>
                  <Show when={(ctx()?.taskStepIds.length ?? 0) > 0} fallback={
                    <p class="text-[11px] text-muted-foreground italic">No task steps in this workflow.</p>
                  }>
                    <div class="space-y-2">
                      <For each={ctx()!.taskStepIds}>
                        {(stepId) => {
                          const subtype = () => ctx()?.taskStepSubtypes[stepId] ?? 'call_tool';
                          const isAgent = () => subtype() === 'invoke_agent' || subtype() === 'invoke_prompt';
                          const mock = () => editStepMocks()[stepId];
                          const enabled = () => mock()?.enabled ?? false;
                          const value = () => mock()?.value ?? '';
                          const toolCalls = () => mock()?.toolCalls ?? [];

                          function setMock(partial: { enabled?: boolean; value?: string; toolCalls?: ExpectedToolCall[] }) {
                            setEditStepMocks(prev => ({
                              ...prev,
                              [stepId]: {
                                enabled: partial.enabled ?? prev[stepId]?.enabled ?? false,
                                value: partial.value ?? prev[stepId]?.value ?? '',
                                toolCalls: partial.toolCalls ?? prev[stepId]?.toolCalls,
                              },
                            }));
                          }

                          // Parse structured fields from JSON value
                          const agentResponse = () => {
                            try { const o = JSON.parse(value()); return typeof o?.result === 'string' ? o.result : ''; }
                            catch { return ''; }
                          };
                          const agentStatus = () => {
                            try { const o = JSON.parse(value()); return o?.status === 'spawned' ? 'spawned' : 'completed'; }
                            catch { return 'completed'; }
                          };
                          const toolResult = () => {
                            try {
                              const o = JSON.parse(value());
                              const r = o?.result;
                              return typeof r === 'string' ? r : (r !== undefined ? JSON.stringify(r, null, 2) : '');
                            } catch { return value(); }
                          };

                          function updateAgentMock(response?: string, status?: string) {
                            const r = response ?? agentResponse();
                            const s = status ?? agentStatus();
                            setMock({ value: JSON.stringify({
                              result: r,
                              agent_id: `mock-agent-${stepId}`,
                              status: s,
                            }, null, 2) });
                          }
                          function updateToolMock(result: string) {
                            try {
                              const parsed = JSON.parse(result);
                              setMock({ value: JSON.stringify({ result: parsed }, null, 2) });
                            } catch {
                              setMock({ value: JSON.stringify({ result }, null, 2) });
                            }
                          }

                          function toggleEnabled() {
                            const wasEnabled = enabled();
                            if (!wasEnabled && !value()) {
                              if (isAgent()) {
                                setMock({ enabled: true, value: JSON.stringify({
                                  result: '',
                                  agent_id: `mock-agent-${stepId}`,
                                  status: 'completed',
                                }, null, 2) });
                              } else {
                                setMock({ enabled: true, value: JSON.stringify({ result: '' }, null, 2) });
                              }
                            } else {
                              setMock({ enabled: !wasEnabled });
                            }
                          }

                          function addToolCall() {
                            setMock({ toolCalls: [...toolCalls(), { tool_id: '' }] });
                          }
                          function removeToolCall(idx: number) {
                            setMock({ toolCalls: toolCalls().filter((_, i) => i !== idx) });
                          }
                          function updateToolCall(idx: number, toolId: string) {
                            const updated = [...toolCalls()];
                            updated[idx] = { ...updated[idx], tool_id: toolId };
                            setMock({ toolCalls: updated });
                          }

                          return (
                            <div class={`rounded border ${enabled() ? 'border-primary/40 bg-primary/5' : toolCalls().length > 0 ? 'border-accent/40 bg-accent/5' : 'border-border bg-background'} p-2`}>
                              <div class="flex items-center gap-2 cursor-pointer" onClick={toggleEnabled}>
                                <input type="checkbox" checked={enabled()} class="accent-[hsl(var(--primary))]" />
                                <span class="text-xs font-mono font-semibold">{stepId}</span>
                                <span class="text-[10px] text-muted-foreground ml-auto">
                                  {isAgent() ? '🤖 agent' : '🔧 tool'}
                                </span>
                              </div>
                              {/* Mock output section — only when step is mocked */}
                              <Show when={enabled()}>
                                <div class="mt-2 space-y-2">
                                  <Show when={isAgent()} fallback={
                                    <>
                                      <label class="text-[11px] text-muted-foreground font-medium">Return value</label>
                                      <input
                                        type="text"
                                        class={inputCls}
                                        value={toolResult()}
                                        onInput={e => updateToolMock(e.currentTarget.value)}
                                        placeholder="e.g. success, 42, or a JSON object"
                                      />
                                    </>
                                  }>
                                    <div>
                                      <label class="text-[11px] text-muted-foreground font-medium">Agent response</label>
                                      <textarea
                                        class={textareaCls}
                                        style={{ 'min-height': '56px', 'font-size': '0.85em' }}
                                        value={agentResponse()}
                                        onInput={e => updateAgentMock(e.currentTarget.value)}
                                        placeholder="What should the agent reply with?"
                                      />
                                    </div>
                                    <div>
                                      <label class="text-[11px] text-muted-foreground font-medium">Status</label>
                                      <select
                                        class={inputCls}
                                        value={agentStatus()}
                                        onChange={e => updateAgentMock(undefined, e.currentTarget.value)}
                                      >
                                        <option value="completed">Completed (synchronous)</option>
                                        <option value="spawned">Spawned (asynchronous)</option>
                                      </select>
                                    </div>
                                    <p class="text-[10px] text-muted-foreground/60 italic">
                                      ⓘ Mocked steps skip execution — use Expected Tool Calls on non-mocked agent steps to assert tool usage.
                                    </p>
                                  </Show>
                                </div>
                              </Show>
                              {/* Expected Tool Calls — only for non-mocked agent steps */}
                              <Show when={!enabled() && isAgent()}>
                                <div class="mt-2">
                                  <div class="flex items-center gap-1 mb-1">
                                    <label class="text-[11px] text-muted-foreground font-medium">Expected Tool Calls</label>
                                    <button
                                      type="button"
                                      class="ml-auto text-[10px] text-primary hover:text-primary/80 font-medium"
                                      onClick={addToolCall}
                                    >
                                      + Add
                                    </button>
                                  </div>
                                  <Show when={toolCalls().length === 0}>
                                    <p class="text-[10px] text-muted-foreground/60 italic">None — click + Add to assert tool calls the agent should make</p>
                                  </Show>
                                  <Index each={toolCalls()}>
                                    {(tc, idx) => {
                                      const toolDefs = () => ctx()?.toolDefinitions ?? [];
                                      const toolGroups = createMemo(() => {
                                        const groups = new Map<string, { id: string; name?: string }[]>();
                                        for (const t of toolDefs()) {
                                          const dot = t.id.indexOf('.');
                                          const prefix = dot > 0 ? t.id.substring(0, dot) : 'other';
                                          if (!groups.has(prefix)) groups.set(prefix, []);
                                          groups.get(prefix)!.push({ id: t.id, name: t.name });
                                        }
                                        return Array.from(groups.entries()).sort((a, b) => a[0].localeCompare(b[0]));
                                      });
                                      const selectedTool = () => toolDefs().find(t => t.id === tc().tool_id);
                                      const inputSchema = () => selectedTool()?.input_schema as Record<string, any> | undefined;
                                      const schemaProps = () => {
                                        const s = inputSchema();
                                        return s?.properties ? Object.entries(s.properties as Record<string, any>) : [];
                                      };
                                      const requiredFields = () => {
                                        const s = inputSchema();
                                        return Array.isArray(s?.required) ? s.required as string[] : [];
                                      };
                                      function updateParams(paramName: string, paramValue: string) {
                                        const updated = [...toolCalls()];
                                        const existing = updated[idx]?.arguments as Record<string, unknown> ?? {};
                                        updated[idx] = { ...updated[idx], arguments: { ...existing, [paramName]: paramValue } };
                                        setMock({ toolCalls: updated });
                                      }
                                      return (
                                        <div class="rounded border border-border/60 p-2 mb-1.5 bg-background">
                                          <div class="flex items-center gap-1 mb-1">
                                            <Show when={toolDefs().length > 0} fallback={
                                              <input
                                                type="text"
                                                class={`${inputCls} text-xs`}
                                                value={tc().tool_id}
                                                onInput={e => updateToolCall(idx, e.currentTarget.value)}
                                                placeholder="e.g. connector.email.send"
                                              />
                                            }>
                                              <select
                                                class={`${inputCls} text-xs`}
                                                value={tc().tool_id}
                                                onChange={e => {
                                                  const updated = [...toolCalls()];
                                                  updated[idx] = { tool_id: e.currentTarget.value };
                                                  setMock({ toolCalls: updated });
                                                }}
                                              >
                                                <option value="">— select tool —</option>
                                                <For each={toolGroups()}>
                                                  {([prefix, tools]) => (
                                                    <optgroup label={prefix}>
                                                      <For each={tools}>
                                                        {(td) => <option value={td.id}>{td.name || td.id}</option>}
                                                      </For>
                                                    </optgroup>
                                                  )}
                                                </For>
                                              </select>
                                            </Show>
                                            <button
                                              type="button"
                                              class="text-muted-foreground hover:text-destructive text-xs px-1 shrink-0"
                                              onClick={() => removeToolCall(idx)}
                                              title="Remove"
                                            >
                                              ×
                                            </button>
                                          </div>
                                          {/* Parameter fields from tool's input_schema */}
                                          <Show when={tc().tool_id && schemaProps().length > 0}>
                                            <div class="pl-2 mt-1 space-y-1 border-l-2 border-primary/20">
                                              <p class="text-[10px] text-muted-foreground/50 italic">
                                                Partial match — only specified fields are checked
                                              </p>
                                              <For each={schemaProps()}>
                                                {([pName, pDef]) => {
                                                  const isRequired = () => requiredFields().includes(pName);
                                                  const paramVal = () => {
                                                    const p = tc().arguments as Record<string, unknown> | undefined;
                                                    return p?.[pName] != null ? String(p[pName]) : '';
                                                  };
                                                  return (
                                                    <div>
                                                      <label class="text-[10px] text-muted-foreground">
                                                        {pName}
                                                        <Show when={isRequired()}><span class="text-destructive ml-0.5">*</span></Show>
                                                        <Show when={pDef.description}><span class="ml-1 opacity-60">— {pDef.description}</span></Show>
                                                      </label>
                                                      <Show when={pDef.type === 'boolean'} fallback={
                                                        <Show when={Array.isArray(pDef.enum)} fallback={
                                                          <input
                                                            type={pDef.type === 'number' || pDef.type === 'integer' ? 'number' : 'text'}
                                                            class={`${inputCls} text-xs`}
                                                            value={paramVal()}
                                                            onInput={e => updateParams(pName, e.currentTarget.value)}
                                                            placeholder={pDef.default != null ? `default: ${pDef.default}` : `(leave empty to skip)`}
                                                          />
                                                        }>
                                                          <select
                                                            class={`${inputCls} text-xs`}
                                                            value={paramVal()}
                                                            onChange={e => updateParams(pName, e.currentTarget.value)}
                                                          >
                                                            <option value="">—</option>
                                                            <For each={pDef.enum as string[]}>
                                                              {(ev) => <option value={ev}>{ev}</option>}
                                                            </For>
                                                          </select>
                                                        </Show>
                                                      }>
                                                        <select
                                                          class={`${inputCls} text-xs`}
                                                          value={paramVal()}
                                                          onChange={e => updateParams(pName, e.currentTarget.value)}
                                                        >
                                                          <option value="">—</option>
                                                          <option value="true">true</option>
                                                          <option value="false">false</option>
                                                        </select>
                                                      </Show>
                                                    </div>
                                                  );
                                                }}
                                              </For>
                                            </div>
                                          </Show>
                                        </div>
                                      );
                                    }}
                                  </Index>
                                </div>
                              </Show>
                            </div>
                          );
                        }}
                      </For>
                    </div>
                  </Show>
                </div>
              </div>

              {/* RIGHT: Expectations */}
              <div class="space-y-3">
                <h4 class="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
                  Expectations
                  <span class="normal-case font-normal ml-1 opacity-60">— what should happen</span>
                </h4>

                <div>
                  <label class={labelCls}>Status</label>
                  <select class={inputCls} value={editExpStatus()} onChange={e => setEditExpStatus(e.currentTarget.value)}>
                    <option value="">Any</option>
                    <option value="completed">Completed</option>
                    <option value="failed">Failed</option>
                  </select>
                </div>

                <div>
                  <label class={labelCls}>Expected Output <span class="opacity-50">(partial match)</span></label>
                  <textarea
                    class={textareaCls}
                    style={{ 'min-height': '60px' }}
                    value={editExpOutput()}
                    onInput={e => setEditExpOutput(e.currentTarget.value)}
                    placeholder='{"result": "expected_value"}'
                  />
                </div>

                <div>
                  <label class={labelCls}>Steps Completed</label>
                  <Show when={(ctx()?.stepIds.length ?? 0) > 0} fallback={
                    <input class={`${inputCls} font-mono`} value={editExpStepsCompleted()} onInput={e => setEditExpStepsCompleted(e.currentTarget.value)} placeholder="step_a, step_b" />
                  }>
                    <div class="flex flex-wrap gap-1">
                      <For each={ctx()!.stepIds}>
                        {(sid) => (
                          <span
                            class={`${chipBase} ${isStepInList(editExpStepsCompleted(), sid) ? chipOn : chipOff}`}
                            onClick={() => setEditExpStepsCompleted(toggleStepInList(editExpStepsCompleted(), sid))}
                            title={isStepInList(editExpStepsCompleted(), sid) ? `Remove ${sid}` : `Expect ${sid} to complete`}
                          >{sid}</span>
                        )}
                      </For>
                    </div>
                  </Show>
                </div>

                <div>
                  <label class={labelCls}>Steps Not Reached</label>
                  <Show when={(ctx()?.stepIds.length ?? 0) > 0} fallback={
                    <input class={`${inputCls} font-mono`} value={editExpStepsNotReached()} onInput={e => setEditExpStepsNotReached(e.currentTarget.value)} placeholder="step_c" />
                  }>
                    <div class="flex flex-wrap gap-1">
                      <For each={ctx()!.stepIds}>
                        {(sid) => (
                          <span
                            class={`${chipBase} ${isStepInList(editExpStepsNotReached(), sid) ? chipOn : chipOff}`}
                            onClick={() => setEditExpStepsNotReached(toggleStepInList(editExpStepsNotReached(), sid))}
                            title={isStepInList(editExpStepsNotReached(), sid) ? `Remove ${sid}` : `Expect ${sid} NOT reached`}
                          >{sid}</span>
                        )}
                      </For>
                    </div>
                  </Show>
                </div>

                <div>
                  <label class={labelCls}>Intercepted Action Counts</label>
                  <textarea
                    class={textareaCls}
                    style={{ 'min-height': '50px' }}
                    value={editExpActionCounts()}
                    onInput={e => setEditExpActionCounts(e.currentTarget.value)}
                    placeholder='{"tool_calls": 3}'
                  />
                  <p class="text-[11px] text-muted-foreground mt-1 italic">
                    Keys: tool_calls, agent_invocations, workflow_launches, scheduled_tasks, total
                  </p>
                </div>
              </div>
            </div>

            <Show when={formError()}>
              <p class="text-sm text-destructive font-medium">{formError()}</p>
            </Show>
          </DialogBody>

          <DialogFooter class="px-6 py-3 border-t border-border">
            <Button variant="outline" size="sm" onClick={closeDialog}>Cancel</Button>
            <Button size="sm" onClick={saveDialog}>
              {dialogMode() === 'new' ? 'Add Test' : 'Save Changes'}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* ═══════════════════ Result Details Dialog ═══════════════════ */}
      <TestResultDetailsDialog
        result={detailsResult()}
        open={detailsResult() !== null}
        onClose={closeDetails}
      />
    </div>
  );
}
