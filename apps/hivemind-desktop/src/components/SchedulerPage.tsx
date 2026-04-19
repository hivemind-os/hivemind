import { For, Show, createEffect, createSignal, onCleanup } from 'solid-js';
import type { ScheduledTask, TaskRun, TaskSchedule, TaskAction, Persona, ToolDefinition, ChatSessionSummary } from '../types';
import { authFetch } from '~/lib/authFetch';
import { YamlBlock } from './YamlHighlight';
import { RefreshCw, Plus, TriangleAlert, ClipboardList, Pause, Trash2, ChevronDown, ChevronRight } from 'lucide-solid';
import { Collapsible, CollapsibleContent } from '~/ui/collapsible';
import { EmptyState } from '~/ui/empty-state';
import { CronBuilder, TopicSelector, PersonaSelector, ToolCallBuilder, WorkflowLauncher, payloadKeysForTopic } from './shared';
import { Switch, SwitchControl, SwitchThumb, SwitchLabel } from '~/ui/switch';
import { Button } from '~/ui/button';
import PermissionRulesEditor from './PermissionRulesEditor';
import type { PermissionRule as PermRuleUI } from './PermissionRulesEditor';
import type { WorkflowLaunchValue, WorkflowDefSummary } from './shared';
import { listen } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';

export interface SchedulerPageProps {
  daemon_url: () => string | undefined;
  personas?: Persona[];
  tools?: ToolDefinition[];
  eventTopics?: { topic: string; description: string; payload_keys?: string[] }[];
  channels?: { id: string; name: string; provider: string; hasComms: boolean }[];
  workflowDefinitions?: WorkflowDefSummary[];
  fetchParsedWorkflow?: (name: string) => Promise<{ definition: any } | null>;
}

function formatTimestamp(ms: number | null | undefined): string {
  if (!ms) return '—';
  return new Date(ms).toLocaleString();
}

function formatSchedule(s: TaskSchedule): string {
  switch (s.type) {
    case 'once': return 'Once (immediate)';
    case 'scheduled': return `Scheduled: ${s.run_at_ms ? new Date(s.run_at_ms).toLocaleString() : '?'}`;
    case 'cron': return `Cron: ${s.expression}`;
    default: return 'Unknown';
  }
}

function formatAction(a: TaskAction): string {
  switch (a.type) {
    case 'send_message': return `SendMessage → ${a.session_id ?? '?'}`;
    case 'http_webhook': return `${a.method ?? 'POST'} ${a.url ?? '?'}`;
    case 'emit_event': return `Event: ${a.topic ?? '?'}`;
    case 'invoke_agent': {
      const taskStr = a.task.length > 80 ? a.task.slice(0, 80) + '…' : a.task;
      return `Invoke Agent: ${a.persona_id} — ${taskStr}`;
    }
    case 'call_tool': return `Call Tool: ${a.tool_id}`;
    case 'composite_action': return `Composite (${a.actions.length} actions)`;
    case 'launch_workflow': return `Launch: ${a.definition}`;
    default: return 'Unknown';
  }
}

function statusBadgeClass(status: string): string {
  switch (status) {
    case 'pending': return 'pill neutral';
    case 'running': return 'pill info';
    case 'completed': return 'pill success';
    case 'failed': return 'pill danger';
    case 'cancelled': return 'pill warning';
    default: return 'pill neutral';
  }
}

const SchedulerPage = (props: SchedulerPageProps) => {
  const [tasks, setTasks] = createSignal<ScheduledTask[]>([]);
  const [expandedTaskId, setExpandedTaskId] = createSignal<string | null>(null);
  const [taskRuns, setTaskRuns] = createSignal<TaskRun[]>([]);
  const [loading, setLoading] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  // Create task form state
  const [showCreateForm, setShowCreateForm] = createSignal(false);
  const [newName, setNewName] = createSignal('');
  const [newDescription, setNewDescription] = createSignal('');
  const [newScheduleType, setNewScheduleType] = createSignal<'once' | 'scheduled' | 'cron'>('once');
  const [newRunAt, setNewRunAt] = createSignal('');
  const [newCronExpr, setNewCronExpr] = createSignal('0 0 * * * * *');
  const [newActionType, setNewActionType] = createSignal<TaskAction['type']>('send_message');
  const [newSessionId, setNewSessionId] = createSignal('');
  const [newContent, setNewContent] = createSignal('');
  const [newUrl, setNewUrl] = createSignal('');
  const [newMethod, setNewMethod] = createSignal('POST');
  const [newBody, setNewBody] = createSignal('');
  const [newHeaders, setNewHeaders] = createSignal<{ key: string; value: string }[]>([]);
  const [newTopic, setNewTopic] = createSignal('');
  const [newPayloadFields, setNewPayloadFields] = createSignal<Record<string, string>>({});
  const [newPayloadJsonMode, setNewPayloadJsonMode] = createSignal(false);
  const [newPayloadJson, setNewPayloadJson] = createSignal('{}');
  const [newPersonaId, setNewPersonaId] = createSignal('');
  const [newAgentTask, setNewAgentTask] = createSignal('');
  const [newFriendlyName, setNewFriendlyName] = createSignal('');
  const [newTimeoutSecs, setNewTimeoutSecs] = createSignal(300);
  const [newAsyncExec, setNewAsyncExec] = createSignal(false);
  const [newPermissions, setNewPermissions] = createSignal<PermRuleUI[]>([]);
  const [newToolId, setNewToolId] = createSignal('');
  const [newToolArgs, setNewToolArgs] = createSignal<Record<string, any>>({});
  const [newWorkflowLaunch, setNewWorkflowLaunch] = createSignal<WorkflowLaunchValue | null>(null);
  // composite_action fields
  type SubActionEntry = { id: number; actionType: TaskAction['type']; fields: Record<string, string> };
  const [subActions, setSubActions] = createSignal<SubActionEntry[]>([]);
  const [newStopOnFailure, setNewStopOnFailure] = createSignal(true);
  let subActionCounter = 0;
  const [creating, setCreating] = createSignal(false);
  const [availableSessions, setAvailableSessions] = createSignal<ChatSessionSummary[]>([]);

  const baseUrl = () => {
    const url = props.daemon_url();
    return url ? `${url}/api/v1/scheduler/tasks` : null;
  };

  const loadTasks = async () => {
    const url = baseUrl();
    if (!url) return;
    setLoading(true);
    setError(null);
    try {
      const resp = await authFetch(url);
      if (!resp.ok) throw new Error(await resp.text());
      setTasks(await resp.json());
    } catch (e: any) {
      setError(e.message ?? 'Failed to load tasks');
    } finally {
      setLoading(false);
    }
  };

  const loadRuns = async (taskId: string) => {
    const url = baseUrl();
    if (!url) return;
    try {
      const resp = await authFetch(`${url}/${taskId}/runs`);
      if (!resp.ok) throw new Error(await resp.text());
      setTaskRuns(await resp.json());
    } catch {
      setTaskRuns([]);
    }
  };

  const toggleExpand = async (taskId: string) => {
    if (expandedTaskId() === taskId) {
      setExpandedTaskId(null);
      setTaskRuns([]);
    } else {
      setExpandedTaskId(taskId);
      await loadRuns(taskId);
    }
  };

  const deleteTask = async (taskId: string) => {
    const url = baseUrl();
    if (!url) return;
    try {
      await authFetch(`${url}/${taskId}`, { method: 'DELETE' });
      await loadTasks();
      if (expandedTaskId() === taskId) {
        setExpandedTaskId(null);
        setTaskRuns([]);
      }
    } catch (e: any) {
      setError(e.message ?? 'Failed to delete task');
    }
  };

  const cancelTask = async (taskId: string) => {
    const url = baseUrl();
    if (!url) return;
    try {
      await authFetch(`${url}/${taskId}/cancel`, { method: 'POST' });
      await loadTasks();
    } catch (e: any) {
      setError(e.message ?? 'Failed to cancel task');
    }
  };

  const buildSubAction = (sa: SubActionEntry): TaskAction => {
    const f = sa.fields;
    switch (sa.actionType) {
      case 'send_message': return { type: 'send_message', session_id: f.session_id ?? '', content: f.content ?? '' };
      case 'http_webhook': return { type: 'http_webhook', url: f.url ?? '', method: f.method ?? 'POST', body: f.body || undefined };
      case 'emit_event': return { type: 'emit_event', topic: f.topic ?? '', payload: {} };
      case 'invoke_agent': return { type: 'invoke_agent', persona_id: f.persona_id ?? '', task: f.task ?? '', friendly_name: f.friendly_name || undefined, async_exec: f.async_exec === 'true' || undefined, timeout_secs: f.async_exec === 'true' ? undefined : (parseInt(f.timeout_secs) || undefined) };
      case 'call_tool': {
        let args: any = {};
        try { args = JSON.parse(f.arguments ?? '{}'); } catch { /* use empty */ }
        return { type: 'call_tool', tool_id: f.tool_id ?? '', arguments: args };
      }
      default: return { type: 'send_message', session_id: '', content: '' };
    }
  };

  const updateSubAction = (id: number, key: string, value: string) => {
    setSubActions((prev) => prev.map((sa) => sa.id === id ? { ...sa, fields: { ...sa.fields, [key]: value } } : sa));
  };

  const updateSubActionType = (id: number, actionType: TaskAction['type']) => {
    setSubActions((prev) => prev.map((sa) => sa.id === id ? { ...sa, actionType, fields: {} } : sa));
  };

  const addSubAction = () => {
    setSubActions((prev) => [...prev, { id: ++subActionCounter, actionType: 'send_message', fields: {} }]);
  };

  const removeSubAction = (id: number) => {
    setSubActions((prev) => prev.filter((sa) => sa.id !== id));
  };

  const createTask = async () => {
    const url = baseUrl();
    if (!url || !newName().trim()) return;

    let schedule: TaskSchedule;
    switch (newScheduleType()) {
      case 'scheduled': {
        const ts = newRunAt() ? new Date(newRunAt()).getTime() : Date.now();
        if (isNaN(ts)) {
          setError('Invalid date/time value');
          return;
        }
        schedule = { type: 'scheduled', run_at_ms: ts };
        break;
      }
      case 'cron':
        schedule = { type: 'cron', expression: newCronExpr() };
        break;
      default:
        schedule = { type: 'once' };
    }

    let action: TaskAction;
    switch (newActionType()) {
      case 'http_webhook': {
        const hdrs: Record<string, string> = {};
        for (const h of newHeaders()) { if (h.key.trim()) hdrs[h.key.trim()] = h.value; }
        action = { type: 'http_webhook', url: newUrl(), method: newMethod(), body: newBody() || undefined, headers: Object.keys(hdrs).length > 0 ? hdrs : undefined };
        break;
      }
      case 'emit_event': {
        let payload: any = {};
        if (newPayloadJsonMode()) {
          try { payload = JSON.parse(newPayloadJson()); } catch { /* use empty */ }
        } else {
          const fields = newPayloadFields();
          for (const [k, v] of Object.entries(fields)) { if (v.trim()) payload[k] = v; }
        }
        action = { type: 'emit_event', topic: newTopic(), payload };
        break;
      }
      case 'invoke_agent':
        action = {
          type: 'invoke_agent',
          persona_id: newPersonaId(),
          task: newAgentTask(),
          friendly_name: newFriendlyName() || undefined,
          async_exec: newAsyncExec() || undefined,
          timeout_secs: newAsyncExec() ? undefined : (newTimeoutSecs() || undefined),
          permissions: newPermissions().length > 0 ? newPermissions() : undefined,
        };
        break;
      case 'call_tool':
        action = { type: 'call_tool', tool_id: newToolId(), arguments: { ...newToolArgs() } };
        break;
      case 'launch_workflow': {
        const wf = newWorkflowLaunch();
        if (!wf?.definition) { setCreating(false); return; }
        action = {
          type: 'launch_workflow',
          definition: wf.definition,
          version: wf.version,
          inputs: wf.inputs ?? {},
          trigger_step_id: wf.trigger_step_id,
        };
        break;
      }
      case 'composite_action':
        action = {
          type: 'composite_action',
          actions: subActions().map((sa) => buildSubAction(sa)),
          stop_on_failure: newStopOnFailure(),
        };
        break;
      default:
        action = { type: 'send_message', session_id: newSessionId(), content: newContent() };
    }

    setCreating(true);
    try {
      const resp = await authFetch(url, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ name: newName(), description: newDescription() || undefined, schedule, action }),
      });
      if (!resp.ok) throw new Error(await resp.text());
      setShowCreateForm(false);
      resetForm();
      await loadTasks();
    } catch (e: any) {
      setError(e.message ?? 'Failed to create task');
    } finally {
      setCreating(false);
    }
  };

  const resetForm = () => {
    setNewName('');
    setNewDescription('');
    setNewScheduleType('once');
    setNewRunAt('');
    setNewCronExpr('0 0 * * * * *');
    setNewActionType('send_message');
    setNewSessionId('');
    setNewContent('');
    setNewUrl('');
    setNewMethod('POST');
    setNewBody('');
    setNewHeaders([]);
    setNewTopic('');
    setNewPayloadFields({});
    setNewPayloadJsonMode(false);
    setNewPayloadJson('{}');
    setNewPersonaId('');
    setNewAgentTask('');
    setNewFriendlyName('');
    setNewTimeoutSecs(300);
    setNewAsyncExec(false);
    setNewPermissions([]);
    setNewToolId('');
    setNewToolArgs({});
    setNewWorkflowLaunch(null);
    setSubActions([]);
    setNewStopOnFailure(true);
  };

  // Subscribe to scheduler SSE events and refetch on changes (debounced)
  let debounceTimer: ReturnType<typeof setTimeout> | undefined;
  let disposed = false;
  const debouncedRefresh = () => {
    if (debounceTimer !== undefined) clearTimeout(debounceTimer);
    debounceTimer = setTimeout(() => {
      if (!disposed) {
        void loadTasks();
        const expanded = expandedTaskId();
        if (expanded) void loadRuns(expanded);
      }
    }, 200);
  };

  invoke('scheduler_subscribe_events').catch(() => {});
  const unlistenPromise = listen('scheduler:event', () => { debouncedRefresh(); });
  onCleanup(() => {
    disposed = true;
    if (debounceTimer !== undefined) clearTimeout(debounceTimer);
    void unlistenPromise.then((fn) => fn());
  });

  // Initial load
  createEffect(() => {
    if (props.daemon_url()) {
      void loadTasks();
      invoke<ChatSessionSummary[] | null>('chat_list_sessions').then(list => {
        setAvailableSessions(Array.isArray(list) ? list : []);
      }).catch(() => {});
    }
  });

  return (
    <div style="display:flex;flex-direction:column;height:100%;overflow:hidden;padding:16px;gap:12px;">
      {/* Header */}
      <div style="display:flex;align-items:center;gap:12px;">
        <h2 style="margin:0;color:hsl(var(--foreground));font-size:1.25em;">⏰ Task Scheduler</h2>
        <button
          class="icon-btn"
          onClick={() => void loadTasks()}
          disabled={loading()}
          title="Refresh"
          data-testid="scheduler-refresh-btn"
          aria-label="Refresh tasks"
          style="font-size:0.9em;"
        >
          <RefreshCw size={14} />
        </button>
        <Show when={showCreateForm()} fallback={
          <Button
            size="sm"
            class="ml-auto"
            data-testid="scheduler-new-task-btn"
            aria-label="New task"
            onClick={() => setShowCreateForm(true)}
          >
            <Plus size={14} /> New Task
          </Button>
        }>
          <Button
            size="sm"
            variant="outline"
            class="ml-auto"
            data-testid="scheduler-new-task-btn"
            aria-label="Cancel new task"
            onClick={() => setShowCreateForm(false)}
          >
            Cancel
          </Button>
        </Show>
      </div>

      {/* Error banner */}
      <Show when={error()}>
        <div style="padding:8px 12px;border-radius:6px;background:hsl(var(--destructive) / 0.15);color:hsl(var(--destructive));font-size:0.85em;display:flex;align-items:center;gap:8px;">
          <span><TriangleAlert size={14} /> {error()}</span>
          <button onClick={() => setError(null)} style="margin-left:auto;background:none;border:none;color:hsl(var(--destructive));cursor:pointer;">✕</button>
        </div>
      </Show>

      {/* Create form */}
      <Collapsible open={showCreateForm()} onOpenChange={setShowCreateForm}>
        <CollapsibleContent>
        <div data-testid="scheduler-create-form" style="border:1px solid hsl(var(--border));border-radius:8px;padding:16px;background:hsl(var(--card));display:flex;flex-direction:column;gap:12px;max-height:70vh;overflow-y:auto;">
          <h3 style="margin:0;color:hsl(var(--foreground));font-size:1em;">Create Task</h3>

          <div style="display:grid;grid-template-columns:1fr 1fr;gap:12px;">
            <div>
              <label style="display:block;font-size:0.8em;color:hsl(var(--muted-foreground));margin-bottom:4px;">Name *</label>
              <input type="text" data-testid="scheduler-task-name" aria-label="Task name" value={newName()} onInput={(e) => setNewName(e.currentTarget.value)} placeholder="My scheduled task" style="width:100%;padding:6px 10px;border-radius:4px;border:1px solid hsl(var(--border));background:hsl(var(--background));color:hsl(var(--foreground));font-size:0.85em;box-sizing:border-box;" />
            </div>
            <div>
              <label style="display:block;font-size:0.8em;color:hsl(var(--muted-foreground));margin-bottom:4px;">Description</label>
              <input type="text" value={newDescription()} onInput={(e) => setNewDescription(e.currentTarget.value)} placeholder="Optional description" style="width:100%;padding:6px 10px;border-radius:4px;border:1px solid hsl(var(--border));background:hsl(var(--background));color:hsl(var(--foreground));font-size:0.85em;box-sizing:border-box;" />
            </div>
          </div>

          {/* Schedule */}
          <div>
            <label style="display:block;font-size:0.8em;color:hsl(var(--muted-foreground));margin-bottom:4px;">Schedule</label>
            <div style="display:flex;gap:8px;align-items:center;flex-wrap:wrap;">
              <select data-testid="scheduler-schedule-type" aria-label="Schedule type" value={newScheduleType()} onInput={(e) => setNewScheduleType(e.currentTarget.value as any)} style="padding:6px 10px;border-radius:4px;border:1px solid hsl(var(--border));background:hsl(var(--background));color:hsl(var(--foreground));font-size:0.85em;">
                <option value="once">Once (immediate)</option>
                <option value="scheduled">Scheduled (specific time)</option>
                <option value="cron">Cron</option>
              </select>
              <Show when={newScheduleType() === 'scheduled'}>
                <input type="datetime-local" value={newRunAt()} onInput={(e) => setNewRunAt(e.currentTarget.value)} style="padding:6px 10px;border-radius:4px;border:1px solid hsl(var(--border));background:hsl(var(--background));color:hsl(var(--foreground));font-size:0.85em;" />
              </Show>
            </div>

            {/* Cron builder */}
            <Show when={newScheduleType() === 'cron'}>
              <div style="margin-top:8px;">
                <CronBuilder value={newCronExpr()} onChange={setNewCronExpr} />
              </div>
            </Show>
          </div>

          {/* Action */}
          <div>
            <label style="display:block;font-size:0.8em;color:hsl(var(--muted-foreground));margin-bottom:4px;">Action</label>
            <div style="display:flex;flex-direction:column;gap:8px;">
              <select data-testid="scheduler-action-type" aria-label="Action type" value={newActionType()} onInput={(e) => setNewActionType(e.currentTarget.value as any)} style="padding:6px 10px;border-radius:4px;border:1px solid hsl(var(--border));background:hsl(var(--background));color:hsl(var(--foreground));font-size:0.85em;width:fit-content;">
                <option value="invoke_agent">Invoke Agent</option>
                <option value="launch_workflow">Launch Workflow</option>
                <option value="send_message">Send Message</option>
                <option value="http_webhook">HTTP Webhook</option>
                <option value="emit_event">Emit Event</option>
                <option value="call_tool">Call Tool</option>
                <option value="composite_action">Composite Action</option>
              </select>

              {/* Send Message */}
              <Show when={newActionType() === 'send_message'}>
                <div style="display:grid;grid-template-columns:1fr 2fr;gap:8px;">
                  <select value={newSessionId()} onChange={(e) => setNewSessionId(e.currentTarget.value)} style="padding:6px 10px;border-radius:4px;border:1px solid hsl(var(--border));background:hsl(var(--background));color:hsl(var(--foreground));font-size:0.85em;">
                    <option value="">Select session…</option>
                    <For each={availableSessions()}>{(s) => <option value={s.id}>{s.title || s.id}</option>}</For>
                  </select>
                  <input type="text" value={newContent()} onInput={(e) => setNewContent(e.currentTarget.value)} placeholder="Message content" style="padding:6px 10px;border-radius:4px;border:1px solid hsl(var(--border));background:hsl(var(--background));color:hsl(var(--foreground));font-size:0.85em;" />
                </div>
              </Show>

              {/* HTTP Webhook */}
              <Show when={newActionType() === 'http_webhook'}>
                <div style="display:grid;grid-template-columns:auto 1fr;gap:8px;">
                  <select value={newMethod()} onInput={(e) => setNewMethod(e.currentTarget.value)} style="padding:6px 10px;border-radius:4px;border:1px solid hsl(var(--border));background:hsl(var(--background));color:hsl(var(--foreground));font-size:0.85em;">
                    <option value="GET">GET</option><option value="POST">POST</option><option value="PUT">PUT</option><option value="DELETE">DELETE</option><option value="PATCH">PATCH</option>
                  </select>
                  <input type="text" value={newUrl()} onInput={(e) => setNewUrl(e.currentTarget.value)} placeholder="https://example.com/webhook" style="padding:6px 10px;border-radius:4px;border:1px solid hsl(var(--border));background:hsl(var(--background));color:hsl(var(--foreground));font-size:0.85em;" />
                </div>
                <textarea value={newBody()} onInput={(e) => setNewBody(e.currentTarget.value)} placeholder='Optional JSON body: {"key":"value"}' rows={2} style="padding:6px 10px;border-radius:4px;border:1px solid hsl(var(--border));background:hsl(var(--background));color:hsl(var(--foreground));font-size:0.85em;resize:vertical;font-family:monospace;" />

                {/* Headers */}
                <div>
                  <div style="display:flex;align-items:center;gap:8px;margin-bottom:4px;">
                    <span style="font-size:0.8em;color:hsl(var(--muted-foreground));">Headers</span>
                    <button onClick={() => setNewHeaders([...newHeaders(), { key: '', value: '' }])} style="padding:2px 8px;border-radius:4px;border:1px solid hsl(var(--primary));background:transparent;color:hsl(var(--primary));cursor:pointer;font-size:0.75em;"><Plus size={12} /> Add</button>
                  </div>
                  <For each={newHeaders()}>
                    {(h, idx) => (
                      <div style="display:flex;gap:6px;margin-bottom:4px;align-items:center;">
                        <input type="text" value={h.key} onInput={(e) => { const arr = [...newHeaders()]; arr[idx()] = { ...arr[idx()], key: e.currentTarget.value }; setNewHeaders(arr); }} placeholder="Header name" style="flex:1;padding:4px 8px;border-radius:4px;border:1px solid hsl(var(--border));background:hsl(var(--background));color:hsl(var(--foreground));font-size:0.8em;" />
                        <input type="text" value={h.value} onInput={(e) => { const arr = [...newHeaders()]; arr[idx()] = { ...arr[idx()], value: e.currentTarget.value }; setNewHeaders(arr); }} placeholder="Value" style="flex:2;padding:4px 8px;border-radius:4px;border:1px solid hsl(var(--border));background:hsl(var(--background));color:hsl(var(--foreground));font-size:0.8em;" />
                        <button onClick={() => setNewHeaders(newHeaders().filter((_, i) => i !== idx()))} style="background:none;border:none;color:hsl(var(--destructive));cursor:pointer;font-size:0.85em;" title="Remove">✕</button>
                      </div>
                    )}
                  </For>
                </div>
              </Show>

              {/* Emit Event */}
              <Show when={newActionType() === 'emit_event'}>
                <label style="display:block;font-size:0.78em;color:hsl(var(--muted-foreground));margin-bottom:3px;">Topic *</label>
                <TopicSelector
                  value={newTopic()}
                  onChange={(t) => { setNewTopic(t); setNewPayloadFields({}); }}
                  topics={props.eventTopics ?? []}
                />

                {/* Payload builder */}
                <Show when={newTopic()}>
                  <div style="display:flex;align-items:center;gap:8px;margin-top:4px;">
                    <span style="font-size:0.78em;color:hsl(var(--muted-foreground));">Payload</span>
                    <button
                      onClick={() => setNewPayloadJsonMode(!newPayloadJsonMode())}
                      style="padding:1px 6px;border-radius:3px;border:1px solid hsl(var(--border));background:transparent;color:hsl(var(--primary));cursor:pointer;font-size:0.7em;"
                    >{newPayloadJsonMode() ? 'Form' : 'JSON'}</button>
                  </div>
                  <Show when={newPayloadJsonMode()}>
                    <textarea value={newPayloadJson()} onInput={(e) => setNewPayloadJson(e.currentTarget.value)} placeholder='{"key": "value"}' rows={3} style="padding:6px 10px;border-radius:4px;border:1px solid hsl(var(--border));background:hsl(var(--background));color:hsl(var(--foreground));font-size:0.85em;resize:vertical;font-family:monospace;" />
                  </Show>
                  <Show when={!newPayloadJsonMode()}>
                    {(() => {
                      const keys = payloadKeysForTopic(newTopic(), props.eventTopics ?? []);
                      if (keys.length === 0) return <span style="font-size:0.78em;color:hsl(var(--muted-foreground));font-style:italic;">No known payload fields. Switch to JSON for custom payload.</span>;
                      return (
                        <div style="display:flex;flex-direction:column;gap:4px;">
                          <For each={keys}>
                            {(key) => (
                              <div style="display:grid;grid-template-columns:120px 1fr;gap:8px;align-items:center;">
                                <label style="font-size:0.78em;color:hsl(var(--muted-foreground));text-align:right;">{key}</label>
                                <input type="text" value={newPayloadFields()[key] ?? ''} onInput={(e) => setNewPayloadFields({ ...newPayloadFields(), [key]: e.currentTarget.value })} placeholder={`Value for ${key}`} style="padding:4px 8px;border-radius:4px;border:1px solid hsl(var(--border));background:hsl(var(--background));color:hsl(var(--foreground));font-size:0.82em;" />
                              </div>
                            )}
                          </For>
                        </div>
                      );
                    })()}
                  </Show>
                </Show>
              </Show>

              {/* Invoke Agent */}
              <Show when={newActionType() === 'invoke_agent'}>
                <label style="display:block;font-size:0.78em;color:hsl(var(--muted-foreground));margin-bottom:3px;">Persona *</label>
                <PersonaSelector
                  value={newPersonaId()}
                  onChange={setNewPersonaId}
                  personas={props.personas ?? []}
                />
                <input type="text" value={newFriendlyName()} onInput={(e) => setNewFriendlyName(e.currentTarget.value)} placeholder="Friendly name (optional)" style="padding:6px 10px;border-radius:4px;border:1px solid hsl(var(--border));background:hsl(var(--background));color:hsl(var(--foreground));font-size:0.85em;" />
                <textarea value={newAgentTask()} onInput={(e) => setNewAgentTask(e.currentTarget.value)} placeholder="Task description for the agent" rows={3} style="padding:6px 10px;border-radius:4px;border:1px solid hsl(var(--border));background:hsl(var(--background));color:hsl(var(--foreground));font-size:0.85em;resize:vertical;font-family:inherit;" />
                <Switch
                  checked={newAsyncExec()}
                  onChange={(checked: boolean) => setNewAsyncExec(checked)}
                  class="flex items-center gap-2"
                >
                  <SwitchControl><SwitchThumb /></SwitchControl>
                  <SwitchLabel style="font-size:0.85em;color:hsl(var(--foreground));">Async Execution</SwitchLabel>
                </Switch>
                <Show when={!newAsyncExec()}>
                  <div style="display:flex;align-items:center;gap:8px;">
                    <label style="font-size:0.8em;color:hsl(var(--muted-foreground));">Timeout (seconds):</label>
                    <input type="number" value={newTimeoutSecs()} onInput={(e) => setNewTimeoutSecs(parseInt(e.currentTarget.value) || 300)} style="width:100px;padding:6px 10px;border-radius:4px;border:1px solid hsl(var(--border));background:hsl(var(--background));color:hsl(var(--foreground));font-size:0.85em;" />
                  </div>
                </Show>
                <div style="margin-top:6px;">
                  <label style="display:block;font-size:0.78em;color:hsl(var(--muted-foreground));margin-bottom:3px;">Permission Rules</label>
                  <div style="font-size:0.78em;color:hsl(var(--muted-foreground));margin-bottom:4px;">Tool approval policies for this agent. Leave empty to use defaults.</div>
                  <PermissionRulesEditor
                    rules={() => newPermissions()}
                    setRules={setNewPermissions}
                    toolDefinitions={props.tools as any}
                  />
                </div>
              </Show>

              {/* Call Tool */}
              <Show when={newActionType() === 'call_tool'}>
                <label style="display:block;font-size:0.78em;color:hsl(var(--muted-foreground));margin-bottom:3px;">Tool *</label>
                <ToolCallBuilder
                  tool_id={newToolId()}
                  arguments={newToolArgs()}
                  onToolChange={(id) => { setNewToolId(id); setNewToolArgs({}); }}
                  onArgsChange={setNewToolArgs}
                  tools={(props.tools ?? []) as any}
                  channels={props.channels}
                />
              </Show>

              {/* Launch Workflow */}
              <Show when={newActionType() === 'launch_workflow'}>
                <label style="display:block;font-size:0.78em;color:hsl(var(--muted-foreground));margin-bottom:3px;">Workflow *</label>
                <WorkflowLauncher
                  definitions={props.workflowDefinitions ?? []}
                  fetchParsedDefinition={props.fetchParsedWorkflow ?? (async () => null)}
                  value={newWorkflowLaunch()}
                  onChange={setNewWorkflowLaunch}
                />
              </Show>

              {/* Composite Action */}
              <Show when={newActionType() === 'composite_action'}>
                <div style="display:flex;flex-direction:column;gap:8px;padding:8px;border:1px solid hsl(var(--border));border-radius:6px;background:hsl(var(--background));">
                  <div style="display:flex;align-items:center;gap:8px;">
                    <Switch checked={newStopOnFailure()} onChange={(checked) => setNewStopOnFailure(checked)} class="flex items-center gap-2">
                      <SwitchControl><SwitchThumb /></SwitchControl>
                      <SwitchLabel>Stop on failure</SwitchLabel>
                    </Switch>
                    <button onClick={() => addSubAction()} style="margin-left:auto;padding:4px 10px;border-radius:4px;border:1px solid hsl(var(--primary));background:transparent;color:hsl(var(--primary));cursor:pointer;font-size:0.8em;">
                      <Plus size={14} /> Add Action
                    </button>
                  </div>
                  <For each={subActions()}>
                    {(sa) => (
                      <div style="border:1px solid hsl(var(--border));border-radius:4px;padding:8px;display:flex;flex-direction:column;gap:6px;">
                        <div style="display:flex;align-items:center;gap:8px;">
                          <select value={sa.actionType} onInput={(e) => updateSubActionType(sa.id, e.currentTarget.value as any)} style="padding:4px 8px;border-radius:4px;border:1px solid hsl(var(--border));background:hsl(var(--card));color:hsl(var(--foreground));font-size:0.8em;">
                            <option value="invoke_agent">Invoke Agent</option>
                            <option value="send_message">Send Message</option>
                            <option value="http_webhook">HTTP Webhook</option>
                            <option value="emit_event">Emit Event</option>
                            <option value="call_tool">Call Tool</option>
                          </select>
                          <button onClick={() => removeSubAction(sa.id)} style="margin-left:auto;background:none;border:none;color:hsl(var(--destructive));cursor:pointer;font-size:0.9em;" title="Remove">✕</button>
                        </div>
                        <Show when={sa.actionType === 'send_message'}>
                          <div style="display:grid;grid-template-columns:1fr 2fr;gap:6px;">
                            <select value={sa.fields.session_id ?? ''} onChange={(e) => updateSubAction(sa.id, 'session_id', e.currentTarget.value)} style="padding:4px 8px;border-radius:4px;border:1px solid hsl(var(--border));background:hsl(var(--card));color:hsl(var(--foreground));font-size:0.8em;">
                              <option value="">Select session…</option>
                              <For each={availableSessions()}>{(s) => <option value={s.id}>{s.title || s.id}</option>}</For>
                            </select>
                            <input type="text" value={sa.fields.content ?? ''} onInput={(e) => updateSubAction(sa.id, 'content', e.currentTarget.value)} placeholder="Message content" style="padding:4px 8px;border-radius:4px;border:1px solid hsl(var(--border));background:hsl(var(--card));color:hsl(var(--foreground));font-size:0.8em;" />
                          </div>
                        </Show>
                        <Show when={sa.actionType === 'http_webhook'}>
                          <div style="display:grid;grid-template-columns:auto 1fr;gap:6px;">
                            <select value={sa.fields.method ?? 'POST'} onInput={(e) => updateSubAction(sa.id, 'method', e.currentTarget.value)} style="padding:4px 8px;border-radius:4px;border:1px solid hsl(var(--border));background:hsl(var(--card));color:hsl(var(--foreground));font-size:0.8em;">
                              <option value="GET">GET</option><option value="POST">POST</option><option value="PUT">PUT</option><option value="DELETE">DELETE</option><option value="PATCH">PATCH</option>
                            </select>
                            <input type="text" value={sa.fields.url ?? ''} onInput={(e) => updateSubAction(sa.id, 'url', e.currentTarget.value)} placeholder="URL" style="padding:4px 8px;border-radius:4px;border:1px solid hsl(var(--border));background:hsl(var(--card));color:hsl(var(--foreground));font-size:0.8em;" />
                          </div>
                          <input type="text" value={sa.fields.body ?? ''} onInput={(e) => updateSubAction(sa.id, 'body', e.currentTarget.value)} placeholder="Optional JSON body" style="padding:4px 8px;border-radius:4px;border:1px solid hsl(var(--border));background:hsl(var(--card));color:hsl(var(--foreground));font-size:0.8em;" />
                        </Show>
                        <Show when={sa.actionType === 'emit_event'}>
                          <input type="text" value={sa.fields.topic ?? ''} onInput={(e) => updateSubAction(sa.id, 'topic', e.currentTarget.value)} placeholder="Event topic" style="padding:4px 8px;border-radius:4px;border:1px solid hsl(var(--border));background:hsl(var(--card));color:hsl(var(--foreground));font-size:0.8em;" />
                        </Show>
                        <Show when={sa.actionType === 'invoke_agent'}>
                          <div style="display:grid;grid-template-columns:1fr 1fr;gap:6px;">
                            <input type="text" value={sa.fields.persona_id ?? ''} onInput={(e) => updateSubAction(sa.id, 'persona_id', e.currentTarget.value)} placeholder="Persona ID" style="padding:4px 8px;border-radius:4px;border:1px solid hsl(var(--border));background:hsl(var(--card));color:hsl(var(--foreground));font-size:0.8em;" />
                            <input type="text" value={sa.fields.friendly_name ?? ''} onInput={(e) => updateSubAction(sa.id, 'friendly_name', e.currentTarget.value)} placeholder="Friendly name (optional)" style="padding:4px 8px;border-radius:4px;border:1px solid hsl(var(--border));background:hsl(var(--card));color:hsl(var(--foreground));font-size:0.8em;" />
                          </div>
                          <textarea value={sa.fields.task ?? ''} onInput={(e) => updateSubAction(sa.id, 'task', e.currentTarget.value)} placeholder="Agent task" rows={2} style="padding:4px 8px;border-radius:4px;border:1px solid hsl(var(--border));background:hsl(var(--card));color:hsl(var(--foreground));font-size:0.8em;resize:vertical;font-family:inherit;" />
                          <div style="display:flex;align-items:center;gap:8px;">
                            <Switch
                              checked={sa.fields.async_exec === 'true'}
                              onChange={(checked: boolean) => updateSubAction(sa.id, 'async_exec', String(checked))}
                              class="flex items-center gap-2"
                            >
                              <SwitchControl><SwitchThumb /></SwitchControl>
                              <SwitchLabel style="font-size:0.8em;color:hsl(var(--foreground));">Async</SwitchLabel>
                            </Switch>
                          </div>
                          <Show when={sa.fields.async_exec !== 'true'}>
                            <div style="display:flex;align-items:center;gap:6px;">
                              <label style="font-size:0.75em;color:hsl(var(--muted-foreground));">Timeout (s):</label>
                              <input type="number" value={sa.fields.timeout_secs ?? '300'} onInput={(e) => updateSubAction(sa.id, 'timeout_secs', e.currentTarget.value)} style="width:80px;padding:4px 8px;border-radius:4px;border:1px solid hsl(var(--border));background:hsl(var(--card));color:hsl(var(--foreground));font-size:0.8em;" />
                            </div>
                          </Show>
                        </Show>
                        <Show when={sa.actionType === 'call_tool'}>
                          <input type="text" value={sa.fields.tool_id ?? ''} onInput={(e) => updateSubAction(sa.id, 'tool_id', e.currentTarget.value)} placeholder="Tool ID" style="padding:4px 8px;border-radius:4px;border:1px solid hsl(var(--border));background:hsl(var(--card));color:hsl(var(--foreground));font-size:0.8em;" />
                          <textarea value={sa.fields.arguments ?? '{}'} onInput={(e) => updateSubAction(sa.id, 'arguments', e.currentTarget.value)} placeholder='{"key": "value"}' rows={2} style="padding:4px 8px;border-radius:4px;border:1px solid hsl(var(--border));background:hsl(var(--card));color:hsl(var(--foreground));font-size:0.8em;resize:vertical;font-family:monospace;" />
                        </Show>
                      </div>
                    )}
                  </For>
                  <Show when={subActions().length === 0}>
                    <p style="font-size:0.8em;color:hsl(var(--muted-foreground));margin:4px 0;text-align:center;">No sub-actions yet. Click "Add Action" to add one.</p>
                  </Show>
                </div>
              </Show>
            </div>
          </div>

          <div style="display:flex;justify-content:flex-end;gap:8px;">
            <button onClick={() => { setShowCreateForm(false); resetForm(); }} style="padding:6px 14px;border-radius:6px;border:1px solid hsl(var(--border));background:transparent;color:hsl(var(--muted-foreground));cursor:pointer;font-size:0.85em;">
              Cancel
            </button>
            <button data-testid="scheduler-create-btn" aria-label="Create task" onClick={() => void createTask()} disabled={creating() || !newName().trim()} style="padding:6px 14px;border-radius:6px;border:1px solid hsl(var(--primary));background:hsl(var(--primary));color:hsl(var(--background));cursor:pointer;font-weight:600;font-size:0.85em;">
              {creating() ? 'Creating…' : 'Create'}
            </button>
          </div>
        </div>
        </CollapsibleContent>
      </Collapsible>

      {/* Task list */}
      <div style="flex:1;overflow-y:auto;">
        <Show when={!loading() || tasks().length > 0} fallback={
          <div style="text-align:center;padding:40px;color:hsl(var(--muted-foreground));">Loading tasks…</div>
        }>
          <Show when={tasks().length > 0} fallback={
            <EmptyState
              icon={<ClipboardList size={32} />}
              title="No scheduled tasks yet"
              description="Create one manually or let an agent schedule tasks using the core.schedule_task tool."
            />
          }>
            <div style="display:flex;flex-direction:column;gap:6px;">
              <For each={tasks()}>
                {(task) => (
                  <Collapsible
                    open={expandedTaskId() === task.id}
                    onOpenChange={(open) => { if (open) void toggleExpand(task.id); else if (expandedTaskId() === task.id) void toggleExpand(task.id); }}
                  >
                  <div style="border:1px solid hsl(var(--border));border-radius:8px;background:hsl(var(--card));overflow:hidden;">
                    {/* Task row */}
                    <div
                      style="display:grid;grid-template-columns:1fr auto auto auto auto;gap:12px;align-items:center;padding:10px 14px;cursor:pointer;"
                      onClick={() => void toggleExpand(task.id)}
                    >
                      <div style="display:flex;flex-direction:column;gap:2px;min-width:0;">
                        <div style="display:flex;align-items:center;gap:8px;">
                          <span style="font-weight:600;color:hsl(var(--foreground));font-size:0.9em;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;">
                            {task.name}
                          </span>
                          <span class={statusBadgeClass(task.status)} style="font-size:0.7em;flex-shrink:0;">
                            {task.status}
                          </span>
                        </div>
                        <Show when={task.description}>
                          <span style="font-size:0.75em;color:hsl(var(--muted-foreground));white-space:nowrap;overflow:hidden;text-overflow:ellipsis;">
                            {task.description}
                          </span>
                        </Show>
                      </div>

                      <div style="text-align:right;min-width:120px;">
                        <div style="font-size:0.75em;color:hsl(var(--muted-foreground));">{formatSchedule(task.schedule)}</div>
                      </div>

                      <div style="text-align:right;min-width:80px;">
                        <div style="font-size:0.75em;color:hsl(var(--muted-foreground));">Runs: {task.run_count}</div>
                        <div style="font-size:0.7em;color:hsl(var(--muted-foreground));">Next: {formatTimestamp(task.next_run_ms)}</div>
                      </div>

                      <div style="display:flex;gap:4px;flex-shrink:0;">
                        <Show when={task.status === 'pending' || task.status === 'running'}>
                          <button
                            class="icon-btn"
                            title="Cancel"
                            style="font-size:0.8em;padding:4px;"
                            onClick={(e) => { e.stopPropagation(); void cancelTask(task.id); }}
                          >
                            <Pause size={14} />
                          </button>
                        </Show>
                        <button
                          class="icon-btn"
                          title="Delete"
                          style="font-size:0.8em;padding:4px;"
                          onClick={(e) => { e.stopPropagation(); void deleteTask(task.id); }}
                        >
                          <Trash2 size={14} />
                        </button>
                      </div>

                      <span style="font-size:0.8em;color:hsl(var(--muted-foreground));flex-shrink:0;">
                        {expandedTaskId() === task.id ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
                      </span>
                    </div>

                    {/* Expanded detail */}
                    <CollapsibleContent>
                      <div style="border-top:1px solid hsl(var(--border));padding:12px 14px;background:hsl(var(--background));">
                        <div style="display:grid;grid-template-columns:1fr 1fr;gap:12px;font-size:0.8em;color:hsl(var(--muted-foreground));margin-bottom:12px;">
                          <div><strong>ID:</strong> {task.id}</div>
                          <div><strong>Action:</strong> {formatAction(task.action)}</div>
                          <div><strong>Created:</strong> {formatTimestamp(task.created_at_ms)}</div>
                          <div><strong>Last run:</strong> {formatTimestamp(task.last_run_ms)}</div>
                          <Show when={task.owner_session_id}>
                            <div><strong>Session:</strong> {task.owner_session_id}</div>
                          </Show>
                          <Show when={task.owner_agent_id}>
                            <div><strong>Agent:</strong> {task.owner_agent_id}</div>
                          </Show>
                          <Show when={task.last_error}>
                            <div style="grid-column:1/-1;color:hsl(var(--destructive));">
                              <strong>Last error:</strong> {task.last_error}
                            </div>
                          </Show>
                        </div>

                        {/* Full action details */}
                        <Show when={task.action.type === 'invoke_agent'}>
                          <div style="font-size:0.8em;color:hsl(var(--muted-foreground));margin-bottom:12px;padding:8px;border:1px solid hsl(var(--border));border-radius:4px;">
                            <div style="margin-bottom:4px;"><strong>Persona:</strong> {(task.action as any).persona_id}</div>
                            <div style="margin-bottom:4px;"><strong>Task:</strong> {(task.action as any).task}</div>
                            <Show when={(task.action as any).friendly_name}>
                              <div style="margin-bottom:4px;"><strong>Friendly Name:</strong> {(task.action as any).friendly_name}</div>
                            </Show>
                            <Show when={(task.action as any).async_exec}>
                              <div style="margin-bottom:4px;"><strong>Async:</strong> Yes</div>
                            </Show>
                            <Show when={!(task.action as any).async_exec}>
                              <div style="margin-bottom:4px;"><strong>Timeout:</strong> {(task.action as any).timeout_secs ?? 300}s</div>
                            </Show>
                            <Show when={(task.action as any).permissions?.length}>
                              <div style="margin-bottom:4px;"><strong>Permissions:</strong> {(task.action as any).permissions.length} rule(s)</div>
                            </Show>
                          </div>
                        </Show>
                        <Show when={task.action.type === 'call_tool'}>
                          <div style="font-size:0.8em;color:hsl(var(--muted-foreground));margin-bottom:12px;padding:8px;border:1px solid hsl(var(--border));border-radius:4px;">
                            <div style="margin-bottom:4px;"><strong>Tool:</strong> {(task.action as any).tool_id}</div>
                            <div><strong>Arguments:</strong></div>
                            <YamlBlock data={(task.action as any).arguments} style="margin:4px 0 0;font-size:0.9em;max-height:200px;" />
                          </div>
                        </Show>
                        <Show when={task.action.type === 'launch_workflow'}>
                          <div style="font-size:0.8em;color:hsl(var(--muted-foreground));margin-bottom:12px;padding:8px;border:1px solid hsl(var(--border));border-radius:4px;">
                            <div style="margin-bottom:4px;"><strong>Workflow:</strong> {(task.action as any).definition}</div>
                            <Show when={(task.action as any).version}>
                              <div style="margin-bottom:4px;"><strong>Version:</strong> {(task.action as any).version}</div>
                            </Show>
                            <Show when={(task.action as any).trigger_step_id}>
                              <div style="margin-bottom:4px;"><strong>Trigger:</strong> {(task.action as any).trigger_step_id}</div>
                            </Show>
                            <div><strong>Inputs:</strong></div>
                            <YamlBlock data={(task.action as any).inputs} style="margin:4px 0 0;font-size:0.9em;max-height:200px;" />
                          </div>
                        </Show>
                        <Show when={task.action.type === 'composite_action'}>
                          <div style="font-size:0.8em;color:hsl(var(--muted-foreground));margin-bottom:12px;padding:8px;border:1px solid hsl(var(--border));border-radius:4px;">
                            <div style="margin-bottom:6px;">
                              <strong>Composite Action</strong> — {(task.action as any).actions?.length ?? 0} sub-actions
                              {(task.action as any).stop_on_failure ? ' (stops on failure)' : ''}
                            </div>
                            <ol style="margin:0;padding-left:20px;">
                              <For each={(task.action as any).actions ?? []}>
                                {(sub: TaskAction, i: () => number) => (
                                  <li style="margin-bottom:4px;">{formatAction(sub)}</li>
                                )}
                              </For>
                            </ol>
                          </div>
                        </Show>

                        {/* Execution history */}
                        <h4 style="margin:0 0 8px;color:hsl(var(--foreground));font-size:0.85em;">Execution History</h4>
                        <Show when={taskRuns().length > 0} fallback={
                          <p style="font-size:0.8em;color:hsl(var(--muted-foreground));margin:0;">No runs recorded yet.</p>
                        }>
                          <div style="max-height:200px;overflow-y:auto;">
                            <table style="width:100%;border-collapse:collapse;font-size:0.8em;">
                              <thead>
                                <tr style="color:hsl(var(--muted-foreground));text-align:left;">
                                  <th style="padding:4px 8px;border-bottom:1px solid hsl(var(--border));">Started</th>
                                  <th style="padding:4px 8px;border-bottom:1px solid hsl(var(--border));">Completed</th>
                                  <th style="padding:4px 8px;border-bottom:1px solid hsl(var(--border));">Status</th>
                                  <th style="padding:4px 8px;border-bottom:1px solid hsl(var(--border));">Error</th>
                                  <th style="padding:4px 8px;border-bottom:1px solid hsl(var(--border));">Result</th>
                                </tr>
                              </thead>
                              <tbody>
                                <For each={taskRuns()}>
                                  {(run) => (
                                    <tr style="color:hsl(var(--foreground));">
                                      <td style="padding:4px 8px;border-bottom:1px solid hsl(var(--border));">{formatTimestamp(run.started_at_ms)}</td>
                                      <td style="padding:4px 8px;border-bottom:1px solid hsl(var(--border));">{formatTimestamp(run.completed_at_ms)}</td>
                                      <td style="padding:4px 8px;border-bottom:1px solid hsl(var(--border));">
                                        <span class={run.status === 'success' ? 'pill success' : 'pill danger'} style="font-size:0.85em;">
                                          {run.status}
                                        </span>
                                      </td>
                                      <td style="padding:4px 8px;border-bottom:1px solid hsl(var(--border));max-width:300px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">
                                        {run.error ?? '—'}
                                      </td>
                                      <td style="padding:4px 8px;border-bottom:1px solid hsl(var(--border));max-width:400px;">
                                        <Show when={run.result != null} fallback={<span>—</span>}>
                                          <YamlBlock data={run.result} style="padding:4px;font-size:0.85em;max-height:120px;" />
                                        </Show>
                                      </td>
                                    </tr>
                                  )}
                                </For>
                              </tbody>
                            </table>
                          </div>
                        </Show>
                      </div>
                    </CollapsibleContent>
                  </div>
                  </Collapsible>
                )}
              </For>
            </div>
          </Show>
        </Show>
      </div>
    </div>
  );
};

export default SchedulerPage;
