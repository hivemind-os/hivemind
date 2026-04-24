/**
 * Shared workflow instance detail panel — step chips, variables, output, error.
 * Reused in SessionWorkflows, WorkflowsPage, and FlightDeck WorkflowsPanel.
 */
import { For, Show } from 'solid-js';
import type { StepState, InterceptedActionPage, ShadowSummary } from '~/types';
import { highlightYaml } from '../YamlHighlight';
import { pendingApprovalToasts } from '../AgentApprovalToast';
import ShadowResultsPanel from '../workflow/ShadowResultsPanel';
import {
  Bell, Bot, Calendar, GitBranch, Hand, Lock, Radio, RotateCcw,
  Square, Timer, TriangleAlert, Wrench, Zap,
} from 'lucide-solid';

// ── Helpers ──────────────────────────────────────────────────────────────

export function getStepTask(step: any): any | undefined {
  return step.task ?? step.step_type?.Task ?? step.Task;
}

export const statusDotColors: Record<string, string> = {
  running: '#34d399',
  completed: '#60a5fa',
  failed: '#f87171',
  killed: '#f87171',
  paused: '#fbbf24',
  waiting_on_input: '#fbbf24',
  waiting_on_event: '#fbbf24',
  waiting_for_delay: '#fbbf24',
  loop_waiting: '#a78bfa',
  pending: '#94a3b8',
  ready: '#60a5fa',
  skipped: '#94a3b8',
};

export function StepIcon(props: { type: any; size?: number }) {
  const s = () => props.size ?? 14;
  const t = props.type;
  if (!t) return <Square size={s()} />;
  if (t === 'trigger' || t.Trigger) return <Bell size={s()} />;
  if (t === 'control_flow' || t.ControlFlow) return <GitBranch size={s()} />;
  if (t === 'task' || t.Task) {
    const task = typeof t === 'object' ? t.Task : null;
    if (!task) return <Wrench size={s()} />;
    if (task.CallTool || task.call_tool) return <Wrench size={s()} />;
    if (task.InvokeAgent || task.invoke_agent) return <Bot size={s()} />;
    if (task.InvokePrompt || task.invoke_prompt) return <Zap size={s()} />;
    if (task.FeedbackGate || task.feedback_gate) return <Hand size={s()} />;
    if (task.Delay || task.delay) return <Timer size={s()} />;
    if (task.SignalAgent || task.signal_agent) return <Radio size={s()} />;
    if (task.LaunchWorkflow || task.launch_workflow) return <RotateCcw size={s()} />;
    if (task.ScheduleTask || task.schedule_task) return <Calendar size={s()} />;
  }
  return <Square size={s()} />;
}

// ── Component ────────────────────────────────────────────────────────────

export interface WorkflowInstanceDetailProps {
  /** Full instance detail (definition, step_states, variables, output, error). */
  detail: {
    id: number;
    definition?: { steps?: any[] };
    step_states: Record<string, StepState>;
    variables?: Record<string, any>;
    output?: any;
    error?: string | null;
    execution_mode?: string;
  };
  /** Called when user clicks a waiting feedback gate step. */
  onOpenFeedbackGate?: (instanceId: number, step: any, state: StepState) => void;
  /** If true, show loading shimmer instead of content. */
  loading?: boolean;
  /** Fetch intercepted actions for shadow results panel. */
  fetchInterceptedActions?: (instanceId: number, limit?: number, offset?: number) => Promise<InterceptedActionPage | null>;
  /** Fetch shadow summary for shadow results panel. */
  fetchShadowSummary?: (instanceId: number) => Promise<ShadowSummary | null>;
}

export default function WorkflowInstanceDetail(props: WorkflowInstanceDetailProps) {
  const steps = () => props.detail.definition?.steps ?? [];
  const stepStates = () => props.detail.step_states ?? {};

  return (
    <div class="wf-instance-detail">
      <Show when={props.loading}>
        <p class="text-muted-foreground text-xs" style="padding:8px 0;">Loading…</p>
      </Show>
      <Show when={!props.loading}>
        {/* Step chips */}
        <Show when={steps().length > 0}>
          <div class="wf-detail-section-title">Steps</div>
          <div class="wf-step-chips">
            <For each={steps()}>
              {(step: any) => {
                const state = () => stepStates()[step.id] as StepState | undefined;
                const stepStatus = () => state()?.status ?? 'pending';
                const isWaiting = () => stepStatus() === 'waiting_on_input';
                const isFeedbackGate = () => {
                  const task = getStepTask(step);
                  return !!(task?.kind === 'feedback_gate' || task?.FeedbackGate || task?.feedback_gate);
                };
                const childAgentId = () => state()?.child_agent_id;
                const childApprovals = () => {
                  const aid = childAgentId();
                  return aid ? pendingApprovalToasts().filter(a => a.agent_id === aid).length : 0;
                };
                const clickable = () => (isWaiting() && isFeedbackGate()) || childApprovals() > 0;

                return (
                  <div
                    class={`wf-step-chip${clickable() ? ' clickable' : ''}`}
                    title={`${step.id}: ${stepStatus()}${state()?.error ? '\nError: ' + state()!.error : ''}${isWaiting() ? '\nClick to respond' : ''}`}
                    onClick={() => {
                      if (isWaiting() && isFeedbackGate() && state() && props.onOpenFeedbackGate) {
                        props.onOpenFeedbackGate(props.detail.id, step, state()!);
                      }
                    }}
                  >
                    <span class="step-dot" style={`background:${statusDotColors[stepStatus()] ?? '#94a3b8'}`} />
                    <StepIcon type={step.step_type ?? step.type} size={12} />
                    {step.id}
                    <Show when={isWaiting()}>
                      <Bell size={10} style="color:#fbbf24;" />
                    </Show>
                    <Show when={stepStatus() === 'waiting_on_event'}>
                      <Bell size={10} style="color:#94a3b8;" />
                    </Show>
                    <Show when={childApprovals() > 0}>
                      <Lock size={10} style="color:hsl(24 93% 75%);" />
                    </Show>
                  </div>
                );
              }}
            </For>
          </div>
        </Show>

        {/* Variables */}
        <Show when={props.detail.variables && Object.keys(props.detail.variables!).length > 0}>
          <div class="mb-2 mt-2">
            <div class="wf-detail-section-title">Variables</div>
            <pre class="wf-detail-yaml" innerHTML={highlightYaml(props.detail.variables)} />
          </div>
        </Show>

        {/* Output */}
        <Show when={props.detail.output}>
          <div class="mb-2">
            <div class="wf-detail-section-title">Output</div>
            <pre class="wf-detail-yaml" innerHTML={highlightYaml(props.detail.output)} />
          </div>
        </Show>

        {/* Error */}
        <Show when={props.detail.error}>
          <div class="wf-error-banner">
            <TriangleAlert size={14} />
            <pre>{props.detail.error}</pre>
          </div>
        </Show>

        {/* Shadow Results */}
        <Show when={props.detail.execution_mode === 'shadow' && props.fetchInterceptedActions && props.fetchShadowSummary}>
          <ShadowResultsPanel
            instanceId={props.detail.id}
            executionMode={props.detail.execution_mode}
            fetchActions={props.fetchInterceptedActions!}
            fetchSummary={props.fetchShadowSummary!}
          />
        </Show>
      </Show>
    </div>
  );
}
