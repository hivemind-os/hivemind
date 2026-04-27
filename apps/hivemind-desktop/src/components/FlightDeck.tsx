import {
  createSignal,
  createEffect,
  createMemo,
  onMount,
  onCleanup,
  Show,
  For,
  lazy,
  type JSX,
} from 'solid-js';
import type { Accessor } from 'solid-js';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import DOMPurify from 'dompurify';
import type {
  SystemHealthSnapshot,
  GlobalAgentEntry,
  SessionTelemetryEntry,
  ChatSessionSummary,
  InstalledModel,
  WorkflowInstanceSummary,
  WorkflowInstance,
  StepState,
  TelemetrySnapshot,
  AgentStatus,
  DownloadProgress,
  ChatRunState,
  TokenUsage,
  ModelRouterSnapshot,
  StoredEvent,
  EventTopic,
  ActiveTriggerSnapshot,
  ActiveEventGateSnapshot,
  ActiveTriggersResponse,
  ServiceSnapshot,
  ServiceLogEntry,
  Persona,
  PromptTemplate,
  KgStats,
} from '../types';
import type { PendingQuestion } from './InlineQuestion';
import { pendingApprovalToasts, dismissAgentApproval, type PendingApproval } from './AgentApprovalToast';
import { logInfo, logError } from './ActivityLog';
import {
  answerQuestion as routeAnswerQuestion,
  respondToApproval as routeRespondToApproval,
  respondToGate as routeRespondToGate,
  type PendingInteraction,
} from '~/lib/interactionRouting';
import EventLogList, { type SupervisorEvent } from './EventLogList';
import PromptParameterDialog from './shared/PromptParameterDialog';
import { highlightYaml } from './YamlHighlight';
import { renderMarkdown } from '../utils';
import { Collapsible, CollapsibleContent } from '~/ui/collapsible';
import { Dialog, DialogContent } from '~/ui/dialog';
import { Tabs, TabsContent } from '~/ui/tabs';
import { Button } from '~/ui/button';
import { DataTable } from './flight-deck/data-table';
import { createAgentColumns, type AgentRow, type AgentColumnCallbacks } from './flight-deck/agents-columns';
import { createWorkflowColumns, type WorkflowRow, type WorkflowColumnCallbacks } from './flight-deck/workflows-columns';
import AgentsPanel from './flight-deck/AgentsPanel';
import WorkflowsPanel from './flight-deck/WorkflowsPanel';
const KnowledgeExplorer = lazy(() => import('./KnowledgeExplorer'));
import { Rocket, ClipboardList, MessageSquare, Wrench, XCircle, CheckCircle2, RefreshCw, Play, Pause, Settings, Send, Upload, Brain, Lock, HelpCircle, CheckCircle, Bot, Zap, Bell, Plug, Radio, BarChart3, ArrowLeftRight, Square, Hand, Timer, Calendar, Map as MapIcon, BookOpen, Unlink, Folder, ShieldAlert, Clock, Mail, MousePointer, PlaneTakeoff, Link, Download, Monitor, Dna, Hourglass, Activity, Search, Copy, X } from 'lucide-solid';

// ---------------------------------------------------------------------------
// Types for agent events (matching AgentStage)
// ---------------------------------------------------------------------------

interface ReasoningEvent {
  type: string;
  model?: string;
  prompt_preview?: string;
  content?: string;
  token_count?: number;
  tool_id?: string;
  input?: any;
  output?: any;
  is_error?: boolean;
  request_id?: string;
  reason?: string;
  result?: string;
  error?: string;
  step_id?: string;
  description?: string;
  agent_id?: string;
  text?: string;
  choices?: string[];
  allow_freeform?: boolean;
  // CodeExecution fields
  code?: string;
  stdout?: string;
  stderr?: string;
  duration_ms?: number;
}

// ---------------------------------------------------------------------------
// Props
// ---------------------------------------------------------------------------

interface FlightDeckProps {
  open: Accessor<boolean>;
  onClose: () => void;
  daemonOnline: Accessor<boolean>;
  modelRouter?: Accessor<ModelRouterSnapshot | null>;
  pendingQuestions?: Accessor<PendingQuestion[]>;
  answeredQuestions?: Accessor<Map<string, string>>;
  onQuestionAnswered?: (request_id: string, answerText: string) => void;
  /** External trigger to open the approval dialog (e.g. from WorkflowsPage) */
  externalApproval?: Accessor<PendingApproval | null>;
  onExternalApprovalHandled?: () => void;
  /** External trigger to open the question dialog */
  externalQuestion?: Accessor<PendingQuestion | null>;
  onExternalQuestionHandled?: () => void;
  personas?: Accessor<Persona[]>;
  daemon_url?: Accessor<string | undefined>;
  kgStats?: Accessor<KgStats | null>;
  loadKgStats?: () => Promise<void>;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`;
}

function formatNumber(n: number | null | undefined): string {
  if (n == null) return '—';
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
  return n.toString();
}

function formatUptime(secs: number): string {
  if (!Number.isFinite(secs)) return '—';
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  return h > 0 ? `${h}h ${m}m` : `${m}m`;
}

function agentStatusColor(status: AgentStatus): string {
  switch (status) {
    case 'spawning': return 'spawning';
    case 'active': return 'active';
    case 'waiting': return 'waiting';
    case 'paused': return 'paused';
    case 'blocked': return 'blocked';
    case 'done': return 'done';
    case 'error': return 'error';
    default: return '';
  }
}

function chatStateClass(state: ChatRunState): string {
  switch (state) {
    case 'idle': return 'idle';
    case 'running': return 'running';
    case 'paused': return 'paused';
    case 'interrupted': return 'interrupted';
    default: return '';
  }
}

function formatTokens(tokens: number): string {
  if (tokens >= 1_000_000) return `${(tokens / 1_000_000).toFixed(1)}M`;
  if (tokens >= 1_000) return `${(tokens / 1_000).toFixed(1)}k`;
  return `${tokens}`;
}

const totalTokens = (usage?: TokenUsage | null) =>
  (usage?.input_tokens ?? 0) + (usage?.output_tokens ?? 0);

const truncate = (str: string, len: number) =>
  str.length > len ? str.slice(0, len) + '…' : str;

const renderEvent = (ev: SupervisorEvent) => {
  switch (ev.type) {
    case 'agent_spawned':
      return <span class="fd-ev-spawn"><Rocket size={14} /> Agent spawned — {ev.spec?.friendly_name || ev.spec?.name}</span>;
    case 'agent_task_assigned':
      return <span class="fd-ev-status"><ClipboardList size={14} /> Task assigned</span>;
    case 'agent_status_changed':
      return <span class="fd-ev-status">◉ Status → {ev.status}</span>;
    case 'message_routed':
      return <span class="fd-ev-msg"><Send size={14} /> {ev.msg_type} from {ev.from}</span>;
    case 'agent_completed':
      return <span class="fd-ev-done"><CheckCircle size={14} /> Completed: {truncate(ev.result ?? '', 200)}</span>;
    case 'agent_output': {
      const re = ev.event;
      if (!re) return <span><Upload size={14} /> Output</span>;
      switch (re.type) {
        case 'model_call_started':
          return <span class="fd-ev-model"><Brain size={14} /> Model call → {re.model}</span>;
        case 'model_call_completed':
          return (
            <span class="fd-ev-model-done">
              <MessageSquare size={14} /> Response ({re.token_count} tokens): {truncate(re.content ?? '', 300)}
            </span>
          );
        case 'tool_call_started':
          return (
            <span class="fd-ev-tool">
              <Wrench size={14} /> Tool: {re.tool_id}({typeof re.input === 'object' ? truncate(JSON.stringify(re.input), 120) : ''})
            </span>
          );
        case 'tool_call_completed':
          return (
            <span class={`fd-ev-tool-done ${re.is_error ? 'fd-ev-error' : ''}`}>
              {re.is_error ? <XCircle size={14} /> : <CheckCircle2 size={14} />} {re.tool_id} →{' '}
              {truncate(typeof re.output === 'object' ? JSON.stringify(re.output) : String(re.output ?? ''), 200)}
            </span>
          );
        case 'user_interaction_required':
          return (
            <span class="fd-ev-approval"><Lock size={14} /> Approval needed: {re.tool_id} — {re.reason}</span>
          );
        case 'question_asked':
          return (
            <span class="fd-ev-approval"><HelpCircle size={14} /> Question: {re.text}</span>
          );
        case 'completed':
          return <span class="fd-ev-done"><CheckCircle size={14} /> {truncate(re.result ?? '', 200)}</span>;
        case 'failed':
          return <span class="fd-ev-error"><XCircle size={14} /> {re.error}</span>;
        case 'step_started':
          return <span class="fd-ev-step"><ClipboardList size={14} /> {re.description}</span>;
        default:
          return <span><Upload size={14} /> {re.type}</span>;
      }
    }
    default:
      return <span>{ev.type}</span>;
  }
};

// ---------------------------------------------------------------------------
// Workflow helpers (adapted from WorkflowsPage)
// ---------------------------------------------------------------------------

function getStepTask(step: any): any | undefined {
  return step.task ?? step.step_type?.Task ?? step.Task;
}

function stepIcon(stepType: any): JSX.Element {
  if (!stepType) return <Square size={14} />;
  if (stepType === 'trigger' || stepType.Trigger) return <Bell size={14} />;
  if (stepType === 'control_flow' || stepType.ControlFlow) return <ArrowLeftRight size={14} />;
  if (stepType === 'task' || stepType.Task) {
    const task = typeof stepType === 'object' ? stepType.Task : null;
    if (!task) return <Wrench size={14} />;
    if (task.CallTool) return <Wrench size={14} />;
    if (task.InvokeAgent) return <Bot size={14} />;
    if (task.FeedbackGate) return <Hand size={14} />;
    if (task.Delay) return <Timer size={14} />;
    if (task.SignalAgent) return <Radio size={14} />;
    if (task.LaunchWorkflow) return <RefreshCw size={14} />;
    if (task.ScheduleTask) return <Calendar size={14} />;
  }
  return <Square size={14} />;
}

function wfDurationStr(startMs: number, endMs: number | null | undefined): string {
  if (!startMs) return '—';
  const end = endMs || Date.now();
  const secs = Math.floor((end - startMs) / 1000);
  if (secs < 60) return `${secs}s`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m ${secs % 60}s`;
  return `${Math.floor(secs / 3600)}h ${Math.floor((secs % 3600) / 60)}m`;
}

function wfStepStatusClass(status: string): string {
  switch (status) {
    case 'completed': return 'fd-step-completed';
    case 'running': return 'fd-step-running';
    case 'failed': return 'fd-step-failed';
    case 'skipped': return 'fd-step-skipped';
    case 'waiting_on_input': case 'waiting_on_event': return 'fd-step-waiting';
    case 'ready': return 'fd-step-ready';
    default: return 'fd-step-pending';
  }
}

function eventTopicColorClass(topic: string): string {
  if (topic.startsWith('chat.')) return 'fd-evt-chat';
  if (topic.startsWith('workflow.')) return 'fd-evt-workflow';
  if (topic.startsWith('tool.')) return 'fd-evt-tool';
  if (topic.startsWith('config.')) return 'fd-evt-config';
  if (topic.startsWith('scheduler.')) return 'fd-evt-scheduler';
  if (topic.startsWith('mcp.')) return 'fd-evt-mcp';
  if (topic.startsWith('daemon.')) return 'fd-evt-daemon';
  if (topic.startsWith('comm.')) return 'fd-evt-comm';
  return 'fd-evt-other';
}

function formatEventTimestamp(ms: number): string {
  const d = new Date(ms);
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}.${String(d.getMilliseconds()).padStart(3, '0')}`;
}

type TabId = 'agents' | 'workflows' | 'triggers' | 'sessions' | 'models' | 'health' | 'events' | 'services' | 'knowledge';

interface PendingConfirmation {
  title: JSX.Element;
  message: string;
  confirmLabel: string;
  destructive: boolean;
  onConfirm: () => Promise<void>;
}

interface TabDef {
  id: TabId;
  icon: JSX.Element;
  label: string;
}

const TABS: TabDef[] = [
  { id: 'agents', icon: <Bot size={18} />, label: 'Agents' },
  { id: 'workflows', icon: <Zap size={18} />, label: 'Workflows' },
  { id: 'triggers', icon: <Bell size={18} />, label: 'Triggers' },
  { id: 'sessions', icon: <MessageSquare size={18} />, label: 'Sessions' },
  { id: 'models', icon: <Brain size={18} />, label: 'Models' },
  { id: 'events', icon: <Radio size={18} />, label: 'Events' },
  { id: 'services', icon: <Activity size={18} />, label: 'Services' },
  { id: 'health', icon: <BarChart3 size={18} />, label: 'Health' },
  { id: 'knowledge', icon: <Dna size={18} />, label: 'Knowledge' },
];

const POLL_INTERVAL_MS = 5_000;

// Tabs that receive real-time push events and skip periodic polling.
// They rely on debounced event listeners for updates instead.
// Note: 'workflows' is intentionally excluded — it uses push events but also
// needs periodic polling as a fallback in case the SSE stream is interrupted.
const PUSH_COVERED_TABS: ReadonlySet<TabId> = new Set([
  'triggers', 'services', 'agents', 'sessions',
]);

// Debounce intervals for push-driven refetches
const PUSH_DEBOUNCE_MS = 1_000;
const SESSION_PUSH_DEBOUNCE_MS = 2_000;

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

const FlightDeck = (props: FlightDeckProps) => {
  const [activeTab, setActiveTab] = createSignal<TabId>('agents');

  // Close on Escape key
  const handleEscape = (e: KeyboardEvent) => {
    if (e.key === 'Escape' && props.open()) { e.stopImmediatePropagation(); props.onClose(); }
  };
  document.addEventListener('keydown', handleEscape, true);
  onCleanup(() => document.removeEventListener('keydown', handleEscape, true));

  // ---- Data signals ----
  const [agents, setAgents] = createSignal<GlobalAgentEntry[]>([]);
  // Hide completed/errored agents from the flight deck
  const activeAgents = createMemo(() => agents().filter(a => a.status !== 'done' && a.status !== 'error'));
  const [workflows, setWorkflows] = createSignal<WorkflowInstanceSummary[]>([]);
  const [polledApprovals, setPolledApprovals] = createSignal<PendingApproval[]>([]);
  const [polledQuestions, setPolledQuestions] = createSignal<PendingQuestion[]>([]);
  const [sessions, setSessions] = createSignal<ChatSessionSummary[]>([]);
  const [sessionTelemetry, setSessionTelemetry] = createSignal<SessionTelemetryEntry[]>([]);
  const [models, setModels] = createSignal<InstalledModel[]>([]);
  const [downloads, setDownloads] = createSignal<DownloadProgress[]>([]);
  const [health, setHealth] = createSignal<SystemHealthSnapshot | null>(null);

  // ---- Services state ----
  const [servicesList, setServicesList] = createSignal<ServiceSnapshot[]>([]);
  const [serviceLogViewer, setServiceLogViewer] = createSignal<string | null>(null);
  const [serviceLogs, setServiceLogs] = createSignal<ServiceLogEntry[]>([]);
  const [serviceLogSearch, setServiceLogSearch] = createSignal('');
  const [serviceLogLevel, setServiceLogLevel] = createSignal<string>('');
  const [serviceLogsLoading, setServiceLogsLoading] = createSignal(false);

  // ---- UI state ----
  const [error, setError] = createSignal<string | null>(null);
  const [pendingConfirm, setPendingConfirm] = createSignal<PendingConfirmation | null>(null);

  // 1-second tick for live runtime counters
  const [runtimeTick, setRuntimeTick] = createSignal(0);
  const runtimeTickInterval = setInterval(() => setRuntimeTick((t) => t + 1), 1000);
  onCleanup(() => clearInterval(runtimeTickInterval));

  // ---- Agent interactivity state ----
  const [expandedAgentId, setExpandedAgentId] = createSignal<string | null>(null);
  const [agentEvents, setAgentEvents] = createSignal<SupervisorEvent[]>([]);
  const [agentEventsTotal, setAgentEventsTotal] = createSignal(0);
  const [eventsLoading, setEventsLoading] = createSignal(false);
  const PAGE_SIZE = 50;
  const [busyAgentId, setBusyAgentId] = createSignal<string | null>(null);
  const [configAgent, setConfigAgent] = createSignal<GlobalAgentEntry | null>(null);
  const [configModel, setConfigModel] = createSignal<string>('');
  const [questionDialogQuestion, setQuestionDialogQuestion] = createSignal<PendingQuestion | null>(null);
  const [questionFreeText, setQuestionFreeText] = createSignal('');
  const [questionSending, setQuestionSending] = createSignal(false);
  const [questionMsSelected, setQuestionMsSelected] = createSignal<Set<number>>(new Set());
  const [approvalDialogItem, setApprovalDialogItem] = createSignal<PendingApproval | null>(null);
  const [approvalSending, setApprovalSending] = createSignal(false);

  // ---- Prompt template state ----
  const [fdPromptPickerFor, setFdPromptPickerFor] = createSignal<string | null>(null);
  const [fdActivePrompt, setFdActivePrompt] = createSignal<{ agent_id: string; persona: Persona; template: PromptTemplate } | null>(null);

  // Watch external triggers for approval/question dialogs (e.g. from WorkflowsPage)
  createEffect(() => {
    const ext = props.externalApproval?.();
    if (ext) {
      setApprovalDialogItem(ext);
      setApprovalSending(false);
      props.onExternalApprovalHandled?.();
    }
  });
  createEffect(() => {
    const ext = props.externalQuestion?.();
    if (ext) {
      setQuestionDialogQuestion(ext);
      setQuestionFreeText('');
      setQuestionSending(false);
      setQuestionMsSelected(new Set<number>());
      props.onExternalQuestionHandled?.();
    }
  });

  // Reset multi-select choices when a new question opens (covers child component code paths)
  createEffect(() => {
    if (questionDialogQuestion()) {
      setQuestionMsSelected(new Set<number>());
    }
  });

  // Build telemetry lookup map from session telemetry per_agent data
  const agentTelemetryMap = createMemo(() => {
    const map = new Map<string, TokenUsage>();
    for (const entry of sessionTelemetry()) {
      for (const [agent_id, usage] of entry.telemetry.per_agent) {
        const existing = map.get(agent_id);
        if (existing) {
          existing.input_tokens += usage.input_tokens;
          existing.output_tokens += usage.output_tokens;
          existing.model_calls += usage.model_calls;
          existing.tool_calls += usage.tool_calls;
        } else {
          map.set(agent_id, { ...usage });
        }
      }
    }
    return map;
  });

  // Build available models list for the model switcher
  const availableModels = createMemo(() => {
    const router = props.modelRouter?.();
    if (!router) return [] as { id: string; label: string }[];
    const models: { id: string; label: string }[] = [];
    for (const p of router.providers) {
      if (!p.available) continue;
      for (const m of p.models) {
        models.push({
          id: `${p.id}:${m}`,
          label: `${m} (${p.name || p.id})`,
        });
      }
    }
    return models;
  });

  // ---- Workflow interactivity state ----
  const [expandedWorkflowId, setExpandedWorkflowId] = createSignal<number | null>(null);
  const [workflowDetail, setWorkflowDetail] = createSignal<WorkflowInstance | null>(null);
  const [workflowDetailLoading, setWorkflowDetailLoading] = createSignal(false);
  const [feedbackStep, setFeedbackStep] = createSignal<{
    instance_id: number;
    step_id: string;
    prompt: string;
    choices: string[];
    allow_freeform: boolean;
  } | null>(null);
  const [feedbackText, setFeedbackText] = createSignal('');

  // ---- Data table selection state (persists across tab switches) ----
  const [selectedAgentId, setSelectedAgentId] = createSignal<string | null>(null);
  const [selectedWfId, setSelectedWfId] = createSignal<number | null>(null);

  // Load workflow detail when selection changes
  createEffect(() => {
    const id = selectedWfId();
    if (id) {
      setExpandedWorkflowId(id);
      void loadWorkflowDetail(id);
    } else {
      setExpandedWorkflowId(null);
      setWorkflowDetail(null);
    }
  });

  // ---- Event bus state ----
  const [events, setEvents] = createSignal<StoredEvent[]>([]);
  const [eventTopics, setEventTopics] = createSignal<EventTopic[]>([]);
  const [eventTopicFilter, setEventTopicFilter] = createSignal('');
  const [expandedEventId, setExpandedEventId] = createSignal<number | null>(null);
  const [eventPaused, setEventPaused] = createSignal(false);
  const [hasMoreEvents, setHasMoreEvents] = createSignal(false);
  const [loadingMoreEvents, setLoadingMoreEvents] = createSignal(false);
  const [eventFetchError, setEventFetchError] = createSignal<string | null>(null);
  let eventsListRef: HTMLDivElement | undefined;
  const EVENT_PAGE_SIZE = 200;

  // ---- Active triggers state ----
  const [activeTriggers, setActiveTriggers] = createSignal<ActiveTriggerSnapshot[]>([]);
  const [activeEventGates, setActiveEventGates] = createSignal<ActiveEventGateSnapshot[]>([]);

  let pollTimer: ReturnType<typeof setInterval> | undefined;
  let workflowEventUnlisten: UnlistenFn | undefined;
  let disposed = false;
  let topicsLoaded = false;
  let justStartedPolling = false;

  // Sequence counters for stale response guards
  let loadAgentEventsSeq = 0;
  let loadWorkflowDetailSeq = 0;
  let fetchCurrentTabSeq = 0;

  // Debounce helper for push-driven refetches
  function makeDebouncedRefetch(tabId: TabId, ms: number): () => void {
    let timer: ReturnType<typeof setTimeout> | undefined;
    return () => {
      if (timer !== undefined) clearTimeout(timer);
      timer = setTimeout(() => {
        if (props.open() && activeTab() === tabId) fetchCurrentTab();
      }, ms);
    };
  }

  // ---- Badge counts ----
  const agentBadge = createMemo(() => {
    const needing = activeAgents().filter(
      (a) => a.status === 'blocked' || a.status === 'waiting',
    );
    return needing.length || undefined;
  });

  const workflowBadge = createMemo(() => {
    const active = workflows().filter(
      (w) =>
        w.status !== 'completed' && w.status !== 'failed' && w.status !== 'killed',
    );
    return active.length || undefined;
  });

  const sessionBadge = createMemo(() => {
    const running = sessions().filter((s) => s.state === 'running');
    return running.length || undefined;
  });

  const triggerBadge = createMemo(() => {
    const count = activeTriggers().filter((t) => t.trigger_kind !== 'manual').length;
    return count || undefined;
  });

  function badgeForTab(id: TabId): number | undefined {
    switch (id) {
      case 'agents': return agentBadge();
      case 'workflows': return workflowBadge();
      case 'triggers': return triggerBadge();
      case 'sessions': return sessionBadge();
      default: return undefined;
    }
  }

  function badgeSeverityForTab(id: TabId): 'warn' | 'alert' {
    switch (id) {
      case 'agents': return 'alert';
      case 'workflows': return 'warn';
      case 'sessions': return 'warn';
      case 'triggers': return 'warn';
      default: return 'warn';
    }
  }

  // ---- Data fetching ----

  async function fetchAgents() {
    try {
      const [data, telData] = await Promise.all([
        invoke<GlobalAgentEntry[]>('flight_deck_all_agents'),
        invoke<SessionTelemetryEntry[]>('flight_deck_sessions_telemetry'),
      ]);
      setAgents(Array.isArray(data) ? data : []);
      setSessionTelemetry(Array.isArray(telData) ? telData : []);
    } catch (e: any) {
      console.error('FlightDeck: failed to fetch agents', e);
    }
  }

  async function fetchWorkflows() {
    try {
      const result = await invoke<{ items: WorkflowInstanceSummary[]; total: number }>(
        'workflow_list_instances',
        { status: 'running,paused,pending,waiting_on_input,waiting_on_event' },
      );
      setWorkflows(Array.isArray(result?.items) ? result.items : []);
      // Re-fetch expanded workflow detail so the panel stays in sync
      const eid = expandedWorkflowId();
      if (eid != null) {
        void loadWorkflowDetail(eid);
      }
    } catch (e: any) {
      console.error('FlightDeck: failed to fetch workflows', e);
    }
  }

  async function fetchPendingInteractions() {
    try {
      const [approvals, questions] = await Promise.all([
        invoke<PendingApproval[]>('list_pending_approvals'),
        invoke<Array<Omit<PendingQuestion, 'timestamp'>>>('list_all_pending_questions'),
      ]);
      setPolledApprovals(Array.isArray(approvals) ? approvals : []);
      const now = Date.now();
      setPolledQuestions(Array.isArray(questions) ? questions.map(q => ({ ...q, timestamp: now })) : []);
    } catch (e) {
      console.error('Failed to fetch pending interactions:', e);
    }
  }

  async function fetchSessions() {
    try {
      const [sessData, telData] = await Promise.all([
        invoke<ChatSessionSummary[]>('chat_list_sessions'),
        invoke<SessionTelemetryEntry[]>('flight_deck_sessions_telemetry'),
      ]);
      setSessions(Array.isArray(sessData) ? sessData : []);
      setSessionTelemetry(Array.isArray(telData) ? telData : []);
    } catch (e: any) {
      console.error('FlightDeck: failed to fetch sessions', e);
    }
  }

  async function fetchModels() {
    try {
      const [modelResult, dlResult] = await Promise.all([
        invoke<{ installed_count: number; total_size_bytes: number; models: InstalledModel[] }>(
          'local_models_list',
        ),
        invoke<DownloadProgress[]>('local_models_downloads'),
      ]);
      setModels(Array.isArray(modelResult?.models) ? modelResult.models : []);
      setDownloads(Array.isArray(dlResult) ? dlResult : []);
    } catch (e: any) {
      console.error('FlightDeck: failed to fetch models', e);
    }
  }

  async function fetchHealth() {
    try {
      const data = await invoke<SystemHealthSnapshot>('flight_deck_system_health');
      setHealth(data);
    } catch (e: any) {
      console.error('FlightDeck: failed to fetch system health', e);
    }
  }

  async function fetchServices() {
    try {
      const data = await invoke<ServiceSnapshot[]>('services_list');
      setServicesList(Array.isArray(data) ? data : []);
    } catch (e: any) {
      console.error('FlightDeck: failed to fetch services', e);
    }
  }

  async function fetchServiceLogs(serviceId: string) {
    setServiceLogsLoading(true);
    try {
      const data = await invoke<ServiceLogEntry[]>('services_get_logs', {
        service_id: serviceId,
        limit: 500,
        level: serviceLogLevel() || null,
        search: serviceLogSearch() || null,
      });
      setServiceLogs(Array.isArray(data) ? data : []);
    } catch (e: any) {
      console.error('FlightDeck: failed to fetch service logs', e);
    } finally {
      setServiceLogsLoading(false);
    }
  }

  async function restartService(serviceId: string) {
    try {
      await invoke('services_restart', { service_id: serviceId });
      await fetchServices();
    } catch (e: any) {
      console.error('FlightDeck: failed to restart service', e);
    }
  }

  function eventTopicArgs() {
    const filter = eventTopicFilter().trim();
    const serverTopic = filter && !filter.includes('*') ? filter : undefined;
    return { serverTopic, filter };
  }

  function applyWildcardFilter(data: StoredEvent[], filter: string): StoredEvent[] {
    if (filter && filter.includes('*')) {
      const regex = new RegExp(
        '^' + filter.replace(/\./g, '\\.').replace(/\*/g, '.*') + '$',
        'i',
      );
      return data.filter((e) => regex.test(e.topic));
    }
    return data;
  }

  async function fetchEvents(scrollToTop = false) {
    if (eventPaused()) return;
    try {
      const { serverTopic, filter } = eventTopicArgs();
      const data = await invoke<StoredEvent[]>('event_bus_query', {
        topic: serverTopic || null,
        since: null,
        before_id: null,
        after_id: null,
        limit: EVENT_PAGE_SIZE,
      });
      const safeData = Array.isArray(data) ? data : [];
      const filtered = applyWildcardFilter(safeData, filter);
      setEvents(filtered);
      setHasMoreEvents(safeData.length >= EVENT_PAGE_SIZE);
      setEventFetchError(null);
      if (scrollToTop && eventsListRef) eventsListRef.scrollTop = 0;
    } catch (e: any) {
      console.error('FlightDeck: failed to fetch events', e);
      setEventFetchError(String(e));
    }
  }

  async function fetchNewerEvents() {
    if (eventPaused()) return;
    const current = events();
    if (current.length === 0) {
      return fetchEvents(true);
    }
    const newestId = Math.max(...current.map((e) => e.id));
    try {
      const { serverTopic, filter } = eventTopicArgs();
      const data = await invoke<StoredEvent[]>('event_bus_query', {
        topic: serverTopic || null,
        since: null,
        before_id: null,
        after_id: newestId,
        limit: EVENT_PAGE_SIZE,
      });
      const safeData = Array.isArray(data) ? data : [];
      if (safeData.length > 0) {
        const filtered = applyWildcardFilter(safeData, filter);
        if (filtered.length > 0) {
          const wasNearTop = eventsListRef ? eventsListRef.scrollTop < 80 : true;
          setEvents((prev) => [...filtered, ...prev]);
          if (wasNearTop && eventsListRef) eventsListRef.scrollTop = 0;
        }
      }
      setEventFetchError(null);
    } catch (e: any) {
      console.error('FlightDeck: failed to fetch newer events', e);
      setEventFetchError(String(e));
    }
  }

  async function loadMoreEvents() {
    const current = events();
    if (current.length === 0 || loadingMoreEvents()) return;
    const oldestId = Math.min(...current.map((e) => e.id));
    setLoadingMoreEvents(true);
    try {
      const { serverTopic, filter } = eventTopicArgs();
      const data = await invoke<StoredEvent[]>('event_bus_query', {
        topic: serverTopic || null,
        since: null,
        before_id: oldestId,
        after_id: null,
        limit: EVENT_PAGE_SIZE,
      });
      const safeData = Array.isArray(data) ? data : [];
      const filtered = applyWildcardFilter(safeData, filter);
      setEvents((prev) => [...prev, ...filtered]);
      setHasMoreEvents(safeData.length >= EVENT_PAGE_SIZE);
    } catch (e: any) {
      console.error('FlightDeck: failed to load more events', e);
    } finally {
      setLoadingMoreEvents(false);
    }
  }

  async function fetchEventTopics() {
    try {
      const result = await invoke<{ topics: EventTopic[] }>('event_bus_topics');
      setEventTopics(Array.isArray(result?.topics) ? result.topics : []);
    } catch (e: any) {
      console.error('FlightDeck: failed to fetch event topics', e);
    } finally {
      topicsLoaded = true;
    }
  }

  async function fetchTriggers() {
    try {
      const result = await invoke<ActiveTriggersResponse>('workflow_list_active_triggers');
      setActiveTriggers(Array.isArray(result.triggers) ? result.triggers : []);
      setActiveEventGates(Array.isArray(result.event_gates) ? result.event_gates : []);
    } catch (e: any) {
      console.error('FlightDeck: failed to fetch active triggers', e);
    }
  }

  async function fetchCurrentTab() {
    if (!props.daemonOnline()) return;
    const mySeq = ++fetchCurrentTabSeq;
    setError(null);
    try {
      switch (activeTab()) {
        case 'agents': await Promise.all([fetchAgents(), fetchPendingInteractions()]); break;
        case 'workflows': await Promise.all([fetchWorkflows(), fetchPendingInteractions()]); break;
        case 'triggers': await fetchTriggers(); break;
        case 'sessions': await fetchSessions(); break;
        case 'models': await fetchModels(); break;
        case 'health': await fetchHealth(); break;
        case 'services': await fetchServices(); break;
        case 'events': await fetchEvents(true); break;
      }
    } catch (e: any) {
      if (mySeq === fetchCurrentTabSeq) setError(String(e));
    }
  }

  let flightDeckPollBusy = false;
  function startPolling() {
    stopPolling();
    flightDeckPollBusy = false;
    justStartedPolling = true;
    fetchCurrentTab();
    pollTimer = setInterval(() => {
      if (flightDeckPollBusy) return;
      // Tabs with push coverage get real-time updates via event listeners;
      // only models & health still need periodic polling.
      const tab = activeTab();
      if (PUSH_COVERED_TABS.has(tab)) return;
      flightDeckPollBusy = true;
      void (async () => {
        try { await fetchCurrentTab(); }
        finally { flightDeckPollBusy = false; }
      })();
    }, POLL_INTERVAL_MS);
  }

  function stopPolling() {
    if (pollTimer !== undefined) {
      clearInterval(pollTimer);
      pollTimer = undefined;
    }
  }

  // Start/stop polling when panel opens/closes or tab changes
  createEffect(() => {
    if (props.open()) {
      startPolling();
    } else {
      stopPolling();
    }
  });

  createEffect(() => {
    // Re-fetch when tab changes while open (startPolling already calls fetchCurrentTab
    // on first open, so only re-fetch on subsequent tab changes)
    const _tab = activeTab();
    if (props.open() && pollTimer !== undefined) {
      if (justStartedPolling) {
        justStartedPolling = false;
        return;
      }
      fetchCurrentTab();
    }
  });

  // Load known topics when the events tab is first activated
  createEffect(() => {
    if (activeTab() === 'events' && !topicsLoaded && eventTopics().length === 0) {
      fetchEventTopics();
    }
  });

  // Load KG stats when the knowledge tab is activated
  createEffect(() => {
    if (activeTab() === 'knowledge' && props.open() && props.loadKgStats) {
      void props.loadKgStats();
    }
  });

  onCleanup(() => {
    disposed = true;
    stopPolling();
    workflowEventUnlisten?.();
  });

  // SSE listener: auto-refresh triggers when workflow definitions change or triggers fire
  onMount(() => {
    // Debounced refetch callbacks for push-covered tabs
    const refetchWorkflows = makeDebouncedRefetch('workflows', PUSH_DEBOUNCE_MS);
    const refetchAgents = makeDebouncedRefetch('agents', PUSH_DEBOUNCE_MS);
    const refetchSessions = makeDebouncedRefetch('sessions', SESSION_PUSH_DEBOUNCE_MS);
    const refetchEvents = (() => {
      let timer: ReturnType<typeof setTimeout> | undefined;
      return () => {
        if (timer !== undefined) clearTimeout(timer);
        timer = setTimeout(() => {
          if (props.open() && activeTab() === 'events') fetchNewerEvents();
        }, PUSH_DEBOUNCE_MS);
      };
    })();

    // Store all listener promises BEFORE any await so cleanup can be registered synchronously
    const stageEventPromise = listen<any>('stage:event', () => {
      if (!disposed) refetchAgents();
    });

    const chatEventPromise = listen<any>('chat:event', () => {
      if (!disposed) refetchSessions();
    });

    const chatDonePromise = listen<any>('chat:done', () => {
      if (!disposed) refetchSessions();
    });

    // NOTE: no event_bus_unsubscribe command exists in the backend yet
    invoke('event_bus_subscribe').catch(() => {});
    const eventBusPromise = listen<any>('eventbus:event', () => {
      if (!disposed) refetchEvents();
    });

    // NOTE: no services_unsubscribe_events command exists in the backend yet
    invoke('services_subscribe_events').catch(() => {});
    const serviceEventPromise = listen<any>('service:event', (event) => {
      if (disposed) return;
      const payload = event.payload;
      if (payload && payload.service_id && payload.status) {
        setServicesList((prev) =>
          prev.map((s) =>
            s.id === payload.service_id ? { ...s, status: payload.status, last_error: payload.last_error ?? s.last_error } : s
          )
        );
      }
    });

    // Register cleanup SYNCHRONOUSLY — critical for SolidJS reactive owner
    onCleanup(() => {
      disposed = true;
      stageEventPromise.then(fn => fn());
      chatEventPromise.then(fn => fn());
      chatDonePromise.then(fn => fn());
      eventBusPromise.then(fn => fn());
      serviceEventPromise.then(fn => fn());
    });

    // Async initialization for workflow:event listener
    // (its cleanup is handled by the separate synchronous onCleanup above)
    (async () => {
      const wfUn = await listen<any>('workflow:event', (event) => {
        if (disposed) return;
        const topic = event.payload?.topic as string | undefined;
        if (
          activeTab() === 'triggers' &&
          topic &&
          (topic.startsWith('workflow.definition.') || topic === 'workflow.trigger.fired')
        ) {
          fetchTriggers();
        }
        refetchWorkflows();
      });
      if (disposed) { wfUn(); } else { workflowEventUnlisten = wfUn; }
    })();
  });

  // ---- Agent actions ----

  function requestPauseAgent(agent: GlobalAgentEntry) {
    setPendingConfirm({
      title: <><Pause size={14} /> Pause Agent</>,
      message: `Are you sure you want to pause "${agent.agent_id}"?`,
      confirmLabel: 'Pause',
      destructive: false,
      onConfirm: async () => {
        try {
          if (agent.session_id) {
            await invoke('pause_session_agent', {
              session_id: agent.session_id,
              agent_id: agent.agent_id,
            });
          } else {
            await invoke('deactivate_bot', { agent_id: agent.agent_id });
          }
          await fetchAgents();
        } catch (e: any) {
          logError('flight-deck', `Failed to pause agent ${agent.agent_id}: ${e}`);
        }
      },
    });
  }

  function requestResumeAgent(agent: GlobalAgentEntry) {
    setPendingConfirm({
      title: <><Play size={14} /> Resume Agent</>,
      message: `Are you sure you want to resume "${agent.agent_id}"?`,
      confirmLabel: 'Resume',
      destructive: false,
      onConfirm: async () => {
        try {
          if (agent.session_id) {
            await invoke('resume_session_agent', {
              session_id: agent.session_id,
              agent_id: agent.agent_id,
            });
          } else {
            await invoke('activate_bot', { agent_id: agent.agent_id });
          }
          await fetchAgents();
        } catch (e: any) {
          logError('flight-deck', `Failed to resume agent ${agent.agent_id}: ${e}`);
        }
      },
    });
  }

  function requestKillAgent(agent: GlobalAgentEntry) {
    setPendingConfirm({
      title: <><Square size={14} /> Kill Agent</>,
      message: `Are you sure you want to kill "${agent.agent_id}"? This cannot be undone.`,
      confirmLabel: 'Kill',
      destructive: true,
      onConfirm: async () => {
        try {
          if (agent.session_id) {
            await invoke('kill_session_agent', {
              session_id: agent.session_id,
              agent_id: agent.agent_id,
            });
          } else {
            await invoke('delete_bot', { agent_id: agent.agent_id });
          }
          await fetchAgents();
        } catch (e: any) {
          logError('flight-deck', `Failed to kill agent ${agent.agent_id}: ${e}`);
        }
      },
    });
  }

  // ---- Agent event log ----

  interface PagedEventsResponse { events: SupervisorEvent[]; total: number; }

  async function loadAgentEvents(agent: GlobalAgentEntry, loadEarlier = false) {
    const mySeq = ++loadAgentEventsSeq;
    setEventsLoading(true);
    try {
      const currentEvents = loadEarlier ? agentEvents() : [];
      let offset: number | undefined;
      let limit = PAGE_SIZE;
      if (!loadEarlier) {
        const probe = agent.session_id
          ? await invoke<PagedEventsResponse>('get_agent_events', {
              session_id: agent.session_id,
              agent_id: agent.agent_id,
              offset: 0,
              limit: 0,
            })
          : await invoke<PagedEventsResponse>('get_bot_events', {
              agent_id: agent.agent_id,
              offset: 0,
              limit: 0,
            });
        if (mySeq !== loadAgentEventsSeq) return;
        const total = probe.total;
        setAgentEventsTotal(total);
        offset = Math.max(0, total - PAGE_SIZE);
        limit = PAGE_SIZE;
      } else {
        const total = agentEventsTotal();
        const alreadyLoaded = currentEvents.length;
        const remaining = total - alreadyLoaded;
        if (remaining <= 0) return;
        offset = Math.max(0, remaining - PAGE_SIZE);
        limit = remaining - offset;
      }

      const resp = agent.session_id
        ? await invoke<PagedEventsResponse>('get_agent_events', {
            session_id: agent.session_id,
            agent_id: agent.agent_id,
            offset,
            limit,
          })
        : await invoke<PagedEventsResponse>('get_bot_events', {
            agent_id: agent.agent_id,
            offset,
            limit,
          });

      if (mySeq !== loadAgentEventsSeq) return;
      setAgentEventsTotal(resp?.total ?? 0);
      const safeEvents = Array.isArray(resp?.events) ? resp.events : [];

      if (loadEarlier) {
        setAgentEvents([...safeEvents, ...currentEvents]);
      } else {
        setAgentEvents(safeEvents);
      }
    } catch {
      if (mySeq !== loadAgentEventsSeq) return;
      if (!loadEarlier) {
        setAgentEvents([]);
        setAgentEventsTotal(0);
      }
    } finally {
      if (mySeq === loadAgentEventsSeq) setEventsLoading(false);
    }
  }

  function toggleExpandAgent(agent: GlobalAgentEntry) {
    if (expandedAgentId() === agent.agent_id) {
      setExpandedAgentId(null);
      setAgentEvents([]);
      setAgentEventsTotal(0);
    } else {
      setExpandedAgentId(agent.agent_id);
      void loadAgentEvents(agent);
    }
  }

  // ---- Agent reconfigure ----

  function openConfigDialog(agent: GlobalAgentEntry) {
    setConfigModel(agent.active_model || agent.spec.model || '');
    setConfigAgent(agent);
  }

  async function restartAgent(agent: GlobalAgentEntry, model?: string) {
    if (!agent.session_id) return; // Bots don't support restart
    setBusyAgentId(agent.agent_id);
    try {
      await invoke('restart_session_agent', {
        session_id: agent.session_id,
        agent_id: agent.agent_id,
        model: model || null,
        allowed_tools: null,
      });
      setConfigAgent(null);
      logInfo('flight-deck', `Restarted agent ${agent.agent_id}${model ? ` with model ${model}` : ''}`);
      await fetchAgents();
    } catch (e: any) {
      logError('flight-deck', `Failed to restart agent: ${e}`);
    } finally {
      setBusyAgentId(null);
    }
  }

  // ---- Agent approval/question responses ----

  async function respondToApproval(agent: GlobalAgentEntry, request_id: string, approved: boolean, opts?: { allow_agent?: boolean; allow_session?: boolean }) {
    dismissAgentApproval(request_id);
    try {
      await routeRespondToApproval(
        {
          request_id,
          entity_id: `agent/${agent.agent_id}`,
          source_name: agent.agent_id,
          type: 'tool_approval',
          session_id: agent.session_id ?? undefined,
          agent_id: agent.agent_id,
        } as PendingInteraction,
        { approved, allow_agent: opts?.allow_agent, allow_session: opts?.allow_session },
      );
      const scope = opts?.allow_session ? ' (session)' : opts?.allow_agent ? ' (agent)' : '';
      logInfo('flight-deck', `${approved ? 'Approved' : 'Denied'}${scope} tool for agent ${agent.agent_id}`);
      setPolledApprovals(prev => prev.filter(a => a.request_id !== request_id));
    } catch (err: any) {
      logError('flight-deck', `Failed to respond to approval ${request_id}: ${err}`);
    }
  }

  async function answerQuestion(agent: GlobalAgentEntry, question: PendingQuestion, choiceIdx?: number, text?: string, selected_choices?: number[]) {
    setQuestionSending(true);
    let label: string;
    if (selected_choices && selected_choices.length > 0) {
      label = selected_choices.map((i) => question.choices[i]).join(', ');
    } else {
      label = text || (choiceIdx !== undefined ? question.choices[choiceIdx] : '');
    }
    try {
      await routeAnswerQuestion(
        {
          request_id: question.request_id,
          entity_id: question.agent_id ? `agent/${question.agent_id}` : `session/${agent.session_id}`,
          source_name: agent.agent_id,
          type: 'question',
          session_id: agent.session_id ?? undefined,
          agent_id: question.agent_id ?? agent.agent_id,
        } as PendingInteraction,
        {
          ...(choiceIdx !== undefined ? { selected_choice: choiceIdx } : {}),
          ...(selected_choices !== undefined ? { selected_choices } : {}),
          ...(text ? { text } : {}),
        },
      );
      props.onQuestionAnswered?.(question.request_id, label);
      setPolledQuestions(prev => prev.filter(q => q.request_id !== question.request_id));
      setQuestionDialogQuestion(null);
    } catch (err) {
      console.error('Failed to respond:', err);
      setQuestionSending(false);
    }
  }

  // ---- Workflow actions ----

  function requestPauseWorkflow(instanceId: number) {
    setPendingConfirm({
      title: <><Pause size={14} /> Pause Workflow</>,
      message: `Are you sure you want to pause workflow "${instanceId}"?`,
      confirmLabel: 'Pause',
      destructive: false,
      onConfirm: async () => {
        try {
          await invoke('workflow_pause', { instance_id: instanceId });
          await fetchWorkflows();
        } catch (e: any) {
          logError('flight-deck', `Failed to pause workflow ${instanceId}: ${e}`);
        }
      },
    });
  }

  function requestResumeWorkflow(instanceId: number) {
    setPendingConfirm({
      title: <><Play size={14} /> Resume Workflow</>,
      message: `Are you sure you want to resume workflow "${instanceId}"?`,
      confirmLabel: 'Resume',
      destructive: false,
      onConfirm: async () => {
        try {
          await invoke('workflow_resume', { instance_id: instanceId });
          await fetchWorkflows();
        } catch (e: any) {
          logError('flight-deck', `Failed to resume workflow ${instanceId}: ${e}`);
        }
      },
    });
  }

  function requestKillWorkflow(instanceId: number) {
    setPendingConfirm({
      title: <><Square size={14} /> Kill Workflow</>,
      message: `Are you sure you want to kill workflow "${instanceId}"? This cannot be undone.`,
      confirmLabel: 'Kill',
      destructive: true,
      onConfirm: async () => {
        try {
          await invoke('workflow_kill', { instance_id: instanceId });
          await fetchWorkflows();
        } catch (e: any) {
          logError('flight-deck', `Failed to kill workflow ${instanceId}: ${e}`);
        }
      },
    });
  }

  // ---- Workflow detail & feedback ----

  async function loadWorkflowDetail(instanceId: number) {
    const mySeq = ++loadWorkflowDetailSeq;
    setWorkflowDetailLoading(true);
    try {
      const data = await invoke<WorkflowInstance>('workflow_get_instance', { instance_id: instanceId });
      if (mySeq !== loadWorkflowDetailSeq) return;
      setWorkflowDetail(data);
    } catch (e: any) {
      if (mySeq !== loadWorkflowDetailSeq) return;
      console.error('Failed to load workflow detail', e);
      setWorkflowDetail(null);
    } finally {
      if (mySeq === loadWorkflowDetailSeq) setWorkflowDetailLoading(false);
    }
  }

  // Drive expansion through selectedWfId so the createEffect keeps both signals in sync.
  function toggleExpandWorkflow(id: number) {
    if (expandedWorkflowId() === id) {
      setSelectedWfId(null);
    } else {
      setSelectedWfId(id);
    }
  }

  function openFeedbackGate(instanceId: number, step: any, state: StepState) {
    if (state?.status !== 'waiting_on_input') return;
    const prompt = state.interaction_prompt
      ?? (() => { const task = getStepTask(step); const gate = task?.FeedbackGate ?? task?.feedback_gate ?? task; return gate?.prompt; })()
      ?? 'Please provide your response:';
    const choices = state.interaction_choices
      ?? (() => { const task = getStepTask(step); const gate = task?.FeedbackGate ?? task?.feedback_gate ?? task; return gate?.choices; })()
      ?? [];
    const allow_freeform = state.interaction_allow_freeform
      ?? (() => { const task = getStepTask(step); const gate = task?.FeedbackGate ?? task?.feedback_gate ?? task; return gate?.allow_freeform ?? gate?.allow_freeform; })()
      ?? true;
    setFeedbackStep({ instance_id: instanceId, step_id: step.id, prompt, choices, allow_freeform });
    setFeedbackText('');
  }

  async function submitFeedback(choice?: string) {
    const gate = feedbackStep();
    if (!gate) return;
    const response = choice
      ? { selected: choice, text: feedbackText() }
      : { selected: feedbackText(), text: feedbackText() };
    try {
      await routeRespondToGate(
        {
          request_id: '',
          entity_id: `workflow/${gate.instance_id}`,
          source_name: '',
          type: 'workflow_gate',
          instance_id: gate.instance_id,
          step_id: gate.step_id,
        } as PendingInteraction,
        response,
      );
      logInfo('flight-deck', `Responded to feedback gate ${gate.step_id}`);
      setFeedbackStep(null);
      setFeedbackText('');
      await fetchWorkflows();
      // Refresh detail if still expanded
      if (expandedWorkflowId() === gate.instance_id) {
        void loadWorkflowDetail(gate.instance_id);
      }
    } catch (e: any) {
      logError('flight-deck', `Failed to submit feedback: ${e}`);
    }
  }


  // ---- Model actions ----

  async function loadModel(modelId: string) {
    try {
      await invoke('local_model_load', { model_id: modelId });
      await fetchModels();
    } catch (e: any) {
      console.error('Failed to load model', e);
    }
  }

  async function unloadModel(modelId: string) {
    try {
      await invoke('local_model_unload', { model_id: modelId });
      await fetchModels();
    } catch (e: any) {
      console.error('Failed to unload model', e);
    }
  }

  // ---- Active workflows (filtered) ----
  const activeWorkflows = createMemo(() =>
    workflows().filter(
      (w) =>
        w.status !== 'completed' && w.status !== 'failed' && w.status !== 'killed',
    ),
  );

  // ---- Telemetry lookup ----
  function telemetryFor(session_id: string): SessionTelemetryEntry | undefined {
    return sessionTelemetry().find((t) => t.session_id === session_id);
  }

  // ---- Overlay click handler ----
  function onOverlayClick(e: MouseEvent) {
    if ((e.target as HTMLElement).classList.contains('flight-deck-overlay')) {
      props.onClose();
    }
  }

  function onOverlayKeyDown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.stopPropagation();
      props.onClose();
    }
  }

  // ===========================================================================
  // Render
  // ===========================================================================

  return (
    <>
    <Show when={props.open()}>
      <div class="flight-deck-overlay" data-testid="flight-deck-overlay" onClick={onOverlayClick} onKeyDown={onOverlayKeyDown}>
        <div class="flight-deck-panel" data-testid="flight-deck-panel">
          {/* ---- Header ---- */}
          <div class="flight-deck-header">
            <span><PlaneTakeoff size={14} /> Flight Deck</span>
            <button class="flight-deck-action-btn" data-testid="flight-deck-close" aria-label="Close flight deck" onClick={() => props.onClose()}>
              ✕
            </button>
          </div>

          {/* ---- Tabs + Content ---- */}
          <Tabs value={activeTab()} onChange={(v) => setActiveTab(v as TabId)} class="flight-deck-tabs-root">
            <nav class="flight-deck-tabs">
              <For each={TABS}>
                {(tab) => (
                  <button
                    class={`flight-deck-tab ${activeTab() === tab.id ? 'active' : ''}`}
                    data-testid={`fd-tab-${tab.id}`}
                    aria-label={tab.label}
                    title={tab.label}
                    onClick={() => setActiveTab(tab.id)}
                  >
                    <span class="flight-deck-tab-icon">{tab.icon}</span>
                    <span class="flight-deck-tab-label">{tab.label}</span>
                    <Show when={badgeForTab(tab.id) !== undefined}>
                      <span class={`flight-deck-tab-badge${badgeSeverityForTab(tab.id) === 'warn' ? ' warn' : ''}`}>
                        {badgeForTab(tab.id)}
                      </span>
                    </Show>
                  </button>
                )}
              </For>
            </nav>

          {/* ---- Content ---- */}
          <div class="flight-deck-content">
            <Show when={error()}>
              <div class="flight-deck-empty text-destructive">
                {error()}
              </div>
            </Show>

            <Show when={!props.daemonOnline()}>
              <div class="flight-deck-empty">Daemon offline</div>
            </Show>

            <Show when={props.daemonOnline()}>
                {/* ============================================================
                    Tab 1: Agents
                    ============================================================ */}
                <TabsContent value="agents" class="flight-deck-tab-panel">
                  <AgentsPanel
                    agents={activeAgents}
                    selectedAgentId={selectedAgentId}
                    setSelectedAgentId={setSelectedAgentId}
                    busyAgentId={busyAgentId}
                    runtimeTick={runtimeTick}
                    agentTelemetryMap={agentTelemetryMap}
                    polledQuestions={polledQuestions}
                    polledApprovals={polledApprovals}
                    pendingQuestions={props.pendingQuestions}
                    setQuestionDialogQuestion={setQuestionDialogQuestion}
                    setQuestionFreeText={setQuestionFreeText}
                    setQuestionSending={setQuestionSending}
                    setApprovalDialogItem={setApprovalDialogItem}
                    setApprovalSending={setApprovalSending}
                    personas={props.personas}
                    fdPromptPickerFor={fdPromptPickerFor}
                    setFdPromptPickerFor={setFdPromptPickerFor}
                    setFdActivePrompt={setFdActivePrompt}
                    requestPauseAgent={requestPauseAgent}
                    requestResumeAgent={requestResumeAgent}
                    requestKillAgent={requestKillAgent}
                    restartAgent={restartAgent}
                    openConfigDialog={openConfigDialog}
                    toggleExpandAgent={toggleExpandAgent}
                  />
                </TabsContent>

                {/* ============================================================
                    Tab 2: Workflows
                    ============================================================ */}
                <TabsContent value="workflows" class="flight-deck-tab-panel">
                  <WorkflowsPanel
                    activeWorkflows={activeWorkflows}
                    selectedWfId={selectedWfId}
                    setSelectedWfId={setSelectedWfId}
                    workflowDetail={workflowDetail}
                    workflowDetailLoading={workflowDetailLoading}
                    runtimeTick={runtimeTick}
                    polledQuestions={polledQuestions}
                    polledApprovals={polledApprovals}
                    pendingQuestions={props.pendingQuestions}
                    setApprovalDialogItem={setApprovalDialogItem}
                    setApprovalSending={setApprovalSending}
                    setQuestionDialogQuestion={setQuestionDialogQuestion}
                    setQuestionFreeText={setQuestionFreeText}
                    setQuestionSending={setQuestionSending}
                    requestPauseWorkflow={requestPauseWorkflow}
                    requestResumeWorkflow={requestResumeWorkflow}
                    requestKillWorkflow={requestKillWorkflow}
                    openFeedbackGate={openFeedbackGate}
                  />
                </TabsContent>

                {/* ============================================================
                    Tab 3: Active Triggers
                    ============================================================ */}
                <TabsContent value="triggers" class="flight-deck-tab-panel">
                  <div class="fd-triggers-container">
                    <Show
                      when={activeTriggers().length > 0 || activeEventGates().length > 0}
                      fallback={
                        <div class="flight-deck-empty">
                          No active triggers registered.
                          <br />
                          <span style={{ 'font-size': '0.85em', opacity: 0.7 }}>
                            Triggers are registered when workflow definitions with trigger steps are saved.
                          </span>
                        </div>
                      }
                    >
                      {/* ---- Trigger registrations ---- */}
                      <Show when={activeTriggers().length > 0}>
                        <div class="fd-triggers-section">
                          <h4 class="fd-triggers-section-title">
                            Trigger Registrations ({activeTriggers().length})
                          </h4>
                          <div class="fd-triggers-list">
                            <For each={activeTriggers()}>
                              {(trigger) => {
                                const icon = (): JSX.Element => {
                                  switch (trigger.trigger_kind) {
                                    case 'schedule': return <Clock size={14} />;
                                    case 'event_pattern': return <Send size={14} />;
                                    case 'incoming_message': return <Mail size={14} />;
                                    case 'mcp_notification': return <Plug size={14} />;
                                    case 'manual': return <MousePointer size={14} />;
                                    default: return <Bell size={14} />;
                                  }
                                };
                                const detail = () => {
                                  const tt = trigger.trigger_type;
                                  switch (tt.type) {
                                    case 'schedule':
                                      return `cron: ${tt.cron}`;
                                    case 'event_pattern':
                                      return `topic: ${tt.topic}${tt.filter ? ` (filter: ${tt.filter})` : ''}`;
                                    case 'incoming_message':
                                      return `channel: ${tt.channel_id}${tt.listen_channel_id ? ` → ${tt.listen_channel_id}` : ''}${tt.from_filter ? ` from: ${tt.from_filter}` : ''}`;
                                    case 'mcp_notification':
                                      return `server: ${tt.server_id}${tt.kind ? ` kind: ${tt.kind}` : ''}`;
                                    case 'manual':
                                      return 'awaiting manual launch';
                                    default:
                                      return '';
                                  }
                                };
                                const nextRun = () => {
                                  runtimeTick(); // subscribe to tick updates
                                  if (!trigger.next_run_ms) return null;
                                  const diffMs = trigger.next_run_ms - Date.now();
                                  if (diffMs <= 0) return 'imminent';
                                  const secs = Math.floor(diffMs / 1000);
                                  if (secs < 60) return `${secs}s`;
                                  const mins = Math.floor(secs / 60);
                                  if (mins < 60) return `${mins}m ${secs % 60}s`;
                                  const hrs = Math.floor(mins / 60);
                                  return `${hrs}h ${mins % 60}m`;
                                };
                                return (
                                  <div class={`fd-trigger-card fd-trigger-${trigger.trigger_kind}`}>
                                    <div class="fd-trigger-header">
                                      <span class="fd-trigger-icon">{icon()}</span>
                                      <span class="fd-trigger-workflow">
                                        {trigger.definition_name}
                                        <span class="fd-trigger-version">v{trigger.definition_version}</span>
                                      </span>
                                      <span class={`fd-trigger-kind fd-trigger-kind-${trigger.trigger_kind}`}>
                                        {trigger.trigger_kind.replace('_', ' ')}
                                      </span>
                                    </div>
                                    <div class="fd-trigger-detail">{detail()}</div>
                                    <Show when={nextRun()}>
                                      {(nr) => (
                                        <div class="fd-trigger-next-run">
                                          <Clock size={14} /> next run: <strong>{nr()}</strong>
                                        </div>
                                      )}
                                    </Show>
                                  </div>
                                );
                              }}
                            </For>
                          </div>
                        </div>
                      </Show>

                      {/* ---- Event gates (from running workflow steps) ---- */}
                      <Show when={activeEventGates().length > 0}>
                        <div class="fd-triggers-section">
                          <h4 class="fd-triggers-section-title">
                            Active Event Gates ({activeEventGates().length})
                          </h4>
                          <div class="fd-triggers-list">
                            <For each={activeEventGates()}>
                              {(gate) => {
                                const expiresIn = () => {
                                  runtimeTick(); // subscribe to tick updates
                                  if (!gate.expires_at_ms) return null;
                                  const diffMs = gate.expires_at_ms - Date.now();
                                  if (diffMs <= 0) return 'expired';
                                  const secs = Math.floor(diffMs / 1000);
                                  if (secs < 60) return `${secs}s`;
                                  const mins = Math.floor(secs / 60);
                                  return `${mins}m ${secs % 60}s`;
                                };
                                return (
                                  <div class="fd-trigger-card fd-trigger-gate">
                                    <div class="fd-trigger-header">
                                      <span class="fd-trigger-icon"><Hourglass size={14} /></span>
                                      <span class="fd-trigger-workflow">
                                        {gate.instance_id}
                                        <span class="fd-trigger-version">step: {gate.step_id}</span>
                                      </span>
                                      <span class="fd-trigger-kind fd-trigger-kind-event_pattern">
                                        event gate
                                      </span>
                                    </div>
                                    <div class="fd-trigger-detail">
                                      topic: {gate.topic}
                                      {gate.filter ? ` (filter: ${gate.filter})` : ''}
                                    </div>
                                    <Show when={expiresIn()}>
                                      {(exp) => (
                                        <div class="fd-trigger-next-run">
                                          <Clock size={14} /> expires in: <strong>{exp()}</strong>
                                        </div>
                                      )}
                                    </Show>
                                  </div>
                                );
                              }}
                            </For>
                          </div>
                        </div>
                      </Show>
                    </Show>
                  </div>
                </TabsContent>

                {/* ============================================================
                    Tab 4: Chat Sessions
                    ============================================================ */}
                <TabsContent value="sessions" class="flight-deck-tab-panel">
                  <Show
                    when={sessions().length > 0}
                    fallback={
                      <div class="flight-deck-empty">No active sessions</div>
                    }
                  >
                    <div class="flight-deck-list">
                      <For each={sessions()}>
                        {(session) => {
                          const tel = () => telemetryFor(session.id);
                          return (
                            <div class="flight-deck-item">
                              <div class="flight-deck-item-header">
                                <span class="flight-deck-item-modality">
                                  {session.modality === 'spatial' ? <MapIcon size={14} /> : <ClipboardList size={14} />}
                                </span>
                                <span class="flight-deck-item-name">
                                  {session.title || 'Untitled'}
                                </span>
                                <span
                                  class={`flight-deck-status ${chatStateClass(session.state)}`}
                                >
                                  {session.state}
                                </span>
                              </div>
                              <div class="flight-deck-item-body">
                                <Show when={session.queued_count > 0}>
                                  <div>
                                    <strong>Queued:</strong> {session.queued_count}
                                  </div>
                                </Show>
                                <Show when={session.last_message_preview}>
                                  <div class="flight-deck-last-message">
                                    {session.last_message_preview}
                                  </div>
                                </Show>
                                <Show when={tel()}>
                                  <div class="flight-deck-session-stats">
                                    <span>
                                      Model calls:{' '}
                                      {formatNumber(tel()!.telemetry.total.model_calls)}
                                    </span>
                                    <span>
                                      Tool calls:{' '}
                                      {formatNumber(tel()!.telemetry.total.tool_calls)}
                                    </span>
                                    <span>
                                      Tokens:{' '}
                                      {formatNumber(
                                        tel()!.telemetry.total.input_tokens +
                                          tel()!.telemetry.total.output_tokens,
                                      )}
                                    </span>
                                  </div>
                                </Show>
                              </div>
                            </div>
                          );
                        }}
                      </For>
                    </div>
                  </Show>
                </TabsContent>

                {/* ============================================================
                    Tab 5: Local Models
                    ============================================================ */}
                <TabsContent value="models" class="flight-deck-tab-panel">
                  {/* Active downloads */}
                  <Show when={downloads().length > 0}>
                    <div class="flight-deck-list">
                      <h4 style="margin: 0 0 8px">Downloads</h4>
                      <For each={downloads()}>
                        {(dl) => {
                          const pct = () =>
                            dl.total_bytes
                              ? Math.round(
                                  (dl.downloaded_bytes / dl.total_bytes) * 100,
                                )
                              : 0;
                          return (
                            <div class="flight-deck-item">
                              <div class="flight-deck-item-header">
                                <span class="flight-deck-item-name">
                                  {dl.model_id}
                                </span>
                                <span class="flight-deck-status downloading">
                                  {dl.status}
                                </span>
                              </div>
                              <div class="flight-deck-item-body">
                                <div class="flight-deck-progress-bar">
                                  <div
                                    class="flight-deck-progress-fill"
                                    style={{ width: `${pct()}%` }}
                                  />
                                </div>
                                <div>
                                  {formatBytes(dl.downloaded_bytes)}
                                  {dl.total_bytes ? ` / ${formatBytes(dl.total_bytes)}` : ''}
                                  {' · '}
                                  {pct()}%
                                </div>
                                <Show when={dl.error}>
                                  <div class="text-destructive">
                                    {dl.error}
                                  </div>
                                </Show>
                              </div>
                            </div>
                          );
                        }}
                      </For>
                    </div>
                  </Show>

                  {/* Installed models */}
                  <Show
                    when={models().length > 0}
                    fallback={
                      <Show when={downloads().length === 0}>
                        <div class="flight-deck-empty">
                          No local models installed
                        </div>
                      </Show>
                    }
                  >
                    <div class="flight-deck-list">
                      <h4 style="margin: 0 0 8px">Installed Models</h4>
                      <For each={models()}>
                        {(model) => (
                          <div class="flight-deck-item">
                            <div class="flight-deck-item-header">
                              <span class="flight-deck-item-name">{model.id}</span>
                              <span class="flight-deck-item-runtime">
                                {model.runtime}
                              </span>
                              <span class={`flight-deck-status ${model.status}`}>
                                {model.status}
                              </span>
                            </div>
                            <div class="flight-deck-item-body">
                              <div>
                                <strong>Size:</strong> {formatBytes(model.size_bytes)}
                              </div>
                              <Show when={model.capabilities}>
                                <div>
                                  <strong>Tasks:</strong>{' '}
                                  {model.capabilities?.tasks?.join(', ') || 'none'}
                                  {model.capabilities?.can_call_tools && <>{' · '}<Wrench size={14} /> tools</>}
                                  {model.capabilities?.has_reasoning && <>{' · '}<Brain size={14} /> reasoning</>}
                                  <Show when={model.capabilities?.context_length}>
                                    {' · '}ctx {formatNumber(model.capabilities!.context_length!)}
                                  </Show>
                                </div>
                              </Show>
                              <div class="flight-deck-actions">
                                <Show when={model.status === 'available'}>
                                  <button
                                    class="flight-deck-action-btn"
                                    title="Load model into memory"
                                    onClick={() => loadModel(model.id)}
                                  >
                                    <Upload size={14} /> Load
                                  </button>
                                </Show>
                                <Show when={model.status === 'loaded'}>
                                  <button
                                    class="flight-deck-action-btn"
                                    title="Unload model from memory"
                                    onClick={() => unloadModel(model.id)}
                                  >
                                    <Download size={14} /> Unload
                                  </button>
                                </Show>
                              </div>
                            </div>
                          </div>
                        )}
                      </For>
                    </div>
                  </Show>
                </TabsContent>

                {/* ============================================================
                    Tab 6: Event Bus
                    ============================================================ */}
                <TabsContent value="events" class="flight-deck-tab-panel">
                  <div class="fd-events-container">
                    {/* Filter bar */}
                    <div class="fd-events-filter-bar">
                      <div class="fd-events-filter-group">
                        <input
                          type="text"
                          class="fd-events-filter-input"
                          placeholder="Filter by topic (e.g. chat.* or workflow.definition.*)"
                          value={eventTopicFilter()}
                          onInput={(e) => setEventTopicFilter(e.currentTarget.value)}
                          onKeyDown={(e) => { if (e.key === 'Enter') fetchEvents(true); }}
                        />
                        <select
                          class="fd-events-topic-dropdown"
                          value=""
                          onChange={(e) => {
                            const v = e.currentTarget.value;
                            if (v) {
                              setEventTopicFilter(v);
                              fetchEvents(true);
                            }
                            e.currentTarget.value = '';
                          }}
                        >
                          <option value="">Known topics…</option>
                          <For each={eventTopics()}>
                            {(t) => (
                              <option value={t.topic} title={t.description}>
                                {t.topic}
                              </option>
                            )}
                          </For>
                        </select>
                      </div>
                      <div class="fd-events-filter-actions">
                        <button
                          class={`btn fd-events-btn ${eventPaused() ? 'fd-events-btn-paused' : ''}`}
                          onClick={() => {
                            setEventPaused(!eventPaused());
                            if (!eventPaused()) fetchEvents(true);
                          }}
                          title={eventPaused() ? 'Resume auto-refresh' : 'Pause auto-refresh'}
                        >
                          {eventPaused() ? <><Play size={14} /> Resume</> : <><Pause size={14} /> Pause</>}
                        </button>
                        <button
                          class="btn fd-events-btn"
                          onClick={() => { setEventTopicFilter(''); fetchEvents(true); }}
                          title="Clear filter"
                        >
                          ✕ Clear
                        </button>
                        <button
                          class="btn fd-events-btn"
                          onClick={() => fetchEvents(true)}
                          title="Refresh now"
                        >
                          <RefreshCw size={14} />
                        </button>
                      </div>
                    </div>

                    {/* Event count */}
                    <div class="fd-events-count">
                      {events().length} event{events().length !== 1 ? 's' : ''}
                      {eventTopicFilter().trim() ? ` matching "${eventTopicFilter().trim()}"` : ''}
                      {eventPaused() ? ' (paused)' : ''}
                    </div>

                    <Show when={eventFetchError()}>
                      <div class="fd-events-error">
                        Failed to fetch events: {eventFetchError()}
                      </div>
                    </Show>

                    {/* Event list */}
                    <Show
                      when={events().length > 0}
                      fallback={
                        <div class="flight-deck-empty">
                          {eventTopicFilter().trim()
                            ? 'No events match the current filter'
                            : 'No events recorded yet'}
                        </div>
                      }
                    >
                      <div class="fd-events-list" ref={eventsListRef}>
                        <For each={events()}>
                          {(evt) => {
                            const isExpanded = () => expandedEventId() === evt.id;
                            return (
                              <Collapsible open={isExpanded()}>
                              <div
                                class={`fd-event-row ${isExpanded() ? 'fd-event-row-expanded' : ''}`}
                                onClick={() => setExpandedEventId(isExpanded() ? null : evt.id)}
                              >
                                <div class="fd-event-header">
                                  <span class="fd-event-time">
                                    {formatEventTimestamp(evt.timestamp_ms)}
                                  </span>
                                  <span class={`fd-event-topic ${eventTopicColorClass(evt.topic)}`}>
                                    {evt.topic}
                                  </span>
                                  <span class="fd-event-source" title={evt.source ?? ''}>
                                    {(evt.source ?? '').length > 30
                                      ? (evt.source ?? '').slice(0, 27) + '…'
                                      : evt.source ?? ''}
                                  </span>
                                  <span class="fd-event-expand-indicator">
                                    {isExpanded() ? '▼' : '▸'}
                                  </span>
                                </div>
                                <CollapsibleContent>
                                  <div class="fd-event-payload" onClick={(e) => e.stopPropagation()}>
                                    <pre innerHTML={DOMPurify.sanitize(highlightYaml(evt.payload))} />
                                  </div>
                                </CollapsibleContent>
                              </div>
                              </Collapsible>
                            );
                          }}
                        </For>
                        <Show when={hasMoreEvents() && !loadingMoreEvents()}>
                          <div style="padding:8px;text-align:center;">
                            <button
                              class="btn fd-events-btn"
                              onClick={() => void loadMoreEvents()}
                            >
                              ▼ Load older events
                            </button>
                          </div>
                        </Show>
                        <Show when={loadingMoreEvents()}>
                          <div style="padding:8px;text-align:center;">
                            <span class="text-xs text-muted-foreground">Loading…</span>
                          </div>
                        </Show>
                      </div>
                    </Show>
                  </div>
                </TabsContent>

                {/* ============================================================
                    Tab: Services
                    ============================================================ */}
                <TabsContent value="services" class="flight-deck-tab-panel">
                  <div class="flight-deck-list" style={{ gap: '12px' }}>
                    <For each={['core', 'agents', 'connector', 'mcp', 'inference'] as const}>
                      {(category) => {
                        const catServices = () => servicesList().filter(s => s.category === category);
                        return (
                          <Show when={catServices().length > 0}>
                            <div>
                              <h4 class="text-muted-foreground" style={{ margin: '0 0 6px', 'text-transform': 'capitalize', 'font-size': '11px', 'letter-spacing': '0.5px' }}>
                                {category === 'mcp' ? 'MCP Servers' : category.charAt(0).toUpperCase() + category.slice(1)}
                              </h4>
                              <For each={catServices()}>
                                {(svc) => (
                                  <div
                                    class="flight-deck-service-row"
                                    onClick={() => {
                                      setServiceLogViewer(svc.id);
                                      setServiceLogs([]);
                                      setServiceLogSearch('');
                                      setServiceLogLevel('');
                                      fetchServiceLogs(svc.id);
                                    }}
                                  >
                                    <span class={`flight-deck-service-status flight-deck-service-status--${svc.status}`} />
                                    <span class="flight-deck-service-name">{svc.display_name}</span>
                                    <span class="flight-deck-service-status-label">{svc.status}</span>
                                    <Show when={svc.last_error}>
                                      <span class="flight-deck-service-error" title={svc.last_error!}>{svc.last_error}</span>
                                    </Show>
                                    <button
                                      class="flight-deck-btn flight-deck-btn--small"
                                      title="Restart service"
                                      onClick={(e) => {
                                        e.stopPropagation();
                                        setPendingConfirm({
                                          title: <><RefreshCw size={14} /> Restart {svc.display_name}</>,
                                          message: `Are you sure you want to restart "${svc.display_name}"?`,
                                          confirmLabel: 'Restart',
                                          destructive: false,
                                          onConfirm: async () => {
                                            await restartService(svc.id);
                                          },
                                        });
                                      }}
                                    >
                                      <RefreshCw size={12} />
                                    </button>
                                  </div>
                                )}
                              </For>
                            </div>
                          </Show>
                        );
                      }}
                    </For>
                    <Show when={servicesList().length === 0}>
                      <div class="flight-deck-empty">No services registered</div>
                    </Show>
                  </div>

                  {/* ---- Service Log Viewer Dialog ---- */}
                  <Dialog open={!!serviceLogViewer()} onOpenChange={(open) => { if (!open) setServiceLogViewer(null); }}>
                  <DialogContent class="max-w-4xl p-0">
                    <Show when={serviceLogViewer()}>
                      {(svcId) => {
                        const svc = () => servicesList().find(s => s.id === svcId());
                        return (
                          <div class="flight-deck-log-dialog">
                            <div class="flight-deck-log-dialog-header">
                              <div style={{ display: 'flex', 'align-items': 'center', gap: '8px' }}>
                                <span class={`flight-deck-service-status flight-deck-service-status--${svc()?.status ?? 'stopped'}`} />
                                <h3 style={{ margin: 0 }}>{svc()?.display_name ?? svcId()}</h3>
                              </div>
                              <div style={{ display: 'flex', gap: '6px', 'align-items': 'center' }}>
                                <button
                                  class="flight-deck-btn flight-deck-btn--small"
                                  title="Copy logs to clipboard"
                                  onClick={() => {
                                    const text = serviceLogs().map(l => `[${new Date(l.timestamp_ms).toISOString()}] ${l.level} ${l.message}`).join('\n');
                                    navigator.clipboard.writeText(text).catch(() => {});
                                  }}
                                >
                                  <Copy size={12} />
                                </button>
                                <button
                                  class="flight-deck-btn flight-deck-btn--small"
                                  title="Refresh logs"
                                  onClick={() => fetchServiceLogs(svcId())}
                                >
                                  <RefreshCw size={12} />
                                </button>
                                <button
                                  class="flight-deck-btn flight-deck-btn--small"
                                  onClick={() => setServiceLogViewer(null)}
                                >
                                  <X size={14} />
                                </button>
                              </div>
                            </div>
                            <div class="flight-deck-log-dialog-filters">
                              <div style={{ position: 'relative', flex: 1 }}>
                                <Search size={12} class="text-muted-foreground" style={{ position: 'absolute', left: '8px', top: '50%', transform: 'translateY(-50%)' }} />
                                <input
                                  type="text"
                                  placeholder="Search logs..."
                                  value={serviceLogSearch()}
                                  onInput={(e) => setServiceLogSearch(e.currentTarget.value)}
                                  onKeyDown={(e) => { if (e.key === 'Enter') fetchServiceLogs(svcId()); }}
                                  style={{ width: '100%', 'padding-left': '28px' }}
                                />
                              </div>
                              <select
                                value={serviceLogLevel()}
                                onChange={(e) => {
                                  setServiceLogLevel(e.currentTarget.value);
                                  fetchServiceLogs(svcId());
                                }}
                              >
                                <option value="">All levels</option>
                                <option value="ERROR">ERROR</option>
                                <option value="WARN">WARN</option>
                                <option value="INFO">INFO</option>
                                <option value="DEBUG">DEBUG</option>
                                <option value="TRACE">TRACE</option>
                              </select>
                            </div>
                            <div class="flight-deck-log-dialog-content">
                              <Show when={!serviceLogsLoading()} fallback={<div class="flight-deck-empty">Loading logs…</div>}>
                                <Show when={serviceLogs().length > 0} fallback={<div class="flight-deck-empty">No log entries</div>}>
                                  <For each={serviceLogs()}>
                                    {(entry) => (
                                      <div class="flight-deck-log-entry">
                                        <span class="flight-deck-log-time" title={new Date(entry.timestamp_ms).toISOString()}>
                                          {new Date(entry.timestamp_ms).toLocaleTimeString()}
                                        </span>
                                        <span class={`flight-deck-log-level flight-deck-log-level--${entry.level.toLowerCase()}`}>
                                          {entry.level}
                                        </span>
                                        <span class="flight-deck-log-msg">{entry.message}</span>
                                      </div>
                                    )}
                                  </For>
                                </Show>
                              </Show>
                            </div>
                          </div>
                        );
                      }}
                    </Show>
                  </DialogContent>
                  </Dialog>
                </TabsContent>

                {/* ============================================================
                    Tab 7: System Health
                    ============================================================ */}
                <TabsContent value="health" class="flight-deck-tab-panel">
                  <Show
                    when={health()}
                    fallback={
                      <div class="flight-deck-empty">Loading health data…</div>
                    }
                  >
                    {(h) => (
                      <div class="flight-deck-stat-grid">
                        {/* Daemon */}
                        <div class="flight-deck-stat-card">
                          <h4><Monitor size={14} /> Daemon</h4>
                          <div>
                            <span class="flight-deck-stat-label">Version</span>
                            <span class="flight-deck-stat-value">
                              {h().version}
                            </span>
                          </div>
                          <div>
                            <span class="flight-deck-stat-label">Uptime</span>
                            <span class="flight-deck-stat-value">
                              {formatUptime(h().uptime_secs)}
                            </span>
                          </div>
                          <div>
                            <span class="flight-deck-stat-label">PID</span>
                            <span class="flight-deck-stat-value">{h().pid}</span>
                          </div>
                          <div>
                            <span class="flight-deck-stat-label">Platform</span>
                            <span class="flight-deck-stat-value">
                              {h().platform}
                            </span>
                          </div>
                        </div>

                        {/* LLM Usage */}
                        <div class="flight-deck-stat-card">
                          <h4><BarChart3 size={14} /> LLM Usage</h4>
                          <div>
                            <span class="flight-deck-stat-label">Model Calls</span>
                            <span class="flight-deck-stat-value">
                              {formatNumber(h().total_llm_calls)}
                            </span>
                          </div>
                          <div>
                            <span class="flight-deck-stat-label">Input Tokens</span>
                            <span class="flight-deck-stat-value">
                              {formatNumber(h().total_input_tokens)}
                            </span>
                          </div>
                          <div>
                            <span class="flight-deck-stat-label">
                              Output Tokens
                            </span>
                            <span class="flight-deck-stat-value">
                              {formatNumber(h().total_output_tokens)}
                            </span>
                          </div>
                        </div>

                        {/* Active Resources */}
                        <div class="flight-deck-stat-card">
                          <h4><Settings size={14} /> Active Resources</h4>
                          <div>
                            <span class="flight-deck-stat-label">Sessions</span>
                            <span class="flight-deck-stat-value">
                              {h().active_session_count}
                            </span>
                          </div>
                          <div>
                            <span class="flight-deck-stat-label">Agents</span>
                            <span class="flight-deck-stat-value">
                              {h().active_agent_count}
                            </span>
                          </div>
                          <div>
                            <span class="flight-deck-stat-label">Workflows</span>
                            <span class="flight-deck-stat-value">
                              {h().active_workflow_count}
                            </span>
                          </div>
                        </div>

                        {/* MCP Servers */}
                        <div class="flight-deck-stat-card">
                          <h4><Plug size={14} /> MCP Servers</h4>
                          <div>
                            <span class="flight-deck-stat-label">Connected</span>
                            <span class="flight-deck-stat-value">
                              {h().mcp_connected_count ?? 0} / {h().mcp_total_count ?? 0}
                            </span>
                          </div>
                        </div>

                        {/* Knowledge Base */}
                        <div class="flight-deck-stat-card">
                          <h4><Dna size={14} /> Knowledge Base</h4>
                          <div>
                            <span class="flight-deck-stat-label">Nodes</span>
                            <span class="flight-deck-stat-value">
                              {formatNumber(h().knowledge_node_count)}
                            </span>
                          </div>
                          <div>
                            <span class="flight-deck-stat-label">Edges</span>
                            <span class="flight-deck-stat-value">
                              {formatNumber(h().knowledge_edge_count)}
                            </span>
                          </div>
                        </div>

                        {/* Local Models */}
                        <div class="flight-deck-stat-card">
                          <h4><Brain size={14} /> Local Models</h4>
                          <div>
                            <span class="flight-deck-stat-label">Installed</span>
                            <span class="flight-deck-stat-value">
                              {h().local_model_count}
                            </span>
                          </div>
                          <div>
                            <span class="flight-deck-stat-label">Loaded</span>
                            <span class="flight-deck-stat-value">
                              {h().loaded_model_count}
                            </span>
                          </div>
                        </div>
                      </div>
                    )}
                  </Show>
                </TabsContent>

                {/* ============================================================
                    Tab 9: Knowledge
                    ============================================================ */}
                <TabsContent value="knowledge" class="flight-deck-tab-panel">
                  <Show
                    when={props.daemon_url && props.kgStats && props.loadKgStats}
                  >
                    <KnowledgeExplorer
                      daemonOnline={props.daemonOnline}
                      daemon_url={props.daemon_url!}
                      kgStats={props.kgStats!}
                      loadKgStats={props.loadKgStats!}
                    />
                  </Show>
                </TabsContent>
            </Show>
          </div>
          </Tabs>
        </div>
      </div>
    </Show>

      {/* ---- Confirmation dialog ---- */}
      <Dialog open={!!pendingConfirm()} onOpenChange={(open) => { if (!open) setPendingConfirm(null); }}>
      <DialogContent class="max-w-md p-0">
        <Show when={pendingConfirm()}>
          {(confirm) => (
            <div class="flight-deck-confirm-dialog">
              <h3>{confirm().title}</h3>
              <p>{confirm().message}</p>
              <div class="flight-deck-confirm-buttons">
                <Button variant="outline" onClick={() => setPendingConfirm(null)}>
                  Cancel
                </Button>
                <Button
                  variant={confirm().destructive ? 'destructive' : 'default'}
                  onClick={async () => {
                    const action = confirm().onConfirm;
                    setPendingConfirm(null);
                    try {
                      await action();
                    } catch (e: any) {
                      console.error('Action failed', e);
                    }
                  }}
                >
                  {confirm().confirmLabel}
                </Button>
              </div>
            </div>
          )}
        </Show>
      </DialogContent>
      </Dialog>

      {/* ---- Event log dialog ---- */}
      <Dialog open={!!expandedAgentId()} onOpenChange={(open) => { if (!open) { setExpandedAgentId(null); setAgentEvents([]); setAgentEventsTotal(0); } }}>
      <DialogContent class="max-w-2xl p-0">
        <Show when={expandedAgentId()}>
          {(eid) => {
            const logAgent = () => agents().find((a) => a.agent_id === eid());
            return (
              <Show when={logAgent()}>
                {(la) => (
                  <div class="flight-deck-confirm-dialog fd-log-dialog">
                    <div class="fd-dialog-header">
                      <span class="flight-deck-item-avatar">{la().spec.avatar || <Bot size={14} />}</span>
                      <div style="flex:1;">
                        <div class="fd-dialog-title">
                          {la().spec.friendly_name || la().spec.name} — Event Log
                          <Show when={agentEventsTotal() > 0}>
                            <span class="text-muted-foreground" style="font-weight:400; font-size:0.85em; margin-left:6px;">
                              ({agentEventsTotal()} events)
                            </span>
                          </Show>
                        </div>
                        <div class="fd-dialog-subtitle">{la().agent_id}</div>
                      </div>
                      <button
                        class="flight-deck-action-btn"
                        onClick={() => { setExpandedAgentId(null); setAgentEvents([]); setAgentEventsTotal(0); }}
                      >
                        ✕
                      </button>
                    </div>
                    <div class="fd-log-body">
                      <EventLogList
                        events={agentEvents()}
                        totalCount={agentEventsTotal()}
                        loading={eventsLoading()}
                        hasMore={agentEvents().length < agentEventsTotal()}
                        onLoadMore={() => {
                          const agent = logAgent();
                          if (agent) void loadAgentEvents(agent, true);
                        }}
                        onApprove={(reqId, approved) => {
                          const agent = logAgent();
                          if (agent) void respondToApproval(agent, reqId, approved);
                        }}
                      />
                    </div>
                  </div>
                )}
              </Show>
            );
          }}
        </Show>
      </DialogContent>
      </Dialog>

      {/* ---- Reconfigure dialog ---- */}
      <Dialog open={!!configAgent()} onOpenChange={(open) => { if (!open) setConfigAgent(null); }}>
      <DialogContent class="max-w-xl p-0">
        <Show when={configAgent()}>
          {(agent) => {
            const a = agent();
            const currentModel = () => a.active_model || a.spec.model || '';
            const modelChanged = () => configModel() !== currentModel();

            return (
              <div class="flight-deck-confirm-dialog fd-config-dialog">
                <div class="fd-dialog-header">
                  <span class="flight-deck-item-avatar">{a.spec.avatar || <Bot size={14} />}</span>
                  <div style="flex:1;">
                    <div class="fd-dialog-title">
                      Reconfigure {a.spec.friendly_name || a.spec.name}
                    </div>
                    <div class="fd-dialog-subtitle">{a.spec.description}</div>
                  </div>
                  <Show when={a.session_id}>
                    <button
                      class="bg-destructive text-foreground"
                      style="border:none;border-radius:6px;padding:4px 12px;font-size:0.82em;cursor:pointer;"
                      disabled={busyAgentId() === a.agent_id}
                      onClick={() => void restartAgent(a, modelChanged() ? configModel() : undefined)}
                      title="Restart agent"
                    >
                      {busyAgentId() === a.agent_id ? 'Restarting…' : <><RefreshCw size={14} /> Restart</>}
                    </button>
                  </Show>
                  <button
                    class="flight-deck-action-btn"
                    onClick={() => setConfigAgent(null)}
                  >
                    ✕
                  </button>
                </div>

                <div class="fd-config-body">
                  <div class="fd-config-section">
                    <label class="fd-config-label">Status</label>
                    <div>
                      <span class={`flight-deck-status ${agentStatusColor(a.status)}`}>
                        {a.status}
                      </span>
                    </div>
                  </div>

                  <Show when={a.status === 'error' && a.last_error}>
                    <div class="fd-config-section">
                      <label class="fd-config-label">Error</label>
                      <div class="text-destructive">{a.last_error}</div>
                    </div>
                  </Show>

                  <div class="fd-config-section">
                    <label class="fd-config-label">Model</label>
                    <Show
                      when={availableModels().length > 0}
                      fallback={<div>{currentModel() || 'Unknown'}</div>}
                    >
                      <select
                        class="fd-config-select"
                        value={configModel()}
                        onInput={(e) => setConfigModel(e.currentTarget.value)}
                      >
                        <Show when={currentModel() && !availableModels().some((m) => m.id === currentModel())}>
                          <option value={currentModel()}>{currentModel()} (current)</option>
                        </Show>
                        <For each={availableModels()}>
                          {(model) => (
                            <option value={model.id} selected={model.id === configModel()}>
                              {model.label}{model.id === currentModel() ? ' (current)' : ''}
                            </option>
                          )}
                        </For>
                      </select>
                    </Show>
                  </div>

                  <div class="fd-config-section">
                    <label class="fd-config-label">Tools ({a.tools.length})</label>
                    <div class="fd-config-tools">
                      <For each={a.tools}>
                        {(tool_id) => <span class="fd-tool-chip">{tool_id}</span>}
                      </For>
                      <Show when={a.tools.length === 0}>
                        <span class="text-muted-foreground">No tools</span>
                      </Show>
                    </div>
                  </div>

                  <Show when={a.session_id}>
                    <div class="fd-config-section">
                      <label class="fd-config-label">Session</label>
                      <div><code>{a.session_id}</code></div>
                    </div>
                  </Show>
                </div>

                <div class="flight-deck-confirm-buttons">
                  <Button variant="outline" onClick={() => setConfigAgent(null)}>
                    Close
                  </Button>
                  <Show when={a.session_id && modelChanged()}>
                    <Button
                      disabled={busyAgentId() === a.agent_id}
                      onClick={() => void restartAgent(a, configModel())}
                    >
                      Apply & Restart
                    </Button>
                  </Show>
                </div>
              </div>
            );
          }}
        </Show>
      </DialogContent>
      </Dialog>

      {/* ---- Question dialog ---- */}
      <Dialog open={!!questionDialogQuestion()} onOpenChange={(open) => { if (!open) setQuestionDialogQuestion(null); }}>
      <DialogContent class="max-w-lg p-0">
        <Show when={questionDialogQuestion()}>
          {(q) => {
            const questionAgent = () => {
              const found = agents().find((a) => a.agent_id === q().agent_id);
              if (found) return found;
              // Fallback for workflow child agents not in the agents list
              if (q().agent_id) {
                const fallbackId = q().agent_id!;
                const fallbackName = q().agent_name ?? fallbackId;
                return {
                  agent_id: fallbackId,
                  spec: {
                    id: fallbackId,
                    name: fallbackName,
                    friendly_name: fallbackName,
                    description: '',
                    role: 'assistant' as const,
                    system_prompt: '',
                    allowed_tools: [],
                  },
                  status: 'running' as AgentStatus,
                  tools: [],
                  session_id: null,
                  started_at_ms: null,
                } satisfies GlobalAgentEntry;
              }
              return undefined;
            };

            return (
              <div class="flight-deck-confirm-dialog fd-question-dialog">
                <div class="fd-dialog-header">
                  <span class="flight-deck-item-avatar"><HelpCircle size={16} /></span>
                  <div style="flex:1;">
                    <div class="fd-dialog-title">Question from agent</div>
                    <Show when={q().agent_name}>
                      <div class="fd-dialog-subtitle">{q().agent_name}</div>
                    </Show>
                  </div>
                  <button
                    class="flight-deck-action-btn"
                    onClick={() => setQuestionDialogQuestion(null)}
                  >
                    ✕
                  </button>
                </div>

                <div class="fd-config-body">
                  <Show when={q().message}>
                    <div class="fd-question-message">{q().message}</div>
                  </Show>
                  <div class="fd-question-text">{q().text}</div>

                  <Show when={q().choices.length > 0}>
                    <div class="fd-question-choices">
                      <For each={q().choices}>
                        {(choice, idx) => (
                          <button
                            class={q().multi_select && questionMsSelected().has(idx()) ? 'fd-question-choice fd-question-choice-selected' : 'fd-question-choice'}
                            disabled={questionSending()}
                            onClick={() => {
                              if (q().multi_select) {
                                setQuestionMsSelected((prev) => {
                                  const next = new Set(prev);
                                  if (next.has(idx())) next.delete(idx());
                                  else next.add(idx());
                                  return next;
                                });
                              } else {
                                const agent = questionAgent();
                                if (agent) void answerQuestion(agent, q(), idx(), choice);
                              }
                            }}
                          >
                            {choice}
                          </button>
                        )}
                      </For>
                    </div>
                    <Show when={q().multi_select}>
                      <button
                        class="fd-question-choice"
                        disabled={questionMsSelected().size === 0 || questionSending()}
                        onClick={() => {
                          const agent = questionAgent();
                          if (agent) {
                            const indices = [...questionMsSelected()].sort((a, b) => a - b);
                            void answerQuestion(agent, q(), undefined, undefined, indices);
                          }
                        }}
                      >
                        {questionSending() ? '…' : 'Submit'}
                      </button>
                    </Show>
                  </Show>

                  <div class="fd-question-freeform">
                    <input
                      type="text"
                      placeholder="Type your answer…"
                      value={questionFreeText()}
                      onInput={(e) => setQuestionFreeText(e.currentTarget.value)}
                      onKeyDown={(e) => {
                        if (e.key === 'Enter' && questionFreeText().trim()) {
                          e.preventDefault();
                          const agent = questionAgent();
                          if (agent) void answerQuestion(agent, q(), undefined, questionFreeText().trim());
                        }
                      }}
                      disabled={questionSending()}
                    />
                    <button
                      disabled={!questionFreeText().trim() || questionSending()}
                      onClick={() => {
                        const agent = questionAgent();
                        if (agent) void answerQuestion(agent, q(), undefined, questionFreeText().trim());
                      }}
                    >
                      {questionSending() ? '…' : '→'}
                    </button>
                  </div>
                </div>
              </div>
            );
          }}
        </Show>
      </DialogContent>
      </Dialog>

      {/* ---- Approval dialog ---- */}
      <Dialog open={!!approvalDialogItem()} onOpenChange={(open) => { if (!open) setApprovalDialogItem(null); }}>
      <DialogContent class="max-w-lg p-0">
        <Show when={approvalDialogItem()}>
          {(item) => {
            const approvalAgent = () => agents().find((a) => a.agent_id === item().agent_id);

            const handleApproval = async (approved: boolean, opts?: { allow_agent?: boolean; allow_session?: boolean }) => {
              if (approvalSending()) return;
              setApprovalSending(true);
              const a = item();
              const agent = approvalAgent();
              dismissAgentApproval(a.request_id);
              try {
                await routeRespondToApproval(
                  {
                    request_id: a.request_id,
                    entity_id: `agent/${a.agent_id}`,
                    source_name: a.agent_name || a.agent_id,
                    type: 'tool_approval',
                    session_id: agent?.session_id ?? undefined,
                    agent_id: a.agent_id,
                  } as PendingInteraction,
                  { approved, allow_agent: opts?.allow_agent, allow_session: opts?.allow_session },
                );
                const scope = opts?.allow_session ? ' (session)' : opts?.allow_agent ? ' (agent)' : '';
                logInfo('flight-deck', `${approved ? 'Approved' : 'Denied'}${scope} ${a.tool_id} for ${a.agent_name}`);
                setPolledApprovals(prev => prev.filter(p => p.request_id !== a.request_id));
                setApprovalDialogItem(null);
              } catch (err: any) {
                logError('flight-deck', `Failed to respond: ${err}`);
                setApprovalSending(false);
              }
            };

            return (
              <div class="flight-deck-confirm-dialog fd-approval-dialog">
                <div class="fd-dialog-header">
                  <span class="flight-deck-item-avatar"><ShieldAlert size={16} /></span>
                  <div style="flex:1;">
                    <div class="fd-dialog-title">Tool Approval Required</div>
                    <div class="fd-dialog-subtitle">
                      {item().agent_name || item().agent_id}
                    </div>
                  </div>
                  <button
                    class="flight-deck-action-btn"
                    onClick={() => setApprovalDialogItem(null)}
                  >
                    ✕
                  </button>
                </div>

                <div class="fd-config-body">
                  <div class="fd-config-section">
                    <label class="fd-config-label">Tool</label>
                    <div><strong>{item().tool_id}</strong></div>
                  </div>

                  <div class="fd-config-section">
                    <label class="fd-config-label">Reason</label>
                    <div>{item().reason}</div>
                  </div>

                  <Show when={item().input}>
                    <div class="fd-config-section">
                      <label class="fd-config-label">Input</label>
                      <pre class="fd-approval-input" innerHTML={DOMPurify.sanitize(highlightYaml(item().input!))} />
                    </div>
                  </Show>
                </div>

                <div class="flight-deck-confirm-buttons" style="flex-wrap:wrap;gap:8px;">
                  <Button
                    variant="outline"
                    disabled={approvalSending()}
                    onClick={() => void handleApproval(false)}
                  >
                    <XCircle size={14} /> Deny
                  </Button>
                  <Button
                    disabled={approvalSending()}
                    onClick={() => void handleApproval(true)}
                  >
                    {approvalSending() ? 'Sending…' : <><CheckCircle size={14} /> Approve</>}
                  </Button>
                  <Button
                    variant="outline"
                    disabled={approvalSending()}
                    onClick={() => void handleApproval(true, { allow_agent: true })}
                  >
                    Allow for Agent
                  </Button>
                  <Button
                    variant="outline"
                    disabled={approvalSending()}
                    onClick={() => void handleApproval(true, { allow_session: true })}
                  >
                    Allow for Session
                  </Button>
                </div>
              </div>
            );
          }}
        </Show>
      </DialogContent>
      </Dialog>

      {/* ---- Workflow feedback dialog ---- */}
      <Dialog open={!!feedbackStep()} onOpenChange={(open) => { if (!open) setFeedbackStep(null); }}>
      <DialogContent class="max-w-lg p-0">
        <Show when={feedbackStep()}>
          {(gate) => (
            <div class="flight-deck-confirm-dialog fd-question-dialog">
              <div class="fd-dialog-header">
                <span class="flight-deck-item-avatar"><Hand size={16} /></span>
                <div style="flex:1;">
                  <div class="fd-dialog-title">Feedback Required</div>
                  <div class="fd-dialog-subtitle">
                    Step: {gate().step_id} • Instance: {gate().instance_id}
                  </div>
                </div>
                <button
                  class="flight-deck-action-btn"
                  onClick={() => setFeedbackStep(null)}
                >
                  ✕
                </button>
              </div>

              <div class="fd-config-body">
                <div class="fd-question-text prose prose-sm max-w-none text-foreground markdown-body" innerHTML={renderMarkdown(gate().prompt)} />

                <Show when={gate().choices.length > 0}>
                  <div class="fd-question-choices">
                    <For each={gate().choices}>
                      {(choice) => (
                        <button
                          class="fd-question-choice"
                          onClick={() => void submitFeedback(choice)}
                        >
                          {choice}
                        </button>
                      )}
                    </For>
                  </div>
                </Show>

                <Show when={gate().allow_freeform || gate().choices.length === 0}>
                  <div class="fd-question-freeform" style="flex-direction:column;">
                    <textarea
                      class="fd-feedback-textarea"
                      placeholder="Type your response…"
                      value={feedbackText()}
                      onInput={(e) => setFeedbackText(e.currentTarget.value)}
                    />
                    <div style="display:flex;justify-content:flex-end;gap:8px;">
                      <Button
                        variant="outline"
                        onClick={() => setFeedbackStep(null)}
                      >
                        Cancel
                      </Button>
                      <Button
                        disabled={!feedbackText().trim()}
                        onClick={() => void submitFeedback()}
                      >
                        Submit
                      </Button>
                    </div>
                  </div>
                </Show>

                <Show when={!gate().allow_freeform && gate().choices.length > 0}>
                  <div style="display:flex;justify-content:flex-end;">
                    <Button variant="outline" onClick={() => setFeedbackStep(null)}>Cancel</Button>
                  </div>
                </Show>
              </div>
            </div>
          )}
        </Show>
      </DialogContent>
      </Dialog>
      {/* Prompt parameter dialog for sending to agent from Flight Deck */}
      <Show when={fdActivePrompt()}>
        {(data) => {
          const d = data();
          return (
            <PromptParameterDialog
              template={d.template}
              persona_id={d.persona.id}
              submitLabel="Send to Agent"
              onSubmit={(rendered, params) => {
                const pid = d.persona.id;
                void invoke('send_prompt_to_bot', {
                  agent_id: d.agent_id,
                  persona_id: pid,
                  prompt_id: d.template.id,
                  params,
                }).catch((err: any) => logError('FlightDeck', `Failed to send prompt: ${err}`));
                setFdActivePrompt(null);
              }}
              onCancel={() => setFdActivePrompt(null)}
            />
          );
        }}
      </Show>
    </>
  );
};

export default FlightDeck;
