/**
 * Shadow Results Panel — Rich viewers for intercepted actions from shadow/test runs.
 *
 * Tabs: All | Emails | HTTP | Agents | Other
 * Each tab shows a filtered, paginated list with tool-specific detail rendering.
 */
import { createSignal, createEffect, For, Show, on, batch } from 'solid-js';
import {
  Mail, Globe, Bot, RotateCcw, Calendar, Radio, ChevronLeft, ChevronRight,
  Eye, Shield, AlertTriangle,
} from 'lucide-solid';
import { highlightYaml } from '../YamlHighlight';
import type { InterceptedAction, InterceptedActionPage, ShadowSummary } from '~/types';

// ── Types ────────────────────────────────────────────────────────────────

type TabKind = 'all' | 'tool_call' | 'agent' | 'other';

interface ShadowResultsPanelProps {
  instanceId: number;
  executionMode?: string;
  fetchActions: (instanceId: number, limit?: number, offset?: number) => Promise<InterceptedActionPage | null>;
  fetchSummary: (instanceId: number) => Promise<ShadowSummary | null>;
}

// ── Helpers ──────────────────────────────────────────────────────────────

function kindToTab(kind: string): TabKind {
  if (kind === 'tool_call') return 'tool_call';
  if (kind === 'agent_invocation' || kind === 'agent_signal' || kind === 'agent_wait') return 'agent';
  return 'other'; // workflow_launch, scheduled_task, event_gate
}

function isEmailAction(a: InterceptedAction): boolean {
  const d = a.details;
  const toolId = (d.tool_id as string) ?? '';
  return toolId.includes('send_message') || toolId.includes('send_email') || toolId.includes('email');
}

function isHttpAction(a: InterceptedAction): boolean {
  const d = a.details;
  const toolId = (d.tool_id as string) ?? '';
  return toolId.includes('http') || toolId.includes('request') || toolId.includes('fetch');
}

function riskBadge(risk: string | undefined) {
  if (!risk) return null;
  const colors: Record<string, string> = {
    safe: '#34d399',
    caution: '#fbbf24',
    danger: '#f87171',
    unknown: '#94a3b8',
  };
  return (
    <span
      class="shadow-risk-dot"
      style={`background:${colors[risk] ?? '#94a3b8'};width:8px;height:8px;border-radius:50%;display:inline-block;margin-right:4px;`}
      title={`Risk: ${risk}`}
    />
  );
}

function formatTime(ms: number): string {
  const d = new Date(ms);
  return d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit', second: '2-digit' });
}

// ── Sub-renderers ────────────────────────────────────────────────────────

function EmailCard(props: { action: InterceptedAction }) {
  const [expanded, setExpanded] = createSignal(false);
  const d = () => props.action.details;
  const args = () => (d().arguments ?? {}) as Record<string, unknown>;
  const to = () => {
    const a = args();
    return (a.to as string) ?? (a.recipient as string) ?? (a.recipients as string) ?? '—';
  };
  const subject = () => {
    const a = args();
    return (a.subject as string) ?? '(no subject)';
  };
  const body = () => {
    const a = args();
    return (a.body as string) ?? (a.message as string) ?? (a.content as string) ?? '';
  };
  const channel = () => {
    const a = args();
    return (a.channel_id as string) ?? (a.channel as string) ?? '';
  };

  return (
    <div class="shadow-email-card">
      <div class="shadow-email-header" onClick={() => setExpanded(!expanded())}>
        <Mail size={14} style="color:#60a5fa;flex-shrink:0;" />
        <div class="shadow-email-meta">
          <span class="shadow-email-to">To: {to()}</span>
          <span class="shadow-email-subject">{subject()}</span>
        </div>
        <span class="shadow-email-time">{formatTime(props.action.timestamp_ms)}</span>
      </div>
      <Show when={expanded()}>
        <div class="shadow-email-body">
          <Show when={channel()}>
            <div class="shadow-email-field"><strong>Channel:</strong> {channel()}</div>
          </Show>
          <div class="shadow-email-field"><strong>Step:</strong> {props.action.step_id}</div>
          <div class="shadow-email-content">{body()}</div>
        </div>
      </Show>
    </div>
  );
}

function HttpCard(props: { action: InterceptedAction }) {
  const [expanded, setExpanded] = createSignal(false);
  const d = () => props.action.details;
  const args = () => (d().arguments ?? {}) as Record<string, unknown>;
  const method = () => (args().method as string) ?? 'GET';
  const url = () => (args().url as string) ?? '—';
  const reqBody = () => args().body;

  const methodColor = () => {
    switch (method().toUpperCase()) {
      case 'GET': return '#34d399';
      case 'POST': return '#60a5fa';
      case 'PUT': return '#fbbf24';
      case 'DELETE': return '#f87171';
      default: return '#94a3b8';
    }
  };

  return (
    <div class="shadow-http-card">
      <div class="shadow-http-header" onClick={() => setExpanded(!expanded())}>
        <span class="shadow-http-method" style={`color:${methodColor()}`}>{method().toUpperCase()}</span>
        <span class="shadow-http-url">{url()}</span>
        <span class="shadow-email-time">{formatTime(props.action.timestamp_ms)}</span>
      </div>
      <Show when={expanded()}>
        <div class="shadow-http-body">
          <div class="shadow-email-field"><strong>Step:</strong> {props.action.step_id}</div>
          <Show when={reqBody()}>
            <div class="shadow-email-field"><strong>Body:</strong></div>
            <pre class="wf-detail-yaml" innerHTML={highlightYaml(reqBody())} />
          </Show>
        </div>
      </Show>
    </div>
  );
}

function AgentCard(props: { action: InterceptedAction }) {
  const [expanded, setExpanded] = createSignal(false);
  const d = () => props.action.details;

  return (
    <div class="shadow-agent-card">
      <div class="shadow-agent-header" onClick={() => setExpanded(!expanded())}>
        <Bot size={14} style="color:#a78bfa;flex-shrink:0;" />
        <div class="shadow-agent-meta">
          <span class="shadow-agent-name">{(d().persona_id as string) ?? (d().agent_id as string) ?? props.action.kind}</span>
          <Show when={d().task}>
            <span class="shadow-agent-task">{String(d().task).slice(0, 80)}{String(d().task).length > 80 ? '…' : ''}</span>
          </Show>
        </div>
        <span class="shadow-email-time">{formatTime(props.action.timestamp_ms)}</span>
      </div>
      <Show when={expanded()}>
        <div class="shadow-agent-body">
          <pre class="wf-detail-yaml" innerHTML={highlightYaml(d())} />
        </div>
      </Show>
    </div>
  );
}

function OtherCard(props: { action: InterceptedAction }) {
  const [expanded, setExpanded] = createSignal(false);
  const d = () => props.action.details;

  const icon = () => {
    switch (props.action.kind) {
      case 'workflow_launch': return <RotateCcw size={14} style="color:#60a5fa;" />;
      case 'scheduled_task': return <Calendar size={14} style="color:#fbbf24;" />;
      case 'agent_signal': return <Radio size={14} style="color:#a78bfa;" />;
      default: return <Shield size={14} style="color:#94a3b8;" />;
    }
  };

  const label = () => {
    switch (props.action.kind) {
      case 'workflow_launch': return `Launch: ${(d().workflow_name as string) ?? '?'}`;
      case 'scheduled_task': return `Schedule: ${(d().name as string) ?? '?'}`;
      case 'agent_signal': return `Signal → ${(d().target as string) ?? '?'}`;
      case 'event_gate': return `Event gate: ${(d().topic as string) ?? '?'}`;
      default: return props.action.kind;
    }
  };

  return (
    <div class="shadow-other-card">
      <div class="shadow-other-header" onClick={() => setExpanded(!expanded())}>
        {icon()}
        <span class="shadow-other-label">{label()}</span>
        <span class="shadow-email-time">{formatTime(props.action.timestamp_ms)}</span>
      </div>
      <Show when={expanded()}>
        <div class="shadow-other-body">
          <pre class="wf-detail-yaml" innerHTML={highlightYaml(d())} />
        </div>
      </Show>
    </div>
  );
}

function ToolCallCard(props: { action: InterceptedAction }) {
  if (isEmailAction(props.action)) return <EmailCard action={props.action} />;
  if (isHttpAction(props.action)) return <HttpCard action={props.action} />;

  const [expanded, setExpanded] = createSignal(false);
  const d = () => props.action.details;

  return (
    <div class="shadow-tool-card">
      <div class="shadow-tool-header" onClick={() => setExpanded(!expanded())}>
        {riskBadge(d().risk_level as string)}
        <span class="shadow-tool-id">{(d().tool_id as string) ?? '?'}</span>
        <span class="shadow-email-time">{formatTime(props.action.timestamp_ms)}</span>
      </div>
      <Show when={expanded()}>
        <div class="shadow-tool-body">
          <div class="shadow-email-field"><strong>Step:</strong> {props.action.step_id}</div>
          <Show when={d().arguments}>
            <div class="shadow-email-field"><strong>Arguments:</strong></div>
            <pre class="wf-detail-yaml" innerHTML={highlightYaml(d().arguments)} />
          </Show>
        </div>
      </Show>
    </div>
  );
}

// ── Main Component ───────────────────────────────────────────────────────

export default function ShadowResultsPanel(props: ShadowResultsPanelProps) {
  const [tab, setTab] = createSignal<TabKind>('all');
  const [summary, setSummary] = createSignal<ShadowSummary | null>(null);
  const [actions, setActions] = createSignal<InterceptedAction[]>([]);
  const [total, setTotal] = createSignal(0);
  const [page, setPage] = createSignal(0);
  const [loading, setLoading] = createSignal(false);
  const PAGE_SIZE = 25;

  // Load summary on mount / instanceId change
  createEffect(on(() => props.instanceId, async (id) => {
    if (!id) return;
    const s = await props.fetchSummary(id);
    setSummary(s);
    // Reset to page 0 on instance change
    batch(() => {
      setPage(0);
      setTab('all');
    });
  }));

  // Load actions when instanceId, page, or tab changes
  createEffect(on(
    () => [props.instanceId, page(), tab()] as const,
    async ([id, pg]) => {
      if (!id) return;
      setLoading(true);
      const result = await props.fetchActions(id, PAGE_SIZE, pg * PAGE_SIZE);
      if (result) {
        setActions(result.items);
        setTotal(result.total);
      }
      setLoading(false);
    },
  ));

  const filteredActions = () => {
    const t = tab();
    if (t === 'all') return actions();
    return actions().filter(a => kindToTab(a.kind) === t);
  };

  const totalPages = () => Math.max(1, Math.ceil(total() / PAGE_SIZE));

  const summaryItems = () => {
    const s = summary();
    if (!s) return [];
    const items: { icon: any; label: string; count: number }[] = [];
    if (s.tool_calls_intercepted > 0) items.push({ icon: Globe, label: 'Tool calls', count: s.tool_calls_intercepted });
    if (s.agent_invocations_intercepted > 0) items.push({ icon: Bot, label: 'Agent invocations', count: s.agent_invocations_intercepted });
    if (s.workflow_launches_intercepted > 0) items.push({ icon: RotateCcw, label: 'Workflow launches', count: s.workflow_launches_intercepted });
    if (s.scheduled_tasks_intercepted > 0) items.push({ icon: Calendar, label: 'Scheduled tasks', count: s.scheduled_tasks_intercepted });
    if (s.agent_signals_intercepted > 0) items.push({ icon: Radio, label: 'Agent signals', count: s.agent_signals_intercepted });
    return items;
  };

  return (
    <div class="shadow-results-panel">
      {/* Summary banner */}
      <Show when={summary()}>
        <div class="shadow-summary-banner">
          <div class="shadow-summary-icon">
            <Eye size={16} />
          </div>
          <div class="shadow-summary-text">
            <strong>{summary()!.total_intercepted}</strong> action{summary()!.total_intercepted !== 1 ? 's' : ''} intercepted
          </div>
          <div class="shadow-summary-chips">
            <For each={summaryItems()}>
              {(item) => (
                <span class="shadow-summary-chip">
                  <item.icon size={12} />
                  {item.count} {item.label.toLowerCase()}
                </span>
              )}
            </For>
          </div>
        </div>
      </Show>

      {/* No intercepted actions */}
      <Show when={summary() && summary()!.total_intercepted === 0}>
        <div class="shadow-empty">
          <Shield size={24} style="color:#34d399;" />
          <p>No side effects detected — this workflow is safe!</p>
        </div>
      </Show>

      {/* Tabs + action list */}
      <Show when={summary() && summary()!.total_intercepted > 0}>
        <div class="shadow-tabs">
          <button class={`shadow-tab${tab() === 'all' ? ' active' : ''}`} onClick={() => { setTab('all'); setPage(0); }}>
            All ({total()})
          </button>
          <button class={`shadow-tab${tab() === 'tool_call' ? ' active' : ''}`} onClick={() => { setTab('tool_call'); setPage(0); }}>
            <Globe size={12} /> Tools
          </button>
          <button class={`shadow-tab${tab() === 'agent' ? ' active' : ''}`} onClick={() => { setTab('agent'); setPage(0); }}>
            <Bot size={12} /> Agents
          </button>
          <button class={`shadow-tab${tab() === 'other' ? ' active' : ''}`} onClick={() => { setTab('other'); setPage(0); }}>
            Other
          </button>
        </div>

        <Show when={loading()}>
          <div class="shadow-loading">Loading…</div>
        </Show>

        <Show when={!loading()}>
          <div class="shadow-action-list">
            <For each={filteredActions()}>
              {(action) => {
                switch (kindToTab(action.kind)) {
                  case 'tool_call': return <ToolCallCard action={action} />;
                  case 'agent': return <AgentCard action={action} />;
                  default: return <OtherCard action={action} />;
                }
              }}
            </For>
          </div>

          {/* Pagination */}
          <Show when={totalPages() > 1}>
            <div class="shadow-pagination">
              <button class="icon-btn" disabled={page() === 0} onClick={() => setPage(p => p - 1)}>
                <ChevronLeft size={14} /> Prev
              </button>
              <span class="text-xs text-muted-foreground">
                Page {page() + 1} of {totalPages()}
              </span>
              <button class="icon-btn" disabled={page() >= totalPages() - 1} onClick={() => setPage(p => p + 1)}>
                Next <ChevronRight size={14} />
              </button>
            </div>
          </Show>
        </Show>
      </Show>
    </div>
  );
}
