import { For, Show, createSignal, type Accessor } from 'solid-js';
import { invoke } from '@tauri-apps/api/core';
import { Pause, Play, CircleStop, ChevronRight, GitBranch } from 'lucide-solid';
import WorkflowInstanceDetail, { getStepTask } from './shared/WorkflowInstanceDetail';
import type { StepState } from '~/types';

interface ChatWorkflowTracker {
  instanceId: number;
  instance: any | null;
  events: any[];
}

export interface SessionWorkflowsProps {
  chatWorkflows: Accessor<ChatWorkflowTracker[]>;
  activeChatWorkflows: Accessor<ChatWorkflowTracker[]>;
  terminalChatWorkflows: Accessor<ChatWorkflowTracker[]>;
  onPause: (instanceId: number) => void;
  onResume: (instanceId: number) => void;
  onKill: (instanceId: number) => void;
  onRespondWorkflowGate?: (instanceId: number, stepId: string, response: any) => Promise<void>;
}

function statusPill(status: string): string {
  switch (status) {
    case 'completed': return 'pill success';
    case 'running': return 'pill info';
    case 'paused': case 'waiting_on_input': case 'waiting_on_event': return 'pill warning';
    case 'failed': case 'killed': return 'pill danger';
    default: return 'pill neutral';
  }
}

function statusLabel(s: string): string { return s.replace(/_/g, ' '); }

function durationStr(startMs: number, endMs: number | null | undefined): string {
  if (!startMs) return '—';
  const secs = Math.floor(((endMs || Date.now()) - startMs) / 1000);
  if (secs < 60) return `${secs}s`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m ${secs % 60}s`;
  return `${Math.floor(secs / 3600)}h ${Math.floor((secs % 3600) / 60)}m`;
}

export default function SessionWorkflows(props: SessionWorkflowsProps) {
  const [expandedId, setExpandedId] = createSignal<number | null>(null);
  const [expandedDetail, setExpandedDetail] = createSignal<any | null>(null);
  const [detailLoading, setDetailLoading] = createSignal(false);

  async function toggleExpand(instanceId: number) {
    if (expandedId() === instanceId) {
      setExpandedId(null);
      setExpandedDetail(null);
      return;
    }
    setExpandedId(instanceId);
    setExpandedDetail(null);
    setDetailLoading(true);
    try {
      const detail = await invoke<any>('workflow_get_instance', { instance_id: instanceId });
      if (expandedId() === instanceId) setExpandedDetail(detail);
    } catch (e) {
      console.error('Failed to load workflow detail:', e);
    } finally {
      setDetailLoading(false);
    }
  }

  function openFeedbackGate(instanceId: number, step: any, state: StepState) {
    if (state?.status !== 'waiting_on_input') return;
    const prompt = state.interaction_prompt
      ?? (() => { const task = getStepTask(step); const gate = task?.FeedbackGate ?? task?.feedback_gate ?? task; return gate?.prompt; })()
      ?? 'Please provide your response:';
    const choices: string[] = (() => {
      if (state.interaction_choices) {
        try { return JSON.parse(state.interaction_choices as any); } catch { /* fall through */ }
      }
      const task = getStepTask(step);
      const gate = task?.FeedbackGate ?? task?.feedback_gate ?? task;
      return gate?.choices ?? [];
    })();
    const allow_freeform = state.interaction_allow_freeform
      ?? (() => { const task = getStepTask(step); const gate = task?.FeedbackGate ?? task?.feedback_gate ?? task; return gate?.allow_freeform; })()
      ?? true;
    if (props.onRespondWorkflowGate) {
      // Use a simple prompt approach — delegate to parent
      const text = window.prompt(prompt + (choices.length ? '\n\nChoices: ' + choices.join(', ') : ''));
      if (text != null) {
        void props.onRespondWorkflowGate(instanceId, step.id, { selected: text, text });
      }
    }
  }

  const allWorkflows = () => {
    const active = props.activeChatWorkflows();
    const terminal = props.terminalChatWorkflows();
    return [...active, ...terminal];
  };

  return (
    <div class="flex flex-1 flex-col" style="padding:12px 16px;overflow-y:auto;">
      <Show when={allWorkflows().length === 0}>
        <div class="flex flex-col items-center justify-center flex-1 gap-3 text-muted-foreground" style="padding:40px 20px;">
          <GitBranch size={32} style="opacity:0.3;" />
          <p style="margin:0;font-size:0.9em;">No workflows for this session</p>
          <p style="margin:0;font-size:0.78em;opacity:0.7;">Launch a workflow from this session to see it here.</p>
        </div>
      </Show>

      <Show when={allWorkflows().length > 0}>
        <div class="wf-timeline" style="padding-top:4px;">
          <For each={allWorkflows()}>
            {(tracker) => {
              const inst = () => tracker.instance;
              const isExpanded = () => expandedId() === tracker.instanceId;
              const isActive = () => {
                const s = inst()?.status;
                return s && !['completed', 'failed', 'killed'].includes(s);
              };
              const progressPct = () => {
                const i = inst();
                if (!i) return 0;
                const total = i.step_count ?? 0;
                const done = i.steps_completed ?? 0;
                return total > 0 ? Math.round((done / total) * 100) : 0;
              };
              const progressClass = () => {
                const s = inst()?.status;
                if (s === 'completed') return 'completed';
                if (s === 'failed' || s === 'killed') return 'failed';
                if (s === 'paused' || s === 'waiting_on_input' || s === 'waiting_on_event') return 'waiting';
                return 'running';
              };

              return (
                <div class="wf-timeline-item">
                  <div class={`wf-timeline-dot ${inst()?.status ?? 'pending'}`} />
                  <div class={`wf-timeline-card${isExpanded() ? ' expanded' : ''}`}>
                    <div
                      class="wf-timeline-summary"
                      onClick={() => void toggleExpand(tracker.instanceId)}
                    >
                      <div class="wf-timeline-meta">
                        <div class="wf-timeline-meta-row">
                          <span class="wf-timeline-name">{inst()?.definition_name ?? tracker.instanceId}</span>
                          <Show when={inst()?.status}>
                            <span class={statusPill(inst()!.status)} style="font-size:0.68em;flex-shrink:0;">
                              {statusLabel(inst()!.status)}
                            </span>
                          </Show>
                        </div>
                        <span class="wf-timeline-sub">
                          {tracker.instanceId}
                          <Show when={inst()?.created_at_ms}>
                            {' • '}{durationStr(inst()!.created_at_ms, inst()?.completed_at_ms)}
                          </Show>
                        </span>
                      </div>

                      <Show when={inst() && (inst()!.step_count ?? 0) > 0}>
                        <div class="wf-progress-wrap">
                          <div class="wf-progress-bar">
                            <div class={`wf-progress-fill ${progressClass()}`} style={`width:${progressPct()}%`} />
                          </div>
                          <span class="wf-progress-label">{inst()!.steps_completed ?? 0}/{inst()!.step_count}</span>
                        </div>
                      </Show>

                      <div class="wf-timeline-actions" onClick={(e: Event) => e.stopPropagation()}>
                        <Show when={isActive() && inst()?.status !== 'paused'}>
                          <button class="icon-btn" title="Pause" onClick={() => props.onPause(tracker.instanceId)}><Pause size={14} /></button>
                        </Show>
                        <Show when={inst()?.status === 'paused'}>
                          <button class="icon-btn" title="Resume" onClick={() => props.onResume(tracker.instanceId)}><Play size={14} /></button>
                        </Show>
                        <Show when={isActive()}>
                          <button class="icon-btn" title="Kill" style="color:hsl(var(--destructive));" onClick={() => props.onKill(tracker.instanceId)}><CircleStop size={14} /></button>
                        </Show>
                      </div>

                      <ChevronRight size={14} class={`wf-timeline-chevron${isExpanded() ? ' open' : ''}`} />
                    </div>

                    <Show when={isExpanded()}>
                      <div style="padding:10px 14px;border-top:1px solid hsl(var(--border));">
                        <Show when={expandedDetail()} fallback={
                          <Show when={detailLoading()}>
                            <p class="text-muted-foreground text-xs">Loading…</p>
                          </Show>
                        }>
                          {(detail) => (
                            <WorkflowInstanceDetail
                              detail={detail()}
                              onOpenFeedbackGate={openFeedbackGate}
                            />
                          )}
                        </Show>
                        <Show when={inst()!.resolved_result_message}>
                          <div style="margin-top:8px;padding:8px;border-radius:6px;background:hsl(var(--accent));font-size:0.9em;white-space:pre-wrap;">
                            {inst()!.resolved_result_message}
                          </div>
                        </Show>
                      </div>
                    </Show>
                  </div>
                </div>
              );
            }}
          </For>
        </div>
      </Show>
    </div>
  );
}
