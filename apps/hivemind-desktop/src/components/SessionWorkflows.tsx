import { createSignal, createMemo, createEffect, onCleanup, Show, For } from 'solid-js';
import type { Accessor, Setter } from 'solid-js';
import { invoke } from '@tauri-apps/api/core';
import type { WorkflowInstanceSummary, WorkflowInstance, StepState } from '~/types';
import type { PendingQuestion } from './InlineQuestion';
import { pendingApprovalToasts, dismissAgentApproval, type PendingApproval } from './AgentApprovalToast';
import WorkflowsPanel from './flight-deck/WorkflowsPanel';
import { getStepTask } from './shared/WorkflowInstanceDetail';
import { highlightYaml } from './YamlHighlight';
import { renderMarkdown } from '../utils';
import { Dialog, DialogContent } from '~/ui/dialog';
import { Button } from '~/ui/button';
import {
  CheckCircle,
  Hand,
  HelpCircle,
  Lock,
  ShieldAlert,
  XCircle,
} from 'lucide-solid';
import {
  answerQuestion as routeAnswerQuestion,
  respondToApproval as routeRespondToApproval,
  type PendingInteraction,
} from '~/lib/interactionRouting';
import DOMPurify from 'dompurify';

interface ChatWorkflowTracker {
  instanceId: number;
  instance: any | null;
  events: any[];
}

export interface SessionWorkflowsProps {
  sessionId: Accessor<string | null>;
  chatWorkflows: Accessor<ChatWorkflowTracker[]>;
  activeChatWorkflows: Accessor<ChatWorkflowTracker[]>;
  terminalChatWorkflows: Accessor<ChatWorkflowTracker[]>;
  onPause: (instanceId: number) => void;
  onResume: (instanceId: number) => void;
  onKill: (instanceId: number) => void;
  onRespondWorkflowGate?: (instanceId: number, stepId: string, response: any) => Promise<void>;
  pendingQuestions?: Accessor<PendingQuestion[]>;
  onQuestionAnswered?: (requestId: string, answerText: string) => void;
}

export default function SessionWorkflows(props: SessionWorkflowsProps) {
  // ── Selection & detail state ──────────────────────────────────────────
  const [selectedWfId, setSelectedWfId] = createSignal<number | null>(null);
  const [workflowDetail, setWorkflowDetail] = createSignal<WorkflowInstance | null>(null);
  const [workflowDetailLoading, setWorkflowDetailLoading] = createSignal(false);

  // 1-second tick for live runtime counters
  const [runtimeTick, setRuntimeTick] = createSignal(0);
  const tickInterval = setInterval(() => setRuntimeTick((t) => t + 1), 1000);
  onCleanup(() => clearInterval(tickInterval));

  // ── Feedback dialog state ─────────────────────────────────────────────
  const [feedbackStep, setFeedbackStep] = createSignal<{
    instance_id: number;
    step_id: string;
    prompt: string;
    choices: string[];
    allow_freeform: boolean;
  } | null>(null);
  const [feedbackText, setFeedbackText] = createSignal('');

  // ── Question dialog state ─────────────────────────────────────────────
  const [questionDialogQuestion, setQuestionDialogQuestion] = createSignal<PendingQuestion | null>(null);
  const [questionFreeText, setQuestionFreeText] = createSignal('');
  const [questionSending, setQuestionSending] = createSignal(false);
  const [questionMsSelected, setQuestionMsSelected] = createSignal<Set<number>>(new Set());

  // ── Approval dialog state ─────────────────────────────────────────────
  const [approvalDialogItem, setApprovalDialogItem] = createSignal<PendingApproval | null>(null);
  const [approvalSending, setApprovalSending] = createSignal(false);

  // ── Reset state when session changes ──────────────────────────────────
  createEffect(() => {
    props.sessionId(); // track
    setSelectedWfId(null);
    setWorkflowDetail(null);
    setFeedbackStep(null);
    setQuestionDialogQuestion(null);
    setApprovalDialogItem(null);
  });

  // ── Load workflow detail when selection changes ───────────────────────
  let detailSeq = 0;
  createEffect(() => {
    const id = selectedWfId();
    if (!id) {
      setWorkflowDetail(null);
      setWorkflowDetailLoading(false);
      return;
    }
    const mySeq = ++detailSeq;
    setWorkflowDetailLoading(true);
    setWorkflowDetail(null);
    invoke<WorkflowInstance>('workflow_get_instance', { instance_id: id })
      .then((detail) => {
        if (mySeq === detailSeq) setWorkflowDetail(detail ?? null);
      })
      .catch((e) => console.error('Failed to load workflow detail:', e))
      .finally(() => {
        if (mySeq === detailSeq) setWorkflowDetailLoading(false);
      });
  });

  // ── Data adaptation ───────────────────────────────────────────────────
  const activeWorkflows = createMemo<WorkflowInstanceSummary[]>(() => {
    const all = [...props.activeChatWorkflows(), ...props.terminalChatWorkflows()];
    return all
      .filter((t) => t.instance != null)
      .map((t) => t.instance as WorkflowInstanceSummary);
  });

  // Session-scoped pending questions & approvals
  const polledQuestions = createMemo(() => props.pendingQuestions?.() ?? []);
  const polledApprovals = createMemo(() => {
    const sid = props.sessionId();
    if (!sid) return [];
    return pendingApprovalToasts().filter((a) => a.session_id === sid);
  });

  // ── Feedback gate handler ─────────────────────────────────────────────
  function openFeedbackGate(instanceId: number, step: any, state: StepState) {
    if (state?.status !== 'waiting_on_input') return;
    const prompt =
      state.interaction_prompt ??
      (() => {
        const task = getStepTask(step);
        const gate = task?.FeedbackGate ?? task?.feedback_gate ?? task;
        return gate?.prompt;
      })() ??
      'Please provide your response:';
    const choices =
      state.interaction_choices ??
      (() => {
        const task = getStepTask(step);
        const gate = task?.FeedbackGate ?? task?.feedback_gate ?? task;
        return gate?.choices;
      })() ??
      [];
    const allow_freeform =
      state.interaction_allow_freeform ??
      (() => {
        const task = getStepTask(step);
        const gate = task?.FeedbackGate ?? task?.feedback_gate ?? task;
        return gate?.allow_freeform;
      })() ??
      true;
    setFeedbackStep({ instance_id: instanceId, step_id: step.id, prompt, choices, allow_freeform });
    setFeedbackText('');
  }

  async function submitFeedback(choice?: string) {
    const gate = feedbackStep();
    if (!gate || !props.onRespondWorkflowGate) return;
    const response = choice
      ? { selected: choice, text: feedbackText() }
      : { selected: feedbackText(), text: feedbackText() };
    try {
      await props.onRespondWorkflowGate(gate.instance_id, gate.step_id, response);
      setFeedbackStep(null);
      setFeedbackText('');
      // Refresh detail if still selected
      if (selectedWfId() === gate.instance_id) {
        const mySeq = ++detailSeq;
        invoke<WorkflowInstance>('workflow_get_instance', { instance_id: gate.instance_id })
          .then((detail) => {
            if (mySeq === detailSeq) setWorkflowDetail(detail ?? null);
          })
          .catch(() => {});
      }
    } catch (err) {
      console.error('Failed to submit feedback:', err);
    }
  }

  // ── Question answering ────────────────────────────────────────────────
  async function answerQuestion(
    question: PendingQuestion,
    choiceIdx?: number,
    text?: string,
    selectedChoices?: number[],
  ) {
    setQuestionSending(true);
    let label: string;
    if (selectedChoices && selectedChoices.length > 0) {
      label = selectedChoices.map((i) => question.choices[i]).join(', ');
    } else {
      label = text || (choiceIdx !== undefined ? question.choices[choiceIdx] : '');
    }
    try {
      await routeAnswerQuestion(
        {
          request_id: question.request_id,
          entity_id: question.agent_id
            ? `agent/${question.agent_id}`
            : `session/${props.sessionId() ?? ''}`,
          source_name: question.agent_name ?? question.agent_id ?? '',
          type: 'question',
          session_id: props.sessionId() ?? undefined,
          agent_id: question.agent_id,
        } as PendingInteraction,
        {
          ...(choiceIdx !== undefined ? { selected_choice: choiceIdx } : {}),
          ...(selectedChoices !== undefined ? { selected_choices: selectedChoices } : {}),
          ...(text ? { text } : {}),
        },
      );
      props.onQuestionAnswered?.(question.request_id, label);
      setQuestionDialogQuestion(null);
    } catch (err) {
      console.error('Failed to respond:', err);
      setQuestionSending(false);
    }
  }

  // ── Approval handling ─────────────────────────────────────────────────
  async function handleApproval(
    approved: boolean,
    opts?: { allow_agent?: boolean; allow_session?: boolean },
  ) {
    const item = approvalDialogItem();
    if (!item || approvalSending()) return;
    setApprovalSending(true);
    dismissAgentApproval(item.request_id);
    try {
      await routeRespondToApproval(
        {
          request_id: item.request_id,
          entity_id: `agent/${item.agent_id}`,
          source_name: item.agent_name || item.agent_id,
          type: 'tool_approval',
          session_id: item.session_id,
          agent_id: item.agent_id,
        } as PendingInteraction,
        { approved, allow_agent: opts?.allow_agent, allow_session: opts?.allow_session },
      );
      setApprovalDialogItem(null);
    } catch {
      setApprovalSending(false);
    }
  }

  // ── Render ────────────────────────────────────────────────────────────
  return (
    <div class="flex flex-1 flex-col overflow-hidden">
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
        requestPauseWorkflow={props.onPause}
        requestResumeWorkflow={props.onResume}
        requestKillWorkflow={props.onKill}
        openFeedbackGate={openFeedbackGate}
      />

      {/* ── Feedback dialog ── */}
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
                  <button class="flight-deck-action-btn" onClick={() => setFeedbackStep(null)}>✕</button>
                </div>
                <div class="fd-config-body">
                  <div class="fd-question-text prose prose-sm max-w-none text-foreground markdown-body" innerHTML={renderMarkdown(gate().prompt)} />
                  <Show when={gate().choices.length > 0}>
                    <div class="fd-question-choices">
                      <For each={gate().choices}>
                        {(choice) => (
                          <button class="fd-question-choice" onClick={() => void submitFeedback(choice)}>{choice}</button>
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
                        <Button variant="outline" onClick={() => setFeedbackStep(null)}>Cancel</Button>
                        <Button disabled={!feedbackText().trim()} onClick={() => void submitFeedback()}>Submit</Button>
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

      {/* ── Question dialog ── */}
      <Dialog open={!!questionDialogQuestion()} onOpenChange={(open) => { if (!open) setQuestionDialogQuestion(null); }}>
        <DialogContent class="max-w-lg p-0">
          <Show when={questionDialogQuestion()}>
            {(q) => (
              <div class="flight-deck-confirm-dialog fd-question-dialog">
                <div class="fd-dialog-header">
                  <span class="flight-deck-item-avatar"><HelpCircle size={16} /></span>
                  <div style="flex:1;">
                    <div class="fd-dialog-title">Question from agent</div>
                    <Show when={q().agent_name}>
                      <div class="fd-dialog-subtitle">{q().agent_name}</div>
                    </Show>
                  </div>
                  <button class="flight-deck-action-btn" onClick={() => setQuestionDialogQuestion(null)}>✕</button>
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
                                void answerQuestion(q(), idx(), choice);
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
                          const indices = [...questionMsSelected()].sort((a, b) => a - b);
                          void answerQuestion(q(), undefined, undefined, indices);
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
                          void answerQuestion(q(), undefined, questionFreeText().trim());
                        }
                      }}
                      disabled={questionSending()}
                    />
                    <button
                      disabled={!questionFreeText().trim() || questionSending()}
                      onClick={() => void answerQuestion(q(), undefined, questionFreeText().trim())}
                    >
                      {questionSending() ? '…' : '→'}
                    </button>
                  </div>
                </div>
              </div>
            )}
          </Show>
        </DialogContent>
      </Dialog>

      {/* ── Approval dialog ── */}
      <Dialog open={!!approvalDialogItem()} onOpenChange={(open) => { if (!open) setApprovalDialogItem(null); }}>
        <DialogContent class="max-w-lg p-0">
          <Show when={approvalDialogItem()}>
            {(item) => (
              <div class="flight-deck-confirm-dialog fd-approval-dialog">
                <div class="fd-dialog-header">
                  <span class="flight-deck-item-avatar"><ShieldAlert size={16} /></span>
                  <div style="flex:1;">
                    <div class="fd-dialog-title">Tool Approval Required</div>
                    <div class="fd-dialog-subtitle">{item().agent_name || item().agent_id}</div>
                  </div>
                  <button class="flight-deck-action-btn" onClick={() => setApprovalDialogItem(null)}>✕</button>
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
                  <Button variant="outline" disabled={approvalSending()} onClick={() => void handleApproval(false)}>
                    <XCircle size={14} /> Deny
                  </Button>
                  <Button disabled={approvalSending()} onClick={() => void handleApproval(true)}>
                    {approvalSending() ? 'Sending…' : <><CheckCircle size={14} /> Approve</>}
                  </Button>
                  <Button variant="outline" disabled={approvalSending()} onClick={() => void handleApproval(true, { allow_agent: true })}>
                    Allow for Agent
                  </Button>
                  <Button variant="outline" disabled={approvalSending()} onClick={() => void handleApproval(true, { allow_session: true })}>
                    Allow for Session
                  </Button>
                </div>
              </div>
            )}
          </Show>
        </DialogContent>
      </Dialog>
    </div>
  );
}
