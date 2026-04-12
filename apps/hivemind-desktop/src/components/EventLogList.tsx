import { For, Show, createSignal, type JSX } from 'solid-js';
import { Dialog, DialogContent, DialogHeader, DialogTitle, Button } from '~/ui';
import { Rocket, ClipboardList, CheckCircle, Upload, Brain, MessageSquare, Wrench, XCircle, KeyRound, CircleHelp, Flag, FileText, Mail, X, Cpu, AlertTriangle, Users, ArrowRightLeft } from 'lucide-solid';
import { highlightYaml } from './YamlHighlight';

// ── Types ────────────────────────────────────────────────────────────

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
  tool_result_counts?: Record<string, number>;
  estimated_tokens?: number;
}

export interface SupervisorEvent {
  type: string;
  agent_id?: string;
  spec?: any;
  status?: string;
  task?: string;
  event?: ReasoningEvent;
  result?: string;
  from?: string;
  to?: string;
  msg_type?: string;
  parent_id?: string | null;
}

// LoopEvent variants (externally tagged serde — variant name is key)
interface LoopEventModelLoading { ModelLoading: { provider_id: string; model: string; tool_result_counts?: Record<string, number>; estimated_tokens?: number } }
interface LoopEventToken { Token: { delta: string } }
interface LoopEventModelDone { ModelDone: { content: string; provider_id: string; model: string } }
interface LoopEventToolCallStart { ToolCallStart: { tool_id: string; input: string } }
interface LoopEventToolCallResult { ToolCallResult: { tool_id: string; output: string; is_error: boolean } }
interface LoopEventUserInteraction { UserInteractionRequired: { request_id: string; kind: any } }
interface LoopEventDone { Done: { content: string; provider_id: string; model: string } }
interface LoopEventError { Error: { message: string } }
interface LoopEventAgentMessage { AgentSessionMessage: { from_agent_id: string; content: string } }
interface LoopEventModelFallback { ModelFallback: { from_provider: string; from_model: string; to_provider: string; to_model: string } }

type LoopEvent =
  | LoopEventModelLoading | LoopEventToken | LoopEventModelDone
  | LoopEventToolCallStart | LoopEventToolCallResult | LoopEventUserInteraction
  | LoopEventDone | LoopEventError | LoopEventAgentMessage | LoopEventModelFallback;

// A SessionEvent is either a SupervisorEvent (has "type" field) or a LoopEvent (variant name key)
export type SessionEvent = SupervisorEvent | LoopEvent;

export interface EventLogListProps {
  events: SessionEvent[];
  totalCount: number;
  loading: boolean;
  hasMore: boolean;
  onLoadMore?: () => void;
  onApprove?: (request_id: string, approved: boolean) => void;
}

// ── Helpers ──────────────────────────────────────────────────────────

const truncate = (str: string, len: number) =>
  str.length > len ? str.slice(0, len) + '…' : str;

function formatTokenCount(n: number): string {
  if (n >= 1000) return `${(n / 1000).toFixed(1)}k`;
  return String(n);
}

function formatJson(value: any): string {
  if (value == null) return '';
  if (typeof value === 'string') return value;
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
}

/** True if the event is a SupervisorEvent (has a top-level "type" field) */
function isSupervisorEvent(ev: SessionEvent): ev is SupervisorEvent {
  return 'type' in ev && typeof (ev as any).type === 'string';
}

/** Get the LoopEvent variant name (first key that is a known variant) */
const LOOP_VARIANTS = new Set([
  'ModelLoading', 'Token', 'ModelDone', 'ToolCallStart', 'ToolCallResult',
  'UserInteractionRequired', 'Done', 'Error', 'AgentSessionMessage', 'ModelFallback',
]);

function loopVariant(ev: SessionEvent): string | null {
  for (const key of Object.keys(ev)) {
    if (LOOP_VARIANTS.has(key)) return key;
  }
  return null;
}

const eventColorClass: Record<string, string> = {
  'evl-spawn': 'text-blue-400',
  'evl-status': 'text-muted-foreground',
  'evl-msg': 'text-cyan-400 dark:text-cyan-300',
  'evl-done': 'text-green-500 dark:text-green-400',
  'evl-model': 'text-purple-500 dark:text-purple-400',
  'evl-model-done': 'text-muted-foreground',
  'evl-tool': 'text-yellow-600 dark:text-yellow-300',
  'evl-tool-done': 'text-muted-foreground',
  'evl-error': 'text-red-500 dark:text-red-400',
  'evl-approval': 'text-orange-500 dark:text-orange-300',
  'evl-step': 'text-blue-400',
  'evl-fallback': 'text-amber-500 dark:text-amber-300',
  'evl-agent-msg': 'text-indigo-500 dark:text-indigo-300',
};

// ── Summary rendering (compact, no detail blobs) ─────────────────────

function loopEventSummary(ev: SessionEvent): { icon: JSX.Element; label: string; cls: string } | null {
  const variant = loopVariant(ev);
  if (!variant) return null;
  const data = (ev as any)[variant];

  switch (variant) {
    case 'ModelLoading': {
      const tool_count = data.tool_result_counts ? Object.values(data.tool_result_counts as Record<string, number>).reduce((a: number, b: number) => a + b, 0) : 0;
      const parts: string[] = [];
      if (data.estimated_tokens) parts.push(`~${formatTokenCount(data.estimated_tokens)} tokens`);
      if (tool_count > 0) parts.push(`${tool_count} tool results`);
      const suffix = parts.length > 0 ? ` (${parts.join(', ')})` : '';
      return { icon: <Brain size={14} />, label: `Model call → ${data.model || ''}${suffix}`, cls: 'evl-model' };
    }
    case 'ModelDone':
      return { icon: <MessageSquare size={14} />, label: `Response from ${data.model || ''}`, cls: 'evl-model-done' };
    case 'ToolCallStart':
      return { icon: <Wrench size={14} />, label: `Tool: ${data.tool_id || ''}`, cls: 'evl-tool' };
    case 'ToolCallResult':
      return {
        icon: data.is_error ? <XCircle size={14} /> : <CheckCircle size={14} />,
        label: `${data.tool_id || ''} ${data.is_error ? '(error)' : ''}`,
        cls: data.is_error ? 'evl-error' : 'evl-tool-done',
      };
    case 'UserInteractionRequired':
      return { icon: <KeyRound size={14} />, label: `Approval required`, cls: 'evl-approval' };
    case 'Error':
      return { icon: <XCircle size={14} />, label: `Error: ${truncate(data.message || '', 80)}`, cls: 'evl-error' };
    case 'AgentSessionMessage':
      return { icon: <Users size={14} />, label: `Agent message from ${data.from_agent_id || ''}`, cls: 'evl-agent-msg' };
    case 'ModelFallback':
      return { icon: <ArrowRightLeft size={14} />, label: `Fallback: ${data.from_model} → ${data.to_model}`, cls: 'evl-fallback' };
    case 'Done':
      return { icon: <CheckCircle size={14} />, label: 'Done', cls: 'evl-done' };
    case 'Token':
      return null; // Skip tokens
    default:
      return { icon: <Cpu size={14} />, label: variant, cls: 'evl-status' };
  }
}

function eventSummary(ev: SessionEvent): { icon: JSX.Element; label: string; cls: string } | null {
  // Check for LoopEvent first (externally tagged — variant name as key)
  const loopSummary = loopEventSummary(ev);
  if (loopSummary !== null) return loopSummary;
  // LoopEvent that we want to skip (Token)
  if (loopVariant(ev) !== null) return null;

  // SupervisorEvent (has "type" field)
  const sev = ev as SupervisorEvent;
  switch (sev.type) {
    case 'agent_spawned':
      return { icon: <Rocket size={14} />, label: `Agent spawned — ${sev.spec?.friendly_name || sev.spec?.name || ''}`, cls: 'evl-spawn' };
    case 'agent_task_assigned':
      return { icon: <ClipboardList size={14} />, label: 'Task assigned', cls: 'evl-status' };
    case 'agent_status_changed':
      return { icon: <Upload size={14} />, label: `Status → ${sev.status}`, cls: 'evl-status' };
    case 'message_routed':
      return { icon: <Mail size={14} />, label: `${sev.msg_type || 'message'} from ${sev.from || '?'}`, cls: 'evl-msg' };
    case 'agent_completed':
      return { icon: <CheckCircle size={14} />, label: `Completed`, cls: 'evl-done' };
    case 'agent_output': {
      const re = sev.event;
      if (!re) return { icon: <Upload size={14} />, label: 'Output', cls: 'evl-status' };
      switch (re.type) {
        case 'model_call_started': {
          const tool_count = re.tool_result_counts ? Object.values(re.tool_result_counts).reduce((a, b) => a + b, 0) : 0;
          const parts: string[] = [];
          if (re.estimated_tokens) parts.push(`~${formatTokenCount(re.estimated_tokens)} tokens`);
          if (tool_count > 0) parts.push(`${tool_count} tool results`);
          const suffix = parts.length > 0 ? ` (${parts.join(', ')})` : '';
          return { icon: <Brain size={14} />, label: `Model call → ${re.model || ''}${suffix}`, cls: 'evl-model' };
        }
        case 'model_call_completed':
          return { icon: <MessageSquare size={14} />, label: `Response (${re.token_count ?? 0} tokens)`, cls: 'evl-model-done' };
        case 'tool_call_started':
          return { icon: <Wrench size={14} />, label: `Tool: ${re.tool_id || ''}`, cls: 'evl-tool' };
        case 'tool_call_completed':
          return {
            icon: re.is_error ? <XCircle size={14} /> : <CheckCircle size={14} />,
            label: `${re.tool_id || ''} ${re.is_error ? '(error)' : ''}`,
            cls: re.is_error ? 'evl-error' : 'evl-tool-done',
          };
        case 'user_interaction_required':
          return { icon: <KeyRound size={14} />, label: `Approval: ${re.tool_id || ''}`, cls: 'evl-approval' };
        case 'question_asked':
          return { icon: <CircleHelp size={14} />, label: `Question: ${truncate(re.text ?? '', 80)}`, cls: 'evl-approval' };
        case 'completed':
          return { icon: <CheckCircle size={14} />, label: 'Done', cls: 'evl-done' };
        case 'failed':
          return { icon: <XCircle size={14} />, label: `Error`, cls: 'evl-error' };
        case 'step_started':
          return { icon: <ClipboardList size={14} />, label: re.description || 'Step', cls: 'evl-step' };
        default:
          return { icon: <Upload size={14} />, label: re.type, cls: 'evl-status' };
      }
    }
    case 'all_complete':
      return { icon: <Flag size={14} />, label: 'All complete', cls: 'evl-done' };
    default:
      return { icon: <FileText size={14} />, label: sev.type, cls: 'evl-status' };
  }
}

// ── Detail rendering (full content for the popup) ────────────────────

function loopEventDetail(ev: SessionEvent): { title: JSX.Element; sections: { label: string; content: string; isCode?: boolean }[] } | null {
  const variant = loopVariant(ev);
  if (!variant) return null;
  const data = (ev as any)[variant];
  const sections: { label: string; content: string; isCode?: boolean }[] = [];

  switch (variant) {
    case 'ModelLoading':
      sections.push({ label: 'Provider', content: data.provider_id ?? '' });
      sections.push({ label: 'Model', content: data.model ?? '' });
      if (data.estimated_tokens) {
        sections.push({ label: 'Estimated Tokens', content: `~${formatTokenCount(data.estimated_tokens)}` });
      }
      if (data.tool_result_counts && Object.keys(data.tool_result_counts).length > 0) {
        const lines = Object.entries(data.tool_result_counts)
          .sort(([a], [b]) => a.localeCompare(b))
          .map(([name, count]) => `${name}: ${count}`)
          .join('\n');
        sections.push({ label: 'Tool Results Included', content: lines });
      }
      return { title: <><Brain size={14} /> Model Call → {data.model ?? ''}</>, sections };

    case 'ModelDone':
      sections.push({ label: 'Provider', content: data.provider_id ?? '' });
      sections.push({ label: 'Model', content: data.model ?? '' });
      sections.push({ label: 'Content', content: data.content ?? '' });
      return { title: <><MessageSquare size={14} /> Model Response — {data.model ?? ''}</>, sections };

    case 'ToolCallStart':
      sections.push({ label: 'Tool', content: data.tool_id ?? '' });
      sections.push({ label: 'Input', content: formatJson(data.input), isCode: true });
      return { title: <><Wrench size={14} /> Tool Call: {data.tool_id ?? ''}</>, sections };

    case 'ToolCallResult':
      sections.push({ label: 'Tool', content: data.tool_id ?? '' });
      sections.push({ label: 'Status', content: data.is_error ? 'Error' : 'Success' });
      sections.push({ label: 'Output', content: formatJson(data.output), isCode: true });
      return { title: <>{data.is_error ? <XCircle size={14} /> : <CheckCircle size={14} />} Tool Result: {data.tool_id ?? ''}</>, sections };

    case 'UserInteractionRequired':
      sections.push({ label: 'Request ID', content: data.request_id ?? '' });
      sections.push({ label: 'Kind', content: formatJson(data.kind), isCode: true });
      return { title: <><KeyRound size={14} /> Approval Required</>, sections };

    case 'Error':
      sections.push({ label: 'Message', content: data.message ?? '' });
      return { title: <><XCircle size={14} /> Error</>, sections };

    case 'AgentSessionMessage':
      sections.push({ label: 'From Agent', content: data.from_agent_id ?? '' });
      sections.push({ label: 'Content', content: data.content ?? '' });
      return { title: <><Users size={14} /> Agent Message</>, sections };

    case 'ModelFallback':
      sections.push({ label: 'From', content: `${data.from_provider}/${data.from_model}` });
      sections.push({ label: 'To', content: `${data.to_provider}/${data.to_model}` });
      return { title: <><ArrowRightLeft size={14} /> Model Fallback</>, sections };

    case 'Done':
      sections.push({ label: 'Provider', content: data.provider_id ?? '' });
      sections.push({ label: 'Model', content: data.model ?? '' });
      sections.push({ label: 'Content', content: data.content ?? '' });
      return { title: <><CheckCircle size={14} /> Done</>, sections };

    default:
      sections.push({ label: 'Raw', content: formatJson(ev), isCode: true });
      return { title: <><Cpu size={14} /> {variant}</>, sections };
  }
}

function eventDetail(ev: SessionEvent): { title: JSX.Element; sections: { label: string; content: string; isCode?: boolean }[] } {
  // Try LoopEvent first
  const loopDetail = loopEventDetail(ev);
  if (loopDetail) return loopDetail;

  // SupervisorEvent
  const sev = ev as SupervisorEvent;
  const sections: { label: string; content: string; isCode?: boolean }[] = [];

  switch (sev.type) {
    case 'agent_spawned':
      sections.push({ label: 'Agent ID', content: sev.agent_id ?? '' });
      if (sev.spec) {
        sections.push({ label: 'Spec', content: formatJson(sev.spec), isCode: true });
      }
      return { title: <><Rocket size={14} /> Agent Spawned — {sev.spec?.friendly_name || sev.spec?.name || ''}</>, sections };

    case 'agent_task_assigned':
      sections.push({ label: 'Task', content: sev.task ?? '(no task content)' });
      return { title: <><ClipboardList size={14} /> Task Assigned</>, sections };

    case 'agent_status_changed':
      sections.push({ label: 'Status', content: sev.status ?? '' });
      return { title: `◉ Status Changed → ${sev.status}`, sections };

    case 'message_routed':
      sections.push({ label: 'From', content: sev.from ?? '' });
      sections.push({ label: 'To', content: sev.to ?? '' });
      sections.push({ label: 'Type', content: sev.msg_type ?? '' });
      return { title: <><Mail size={14} /> Message Routed</>, sections };

    case 'agent_completed':
      sections.push({ label: 'Result', content: sev.result ?? '' });
      return { title: <><CheckCircle size={14} /> Agent Completed</>, sections };

    case 'agent_output': {
      const re = sev.event;
      if (!re) return { title: <><Upload size={14} /> Output</>, sections };

      switch (re.type) {
        case 'model_call_started':
          sections.push({ label: 'Model', content: re.model ?? '' });
          if (re.estimated_tokens) {
            sections.push({ label: 'Estimated Tokens', content: `~${formatTokenCount(re.estimated_tokens)}` });
          }
          if (re.prompt_preview) sections.push({ label: 'Prompt Preview', content: re.prompt_preview });
          if (re.tool_result_counts && Object.keys(re.tool_result_counts).length > 0) {
            const lines = Object.entries(re.tool_result_counts)
              .sort(([a], [b]) => a.localeCompare(b))
              .map(([name, count]) => `${name}: ${count}`)
              .join('\n');
            sections.push({ label: 'Tool Results Included', content: lines });
          }
          return { title: <><Brain size={14} /> Model Call Started → {re.model ?? ''}</>, sections };

        case 'model_call_completed':
          sections.push({ label: 'Model', content: re.model ?? '' });
          sections.push({ label: 'Tokens', content: String(re.token_count ?? 0) });
          sections.push({ label: 'Content', content: re.content ?? '' });
          return { title: <><MessageSquare size={14} /> Model Response ({re.token_count ?? 0} tokens)</>, sections };

        case 'tool_call_started':
          sections.push({ label: 'Tool', content: re.tool_id ?? '' });
          sections.push({ label: 'Input', content: formatJson(re.input), isCode: true });
          return { title: <><Wrench size={14} /> Tool Call: {re.tool_id ?? ''}</>, sections };

        case 'tool_call_completed':
          sections.push({ label: 'Tool', content: re.tool_id ?? '' });
          sections.push({ label: 'Status', content: re.is_error ? 'Error' : 'Success' });
          sections.push({ label: 'Output', content: formatJson(re.output), isCode: true });
          return { title: <>{re.is_error ? <XCircle size={14} /> : <CheckCircle size={14} />} Tool Result: {re.tool_id ?? ''}</>, sections };

        case 'user_interaction_required':
          sections.push({ label: 'Tool', content: re.tool_id ?? '' });
          sections.push({ label: 'Reason', content: re.reason ?? '' });
          sections.push({ label: 'Input', content: re.input ?? '' });
          sections.push({ label: 'Request ID', content: re.request_id ?? '' });
          return { title: <><KeyRound size={14} /> Approval Required: {re.tool_id ?? ''}</>, sections };

        case 'question_asked':
          sections.push({ label: 'Question', content: re.text ?? '' });
          if (re.choices?.length) sections.push({ label: 'Choices', content: re.choices.join(', ') });
          sections.push({ label: 'Allow Freeform', content: String(re.allow_freeform ?? false) });
          return { title: <><CircleHelp size={14} /> Question Asked</>, sections };

        case 'completed':
          sections.push({ label: 'Result', content: re.result ?? '' });
          return { title: <><CheckCircle size={14} /> Completed</>, sections };

        case 'failed':
          sections.push({ label: 'Error', content: re.error ?? '' });
          return { title: <><XCircle size={14} /> Failed</>, sections };

        case 'step_started':
          sections.push({ label: 'Step ID', content: re.step_id ?? '' });
          sections.push({ label: 'Description', content: re.description ?? '' });
          return { title: <><ClipboardList size={14} /> Step: {re.description ?? ''}</>, sections };

        default:
          sections.push({ label: 'Raw', content: formatJson(re), isCode: true });
          return { title: <><Upload size={14} /> {re.type}</>, sections };
      }
    }

    default:
      sections.push({ label: 'Raw', content: formatJson(ev), isCode: true });
      return { title: sev.type ?? 'Unknown', sections };
  }
}

// ── Component ────────────────────────────────────────────────────────

const EventLogList = (props: EventLogListProps) => {
  const [selectedEvent, setSelectedEvent] = createSignal<SessionEvent | null>(null);

  return (
    <>
      {/* Load more (older events) at the top */}
      <Show when={props.hasMore && !props.loading}>
        <div class="py-1.5 text-center">
          <button
            class="cursor-pointer rounded-md border border-blue-400/25 bg-blue-400/10 px-3.5 py-1 text-xs text-blue-400 transition-colors hover:bg-blue-400/25"
            onClick={() => props.onLoadMore?.()}
          >
            ▲ Load earlier events ({props.totalCount - props.events.length} remaining)
          </button>
        </div>
      </Show>

      <Show when={props.loading && props.events.length === 0}>
        <p class="py-5 text-center text-xs text-muted-foreground">Loading events…</p>
      </Show>

      <Show when={!props.loading && props.events.length === 0}>
        <p class="py-5 text-center text-xs text-muted-foreground">No events recorded yet.</p>
      </Show>

      <div class="flex flex-col gap-0.5">
        <For each={props.events}>
          {(ev) => {
            const summary = eventSummary(ev);
            if (!summary) return null; // Skip events like Token

            const isApproval =
              (isSupervisorEvent(ev) && ev.type === 'agent_output' && ev.event?.type === 'user_interaction_required') ||
              loopVariant(ev) === 'UserInteractionRequired';
            const reqId = isSupervisorEvent(ev) ? ev.event?.request_id : (ev as any).UserInteractionRequired?.request_id;

            return (
              <div
                class={`cursor-pointer select-none rounded-md border border-transparent px-2.5 py-1 text-xs leading-relaxed transition-colors hover:bg-secondary/60 ${isApproval ? 'border-orange-400/30 bg-orange-400/5' : 'bg-card/40'}`}
                onClick={() => setSelectedEvent(ev)}
              >
                <span class={`block truncate ${eventColorClass[summary.cls] || 'text-muted-foreground'}`}>
                  {summary.icon} {summary.label}
                </span>
                <Show when={isApproval && reqId && props.onApprove}>
                  <div class="mt-1 flex gap-1.5">
                    <button
                      class="cursor-pointer rounded-[5px] border-none bg-green-400/20 px-2.5 py-0.5 text-xs text-green-400 hover:bg-green-400/35"
                      onClick={(e) => {
                        e.stopPropagation();
                        props.onApprove?.(reqId!, true);
                      }}
                    >
                      <CheckCircle size={14} /> Approve
                    </button>
                    <button
                      class="cursor-pointer rounded-[5px] border-none bg-red-400/20 px-2.5 py-0.5 text-xs text-red-400 hover:bg-red-400/35"
                      onClick={(e) => {
                        e.stopPropagation();
                        props.onApprove?.(reqId!, false);
                      }}
                    >
                      <XCircle size={14} /> Deny
                    </button>
                  </div>
                </Show>
              </div>
            );
          }}
        </For>
      </div>

      <Show when={props.loading && props.events.length > 0}>
        <p class="py-5 text-center text-xs text-muted-foreground">Loading…</p>
      </Show>

      {/* ── Detail popup ── */}
      <Dialog open={!!selectedEvent()} onOpenChange={(open) => { if (!open) setSelectedEvent(null); }}>
        <DialogContent class="max-w-[680px]" data-testid="event-detail-dialog">
          <Show when={selectedEvent()}>
            {(ev) => {
              const detail = () => eventDetail(ev());
              return (
                <>
                  <DialogHeader>
                    <DialogTitle class="flex items-center gap-1.5 text-sm">{detail().title}</DialogTitle>
                  </DialogHeader>
                  <div class="flex max-h-[60vh] flex-col gap-3 overflow-y-auto py-2">
                    <For each={detail().sections}>
                      {(section) => (
                        <div class="flex flex-col gap-1">
                          <div class="text-[0.72rem] font-semibold uppercase tracking-wide text-muted-foreground">{section.label}</div>
                          <Show when={section.isCode} fallback={
                            <div class="whitespace-pre-wrap break-words text-sm leading-relaxed text-foreground">{section.content || '—'}</div>
                          }>
                            <pre class="m-0 max-h-[300px] overflow-auto whitespace-pre-wrap break-words rounded-md border border-secondary bg-card/60 px-3 py-2.5 font-mono text-xs leading-relaxed text-foreground" innerHTML={highlightYaml(section.content || '—')} />
                          </Show>
                        </div>
                      )}
                    </For>
                  </div>
                </>
              );
            }}
          </Show>
        </DialogContent>
      </Dialog>
    </>
  );
};

export default EventLogList;
