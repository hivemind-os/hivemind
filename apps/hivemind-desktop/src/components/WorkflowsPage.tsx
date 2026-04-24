import { For, Show, createEffect, createMemo, createSignal, onCleanup, onMount, untrack, type JSX } from 'solid-js';
import type { WorkflowStore } from '../stores/workflowStore';
import type { WorkflowStatus, StepStatus, WorkflowInstanceSummary, WorkflowDefinitionSummary, StepState, ToolDefinition, Persona, WorkflowImpactEstimate } from '../types';
import type { InteractionStore } from '../stores/interactionStore';
import WorkflowDesigner from './WorkflowDesigner';
import { YamlBlock } from './YamlHighlight';
import { pendingApprovalToasts, dismissAgentApproval, type PendingApproval } from './AgentApprovalToast';
import { invoke } from '@tauri-apps/api/core';
import { answerQuestion, respondToApproval, type PendingInteraction } from '~/lib/interactionRouting';
import { evaluateFieldCondition } from '~/lib/formConditions';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { useAbortableEffect } from '~/lib/useAbortableEffect';
import { renderMarkdown } from '~/utils';
import { EmptyState } from '~/ui/empty-state';
import { Bell, Wrench, Bot, Hand, Timer, Radio, RotateCcw, Calendar, GitBranch, Square, Play, PenTool, RefreshCw, Trash2, EyeOff, ClipboardList, Plus, ChevronRight, ChevronDown, Pause, CircleStop, TriangleAlert, Lock, HelpCircle, Rocket, Hourglass, Check, Zap, ArrowRight, ArrowLeft, Filter, Archive, ArchiveRestore, Package, Mail, Clock } from 'lucide-solid';
import WorkflowCreationWizard from './WorkflowCreationWizard';
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter } from '~/ui/dialog';
import { ConfirmDialog } from '~/ui/confirm-dialog';
import { Button } from '~/ui/button';
import { Switch, SwitchControl, SwitchThumb, SwitchLabel } from '~/ui/switch';
import { Popover, PopoverTrigger, PopoverContent } from '~/ui/popover';
import { buildNamespaceTree, flattenNamespaceTree, collectAllPaths, type NamespaceNode } from '~/lib/workflowGrouping';
import WorkflowInstanceDetail, { getStepTask, statusDotColors } from './shared/WorkflowInstanceDetail';

interface WorkflowsPageProps {
  store: WorkflowStore;
  interactionStore?: InteractionStore;
  toolDefinitions?: ToolDefinition[];
  personas?: Persona[];
  channels?: { id: string; name: string; provider?: string; hasComms?: boolean }[];
  eventTopics?: { topic: string; description: string }[];
  onApprovalClick?: (approval: PendingApproval) => void;
  onExportToKit?: (workflowName: string) => void;
}

function statusPill(status: string): string {
  switch (status) {
    case 'completed': return 'pill success';
    case 'running': return 'pill info';
    case 'paused': return 'pill warning';
    case 'waiting_on_input': case 'waiting_on_event': return 'pill warning';
    case 'failed': return 'pill danger';
    case 'killed': return 'pill danger';
    case 'pending': return 'pill neutral';
    case 'ready': return 'pill info';
    case 'skipped': return 'pill neutral';
    default: return 'pill neutral';
  }
}

function statusLabel(status: string): string {
  return status.replace(/_/g, ' ');
}

function triggerIcon(type: string) {
  switch (type) {
    case 'manual': return Hand;
    case 'schedule': case 'cron': return Calendar;
    case 'event': return Radio;
    default: return Bell;
  }
}

/** Format raw backend error messages into something user-friendly */
function formatSaveError(raw: string | null | undefined): string {
  if (!raw) return '';
  // Strip wrapping like "Error: ..." or "workflow_save_definition: ..."
  let msg = raw.replace(/^(Error:\s*|workflow_save_definition:\s*)/i, '');
  // Strip HTTP status prefix like "500 Internal Server Error: " or "400 Bad Request: "
  msg = msg.replace(/^\d{3}\s+[A-Za-z ]+:\s*/, '');

  // Improve serde_yaml / serde_json deserialization messages
  if (msg.includes('unknown field') || msg.includes('missing field')) {
    const match = msg.match(/(unknown field `([^`]+)`|missing field `([^`]+)`)/);
    if (match) {
      const field = match[2] || match[3];
      const verb = msg.includes('unknown') ? 'Unrecognized' : 'Missing required';
      return `${verb} field: "${field}"\n\nDetails:\n${msg}`;
    }
  } else if (msg.includes('unknown variant')) {
    const match = msg.match(/unknown variant `([^`]+)`/);
    if (match) {
      return `Unknown type or kind: "${match[1]}"\n\nDetails:\n${msg}`;
    }
  } else if (msg.includes('invalid type')) {
    return `Type mismatch in YAML:\n\n${msg}`;
  } else if (msg.includes('Duplicate step ID')) {
    return `Duplicate step ID found.\n\n${msg}`;
  } else if (msg.includes('Cycle detected')) {
    return `Circular dependency detected in workflow.\n\n${msg}`;
  } else if (msg.includes('No entry point')) {
    return 'Workflow has no entry point.\nAdd a trigger step to define where the workflow starts.';
  }

  return msg.trim();
}



function formatTime(ms: number): string {
  if (!ms) return '—';
  const d = new Date(ms);
  return d.toLocaleString(undefined, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' });
}

function durationStr(startMs: number, endMs: number | null | undefined, nowMs?: number): string {
  if (!startMs) return '—';
  const end = endMs || nowMs || Date.now();
  const secs = Math.floor((end - startMs) / 1000);
  if (secs < 60) return `${secs}s`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m ${secs % 60}s`;
  return `${Math.floor(secs / 3600)}h ${Math.floor((secs % 3600) / 60)}m`;
}

interface TriggerInput {
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

interface TriggerOption {
  stepId: string;
  label: string;
  triggerType: string;  // 'manual' | 'incoming_message' | 'event_pattern' | 'schedule' | etc.
  schema: TriggerInput[];
}

export default function WorkflowsPage(props: WorkflowsPageProps) {
  const store = props.store;
  const [expandedId, setExpandedId] = createSignal<number | null>(null);
  const [launchDef, setLaunchDef] = createSignal<string | null>(null);
  const [launchInputs, setLaunchInputs] = createSignal('{}');
  const [launching, setLaunching] = createSignal(false);
  const [launchResult, setLaunchResult] = createSignal<number | null>(null);
  const [launchError, setLaunchError] = createSignal<string | null>(null);
  const [launchSchema, setLaunchSchema] = createSignal<TriggerInput[]>([]);
  const [launchValues, setLaunchValues] = createSignal<Record<string, any>>({});
  const [triggerOptions, setTriggerOptions] = createSignal<TriggerOption[]>([]);
  const [selectedTrigger, setSelectedTrigger] = createSignal<TriggerOption | null>(null);
  const [testRunEnabled, setTestRunEnabled] = createSignal(false);
  const [impactEstimate, setImpactEstimate] = createSignal<WorkflowImpactEstimate | null>(null);
  const [confirmKill, setConfirmKill] = createSignal<number | null>(null);
  const [confirmReset, setConfirmReset] = createSignal<string | null>(null);
  const [confirmDelete, setConfirmDelete] = createSignal<{ name: string; version: string; triggers: any[]; scheduledTasks: any[] } | null>(null);
  const [confirmArchiveDef, setConfirmArchiveDef] = createSignal<{ name: string; version: string } | null>(null);
  const [confirmArchiveInst, setConfirmArchiveInst] = createSignal<number | null>(null);
  const [feedbackStep, setFeedbackStep] = createSignal<{ instanceId: number; stepId: string; prompt: string; choices: string[]; allow_freeform: boolean } | null>(null);
  const [feedbackText, setFeedbackText] = createSignal('');
  const [feedbackError, setFeedbackError] = createSignal<string | null>(null);
  const [saving, setSaving] = createSignal(false);
  const [now, setNow] = createSignal(Date.now());
  const [activeSection, setActiveSection] = createSignal<'instances' | 'definitions'>('instances');
  const [wizardStep, setWizardStep] = createSignal(0);

  const [agentQuestion, setAgentQuestion] = createSignal<{
    request_id: string;
    text: string;
    choices: string[];
    allow_freeform: boolean;
    multi_select?: boolean;
    agent_name?: string;
    message?: string;
    agent_id: string;
    session_id: string;
    routing?: string;
  } | null>(null);
  const [questionText, setQuestionText] = createSignal('');
  const [questionSending, setQuestionSending] = createSignal(false);

  // Inline approval dialog state
  const [wfPendingApproval, setWfPendingApproval] = createSignal<PendingInteraction | null>(null);
  const [approvalSending, setApprovalSending] = createSignal(false);

  // Workflow creation wizard
  const [showWizard, setShowWizard] = createSignal(false);
  const [designerAiPrompt, setDesignerAiPrompt] = createSignal<string | undefined>(undefined);
  const [designerLockMode, setDesignerLockMode] = createSignal(false);

  // Namespace grouping for definitions (sorted alphabetically at both levels)
  const [expandedNs, setExpandedNs] = createSignal<Set<string>>(new Set());
  const namespaceTree = () => buildNamespaceTree(store.definitions());
  const toggleNs = (ns: string) => {
    setExpandedNs(prev => {
      const next = new Set(prev);
      next.has(ns) ? next.delete(ns) : next.add(ns);
      return next;
    });
  };

  // Auto-expand all namespaces when definitions change
  createEffect(() => {
    const tree = namespaceTree();
    if (tree.length > 0) {
      setExpandedNs(collectAllPaths(tree));
    }
  });

  // Sync external "view definitions" request from sidebar gear icon
  createEffect(() => {
    if (store.viewDefinitions()) {
      setActiveSection('definitions');
      store.setViewDefinitions(false);
    }
  });

  // Live-updating timer for running workflow durations
  const elapsedTimer = setInterval(() => setNow(Date.now()), 1000);
  onCleanup(() => clearInterval(elapsedTimer));

  // Load data on mount — SSE subscription is always active (managed by App)
  onMount(() => {
    void store.refresh();
  });

  function toggleExpand(id: number) {
    if (expandedId() === id) {
      setExpandedId(null);
      store.setSelectedInstance(null);
    } else {
      setExpandedId(id);
      void store.loadInstance(id);
    }
  }

  function triggerTypeIcon(type: string): JSX.Element {
    switch (type) {
      case 'incoming_message': return <Mail size={18} />;
      case 'event_pattern': case 'event': return <Zap size={18} />;
      case 'schedule': return <Clock size={18} />;
      case 'mcp_notification': return <Bell size={18} />;
      default: return <Hand size={18} />;
    }
  }

  function triggerTypeLabel(type: string): string {
    switch (type) {
      case 'incoming_message': return 'Incoming Message';
      case 'event_pattern': case 'event': return 'Event';
      case 'schedule': return 'Schedule';
      case 'mcp_notification': return 'MCP Notification';
      case 'manual': return 'Manual';
      default: return type;
    }
  }

  async function loadLaunchSchema(defName: string) {
    try {
      const result = await store.getDefinitionParsed(defName);
      if (!result) { setLaunchSchema([]); setLaunchValues({}); setTriggerOptions([]); setSelectedTrigger(null); return; }
      const def = result.definition;
      const steps: any[] = (def.steps as any[]) || [];

      const options: TriggerOption[] = [];
      for (const step of steps) {
        if (step.type !== 'trigger') continue;
        const trigDef = step.trigger;
        if (!trigDef) continue;

        const trigType: string = trigDef.type || 'manual';
        let inputFields: TriggerInput[] = [];

        if (trigType === 'manual') {
          inputFields = buildManualTriggerFields(trigDef, def);
        } else if (trigType === 'incoming_message') {
          inputFields = buildIncomingMessageFields(trigDef);
        } else if (trigType === 'event_pattern' || trigType === 'event') {
          inputFields = buildEventPatternFields();
        } else if (trigType === 'schedule') {
          // Schedule triggers need no user input — scheduled_time is auto-injected
          inputFields = [];
        } else if (trigType === 'mcp_notification') {
          inputFields = buildMcpNotificationFields();
        } else {
          // Unknown trigger type — show a raw JSON textarea
          inputFields = [{ name: 'payload', input_type: 'string', required: false, description: `Payload for ${trigType} trigger (JSON)` }];
        }

        options.push({
          stepId: step.id,
          label: step.id,
          triggerType: trigType,
          schema: inputFields,
        });
      }

      setTriggerOptions(options);

      if (options.length > 0) {
        selectTriggerOption(options[0]);
      } else {
        setSelectedTrigger(null);
        setLaunchSchema([]);
        setLaunchValues({});
      }
    } catch {
      setLaunchSchema([]);
      setLaunchValues({});
      setTriggerOptions([]);
      setSelectedTrigger(null);
    }
  }

  /** Build input fields for manual triggers from input_schema or legacy inputs. */
  function buildManualTriggerFields(trigDef: any, def: any): TriggerInput[] {
    if (trigDef.input_schema && typeof trigDef.input_schema === 'object' && trigDef.input_schema.properties) {
      const schemaProps = trigDef.input_schema.properties;
      const schemaRequired: string[] = trigDef.input_schema.required || [];
      return Object.entries(schemaProps as Record<string, any>).map(([pName, pDef]) => ({
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
      }));
    }

    // Fall back to legacy inputs
    const fields: TriggerInput[] = (trigDef.inputs || []).map((inp: any) => ({
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
    // Merge schema from variables definition into trigger inputs (legacy behavior)
    const vars = def.variables as { properties?: Record<string, any> } | undefined;
    if (vars?.properties) {
      for (const inp of fields) {
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
    return fields;
  }

  /** Build structured fields for incoming_message triggers. */
  function buildIncomingMessageFields(trigDef: any): TriggerInput[] {
    // Pre-fill channel_id and provider from the trigger definition if available
    const channelId = trigDef.channel_id || trigDef.channel || '';
    // Derive provider from the channels list if we know the channel_id
    const channel = channelId ? props.channels?.find(c => c.id === channelId) : undefined;
    const provider = channel?.provider || '';
    return [
      { name: 'from', input_type: 'string', required: false, description: 'Sender address or name' },
      { name: 'to', input_type: 'string', required: false, description: 'Recipient address' },
      { name: 'subject', input_type: 'string', required: false, description: 'Message subject' },
      { name: 'body', input_type: 'string', required: false, description: 'Message body', xUi: { widget: 'textarea' } },
      { name: 'channel_id', input_type: 'string', required: false, description: 'Channel identifier', default: channelId || undefined },
      { name: 'provider', input_type: 'string', required: false, description: 'Connector provider (e.g. microsoft, discord)', default: provider || undefined },
      { name: 'external_id', input_type: 'string', required: false, description: 'External message ID (optional)' },
    ];
  }

  /** Build fields for event_pattern triggers — a single JSON editor for the event payload. */
  function buildEventPatternFields(): TriggerInput[] {
    return [
      { name: '_event_payload', input_type: 'string', required: false, description: 'Event payload (JSON object)', xUi: { widget: 'code-editor' } },
    ];
  }

  /** Build fields for mcp_notification triggers. */
  function buildMcpNotificationFields(): TriggerInput[] {
    return [
      { name: 'method', input_type: 'string', required: false, description: 'Notification method' },
      { name: '_notification_params', input_type: 'string', required: false, description: 'Parameters (JSON object)', xUi: { widget: 'code-editor' } },
    ];
  }

  function selectTriggerOption(opt: TriggerOption) {
    setSelectedTrigger(opt);
    setLaunchSchema(opt.schema);
    const defaults: Record<string, any> = {};
    for (const inp of opt.schema) {
      if (inp.default != null) defaults[inp.name] = inp.default;
      else if (inp.input_type === 'boolean') defaults[inp.name] = false;
      else if (inp.input_type === 'number') defaults[inp.name] = 0;
      else defaults[inp.name] = '';
    }
    setLaunchValues(defaults);
  }

  useAbortableEffect((signal) => {
    const defName = launchDef();
    if (defName) {
      setWizardStep(0);
      setLaunchError(null);
      setImpactEstimate(null);
      void loadLaunchSchema(defName).then(() => {
        if (signal.aborted) return;
      });
      // Fetch impact estimate for the launch dialog
      void store.analyzeWorkflow(defName).then((est) => {
        if (signal.aborted) return;
        setImpactEstimate(est);
        // Auto-default test run ON for high-volume or untested workflows
        const defSummary = store.definitions().find(d => d.name === defName);
        const isUntested = defSummary?.is_untested ?? false;
        if (isUntested) {
          setTestRunEnabled(true);
        }
        if (est) {
          const totalDanger = (est.totals.external_messages.min || 0)
            + (est.totals.http_calls.min || 0)
            + (est.totals.destructive_ops.min || 0);
          const hasLoopMultiplier = est.totals.external_messages.max === null
            || est.totals.http_calls.max === null
            || est.totals.destructive_ops.max === null;
          if (hasLoopMultiplier || totalDanger > 100) {
            setTestRunEnabled(true);
          }
        }
      });
    }
  });

  const canLaunch = createMemo(() => {
    const schema = launchSchema();
    const values = launchValues();
    for (const input of schema) {
      if (input.required) {
        const val = values[input.name];
        if (val == null || val === '' || val === undefined) return false;
      }
    }
    return true;
  });

  async function handleLaunch() {
    const def = launchDef();
    if (!def || launching()) return;

    const trigger = selectedTrigger();
    if (!trigger) {
      setLaunchError('No trigger selected. This workflow may not have any launchable triggers.');
      return;
    }

    console.log('[workflow] handleLaunch called for:', def, 'trigger:', trigger.stepId, 'type:', trigger.triggerType);
    setLaunching(true);
    setLaunchResult(null);
    setLaunchError(null);

    // Build inputs from form values
    let inputs: any = {};
    const schema = launchSchema();
    if (schema.length > 0) {
      inputs = { ...launchValues() };
    } else {
      try { inputs = JSON.parse(launchInputs()); } catch { /* empty */ }
    }

    // For event_pattern: parse the _event_payload JSON field into a flat object
    if (trigger.triggerType === 'event_pattern' || trigger.triggerType === 'event') {
      const raw = inputs._event_payload;
      delete inputs._event_payload;
      if (raw && typeof raw === 'string') {
        try { inputs = { ...inputs, ...JSON.parse(raw) }; } catch { /* leave as-is */ }
      }
    }

    // For mcp_notification: parse the _notification_params JSON field
    if (trigger.triggerType === 'mcp_notification') {
      const raw = inputs._notification_params;
      delete inputs._notification_params;
      if (raw && typeof raw === 'string') {
        try { inputs = { ...inputs, ...JSON.parse(raw) }; } catch { /* leave as-is */ }
      }
    }

    // For schedule: inject scheduled_time
    if (trigger.triggerType === 'schedule') {
      inputs.scheduled_time = new Date().toISOString();
    }

    // Auto-fill timestamp for incoming_message if not provided
    if (trigger.triggerType === 'incoming_message' && !inputs.timestamp_ms) {
      inputs.timestamp_ms = Date.now();
    }

    const executionMode = testRunEnabled() ? 'shadow' : 'normal';
    const isSimulated = trigger.triggerType !== 'manual';

    console.log('[workflow] launching with inputs:', inputs, 'triggerStepId:', trigger.stepId, 'simulated:', isSimulated);
    let timeoutId: ReturnType<typeof setTimeout> | undefined;
    try {
      let launchPromise: Promise<number | null>;
      if (isSimulated) {
        // Non-manual triggers use the simulate-trigger API
        launchPromise = store.simulateTrigger(def, trigger.stepId, inputs, undefined, executionMode as any)
          .then(r => r?.instance_id ?? null);
      } else {
        launchPromise = store.launchWorkflow(def, inputs, 'manual', trigger.stepId, executionMode === 'shadow' ? 'shadow' : undefined);
      }
      const timeoutPromise = new Promise<null>((resolve) => {
        timeoutId = setTimeout(() => resolve(null), 30_000);
      });
      const instanceId = await Promise.race([launchPromise, timeoutPromise]);
      if (timeoutId) clearTimeout(timeoutId);
      // Stale check: if launchDef changed during the await, discard result
      if (launchDef() !== def) return;
      setLaunching(false);
      if (instanceId != null) {
        setLaunchResult(instanceId);
        setActiveSection('instances');
      } else {
        const err = store.error() || 'Launch timed out or failed — check that the daemon is running.';
        console.error('[workflow] launch failed:', err);
        setLaunchError(err);
      }
    } catch (e: any) {
      if (timeoutId) clearTimeout(timeoutId);
      console.error('[workflow] launch exception:', e);
      setLaunching(false);
      setLaunchError(e?.toString() ?? 'Unexpected error launching workflow.');
    }
  }

  function dismissWizard() {
    setLaunchDef(null);
    setLaunchInputs('{}');
    setLaunchResult(null);
    setLaunchError(null);
    setLaunchSchema([]);
    setLaunchValues({});
    setTriggerOptions([]);
    setSelectedTrigger(null);
    setTestRunEnabled(false);
  }

  async function handleKillConfirmed(id: number) {
    await store.killInstance(id);
    setConfirmKill(null);
  }

  async function handleDeleteClick(name: string, version: string) {
    const deps = await store.checkDefinitionDependents(name, version);
    const triggers = deps?.triggers ?? [];
    const tasks = deps?.scheduled_tasks ?? [];
    setConfirmDelete({ name, version, triggers, scheduledTasks: tasks });
  }

  async function handleDeleteConfirmed() {
    const info = confirmDelete();
    if (!info) return;
    setConfirmDelete(null);
    await store.deleteDefinition(info.name, info.version);
  }

  function openFeedbackGate(instanceId: number, step: any, state: StepState) {
    if (state?.status !== 'waiting_on_input') return;
    // Prefer resolved values from step state (templates already resolved by executor)
    const prompt = state.interaction_prompt
      ?? (() => { const task = getStepTask(step); const gate = task?.FeedbackGate ?? task?.feedback_gate ?? task; return gate?.prompt; })()
      ?? 'Please provide your response:';
    const choices = state.interaction_choices
      ?? (() => { const task = getStepTask(step); const gate = task?.FeedbackGate ?? task?.feedback_gate ?? task; return gate?.choices; })()
      ?? [];
    const allow_freeform = state.interaction_allow_freeform
      ?? (() => { const task = getStepTask(step); const gate = task?.FeedbackGate ?? task?.feedback_gate ?? task; return gate?.allow_freeform ?? gate?.allow_freeform; })()
      ?? true;
    setFeedbackStep({ instanceId, stepId: step.id, prompt, choices, allow_freeform });
    setFeedbackText('');
    setFeedbackError(null);
  }

  async function submitFeedback(choice?: string) {
    const gate = feedbackStep();
    if (!gate) return;
    setFeedbackError(null);
    const response = choice
      ? { selected: choice, text: feedbackText() }
      : { selected: feedbackText(), text: feedbackText() };
    try {
      await store.respondToGate(gate.instanceId, gate.stepId, response);
      setFeedbackStep(null);
      setFeedbackText('');
      await store.refresh();
    } catch (e: any) {
      setFeedbackError(e?.message ?? String(e) ?? 'Failed to submit feedback');
    }
  }

  async function openAgentQuestion(inst: WorkflowInstanceSummary) {
    const childIds = inst.child_agent_ids ?? [];
    if (childIds.length === 0) return;
    try {
      const allQuestions = await invoke<Array<any>>('list_all_pending_questions');
      const match = allQuestions.find((q: any) => q.agent_id && childIds.includes(q.agent_id));
      if (match) {
        setAgentQuestion({
          request_id: match.request_id,
          text: match.text,
          choices: match.choices ?? [],
          allow_freeform: match.allow_freeform !== false,
          agent_name: match.agent_name,
          message: match.message,
          agent_id: match.agent_id,
          session_id: match.session_id ?? inst.parent_session_id,
          routing: match.routing,
        });
        setQuestionText('');
        setQuestionSending(false);
      }
    } catch (err) {
      console.error('Failed to fetch pending questions:', err);
    }
  }

  async function submitAgentAnswer(choiceIdx?: number, text?: string, selected_choices?: number[]) {
    const q = agentQuestion();
    if (!q) return;
    setQuestionSending(true);
    try {
      await answerQuestion(
        {
          request_id: q.request_id,
          entity_id: `agent/${q.agent_id}`,
          source_name: q.agent_name ?? '',
          type: 'question',
          routing: q.routing as any,
          session_id: q.session_id,
          agent_id: q.agent_id,
        } as PendingInteraction,
        {
          ...(choiceIdx !== undefined ? { selected_choice: choiceIdx } : {}),
          ...(selected_choices !== undefined ? { selected_choices } : {}),
          ...(text ? { text } : {}),
        },
      );
      setAgentQuestion(null);
      setQuestionText('');
      await store.refresh();
      props.interactionStore?.poll();
    } catch (err) {
      console.error('Failed to answer question:', err);
      setQuestionSending(false);
    }
  }

  function openApprovalDialog(inst: WorkflowInstanceSummary) {
    const childIds = inst.child_agent_ids ?? [];
    // Search interaction store by child agent entity IDs (interactions are keyed by agent, not workflow)
    const allInteractions = props.interactionStore?.interactions() ?? [];
    const approval = allInteractions.find(
      i => i.type === 'tool_approval' && i.agent_id && childIds.includes(i.agent_id)
    );
    if (approval) {
      setWfPendingApproval(approval);
      return;
    }
    // Fallback: look up from toast data (omit sentinel session_id to let routing fallback work)
    const toast = pendingApprovalToasts().find(a => childIds.includes(a.agent_id));
    if (toast) {
      setWfPendingApproval({
        request_id: toast.request_id,
        entity_id: `agent/${toast.agent_id}`,
        source_name: toast.agent_name || toast.agent_id,
        type: 'tool_approval',
        agent_id: toast.agent_id,
        tool_id: toast.tool_id,
        input: toast.input,
        reason: toast.reason,
      } as PendingInteraction);
    }
  }

  async function submitApproval(approved: boolean, opts?: { allow_agent?: boolean; allow_session?: boolean }) {
    const a = wfPendingApproval();
    if (!a) return;
    setApprovalSending(true);
    try {
      await respondToApproval(a, { approved, allow_agent: opts?.allow_agent, allow_session: opts?.allow_session });
      dismissAgentApproval(a.request_id);
      setWfPendingApproval(null);
      await store.refresh();
      props.interactionStore?.poll();
    } catch (err) {
      console.error('Failed to respond to approval:', err);
    } finally {
      setApprovalSending(false);
    }
  }

  const ALL_STATUSES = ['pending','running','paused','waiting_on_input','waiting_on_event','completed','failed','killed'] as const;
  const ACTIVE_STATUSES = ['pending','running','paused','waiting_on_input','waiting_on_event'] as const;
  const TERMINAL_STATUSES = ['completed','failed','killed'] as const;

  const activeFilterCount = createMemo(() => {
    const sf = store.statusFilter();
    const uncheckedStatuses = ALL_STATUSES.filter(s => !sf[s]).length;
    const df = store.definitionFilter();
    const defEntries = Object.entries(df);
    const uncheckedDefs = defEntries.filter(([_, v]) => !v).length;
    return uncheckedStatuses + uncheckedDefs;
  });

  function renderNamespaceNode(node: NamespaceNode<WorkflowDefinitionSummary>, depth: number): JSX.Element {
    return (
      <>
        <div class="wf-ns-header" style={`display: flex; align-items: center; gap: 8px; padding: 8px 4px 4px; padding-left: ${depth * 16 + 4}px; cursor: pointer; user-select: none; grid-column: 1 / -1;`} onClick={() => toggleNs(node.fullPath)}>
          <ChevronDown size={14} style={`transform: ${expandedNs().has(node.fullPath) ? '' : 'rotate(-90deg)'}; transition: transform 0.15s;`} />
          <span style="font-weight: 600; font-size: 0.88em;">{node.segment}</span>
          <Show when={node.items.length > 0}>
            <span style="font-size: 0.72em; color: var(--muted-foreground, #888); background: var(--muted, rgba(128,128,128,0.15)); padding: 1px 7px; border-radius: 8px;">{node.items.length}</span>
          </Show>
        </div>
        <Show when={expandedNs().has(node.fullPath)}>
          <For each={node.items}>
            {(def) => (
              <div class="wf-def-card">
                <Show when={def.bundled}>
                  <div class="wf-def-card-bundled">Built-in</div>
                </Show>
                <div class="wf-def-card-header">
                  <div class={`wf-def-card-icon${def.mode === 'chat' ? ' chat' : ''}`}>
                    {def.mode === 'chat' ? <Bot size={20} /> : <Zap size={20} />}
                  </div>
                  <div class="wf-def-card-info">
                    <div class="wf-def-card-name">
                      {def.name}
                      <span class="wf-def-card-version">v{def.version}</span>
                    </div>
                    <Show when={def.description}>
                      <div class="wf-def-card-desc">{def.description}</div>
                    </Show>
                  </div>
                </div>
                <div class="wf-def-card-badges">
                  <For each={def.trigger_types ?? []}>
                    {(tt) => {
                      const Icon = triggerIcon(tt);
                      return (
                        <span class="wf-trigger-pill" classList={{ 'wf-trigger-pill-paused': tt !== 'manual' && !!def.triggers_paused }}>
                          <Icon size={10} /> {tt}
                        </span>
                      );
                    }}
                  </For>
                  <span class="wf-step-count">
                    <ClipboardList size={10} /> {def.step_count} steps
                  </span>
                </div>
                <div class="wf-def-card-actions">
                  <Show when={(def.trigger_types ?? []).some(tt => tt !== 'manual')}>
                    <button
                      class="icon-btn"
                      classList={{ 'wf-triggers-paused': !!def.triggers_paused }}
                      title={def.triggers_paused ? 'Resume auto-triggers' : 'Pause auto-triggers'}
                      aria-label={def.triggers_paused ? 'Resume auto-triggers' : 'Pause auto-triggers'}
                      onClick={() => void store.setTriggersPaused(def.name, def.version, !def.triggers_paused)}
                    >
                      {def.triggers_paused ? <Play size={14} /> : <Pause size={14} />}
                    </button>
                  </Show>
                  <Show when={props.onExportToKit && !def.name.startsWith('system/')}>
                    <button class="icon-btn" title="Export as Agent Kit" aria-label="Export as Agent Kit" onClick={() => props.onExportToKit?.(def.name)}>
                      <Package size={14} />
                    </button>
                  </Show>
                  <button class="icon-btn" title="Edit in Designer" data-testid="wf-edit-btn" aria-label="Edit in designer" onClick={() => { setDesignerLockMode(true); setDesignerAiPrompt(undefined); void store.openDesigner(def.name, def.version); }}>
                    <PenTool size={14} />
                  </button>
                  <Show when={def.bundled}>
                    <button class="icon-btn" title="Reset to factory defaults" aria-label="Reset workflow" onClick={() => setConfirmReset(def.name)}>
                      <RotateCcw size={14} />
                    </button>
                  </Show>
                  {def.bundled ? (
                    <button class="icon-btn" title="Hide" aria-label="Hide workflow" onClick={() => setConfirmArchiveDef({ name: def.name, version: def.version })}>
                      <EyeOff size={14} />
                    </button>
                  ) : (
                    <button class="icon-btn" title="Delete" data-testid="wf-delete-btn" aria-label="Delete workflow" onClick={() => void handleDeleteClick(def.name, def.version)}>
                      <Trash2 size={14} />
                    </button>
                  )}
                  <button class="launch-btn" data-testid="wf-launch-btn" aria-label="Launch workflow" onClick={() => setLaunchDef(def.name)}>
                    <Play size={12} /> Launch
                  </button>
                </div>
              </div>
            )}
          </For>
          <For each={node.children}>
            {(child) => renderNamespaceNode(child, depth + 1)}
          </For>
        </Show>
      </>
    );
  }

  return (
    <div class="flex flex-col h-full overflow-hidden">
      {/* Header */}
      <div class="wf-header">
        <Show when={activeSection() === 'definitions'}>
          <button class="wf-header-back" aria-label="Back to instances" onClick={() => setActiveSection('instances')}>
            <ArrowLeft size={16} />
          </button>
        </Show>
        <div class="wf-header-title">
          <Zap size={22} />
          <Show when={activeSection() === 'instances'} fallback="Definitions">
            Workflows
          </Show>
        </div>
        <div class="wf-header-badges">
          <Show when={store.activeCount() > 0}>
            <span class="wf-status-chip active">{store.activeCount()} active</span>
          </Show>
          <Show when={store.waitingCount() > 0}>
            <span class="wf-status-chip waiting">{store.waitingCount()} waiting</span>
          </Show>
        </div>
        <div class="wf-header-actions">
          <Show when={activeSection() === 'instances'}>
            <Popover>
              <PopoverTrigger class="wf-header-btn" aria-label="Filter workflows">
                <Filter size={14} />
                <Show when={activeFilterCount() > 0}>
                  <span class="wf-filter-badge">{activeFilterCount()}</span>
                </Show>
              </PopoverTrigger>
              <PopoverContent class="wf-filter-popover">
                <div class="wf-filter-popover-section">
                  <div class="wf-filter-popover-label">Active</div>
                  <For each={[...ACTIVE_STATUSES]}>
                    {(status) => (
                      <label class="wf-filter-popover-item">
                        <input
                          type="checkbox"
                          checked={store.statusFilter()[status] ?? false}
                          onChange={() => store.toggleStatus(status)}
                        />
                        <span class="wf-filter-popover-dot" style={`background:${statusDotColors[status] ?? '#94a3b8'}`} />
                        {statusLabel(status)}
                      </label>
                    )}
                  </For>
                </div>
                <div class="wf-filter-popover-section">
                  <div class="wf-filter-popover-label">Terminal</div>
                  <For each={[...TERMINAL_STATUSES]}>
                    {(status) => (
                      <label class="wf-filter-popover-item">
                        <input
                          type="checkbox"
                          checked={store.statusFilter()[status] ?? false}
                          onChange={() => store.toggleStatus(status)}
                        />
                        <span class="wf-filter-popover-dot" style={`background:${statusDotColors[status] ?? '#94a3b8'}`} />
                        {statusLabel(status)}
                      </label>
                    )}
                  </For>
                </div>
                <Show when={store.definitions().length > 0}>
                  <div class="wf-filter-popover-section">
                    <div class="wf-filter-popover-label">Workflow</div>
                    <For each={flattenNamespaceTree(buildNamespaceTree(store.definitions()))}>
                      {([ns, defs]) => (
                        <div style="margin-bottom: 4px;">
                          <div style="font-size: 0.78em; font-weight: 600; color: var(--muted-foreground, #888); padding: 2px 0; text-transform: uppercase; letter-spacing: 0.04em;">{ns}</div>
                          <For each={defs}>
                            {(def) => (
                              <label class="wf-filter-popover-item">
                                <input
                                  type="checkbox"
                                  checked={store.definitionFilter()[def.name] ?? true}
                                  onChange={() => store.toggleDefinition(def.name)}
                                />
                                {def.name}
                              </label>
                            )}
                          </For>
                        </div>
                      )}
                    </For>
                  </div>
                </Show>
                <div class="wf-filter-popover-section">
                  <label class="wf-filter-popover-item">
                    <input type="checkbox" checked={store.showArchived()} onChange={() => store.toggleShowArchived()} />
                    Show archived
                  </label>
                </div>
              </PopoverContent>
            </Popover>
          </Show>
          <Show when={activeSection() === 'definitions'}>
            <button class="wf-header-btn" data-testid="wf-new-definition-btn" aria-label="New workflow definition" onClick={() => setShowWizard(true)}>
              <Plus size={14} /> New
            </button>
          </Show>
          <button class="wf-header-btn" data-testid="wf-refresh-btn" aria-label="Refresh workflows" onClick={() => void store.refresh()}>
            <RefreshCw size={14} />
          </button>
        </div>
      </div>

      {/* Error Dialog */}
      <Dialog open={!!store.error()} onOpenChange={(open) => { if (!open) store.setError(null); }}>
        <DialogContent class="max-w-[560px] min-w-[340px]">
            <DialogHeader>
              <DialogTitle class="flex items-center gap-2 text-destructive">
                <TriangleAlert size={14} /> Save Failed
              </DialogTitle>
            </DialogHeader>
            <pre class="bg-background text-foreground p-3 rounded-md text-[0.82em] whitespace-pre-wrap break-words max-h-[300px] overflow-y-auto m-0">{formatSaveError(store.error())}</pre>
            <DialogFooter>
              <Button onClick={() => store.setError(null)}>OK</Button>
            </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* Definitions Card Grid */}
      <Show when={activeSection() === 'definitions'}>
        <div class="wf-def-grid">
          <Show when={store.definitions().length === 0}>
            <EmptyState
              title="No workflow definitions yet"
              description="Create one with the New button or open the Visual Designer."
              compact
            />
          </Show>
          <For each={namespaceTree()}>
            {(node) => renderNamespaceNode(node, 0)}
          </For>
        </div>
      </Show>

      {/* Instance Timeline */}
      <Show when={activeSection() === 'instances'}>
        <Show when={store.instances().length === 0}>
          <div class="flex-1 flex items-center justify-center">
            <EmptyState title="No workflow instances found" compact />
          </div>
        </Show>
        <div class="wf-timeline flex-1">
          <For each={store.instances()}>
            {(inst) => {
              const isExpanded = () => expandedId() === inst.id;
              const progressPct = () => {
                const total = inst.step_count ?? 0;
                const done = inst.steps_completed ?? 0;
                return total > 0 ? Math.round((done / total) * 100) : 0;
              };
              const progressClass = () => {
                if (inst.status === 'completed') return 'completed';
                if (inst.status === 'failed' || inst.status === 'killed') return 'failed';
                if (inst.status === 'paused' || inst.status === 'waiting_on_input' || inst.status === 'waiting_on_event') return 'waiting';
                return 'running';
              };
              return (
                <div class="wf-timeline-item">
                  <div class={`wf-timeline-dot ${inst.status}`} />
                  <div class={`wf-timeline-card${isExpanded() ? ' expanded' : ''}`}>
                    <div class="wf-timeline-summary" onClick={() => toggleExpand(inst.id)}>
                      <div class="wf-timeline-meta">
                        <div class="wf-timeline-meta-row">
                          <span class="wf-timeline-name">{inst.definition_name}</span>
                          <span class={statusPill(inst.status)} style="font-size:0.68em;flex-shrink:0;">
                            {statusLabel(inst.status)}
                          </span>
                          <Show when={(inst as any).execution_mode === 'shadow'}>
                            <span class="wf-test-badge">TEST</span>
                          </Show>
                          <Show when={inst.status === 'waiting_on_input'}>
                            <Bell size={13} style="color:#fbbf24;animation:pulse 2s infinite;" />
                          </Show>
                          <div class="wf-timeline-badges">
                            {(() => {
                              const c = props.interactionStore?.badgeCountForEntity(`workflow/${inst.id}`);
                              const approvals = Math.max(inst.pending_agent_approvals ?? 0, c?.approvals ?? 0);
                              const questions = Math.max(inst.pending_agent_questions ?? 0, c?.questions ?? 0);
                              return <>
                                <Show when={approvals > 0}>
                                  <span
                                    class="wf-alert-badge approval"
                                    title="Child agents need tool approval — click to review"
                                    onClick={(e: MouseEvent) => {
                                      e.stopPropagation();
                                      openApprovalDialog(inst);
                                    }}
                                  >
                                    <Lock size={11} /> {approvals}
                                  </span>
                                </Show>
                                <Show when={questions > 0}>
                                  <span
                                    class="wf-alert-badge question"
                                    title="Child agents have pending questions — click to answer"
                                    onClick={(e: MouseEvent) => {
                                      e.stopPropagation();
                                      void openAgentQuestion(inst);
                                    }}
                                  >
                                    <HelpCircle size={11} /> {questions}
                                  </span>
                                </Show>
                              </>;
                            })()}
                          </div>
                        </div>
                        <span class="wf-timeline-sub">
                          {inst.id} • v{inst.definition_version} • {formatTime(inst.created_at_ms)}
                        </span>
                      </div>

                      <Show when={(inst.step_count ?? 0) > 0}>
                        <div class="wf-progress-wrap">
                          <div class="wf-progress-bar">
                            <div class={`wf-progress-fill ${progressClass()}`} style={`width:${progressPct()}%`} />
                          </div>
                          <span class="wf-progress-label">{inst.steps_completed ?? 0}/{inst.step_count}</span>
                        </div>
                      </Show>

                      <span class="wf-timeline-duration">
                        {durationStr(inst.created_at_ms, inst.completed_at_ms, now())}
                      </span>

                      <div class="wf-timeline-actions" onClick={(e: Event) => e.stopPropagation()}>
                        <Show when={inst.status === 'running' || inst.status === 'waiting_on_input' || inst.status === 'waiting_on_event'}>
                          <button class="icon-btn" title="Pause" onClick={() => void store.pauseInstance(inst.id)}><Pause size={14} /></button>
                        </Show>
                        <Show when={inst.status === 'paused'}>
                          <button class="icon-btn" title="Resume" onClick={() => void store.resumeInstance(inst.id)}><Play size={14} /></button>
                        </Show>
                        <Show when={['running', 'paused', 'waiting_on_input', 'waiting_on_event', 'pending'].includes(inst.status)}>
                          <button class="icon-btn" title="Kill" style="color:hsl(var(--destructive));" onClick={() => setConfirmKill(inst.id)}><CircleStop size={14} /></button>
                        </Show>
                        <Show when={['completed', 'failed', 'killed'].includes(inst.status) && !inst.archived}>
                          <button class="icon-btn" title="Archive" onClick={() => setConfirmArchiveInst(inst.id)}><Archive size={14} /></button>
                        </Show>
                        <Show when={inst.archived}>
                          <button class="icon-btn" title="Unarchive" onClick={() => void store.archiveInstance(inst.id, false)}><ArchiveRestore size={14} /></button>
                        </Show>
                      </div>

                      <ChevronRight size={14} class={`wf-timeline-chevron${isExpanded() ? ' open' : ''}`} />
                    </div>

                    {/* Expanded Detail */}
                    <Show when={isExpanded() && store.selectedInstance()}>
                      {(_) => {
                        const detail = () => store.selectedInstance()!;
                        return (
                          <div class="wf-timeline-detail">
                            <WorkflowInstanceDetail
                              detail={detail()}
                              onOpenFeedbackGate={openFeedbackGate}
                              fetchInterceptedActions={store.fetchInterceptedActions}
                              fetchShadowSummary={store.fetchShadowSummary}
                            />
                          </div>
                        );
                      }}
                    </Show>
                  </div>
                </div>
              );
            }}
          </For>
        </div>

        {/* Pagination */}
        <Show when={store.totalPages() > 1}>
          <div class="wf-pagination">
            <button class="icon-btn" data-testid="wf-page-prev" aria-label="Previous page" disabled={store.page() === 0} onClick={() => store.prevPage()}>← Prev</button>
            <span class="text-xs text-muted-foreground">
              Page {store.page() + 1} of {store.totalPages()} ({store.totalCount()} total)
            </span>
            <button class="icon-btn" data-testid="wf-page-next" aria-label="Next page" disabled={store.page() >= store.totalPages() - 1} onClick={() => store.nextPage()}>Next →</button>
          </div>
        </Show>
      </Show>

      {/* YAML Editor Dialog */}
      <Dialog open={store.showEditor()} onOpenChange={(open) => { if (!open) store.setShowEditor(false); }}>
        <DialogContent class="w-[600px] max-h-[80vh] flex flex-col gap-3 bg-card border border-border">
          <DialogHeader>
            <DialogTitle>New Workflow Definition</DialogTitle>
          </DialogHeader>
          <textarea
            class="bg-background text-foreground border border-border rounded-md p-2.5 font-mono text-[0.85em] resize-y min-h-[300px] flex-1"
            placeholder="Paste workflow YAML here..."
            data-testid="wf-yaml-editor"
            aria-label="YAML editor"
            value={store.yamlEditor()}
            onInput={(e) => store.setYamlEditor(e.currentTarget.value)}
          />
          <DialogFooter class="flex-row justify-end gap-2">
            <Button variant="outline" data-testid="wf-yaml-cancel-btn" aria-label="Cancel editing" onClick={() => store.setShowEditor(false)}>Cancel</Button>
            <Button data-testid="wf-yaml-save-btn" aria-label="Save definition" onClick={() => void store.saveDefinition(store.yamlEditor())} disabled={!store.yamlEditor().trim()}>Save Definition</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* Launch Wizard Overlay */}
      <Show when={launchDef()}>
        <div class="wf-wizard-overlay" onKeyDown={(e) => { if (e.key === 'Escape') dismissWizard(); }}>
          <div class="wf-wizard">
            <div class="wf-wizard-header">
              <div class="wf-wizard-title">Launch: {launchDef()}</div>
              <div class="wf-wizard-subtitle">Configure and launch your workflow</div>
            </div>

            {/* Step indicator */}
            <Show when={!launchResult()}>
              <div class="wf-wizard-steps">
                <div class={`wf-wizard-step-dot${wizardStep() === 0 ? ' active' : (wizardStep() > 0 ? ' done' : '')}`} />
                <div class={`wf-wizard-step-line${wizardStep() > 0 ? ' done' : ''}`} />
                <div class={`wf-wizard-step-dot${wizardStep() === 1 ? ' active' : (wizardStep() > 1 ? ' done' : '')}`} />
                <div class={`wf-wizard-step-line${wizardStep() > 1 ? ' done' : ''}`} />
                <div class={`wf-wizard-step-dot${wizardStep() === 2 ? ' active' : ''}`} />
              </div>
            </Show>

            {/* Success state */}
            <Show when={launchResult()}>
              <div class="wf-wizard-body">
                <div class="wf-wizard-success">
                  <div class="success-icon"><Check size={28} /></div>
                  <h3>Workflow Launched!</h3>
                  <p class="text-sm text-muted-foreground">Instance: <code>{launchResult()!}</code></p>
                </div>
              </div>
              <div class="wf-wizard-nav" style="justify-content: center;">
                <button class="wf-btn-next" onClick={dismissWizard}>
                  <Check size={14} /> Done
                </button>
              </div>
            </Show>

            <Show when={!launchResult()}>
              {/* Step 0: Trigger selection (skip if only one) */}
              <Show when={wizardStep() === 0}>
                <div class="wf-wizard-body">
                  <Show when={triggerOptions().length > 1} fallback={
                    <Show when={triggerOptions().length === 1} fallback={
                      <div>
                        <h3>No triggers found</h3>
                        <p class="text-sm text-muted-foreground">This workflow has no triggers configured.</p>
                      </div>
                    }>
                      <div>
                        <h3>{triggerTypeLabel(triggerOptions()[0].triggerType)} trigger</h3>
                        <p class="text-sm text-muted-foreground">
                          {launchSchema().length > 0
                            ? `Configure ${launchSchema().length} input${launchSchema().length > 1 ? 's' : ''} for this trigger.`
                            : triggerOptions()[0].triggerType === 'schedule'
                              ? 'This will simulate a scheduled trigger firing now.'
                              : 'This workflow has no required inputs.'}
                        </p>
                      </div>
                    </Show>
                  }>
                    <h3>Choose a trigger</h3>
                    <div class="wf-trigger-cards">
                      <For each={triggerOptions()}>
                        {(opt) => (
                          <button
                            class={`wf-trigger-card${selectedTrigger()?.stepId === opt.stepId ? ' selected' : ''}`}
                            onClick={() => selectTriggerOption(opt)}
                          >
                            <div class="wf-trigger-card-icon">{triggerTypeIcon(opt.triggerType)}</div>
                            <div>
                              <div class="font-semibold">{opt.label}</div>
                              <div class="text-xs text-muted-foreground">{triggerTypeLabel(opt.triggerType)}{opt.schema.length > 0 ? ` · ${opt.schema.length} field${opt.schema.length !== 1 ? 's' : ''}` : ''}</div>
                            </div>
                          </button>
                        )}
                      </For>
                    </div>
                  </Show>
                </div>
              </Show>

              {/* Step 1: Fill inputs */}
              <Show when={wizardStep() === 1}>
                <div class="wf-wizard-body">
                  <Show when={launchSchema().length > 0} fallback={
                    <div>
                      <h3>Inputs (JSON)</h3>
                      <p class="text-xs text-muted-foreground mb-2">No schema defined — enter raw JSON or leave empty.</p>
                      <textarea
                        class="wf-launch-input"
                        style="font-family:monospace;min-height:100px;resize:vertical;"
                        value={launchInputs()}
                        onInput={(e) => setLaunchInputs(e.currentTarget.value)}
                        disabled={launching()}
                      />
                    </div>
                  }>
                    <h3>Configure inputs</h3>
                    <div class="wf-wizard-fields">
                      <For each={launchSchema()}>
                        {(input) => (
                          <Show when={evaluateFieldCondition((input as any).xUi?.condition, launchValues())}>
                          <div class="wf-wizard-field">
                            <label>
                              {input.name}
                              <Show when={input.required}>
                                <span class="required-star">*</span>
                              </Show>
                            </label>
                            <Show when={input.description}>
                              <div class="field-desc">{input.description}</div>
                            </Show>
                            {(() => {
                              const widget = (input as any).xUi?.widget;
                              if (widget === 'textarea' || widget === 'code-editor') {
                                return (
                                  <textarea
                                    value={String(launchValues()[input.name] ?? '')}
                                    placeholder={input.description || input.name}
                                    maxLength={input.maxLength}
                                    rows={(input as any).xUi?.rows ?? 4}
                                    onInput={(e) => setLaunchValues(prev => ({...prev, [input.name]: e.currentTarget.value}))}
                                    disabled={launching()}
                                    style={widget === 'code-editor' ? 'font-family:monospace;' : undefined}
                                  />
                                );
                              }
                              if (widget === 'password') {
                                return <input type="password" value={String(launchValues()[input.name] ?? '')} placeholder={input.description || input.name} maxLength={input.maxLength} onInput={(e) => setLaunchValues(prev => ({...prev, [input.name]: e.currentTarget.value}))} disabled={launching()} />;
                              }
                              if (widget === 'date') {
                                return <input type="date" value={String(launchValues()[input.name] ?? '')} onInput={(e) => setLaunchValues(prev => ({...prev, [input.name]: e.currentTarget.value}))} disabled={launching()} />;
                              }
                              if (widget === 'color-picker') {
                                return <input type="color" value={String(launchValues()[input.name] ?? '#000000')} onInput={(e) => setLaunchValues(prev => ({...prev, [input.name]: e.currentTarget.value}))} disabled={launching()} style="width:48px;height:28px;padding:2px;cursor:pointer;border:1px solid hsl(var(--border));border-radius:4px;" />;
                              }
                              if (widget === 'slider' && input.input_type === 'number') {
                                return (
                                  <div class="flex items-center gap-2">
                                    <input type="range" value={launchValues()[input.name] ?? input.minimum ?? 0} min={input.minimum ?? 0} max={input.maximum ?? 100} step={(input as any).xUi?.step ?? 1} onInput={(e) => setLaunchValues(prev => ({...prev, [input.name]: Number(e.currentTarget.value)}))} disabled={launching()} style="flex:1;" />
                                    <span class="text-sm text-foreground min-w-[28px] text-right">{launchValues()[input.name] ?? input.minimum ?? 0}</span>
                                  </div>
                                );
                              }
                              if (input.enum && input.enum.length > 0) {
                                return (
                                  <select value={String(launchValues()[input.name] ?? '')} onChange={(e) => setLaunchValues(prev => ({...prev, [input.name]: e.currentTarget.value}))} disabled={launching()}>
                                    <option value="">— select —</option>
                                    <For each={input.enum}>{(opt) => <option value={opt}>{opt}</option>}</For>
                                  </select>
                                );
                              }
                              if (input.input_type === 'boolean') {
                                return (
                                  <Switch checked={!!launchValues()[input.name]} onChange={(checked) => setLaunchValues(prev => ({...prev, [input.name]: checked}))} disabled={launching()} class="flex items-center gap-2">
                                    <SwitchControl><SwitchThumb /></SwitchControl>
                                    <SwitchLabel>{input.name}</SwitchLabel>
                                  </Switch>
                                );
                              }
                              if (input.input_type === 'number') {
                                return <input type="number" value={launchValues()[input.name] ?? ''} min={input.minimum} max={input.maximum} onInput={(e) => setLaunchValues(prev => ({...prev, [input.name]: Number(e.currentTarget.value)}))} disabled={launching()} />;
                              }
                              return <input type="text" value={String(launchValues()[input.name] ?? '')} placeholder={input.description || input.name} maxLength={input.maxLength} onInput={(e) => setLaunchValues(prev => ({...prev, [input.name]: e.currentTarget.value}))} disabled={launching()} />;
                            })()}
                          </div>
                          </Show>
                        )}
                      </For>
                    </div>
                  </Show>
                </div>
              </Show>

              {/* Step 2: Review & Launch */}
              <Show when={wizardStep() === 2}>
                <div class="wf-wizard-body">
                  <h3>Review & Launch</h3>
                  <div class="wf-wizard-review">
                    <div class="wf-wizard-review-row">
                      <span class="review-key">Workflow</span>
                      <span class="review-val">{launchDef()}</span>
                    </div>
                    <Show when={selectedTrigger()}>
                      <div class="wf-wizard-review-row">
                        <span class="review-key">Trigger</span>
                        <span class="review-val">{selectedTrigger()!.label} ({triggerTypeLabel(selectedTrigger()!.triggerType)})</span>
                      </div>
                    </Show>
                    <Show when={launchSchema().length > 0}>
                      <For each={launchSchema()}>
                        {(input) => (
                          <div class="wf-wizard-review-row">
                            <span class="review-key">{input.name}</span>
                            <span class="review-val">{String(launchValues()[input.name] ?? '—')}</span>
                          </div>
                        )}
                      </For>
                    </Show>
                  </div>

                  {/* Impact Estimate Preview */}
                  <Show when={impactEstimate()}>
                    {(est) => {
                      const totals = est().totals;
                      const hasImpact = totals.external_messages.min > 0
                        || totals.http_calls.min > 0
                        || totals.agent_invocations.min > 0
                        || totals.destructive_ops.min > 0
                        || totals.scheduled_tasks.min > 0;
                      const hasLoopMultiplier = totals.external_messages.max === null
                        || totals.http_calls.max === null
                        || totals.destructive_ops.max === null;
                      const isHighVolume = hasLoopMultiplier
                        || (totals.external_messages.min + totals.http_calls.min + totals.destructive_ops.min) > 100;
                      return (
                        <Show when={hasImpact}>
                          <div class={`wf-impact-preview ${isHighVolume ? 'wf-impact-warning' : ''}`}>
                            <div class="wf-impact-header">📊 Impact Estimate</div>
                            <div class="wf-impact-items">
                              <Show when={totals.external_messages.min > 0}>
                                <div class="wf-impact-row">
                                  <span>📧</span>
                                  <span>{totals.external_messages.max != null ? `${totals.external_messages.min}` : `${totals.external_messages.min}+`} messages</span>
                                  <Show when={totals.external_messages.max === null}>
                                    <span class="wf-impact-expr">({totals.external_messages.expression})</span>
                                  </Show>
                                </div>
                              </Show>
                              <Show when={totals.http_calls.min > 0}>
                                <div class="wf-impact-row">
                                  <span>🌐</span>
                                  <span>{totals.http_calls.max != null ? `${totals.http_calls.min}` : `${totals.http_calls.min}+`} HTTP calls</span>
                                </div>
                              </Show>
                              <Show when={totals.agent_invocations.min > 0}>
                                <div class="wf-impact-row">
                                  <span>🤖</span>
                                  <span>{totals.agent_invocations.min} agent invocation{totals.agent_invocations.min > 1 ? 's' : ''}</span>
                                </div>
                              </Show>
                              <Show when={totals.destructive_ops.min > 0}>
                                <div class="wf-impact-row">
                                  <span>⚠️</span>
                                  <span>{totals.destructive_ops.min} destructive op{totals.destructive_ops.min > 1 ? 's' : ''}</span>
                                </div>
                              </Show>
                              <Show when={totals.scheduled_tasks.min > 0}>
                                <div class="wf-impact-row">
                                  <span>⏰</span>
                                  <span>{totals.scheduled_tasks.min} scheduled task{totals.scheduled_tasks.min > 1 ? 's' : ''}</span>
                                </div>
                              </Show>
                            </div>
                            <Show when={isHighVolume}>
                              <div class="wf-impact-warn-text">⚠️ High volume — test run recommended</div>
                            </Show>
                          </div>
                        </Show>
                      );
                    }}
                  </Show>

                  {/* Untested workflow banner */}
                  <Show when={store.definitions().find(d => d.name === launchDef())?.is_untested}>
                    <div class="wf-untested-banner">
                      <TriangleAlert size={14} />
                      <span>Modified since last successful run — test run recommended</span>
                    </div>
                  </Show>

                  {/* Test Run Toggle */}
                  <div class="wf-test-run-toggle">
                    <Switch checked={testRunEnabled()} onChange={setTestRunEnabled} disabled={launching()} class="flex items-center gap-2">
                      <SwitchControl><SwitchThumb /></SwitchControl>
                      <SwitchLabel class="wf-test-run-label">🧪 Test Run</SwitchLabel>
                    </Switch>
                    <p class="wf-test-run-hint">Intercepts emails, HTTP calls, and other side effects. No real actions are performed.</p>
                  </div>
                </div>
              </Show>

              {/* Error banner */}
              <Show when={launchError()}>
                <div class="wf-wizard-error">
                  <TriangleAlert size={16} />
                  <span>{launchError()}</span>
                </div>
              </Show>

              {/* Navigation */}
              <div class="wf-wizard-nav">
                <Show when={wizardStep() === 0} fallback={
                  <button class="wf-btn-back" onClick={() => setWizardStep(s => s - 1)}>
                    <ArrowLeft size={14} /> Back
                  </button>
                }>
                  <button class="wf-btn-back" onClick={dismissWizard}>
                    Cancel
                  </button>
                </Show>
                <Show when={wizardStep() < 2} fallback={
                  <button
                    class="wf-btn-next"
                    data-testid="wf-launch-submit-btn"
                    aria-label="Launch workflow"
                    onClick={() => void handleLaunch()}
                    disabled={launching() || (launchSchema().length > 0 && !canLaunch())}
                  >
                    {launching()
                      ? <><Hourglass size={14} /> Launching…</>
                      : testRunEnabled()
                        ? <>🧪 Launch Test Run</>
                        : selectedTrigger()?.triggerType !== 'manual'
                          ? <><Rocket size={14} /> Simulate & Launch</>
                          : <><Rocket size={14} /> Launch</>}
                  </button>
                }>
                  <button class="wf-btn-next" onClick={() => setWizardStep(s => s + 1)}>
                    Next <ArrowRight size={14} />
                  </button>
                </Show>
              </div>
            </Show>
          </div>
        </div>
      </Show>

      {/* Kill Confirmation */}
      <Dialog open={!!confirmKill()} onOpenChange={(open) => { if (!open) setConfirmKill(null); }}>
        <DialogContent class="w-[350px]" style={{ background: 'hsl(var(--card))', border: '1px solid hsl(var(--destructive))' }} onInteractOutside={(e) => e.preventDefault()} onEscapeKeyDown={(e) => e.preventDefault()}>
            <DialogHeader>
              <DialogTitle class="text-destructive">Kill Workflow?</DialogTitle>
            </DialogHeader>
            <p class="text-sm m-0 text-muted-foreground">This will permanently stop the workflow and all child agents.</p>
            <DialogFooter class="flex-row justify-end gap-2">
              <Button variant="outline" onClick={() => setConfirmKill(null)}>Cancel</Button>
              <Button variant="destructive" onClick={() => void handleKillConfirmed(confirmKill()!)}>Kill</Button>
            </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* Reset to factory defaults */}
      <ConfirmDialog
        open={!!confirmReset()}
        onOpenChange={(open) => { if (!open) setConfirmReset(null); }}
        title={`Reset "${confirmReset()}"?`}
        description="This will restore the workflow to factory defaults. Your customizations will be lost."
        confirmLabel="Reset"
        variant="destructive"
        onConfirm={() => {
          const name = confirmReset();
          if (name) void store.resetDefinition(name);
        }}
      />

      {/* Delete Definition Confirmation */}
      <Dialog open={!!confirmDelete()} onOpenChange={(open) => { if (!open) setConfirmDelete(null); }}>
        <DialogContent class="w-[400px]" style={{ background: 'hsl(var(--card))', border: '1px solid hsl(var(--destructive))' }}>
        <Show when={confirmDelete()}>
          {(info) => (<>
              <DialogHeader>
                <DialogTitle class="text-destructive">Delete "{info().name}" v{info().version}?</DialogTitle>
              </DialogHeader>
              <Show when={info().triggers.length > 0 || info().scheduledTasks.length > 0} fallback={
                <p class="text-sm m-0 text-muted-foreground">
                  This will permanently delete the workflow definition. This action cannot be undone.
                </p>
              }>
                <p class="text-sm m-0 text-muted-foreground">
                  This workflow has connected resources that will also be deleted:
                </p>
                <div class="flex flex-col gap-1.5 p-2 rounded-md text-sm bg-background">
                  <Show when={info().triggers.length > 0}>
                    <div class="text-foreground">
                      <strong>Active triggers ({info().triggers.length}):</strong>
                      <For each={info().triggers}>
                        {(t: any) => <div class="pl-3 text-muted-foreground">• {t.trigger_kind ?? t.trigger_type?.type ?? 'trigger'}</div>}
                      </For>
                    </div>
                  </Show>
                  <Show when={info().scheduledTasks.length > 0}>
                    <div class="text-foreground">
                      <strong>Scheduled tasks ({info().scheduledTasks.length}):</strong>
                      <For each={info().scheduledTasks}>
                        {(t: any) => <div class="pl-3 text-muted-foreground">• {t.name} <span class="opacity-60">({t.status})</span></div>}
                      </For>
                    </div>
                  </Show>
                </div>
              </Show>
              <DialogFooter class="flex-row justify-end gap-2">
                <Button variant="outline" onClick={() => setConfirmDelete(null)}>Cancel</Button>
                <Button variant="destructive" onClick={() => void handleDeleteConfirmed()}>Delete</Button>
              </DialogFooter>
          </>)}
        </Show>
        </DialogContent>
      </Dialog>

      {/* Hide (Archive) Definition Confirmation */}
      <ConfirmDialog
        open={!!confirmArchiveDef()}
        onOpenChange={(open) => { if (!open) setConfirmArchiveDef(null); }}
        title={`Hide "${confirmArchiveDef()?.name}"?`}
        description="This will hide the workflow from the list. You can unhide it later from the archived section."
        confirmLabel="Hide"
        variant="default"
        onConfirm={() => {
          const info = confirmArchiveDef();
          if (info) void store.archiveDefinition(info.name, info.version);
        }}
      />

      {/* Archive Instance Confirmation */}
      <ConfirmDialog
        open={confirmArchiveInst() !== null}
        onOpenChange={(open) => { if (!open) setConfirmArchiveInst(null); }}
        title="Archive this workflow run?"
        description="The run will be moved to the archive. You can restore it later."
        confirmLabel="Archive"
        variant="default"
        onConfirm={() => {
          const id = confirmArchiveInst();
          if (id !== null) void store.archiveInstance(id);
        }}
      />

      {/* Feedback Response Dialog */}
      <Dialog open={!!feedbackStep()} onOpenChange={(open) => { if (!open) setFeedbackStep(null); }}>
        <DialogContent class="w-[450px]" style={{ background: 'hsl(var(--card))', border: '1px solid hsl(45 93% 58%)' }} onInteractOutside={(e) => e.preventDefault()} onEscapeKeyDown={(e) => e.preventDefault()}>
        <Show when={feedbackStep()}>
          {(_) => {
            const gate = () => feedbackStep()!;
            return (<>
                <DialogHeader>
                  <DialogTitle class="flex items-center gap-2">
                    <Hand size={18} /> Feedback Required
                  </DialogTitle>
                </DialogHeader>
                <div class="prose prose-sm dark:prose-invert max-w-none text-foreground text-[0.9em] leading-relaxed" innerHTML={renderMarkdown(gate().prompt)} />
                <Show when={feedbackError()}>
                  <p class="text-xs text-destructive">{feedbackError()}</p>
                </Show>
                <Show when={gate().choices.length > 0}>
                  <div class="flex flex-wrap gap-1.5">
                    <For each={gate().choices}>
                      {(choice) => (
                        <Button onClick={() => void submitFeedback(choice)}>{choice}</Button>
                      )}
                    </For>
                  </div>
                </Show>
                <Show when={gate().allow_freeform || gate().choices.length === 0}>
                  <textarea
                    class="w-full min-h-[60px] p-2 text-[0.85em] rounded-md border border-border bg-background text-foreground"
                    placeholder="Type your response..."
                    value={feedbackText()}
                    onInput={(e) => setFeedbackText(e.currentTarget.value)}
                  />
                  <DialogFooter class="flex-row justify-end gap-2">
                    <Button variant="outline" onClick={() => setFeedbackStep(null)}>Cancel</Button>
                    <Button onClick={() => void submitFeedback()} disabled={!feedbackText().trim()}>Submit</Button>
                  </DialogFooter>
                </Show>
                <Show when={!gate().allow_freeform && gate().choices.length > 0}>
                  <DialogFooter class="justify-end">
                    <Button variant="outline" onClick={() => setFeedbackStep(null)}>Cancel</Button>
                  </DialogFooter>
                </Show>
                <div class="text-xs text-muted-foreground">
                  Step: {gate().stepId} • Instance: {gate().instanceId}
                </div>
            </>);
          }}
        </Show>
        </DialogContent>
      </Dialog>

      {/* Agent question dialog */}
      <Dialog open={!!agentQuestion()} onOpenChange={(open) => { if (!open) setAgentQuestion(null); }}>
        <DialogContent class="max-w-[450px]">
          <Show when={agentQuestion()}>
            {(q) => {
              const [wfQMsSelected, setWfQMsSelected] = createSignal<Set<number>>(new Set());
              return (
              <>
                <DialogHeader>
                  <DialogTitle>
                    Question from {q().agent_name || 'agent'}
                  </DialogTitle>
                </DialogHeader>
                <Show when={q().message}>
                  <p style="font-size:0.85em;color:var(--text-secondary, #a6adc8);margin:4px 0 8px;">{q().message}</p>
                </Show>
                <p style="font-size:0.85em;color:var(--text-primary, #cdd6f4);margin:8px 0;">{q().text}</p>
                <Show when={q().choices.length > 0}>
                  <div style="display:flex;flex-wrap:wrap;gap:6px;margin-bottom:8px;">
                    <For each={q().choices}>
                      {(choice, idx) => (
                        <Button
                          variant={q().multi_select && wfQMsSelected().has(idx()) ? 'default' : 'outline'}
                          style="font-size:0.85em;"
                          disabled={questionSending()}
                          onClick={() => {
                            if (q().multi_select) {
                              setWfQMsSelected((prev) => {
                                const next = new Set(prev);
                                if (next.has(idx())) next.delete(idx());
                                else next.add(idx());
                                return next;
                              });
                            } else {
                              void submitAgentAnswer(idx(), choice);
                            }
                          }}
                        >
                          {choice}
                        </Button>
                      )}
                    </For>
                  </div>
                  <Show when={q().multi_select}>
                    <div style="margin-bottom:8px;">
                      <Button
                        size="sm"
                        disabled={wfQMsSelected().size === 0 || questionSending()}
                        onClick={() => {
                          const indices = [...wfQMsSelected()].sort((a, b) => a - b);
                          void submitAgentAnswer(undefined, undefined, indices);
                        }}
                      >
                        {questionSending() ? 'Sending…' : 'Submit'}
                      </Button>
                    </div>
                  </Show>
                </Show>
                <Show when={q().allow_freeform}>
                  <textarea
                    style="width:100%;min-height:60px;resize:vertical;background:var(--bg-primary, #1e1e2e);color:var(--text-primary, #cdd6f4);border:1px solid var(--border, #45475a);border-radius:6px;padding:8px;font-size:0.85em;"
                    placeholder="Type your answer…"
                    value={questionText()}
                    onInput={(e) => setQuestionText(e.currentTarget.value)}
                    onKeyDown={(e) => {
                      if (e.key === 'Enter' && !e.shiftKey && questionText().trim()) {
                        e.preventDefault();
                        void submitAgentAnswer(undefined, questionText().trim());
                      }
                    }}
                    disabled={questionSending()}
                  />
                  <DialogFooter class="flex-row justify-end gap-2">
                    <Button variant="outline" onClick={() => setAgentQuestion(null)}>Cancel</Button>
                    <Button disabled={!questionText().trim() || questionSending()} onClick={() => void submitAgentAnswer(undefined, questionText().trim())}>
                      {questionSending() ? 'Sending…' : 'Send'}
                    </Button>
                  </DialogFooter>
                </Show>
                <Show when={!q().allow_freeform && q().choices.length === 0}>
                  <DialogFooter>
                    <Button variant="outline" onClick={() => setAgentQuestion(null)}>Cancel</Button>
                  </DialogFooter>
                </Show>
              </>
              );
            }}
          </Show>
        </DialogContent>
      </Dialog>

      {/* Inline approval dialog */}
      <Dialog open={!!wfPendingApproval()} onOpenChange={(open) => { if (!open) setWfPendingApproval(null); }}>
        <DialogContent class="max-w-[500px]">
          <Show when={wfPendingApproval()}>
            {(a) => (
              <>
                <DialogHeader>
                  <DialogTitle>
                    <Lock size={16} class="inline mr-1" /> Tool Approval — {a().source_name || 'agent'}
                  </DialogTitle>
                </DialogHeader>
                <div style="font-size:0.85em;color:var(--text-primary, #cdd6f4);margin:8px 0;">
                  <p style="margin-bottom:6px;"><strong>Tool:</strong> {a().tool_id ?? 'unknown'}</p>
                  <Show when={a().reason}>
                    <p style="margin-bottom:6px;color:var(--text-secondary, #a6adc8);">{a().reason}</p>
                  </Show>
                  <Show when={a().input}>
                    <details style="margin-top:4px;">
                      <summary style="cursor:pointer;color:var(--text-secondary, #a6adc8);font-size:0.9em;">Show input</summary>
                      <pre style="margin-top:4px;padding:8px;background:var(--bg-primary, #1e1e2e);color:var(--text-primary, #cdd6f4);border:1px solid var(--border, #45475a);border-radius:6px;overflow-x:auto;font-size:0.85em;max-height:200px;overflow-y:auto;white-space:pre-wrap;">{a().input}</pre>
                    </details>
                  </Show>
                </div>
                <DialogFooter class="flex-row flex-wrap justify-end gap-2">
                  <Button variant="outline" onClick={() => setWfPendingApproval(null)}>Cancel</Button>
                  <Button variant="destructive" disabled={approvalSending()} onClick={() => void submitApproval(false)}>
                    {approvalSending() ? 'Sending…' : 'Deny'}
                  </Button>
                  <Button disabled={approvalSending()} onClick={() => void submitApproval(true)}>
                    {approvalSending() ? 'Sending…' : 'Approve'}
                  </Button>
                  <Button variant="outline" disabled={approvalSending()} onClick={() => void submitApproval(true, { allow_agent: true })}>
                    Allow for Agent
                  </Button>
                  <Button variant="outline" disabled={approvalSending()} onClick={() => void submitApproval(true, { allow_session: true })}>
                    Allow for Session
                  </Button>
                </DialogFooter>
              </>
            )}
          </Show>
        </DialogContent>
      </Dialog>

      {/* Workflow Creation Wizard */}
      <WorkflowCreationWizard
        open={showWizard()}
        onClose={() => setShowWizard(false)}
        definitions={store.definitions()}
        channels={props.channels}
        eventTopics={props.eventTopics}
        onCopy={async (source_name, sourceVersion, newName) => {
          return await store.copyDefinition(source_name, sourceVersion, newName);
        }}
        onComplete={(yaml, openAiAssist, aiPrompt) => {
          setShowWizard(false);
          store.setDesignerYaml(yaml);
          setDesignerAiPrompt(aiPrompt);
          setDesignerLockMode(false);
          store.setShowDesigner(true);
        }}
      />

      {/* Visual Designer */}
      <Show when={store.showDesigner()}>
        {(_) => {
          const initialYaml = untrack(() => store.designerYaml());
          return (
            <div class="fixed inset-0 bg-background z-[1000] flex flex-col">
              <div class="flex-1 overflow-hidden">
                <WorkflowDesigner
                  initialYaml={initialYaml}
                  initialAiPrompt={designerAiPrompt()}
                  lockMode={designerLockMode()}
                  onYamlChange={(yaml) => store.setDesignerYaml(yaml)}
                  onClose={() => store.setShowDesigner(false)}
                  saving={saving()}
                  onSave={async () => {
                    const yaml = store.designerYaml();
                    if (!yaml.trim()) {
                      store.setError('Nothing to save — the workflow is empty.');
                      return;
                    }
                    setSaving(true);
                    let timeoutId: ReturnType<typeof setTimeout> | undefined;
                    try {
                      const timeout = new Promise<never>((_, reject) => {
                        timeoutId = setTimeout(() => reject(new Error('Save timed out — check that the daemon is running and rebuilt with latest changes.')), 30000);
                      });
                      await Promise.race([store.saveFromDesigner(yaml), timeout]);
                    } catch (e: any) {
                      store.setError(e?.message ?? e?.toString() ?? 'Save failed');
                    } finally {
                      if (timeoutId) clearTimeout(timeoutId);
                      setSaving(false);
                    }
                  }}
                  toolDefinitions={props.toolDefinitions}
                  personas={props.personas}
                  channels={props.channels}
                  eventTopics={props.eventTopics}
                  onAiAssist={async (yaml, prompt, agent_id) => {
                    // Ensure bot SSE stream is connected (idempotent — won't restart existing)
                    await invoke('ensure_bot_stream');
                    const result = await invoke<{ agent_id: string }>('workflow_ai_assist', {
                      yaml, prompt, agent_id: agent_id ?? undefined,
                    });
                    return { agent_id: result.agent_id };
                  }}
                  onAiAssistCleanup={(agent_id) => {
                    invoke('deactivate_bot', { agent_id }).catch(() => {});
                  }}
                  onAiAssistSubscribe={(agent_id, callbacks) => {
                    let terminated = false;
                    let turnActive = false;
                    let unlisten: UnlistenFn | null = null;

                    function signalTurnDone() {
                      if (turnActive) { turnActive = false; callbacks.onDone(); }
                    }

                    function processEvent(evt: any) {
                      if (!evt || terminated) return;

                      // Events come as bare SupervisorEvent objects: { type, agent_id, event?, ... }
                      // OR wrapped: { Supervisor: { type, ... } } / { Loop: { type, ... } }
                      // Normalize: unwrap if wrapped
                      let sup = evt;
                      if (evt.Supervisor) sup = evt.Supervisor;
                      else if (evt.Loop) {
                        const loop = evt.Loop;
                        const t = loop.type;
                        if (t === 'token') callbacks.onToken(loop.delta || '');
                        else if (t === 'tool_call_start') {
                          let input = loop.input;
                          if (typeof input === 'string') { try { input = JSON.parse(input); } catch {} }
                          const toolName = (loop.tool_id || '').replace('workflow_author.', '');
                          callbacks.onToken(`🔧 Using ${toolName}...\n`);
                          callbacks.onToolCall(loop.tool_id || '', input);
                        } else if (t === 'question_asked' && loop.request_id) {
                          callbacks.onQuestion(loop.request_id, agent_id, loop.text ?? '', loop.choices ?? [], loop.allow_freeform !== false, loop.message);
                        } else if (t === 'done') {
                          signalTurnDone();
                        } else if (t === 'error') {
                          terminated = true;
                          callbacks.onError(loop.message || 'Agent error');
                        }
                        return;
                      }

                      // Filter by agent_id
                      if (sup.agent_id && sup.agent_id !== agent_id) return;

                      const t = sup.type;
                      if (t === 'agent_spawned') {
                        callbacks.onToken('🚀 Agent started\n');
                      } else if (t === 'agent_task_assigned') {
                        turnActive = true;
                        callbacks.onToken('⚡ Processing your request...\n');
                      } else if (t === 'agent_output' && sup.event) {
                        const re = sup.event;
                        if (re.type === 'token_delta') {
                          callbacks.onToken(re.token || '');
                        } else if (re.type === 'model_call_started') {
                          callbacks.onToken('💭 Thinking...\n');
                        } else if (re.type === 'tool_call_started') {
                          const toolName = (re.tool_id || '').replace('workflow_author.', '');
                          callbacks.onToken(`🔧 Using ${toolName}...\n`);
                          callbacks.onToolCall(re.tool_id || '', re.input);
                        } else if (re.type === 'completed') {
                          signalTurnDone();
                        } else if (re.type === 'failed') {
                          terminated = true;
                          callbacks.onError(re.error || 'Agent failed');
                        } else if (re.type === 'question_asked' && re.request_id) {
                          callbacks.onQuestion(re.request_id, sup.agent_id || agent_id, re.text ?? '', re.choices ?? [], re.allow_freeform !== false, re.message);
                        }
                      } else if (t === 'agent_status_changed') {
                        if (sup.status === 'error') {
                          terminated = true;
                          signalTurnDone();
                        } else if (sup.status === 'done' || sup.status === 'waiting') {
                          // Don't set terminated — agent may accept follow-up tasks (IdleAfterTask)
                          signalTurnDone();
                        }
                      } else if (t === 'agent_completed') {
                        // Don't set terminated — IdleAfterTask agents emit agent_completed
                        // after every task but remain alive for follow-up messages.
                        signalTurnDone();
                      }
                    }

                    listen<any>('stage:event', (e) => {
                      const p = e.payload;
                      if (p?.session_id !== '__service__') return;
                      processEvent(p?.event);
                    }).then(u => { unlisten = u; });

                    return () => {
                      terminated = true;
                      if (unlisten) unlisten();
                    };
                  }}
                  onUploadAttachment={(workflowId, version, filePath, description) =>
                    store.uploadAttachment(workflowId, version, filePath, description)
                  }
                  onDeleteAttachment={(workflowId, version, attachmentId) =>
                    store.deleteAttachment(workflowId, version, attachmentId)
                  }
                />
              </div>
              <Dialog open={!!store.error()} onOpenChange={(open) => { if (!open) store.setError(null); }}>
                <DialogContent class="max-w-[560px] min-w-[340px] z-[2000]">
                    <DialogHeader>
                      <DialogTitle class="flex items-center gap-2 text-destructive">
                        <TriangleAlert size={14} /> Save Failed
                      </DialogTitle>
                    </DialogHeader>
                    <pre class="bg-background text-foreground p-3 rounded-md text-[0.82em] whitespace-pre-wrap break-words max-h-[300px] overflow-y-auto m-0">{formatSaveError(store.error())}</pre>
                    <DialogFooter>
                      <Button onClick={() => store.setError(null)}>OK</Button>
                    </DialogFooter>
                </DialogContent>
              </Dialog>
            </div>
          );
        }}
      </Show>
    </div>
  );
}
