import { For, Show, createEffect, createSignal, onCleanup } from 'solid-js';
import type { WorkflowStore } from '../stores/workflowStore';
import type { StepState } from '../types';
import type { InteractionStore } from '../stores/interactionStore';
import { YamlBlock } from './YamlHighlight';
import { pendingApprovalToasts, dismissAgentApproval, type PendingApproval } from './AgentApprovalToast';
import { invoke } from '@tauri-apps/api/core';
import { answerQuestion, respondToApproval, type PendingInteraction } from '~/lib/interactionRouting';
import { useAbortableEffect } from '~/lib/useAbortableEffect';
import { renderMarkdown } from '~/utils';
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter } from '~/ui/dialog';
import { Button } from '~/ui/button';
import {
  Bell, Wrench, Bot, Hand, Timer, Radio, RotateCcw, Calendar, GitBranch, Square,
  Play, Pause, CircleStop, Lock, HelpCircle,
} from 'lucide-solid';

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

interface WorkflowDetailPanelProps {
  store: WorkflowStore;
  interactionStore?: InteractionStore;
  instanceId: number;
  onApprovalClick?: (approval: PendingApproval) => void;
}

// ---------------------------------------------------------------------------
// Helpers (mirrored from WorkflowsPage)
// ---------------------------------------------------------------------------

/** Extract the task object from a step, handling multiple backend serialization shapes. */
function getStepTask(step: any): any | undefined {
  return step.task ?? step.step_type?.Task ?? step.Task;
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

function StepIcon(props: { type: any; size?: number }) {
  const s = () => props.size ?? 14;
  const t = props.type;
  if (!t) return <Square size={s()} />;
  if (t === 'trigger' || t.Trigger) return <Bell size={s()} />;
  if (t === 'control_flow' || t.ControlFlow) return <GitBranch size={s()} />;
  if (t === 'task' || t.Task) {
    const task = typeof t === 'object' ? t.Task : null;
    if (!task) return <Wrench size={s()} />;
    if (task.CallTool) return <Wrench size={s()} />;
    if (task.InvokeAgent) return <Bot size={s()} />;
    if (task.FeedbackGate) return <Hand size={s()} />;
    if (task.Delay) return <Timer size={s()} />;
    if (task.SignalAgent) return <Radio size={s()} />;
    if (task.LaunchWorkflow) return <RotateCcw size={s()} />;
    if (task.ScheduleTask) return <Calendar size={s()} />;
  }
  return <Square size={s()} />;
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

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

export default function WorkflowDetailPanel(props: WorkflowDetailPanelProps) {
  const store = props.store;

  const [feedbackStep, setFeedbackStep] = createSignal<{
    instanceId: number;
    stepId: string;
    prompt: string;
    choices: string[];
    allow_freeform: boolean;
  } | null>(null);
  const [feedbackText, setFeedbackText] = createSignal('');
  const [confirmKill, setConfirmKill] = createSignal<number | null>(null);
  const [now, setNow] = createSignal(Date.now());

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
  const [pendingApproval, setPendingApproval] = createSignal<PendingInteraction | null>(null);
  const [approvalSending, setApprovalSending] = createSignal(false);

  const [feedbackError, setFeedbackError] = createSignal<string | null>(null);

  // Live-updating timer for running workflow durations
  const elapsedTimer = setInterval(() => setNow(Date.now()), 1000);
  onCleanup(() => clearInterval(elapsedTimer));

  // Load (or reload) the full instance when instanceId changes (with stale-response guard)
  useAbortableEffect((signal) => {
    const id = props.instanceId;
    if (id != null) {
      void store.loadInstance(id).then(() => {
        if (signal.aborted) return;
      });
    }
  });

  // Derived data
  const summary = () => store.instances().find((i) => i.id === props.instanceId);
  const detail = () => store.selectedInstance();

  // ---- Feedback gate helpers ----

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
    setFeedbackStep({ instanceId, stepId: step.id, prompt, choices, allow_freeform });
    setFeedbackText('');
    setFeedbackError(null);
  }

  async function submitFeedback(choice?: string) {
    const gate = feedbackStep();
    if (!gate) return;
    const response = choice
      ? { selected: choice, text: feedbackText() }
      : { selected: feedbackText(), text: feedbackText() };
    try {
      await store.respondToGate(gate.instanceId, gate.stepId, response);
      setFeedbackStep(null);
      setFeedbackText('');
      setFeedbackError(null);
      await store.refresh();
    } catch (e: any) {
      setFeedbackError(e?.message ?? 'Failed to submit feedback');
    }
  }

  async function handleKillConfirmed(id: number) {
    await store.killInstance(id);
    setConfirmKill(null);
  }

  async function openAgentQuestion() {
    const s = summary();
    if (!s) return;
    const childIds = s.child_agent_ids ?? [];
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
          session_id: match.session_id ?? s.parent_session_id,
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

  function openApprovalDialog() {
    const childIds = summary()?.child_agent_ids ?? [];
    // Search interaction store by child agent entity IDs (interactions are keyed by agent, not workflow)
    const allInteractions = props.interactionStore?.interactions() ?? [];
    const approval = allInteractions.find(
      i => i.type === 'tool_approval' && i.agent_id && childIds.includes(i.agent_id)
    );
    if (approval) {
      setPendingApproval(approval);
      return;
    }
    // Fallback: look up from toast data
    const toast = pendingApprovalToasts().find(a => childIds.includes(a.agent_id));
    if (toast) {
      setPendingApproval({
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
    const a = pendingApproval();
    if (!a) return;
    setApprovalSending(true);
    try {
      await respondToApproval(a, { approved, allow_agent: opts?.allow_agent, allow_session: opts?.allow_session });
      dismissAgentApproval(a.request_id);
      setPendingApproval(null);
      await store.refresh();
      props.interactionStore?.poll();
    } catch (err) {
      console.error('Failed to respond to approval:', err);
    } finally {
      setApprovalSending(false);
    }
  }

  return (
    <div style="display:flex;flex-direction:column;height:100%;overflow:hidden;">
      {/* Header bar */}
      <div class="flex items-center justify-between border-b border-input px-4 py-2">
        <div style="display:flex;align-items:center;gap:10px;min-width:0;">
          <span style="font-weight:600;color:var(--text-primary, #cdd6f4);font-size:1em;">
            {summary()?.definition_name ?? detail()?.definition?.name ?? 'Workflow'}
          </span>
          <Show when={summary()?.status ?? detail()?.status}>
            {(status) => (
              <span class={statusPill(status())} style="font-size:0.75em;flex-shrink:0;">
                {statusLabel(status())}
              </span>
            )}
          </Show>
          <Show when={summary()?.status === 'waiting_on_input'}>
            <span style="animation:pulse 2s infinite;font-size:0.9em;"><Bell size={14} /></span>
          </Show>
          {/* Child agent approval badge */}
          {(() => {
            const c = props.interactionStore?.badgeCountForEntity(`workflow/${props.instanceId}`);
            const approvals = Math.max(summary()?.pending_agent_approvals ?? 0, c?.approvals ?? 0);
            const questions = Math.max(summary()?.pending_agent_questions ?? 0, c?.questions ?? 0);
            return <>
              <Show when={approvals > 0}>
                <span
                  style="animation:pulse 2s infinite;font-size:0.75em;padding:1px 6px;border-radius:8px;background:rgba(250,179,135,0.15);color:#fab387;cursor:pointer;"
                  title="Child agents need tool approval — click to review"
                  onClick={() => openApprovalDialog()}
                >
                  <Lock size={14} /> {approvals}
                </span>
              </Show>
              {/* Child agent question badge */}
              <Show when={questions > 0}>
                <span
                  style="animation:pulse 2s infinite;font-size:0.75em;padding:1px 6px;border-radius:8px;background:rgba(137,180,250,0.15);color:#89b4fa;cursor:pointer;"
                  title="Child agents have pending questions — click to answer"
                  onClick={() => void openAgentQuestion()}
                >
                  <HelpCircle size={14} /> {questions}
                </span>
              </Show>
            </>;
          })()}
        </div>

        {/* Right side: meta + action buttons */}
        <div style="display:flex;align-items:center;gap:12px;flex-shrink:0;">
          <span style="font-size:0.75em;color:var(--text-secondary, #a6adc8);white-space:nowrap;">
            {props.instanceId} • v{summary()?.definition_version ?? '?'} • {formatTime(summary()?.created_at_ms ?? detail()?.created_at_ms ?? 0)}
          </span>
          <span style="font-size:0.8em;color:var(--text-secondary, #a6adc8);">
            {durationStr(
              summary()?.created_at_ms ?? detail()?.created_at_ms ?? 0,
              summary()?.completed_at_ms ?? detail()?.completed_at_ms,
              now(),
            )}
          </span>
          <div style="display:flex;gap:4px;">
            <Show when={['running', 'waiting_on_input', 'waiting_on_event'].includes(summary()?.status ?? detail()?.status ?? '')}>
              <button class="icon-btn" title="Pause" style="font-size:0.8em;padding:4px;" onClick={() => void store.pauseInstance(props.instanceId)}>
                <Pause size={14} />
              </button>
            </Show>
            <Show when={(summary()?.status ?? detail()?.status) === 'paused'}>
              <button class="icon-btn" title="Resume" style="font-size:0.8em;padding:4px;" onClick={() => void store.resumeInstance(props.instanceId)}>
                <Play size={14} />
              </button>
            </Show>
            <Show when={['running', 'paused', 'waiting_on_input', 'waiting_on_event', 'pending'].includes(summary()?.status ?? detail()?.status ?? '')}>
              <button class="icon-btn" title="Kill" style="font-size:0.8em;padding:4px;color:var(--danger-text, #f38ba8);" onClick={() => setConfirmKill(props.instanceId)}>
                <CircleStop size={14} />
              </button>
            </Show>
          </div>
        </div>
      </div>

      {/* Main content area */}
      <div style="flex:1;overflow-y:auto;padding:14px;">
        <Show when={detail()} fallback={
          <div style="color:var(--text-secondary, #a6adc8);font-size:0.85em;padding:20px;text-align:center;">Loading instance…</div>
        }>
          {(_) => {
            const d = () => detail()!;
            return (
              <>
                {/* Steps */}
                <div style="margin-bottom:14px;">
                  <div style="font-weight:600;color:var(--text-primary, #cdd6f4);font-size:0.85em;margin-bottom:6px;">Steps</div>
                  <div style="display:flex;flex-wrap:wrap;gap:6px;">
                    <For each={d().definition?.steps ?? []}>
                      {(step: any) => {
                        const state = () => d().step_states[step.id];
                        const isWaiting = () => state()?.status === 'waiting_on_input';
                        const isFeedbackGate = () => {
                          const task = getStepTask(step);
                          return !!(task?.kind === 'feedback_gate' || task?.FeedbackGate || task?.feedback_gate);
                        };
                        const childAgentId = () => state()?.child_agent_id;
                        const childApprovals = () => {
                          const aid = childAgentId();
                          return aid ? pendingApprovalToasts().filter(a => a.agent_id === aid).length : 0;
                        };
                        const hasChildInteraction = () => childApprovals() > 0;
                        return (
                          <div
                            style={`display:flex;flex-direction:column;align-items:center;gap:2px;padding:8px 12px;border-radius:6px;border:1px solid var(--border, #45475a);background:var(--bg-primary, #1e1e2e);min-width:80px;${isWaiting() ? 'animation:pulse 2s infinite;border-color:var(--warning-border, #f9e2af);cursor:pointer;' : ''}${hasChildInteraction() ? 'animation:pulse 2s infinite;border-color:rgba(250,179,135,0.6);' : ''}`}
                            title={`${step.id}: ${state()?.status ?? 'unknown'}${state()?.error ? '\nError: ' + state()!.error : ''}${isWaiting() ? '\nClick to respond' : ''}${hasChildInteraction() ? '\nChild agent needs approval' : ''}`}
                            onClick={() => isWaiting() && isFeedbackGate() ? openFeedbackGate(d().id, step, state()) : undefined}
                          >
                            <StepIcon type={step.step_type ?? step.type} size={18} />
                            <span style="font-size:0.75em;font-weight:500;color:var(--text-primary, #cdd6f4);text-align:center;">{step.id}</span>
                            <span class={statusPill(state()?.status ?? 'pending')} style="font-size:0.65em;">
                              {statusLabel(state()?.status ?? 'pending')}
                            </span>
                            <Show when={isWaiting()}>
                              <span style="font-size:0.65em;color:var(--warning-border, #f9e2af);margin-top:2px;">click to respond</span>
                            </Show>
                            <Show when={state()?.status === 'waiting_on_event' && (() => { const task = getStepTask(step); return task?.kind === 'event_gate' || task?.EventGate || task?.event_gate; })()}>
                              <span style="font-size:0.6em;color:var(--warning-border, #f9e2af);margin-top:2px;text-align:center;">
                                <Bell size={14} /> {(() => { const task = getStepTask(step); return task?.topic ?? task?.EventGate?.topic ?? task?.event_gate?.topic ?? 'event'; })()}
                              </span>
                            </Show>
                            <Show when={hasChildInteraction()}>
                              <span style="font-size:0.6em;color:#fab387;margin-top:2px;">
                                <Lock size={14} /> {childApprovals()} pending
                              </span>
                            </Show>
                          </div>
                        );
                      }}
                    </For>
                  </div>
                </div>

                {/* Variables */}
                <Show when={d().variables && Object.keys(d().variables).length > 0}>
                  <div style="margin-bottom:14px;">
                    <div style="font-weight:600;color:var(--text-primary, #cdd6f4);font-size:0.85em;margin-bottom:4px;">Variables</div>
                    <YamlBlock data={d().variables} style="border:1px solid var(--border, #45475a);font-size:0.75em;max-height:200px;" />
                  </div>
                </Show>

                {/* Output */}
                <Show when={d().output}>
                  <div style="margin-bottom:14px;">
                    <div style="font-weight:600;color:var(--text-primary, #cdd6f4);font-size:0.85em;margin-bottom:4px;">Output</div>
                    <YamlBlock data={d().output} style="border:1px solid var(--border, #45475a);font-size:0.75em;max-height:200px;" />
                  </div>
                </Show>

                {/* Error */}
                <Show when={d().error}>
                  <div style="margin-bottom:14px;background:var(--danger-bg, #45283c);border:1px solid var(--danger-border, #f38ba8);color:var(--danger-text, #f38ba8);padding:8px;border-radius:4px;font-size:0.8em;">
                    {d().error}
                  </div>
                </Show>

                {/* Metadata */}
                <div style="margin-bottom:14px;">
                  <div style="font-weight:600;color:var(--text-primary, #cdd6f4);font-size:0.85em;margin-bottom:4px;">Metadata</div>
                  <div style="display:grid;grid-template-columns:auto 1fr;gap:4px 12px;font-size:0.8em;color:var(--text-secondary, #a6adc8);">
                    <span>Instance ID</span>
                    <span style="color:var(--text-primary, #cdd6f4);font-family:monospace;font-size:0.9em;">{props.instanceId}</span>
                    <span>Created</span>
                    <span>{formatTime(d().created_at_ms)}</span>
                    <Show when={d().updated_at_ms}>
                      <span>Updated</span>
                      <span>{formatTime(d().updated_at_ms)}</span>
                    </Show>
                    <Show when={d().completed_at_ms}>
                      <span>Completed</span>
                      <span>{formatTime(d().completed_at_ms!)}</span>
                    </Show>
                    <Show when={summary()?.step_count != null}>
                      <span>Progress</span>
                      <span>
                        {summary()?.steps_completed ?? 0} / {summary()?.step_count ?? 0} steps completed
                        <Show when={(summary()?.steps_running ?? 0) > 0}>{' '}({summary()!.steps_running} running)</Show>
                        <Show when={(summary()?.steps_failed ?? 0) > 0}>{' '}({summary()!.steps_failed} failed)</Show>
                      </span>
                    </Show>
                  </div>
                </div>
              </>
            );
          }}
        </Show>
      </div>

      {/* Kill confirmation dialog */}
      <Dialog open={!!confirmKill()} onOpenChange={(open) => { if (!open) setConfirmKill(null); }}>
        <DialogContent class="max-w-[400px]">
          <DialogHeader>
            <DialogTitle>Kill Workflow?</DialogTitle>
          </DialogHeader>
          <p style="font-size:0.85em;color:var(--text-secondary, #a6adc8);">
            This will immediately terminate the workflow instance. This action cannot be undone.
          </p>
          <DialogFooter class="flex-row justify-end gap-2">
            <Button variant="outline" onClick={() => setConfirmKill(null)}>Cancel</Button>
            <Button variant="destructive" onClick={() => void handleKillConfirmed(confirmKill()!)}>Kill</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* Feedback gate dialog */}
      <Dialog open={!!feedbackStep()} onOpenChange={(open) => { if (!open) setFeedbackStep(null); }}>
        <DialogContent class="max-w-[450px]">
          <Show when={feedbackStep()}>
            {(gate) => (
              <>
                <DialogHeader>
                  <DialogTitle>Respond to Step: {gate().stepId}</DialogTitle>
                </DialogHeader>
                <div class="prose prose-sm dark:prose-invert max-w-none" style="font-size:0.85em;margin:8px 0;" innerHTML={renderMarkdown(gate().prompt)} />
                <Show when={feedbackError()}>
                  <p style="font-size:0.8em;color:var(--danger-text, #f38ba8);margin:4px 0;">{feedbackError()}</p>
                </Show>
                <Show when={gate().choices.length > 0}>
                  <div style="display:flex;flex-wrap:wrap;gap:6px;margin-bottom:8px;">
                    <For each={gate().choices}>
                      {(choice) => (
                        <Button variant="outline" style="font-size:0.85em;" onClick={() => void submitFeedback(choice)}>{choice}</Button>
                      )}
                    </For>
                  </div>
                </Show>
                <Show when={gate().allow_freeform}>
                  <textarea
                    style="width:100%;min-height:60px;resize:vertical;background:var(--bg-primary, #1e1e2e);color:var(--text-primary, #cdd6f4);border:1px solid var(--border, #45475a);border-radius:6px;padding:8px;font-size:0.85em;"
                    placeholder="Type your response…"
                    value={feedbackText()}
                    onInput={(e) => setFeedbackText(e.currentTarget.value)}
                  />
                  <DialogFooter class="flex-row justify-end gap-2">
                    <Button variant="outline" onClick={() => setFeedbackStep(null)}>Cancel</Button>
                    <Button disabled={!feedbackText().trim()} onClick={() => void submitFeedback()}>Send</Button>
                  </DialogFooter>
                </Show>
                <Show when={!gate().allow_freeform && gate().choices.length === 0}>
                  <DialogFooter>
                    <Button variant="outline" onClick={() => setFeedbackStep(null)}>Cancel</Button>
                  </DialogFooter>
                </Show>
              </>
            )}
          </Show>
        </DialogContent>
      </Dialog>

      {/* Agent question dialog */}
      <Dialog open={!!agentQuestion()} onOpenChange={(open) => { if (!open) setAgentQuestion(null); }}>
        <DialogContent class="max-w-[450px]">
          <Show when={agentQuestion()}>
            {(q) => {
              const [wdpQMsSelected, setWdpQMsSelected] = createSignal<Set<number>>(new Set());
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
                          variant={q().multi_select && wdpQMsSelected().has(idx()) ? 'default' : 'outline'}
                          style="font-size:0.85em;"
                          disabled={questionSending()}
                          onClick={() => {
                            if (q().multi_select) {
                              setWdpQMsSelected((prev) => {
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
                        disabled={wdpQMsSelected().size === 0 || questionSending()}
                        onClick={() => {
                          const indices = [...wdpQMsSelected()].sort((a, b) => a - b);
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
      <Dialog open={!!pendingApproval()} onOpenChange={(open) => { if (!open) setPendingApproval(null); }}>
        <DialogContent class="max-w-[500px]">
          <Show when={pendingApproval()}>
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
                      <pre style="margin-top:4px;padding:8px;background:var(--bg-primary, #1e1e2e);border:1px solid var(--border, #45475a);border-radius:6px;overflow-x:auto;font-size:0.85em;max-height:200px;overflow-y:auto;white-space:pre-wrap;">{a().input}</pre>
                    </details>
                  </Show>
                </div>
                <DialogFooter class="flex-row flex-wrap justify-end gap-2">
                  <Button variant="outline" onClick={() => setPendingApproval(null)}>Cancel</Button>
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
    </div>
  );
}
