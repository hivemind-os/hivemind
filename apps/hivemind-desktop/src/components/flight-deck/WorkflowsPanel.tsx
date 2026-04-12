import { createMemo, Show, For } from 'solid-js';
import type { Accessor, Setter, JSX } from 'solid-js';
import type {
  WorkflowInstanceSummary,
  WorkflowInstance,
  StepState,
} from '~/types';
import type { PendingQuestion } from '../InlineQuestion';
import { pendingApprovalToasts, type PendingApproval } from '../AgentApprovalToast';
import { highlightYaml } from '../YamlHighlight';
import { DataTable } from './data-table';
import { createWorkflowColumns, type WorkflowRow, type WorkflowColumnCallbacks } from './workflows-columns';
import {
  ArrowLeftRight,
  Bell,
  Bot,
  Calendar,
  Clock,
  Hand,
  HelpCircle,
  Lock,
  Pause,
  Play,
  Radio,
  RefreshCw,
  Square,
  Timer,
  Wrench,
  X,
  XCircle,
} from 'lucide-solid';

// ---------------------------------------------------------------------------
// Helpers (pure functions copied from FlightDeck)
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

// ---------------------------------------------------------------------------
// Props
// ---------------------------------------------------------------------------

export interface WorkflowsPanelProps {
  activeWorkflows: Accessor<WorkflowInstanceSummary[]>;
  selectedWfId: Accessor<number | null>;
  setSelectedWfId: Setter<number | null>;
  workflowDetail: Accessor<WorkflowInstance | null>;
  workflowDetailLoading: Accessor<boolean>;
  runtimeTick: Accessor<number>;
  polledQuestions: Accessor<PendingQuestion[]>;
  polledApprovals: Accessor<PendingApproval[]>;
  pendingQuestions: Accessor<PendingQuestion[]> | undefined;
  setApprovalDialogItem: Setter<PendingApproval | null>;
  setApprovalSending: Setter<boolean>;
  setQuestionDialogQuestion: Setter<PendingQuestion | null>;
  setQuestionFreeText: Setter<string>;
  setQuestionSending: Setter<boolean>;
  requestPauseWorkflow: (instanceId: number) => void;
  requestResumeWorkflow: (instanceId: number) => void;
  requestKillWorkflow: (instanceId: number) => void;
  openFeedbackGate: (instanceId: number, step: any, state: StepState) => void;
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

export default function WorkflowsPanel(props: WorkflowsPanelProps) {
  const getWfApprovals = (wfId: number) => {
    const wf = props.activeWorkflows().find((w) => w.id === wfId);
    const ids = wf?.child_agent_ids ?? [];
    const fromSse = pendingApprovalToasts().filter(
      (a) => ids.includes(a.agent_id),
    );
    if (fromSse.length > 0) return fromSse;
    const polled = props.polledApprovals();
    return Array.isArray(polled) ? polled.filter((a) => ids.includes(a.agent_id)) : [];
  };
  const getWfQuestions = (wfId: number) => {
    const wf = props.activeWorkflows().find((w) => w.id === wfId);
    const ids = wf?.child_agent_ids ?? [];
    const fromProps = (props.pendingQuestions?.() ?? []).filter(
      (q) => q.agent_id && ids.includes(q.agent_id),
    );
    if (fromProps.length > 0) return fromProps;
    const polled = props.polledQuestions();
    return Array.isArray(polled) ? polled.filter((q) => q.agent_id && ids.includes(q.agent_id)) : [];
  };
  const wfColumnCallbacks: WorkflowColumnCallbacks = {
    tick: props.runtimeTick,
    onApprovalClick: (wfId) => {
      const approvals = getWfApprovals(wfId);
      if (approvals.length > 0) {
        props.setApprovalDialogItem(approvals[0]);
        props.setApprovalSending(false);
      }
    },
    onQuestionClick: (wfId) => {
      const questions = getWfQuestions(wfId);
      if (questions.length > 0) {
        props.setQuestionDialogQuestion(questions[0]);
        props.setQuestionFreeText('');
        props.setQuestionSending(false);
      }
    },
  };
  const wfColumns = createWorkflowColumns(wfColumnCallbacks);
  const workflowRows = createMemo<WorkflowRow[]>(() =>
    props.activeWorkflows().map((wf) => {
      const childIds = wf.child_agent_ids ?? [];
      const aFromSse = pendingApprovalToasts().filter(
        (a) => childIds.includes(a.agent_id),
      );
      const polledApprovals = props.polledApprovals();
      const approvalCount = aFromSse.length > 0
        ? aFromSse.length
        : (Array.isArray(polledApprovals) ? polledApprovals : []).filter((a) => childIds.includes(a.agent_id)).length;
      const qFromProps = (props.pendingQuestions?.() ?? []).filter(
        (q) => q.agent_id && childIds.includes(q.agent_id),
      );
      const polledQ = props.polledQuestions();
      const questionCount = qFromProps.length > 0
        ? qFromProps.length
        : (Array.isArray(polledQ) ? polledQ : []).filter((q) => q.agent_id && childIds.includes(q.agent_id)).length;
      return { workflow: wf, questionCount, approvalCount };
    }),
  );
  const selectedWf = () => {
    const id = props.selectedWfId();
    return id ? props.activeWorkflows().find((w) => w.id === id) ?? null : null;
  };

  return (
    <DataTable
      columns={wfColumns}
      data={workflowRows()}
      getRowId={(row: WorkflowRow) => String(row.workflow.id)}
      selectedRowId={props.selectedWfId() != null ? String(props.selectedWfId()) : null}
      onRowClick={(row: WorkflowRow) =>
        props.setSelectedWfId(
          props.selectedWfId() === row.workflow.id ? null : row.workflow.id,
        )
      }
      emptyMessage="No active workflows"
      detailPanel={() =>
        <Show when={selectedWf()}>
          {(wf) => {
            const isWaiting = () =>
              wf().status === 'waiting_on_input' || wf().status === 'waiting_on_event';
            const detail = () => props.workflowDetail();
            const childIds = () => wf().child_agent_ids ?? [];
            const wfApprovals = () => getWfApprovals(wf().id);
            const wfQuestions = () => getWfQuestions(wf().id);

            return (
              <div class="fd-detail-panel">
                <div class="fd-detail-panel-header">
                  <span class="fd-detail-panel-title">
                    {wf().definition_name}{' '}
                    <span class="text-muted-foreground">v{wf().definition_version}</span>
                  </span>
                  <span class={`flight-deck-status ${wf().status}`}>
                    {wf().status.replace(/_/g, ' ')}
                  </span>
                  <button
                    class="flight-deck-action-btn"
                    style="margin-left:auto;"
                    onClick={() => props.setSelectedWfId(null)}
                  >
                    <X size={14} />
                  </button>
                </div>
                <div class="fd-detail-panel-body">
                  <div class="fd-telemetry-row">
                    <span><Clock size={14} /> {wfDurationStr(wf().created_at_ms, wf().completed_at_ms)}</span>
                    <span>{new Date(wf().created_at_ms).toLocaleTimeString()}</span>
                  </div>
                  <div>
                    <strong>Session:</strong>{' '}
                    <code>{wf().parent_session_id.slice(0, 8)}…</code>
                  </div>
                  <Show when={wf().parent_agent_id}>
                    <div>
                      <strong>Agent:</strong>{' '}
                      <code>{wf().parent_agent_id!.slice(0, 8)}…</code>
                    </div>
                  </Show>
                  <Show when={wf().trigger_step_id}>
                    <div>
                      <strong>Trigger:</strong>{' '}
                      <code>{wf().trigger_step_id}</code>
                    </div>
                  </Show>
                  <Show when={wf().error}>
                    <div class="text-destructive">
                      <strong>Error:</strong> {wf().error}
                    </div>
                  </Show>
                  {/* Interaction badges */}
                  <Show when={wfApprovals().length > 0}>
                    <div>
                      <span
                        class="fd-badge fd-badge-approval"
                        style="cursor:pointer;"
                        onClick={(e) => {
                          e.stopPropagation();
                          props.setApprovalDialogItem(wfApprovals()[0]);
                          props.setApprovalSending(false);
                        }}
                      >
                        <Lock size={14} /> {wfApprovals().length} pending approval(s)
                      </span>
                    </div>
                  </Show>
                  <Show when={wfQuestions().length > 0}>
                    <div>
                      <span
                        class="fd-badge fd-badge-question"
                        style="cursor:pointer;"
                        onClick={(e) => {
                          e.stopPropagation();
                          props.setQuestionDialogQuestion(wfQuestions()[0]);
                          props.setQuestionFreeText('');
                          props.setQuestionSending(false);
                        }}
                      >
                        <HelpCircle size={14} /> {wfQuestions().length} pending question(s)
                      </span>
                    </div>
                  </Show>
                  {/* Action buttons */}
                  <div class="flight-deck-actions">
                    <Show when={wf().status === 'running' || isWaiting()}>
                      <button
                        class="flight-deck-action-btn"
                        title="Pause"
                        onClick={() => props.requestPauseWorkflow(wf().id)}
                      >
                        <Pause size={14} />
                      </button>
                    </Show>
                    <Show when={wf().status === 'paused'}>
                      <button
                        class="flight-deck-action-btn"
                        title="Resume"
                        onClick={() => props.requestResumeWorkflow(wf().id)}
                      >
                        <Play size={14} />
                      </button>
                    </Show>
                    <button
                      class="flight-deck-action-btn"
                      title="Kill"
                      onClick={() => props.requestKillWorkflow(wf().id)}
                    >
                      <Square size={14} />
                    </button>
                  </div>
                  {/* Expanded step detail */}
                  <Show
                    when={!props.workflowDetailLoading()}
                    fallback={<p class="flight-deck-empty">Loading…</p>}
                  >
                    <Show when={detail()}>
                      {(d) => {
                        const steps = () => d().definition?.steps ?? [];
                        const stepStates = () => d().step_states ?? {};

                        return (
                          <>
                            <div class="fd-wf-section">
                              <div class="fd-config-label">Steps</div>
                              <div class="fd-wf-steps">
                                <For each={steps()}>
                                  {(step: any) => {
                                    const state = () => stepStates()[step.id] as StepState | undefined;
                                    const status = () => state()?.status ?? 'pending';
                                    const isStepWaiting = () => status() === 'waiting_on_input';
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
                                        class={`fd-wf-step ${wfStepStatusClass(status())} ${isStepWaiting() ? 'fd-step-clickable' : ''} ${hasChildInteraction() ? 'fd-step-clickable' : ''}`}
                                        title={`${step.id}: ${status()}${state()?.error ? '\nError: ' + state()!.error : ''}${isStepWaiting() ? '\nClick to respond' : ''}${hasChildInteraction() ? '\nChild agent needs approval' : ''}`}
                                        onClick={() => {
                                          if (isStepWaiting() && isFeedbackGate() && state()) {
                                            props.openFeedbackGate(d().id, step, state()!);
                                          }
                                        }}
                                      >
                                        <span class="fd-wf-step-icon">{stepIcon(step.step_type ?? step.type)}</span>
                                        <span class="fd-wf-step-id">{step.id}</span>
                                        <span class={`fd-wf-step-status ${wfStepStatusClass(status())}`}>
                                          {status().replace(/_/g, ' ')}
                                        </span>
                                        <Show when={isStepWaiting()}>
                                          <span class="fd-wf-step-hint">click to respond</span>
                                        </Show>
                                        <Show when={status() === 'waiting_on_event'}>
                                          <span class="fd-wf-step-hint">
                                            <Bell size={14} /> {(() => { const task = getStepTask(step); return task?.topic ?? task?.EventGate?.topic ?? task?.event_gate?.topic ?? 'event'; })()}
                                          </span>
                                        </Show>
                                        <Show when={hasChildInteraction()}>
                                          <span class="fd-wf-step-hint text-orange-400">
                                            <Lock size={14} /> {childApprovals()} pending
                                          </span>
                                        </Show>
                                      </div>
                                    );
                                  }}
                                </For>
                              </div>
                            </div>

                            <Show when={d().variables && Object.keys(d().variables).length > 0}>
                              <div class="fd-wf-section">
                                <div class="fd-config-label">Variables</div>
                                <pre class="fd-wf-json" innerHTML={highlightYaml(d().variables)} />
                              </div>
                            </Show>

                            <Show when={d().output}>
                              <div class="fd-wf-section">
                                <div class="fd-config-label">Output</div>
                                <pre class="fd-wf-json" innerHTML={highlightYaml(d().output)} />
                              </div>
                            </Show>

                            <Show when={d().error}>
                              <div class="fd-wf-error">
                                <XCircle size={14} /> {d().error}
                              </div>
                            </Show>
                          </>
                        );
                      }}
                    </Show>
                  </Show>
                </div>
              </div>
            );
          }}
        </Show>
      }
    />
  );
}
