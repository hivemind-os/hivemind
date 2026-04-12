import { createMemo, Show, For } from 'solid-js';
import type { Accessor, Setter } from 'solid-js';
import { invoke } from '@tauri-apps/api/core';
import type {
  GlobalAgentEntry,
  TokenUsage,
  Persona,
  PromptTemplate,
} from '~/types';
import type { PendingQuestion } from '../InlineQuestion';
import { pendingApprovalToasts, type PendingApproval } from '../AgentApprovalToast';
import { logError } from '../ActivityLog';
import { DataTable } from './data-table';
import { createAgentColumns, type AgentRow, type AgentColumnCallbacks } from './agents-columns';
import { Dialog, DialogContent } from '~/ui/dialog';
import { Popover, PopoverTrigger, PopoverContent } from '~/ui/popover';
import {
  Bot,
  HelpCircle,
  Lock,
  Pause,
  Play,
  Settings,
  ClipboardList,
  RefreshCw,
  BookOpen,
  Square,
  X,
} from 'lucide-solid';

// ---------------------------------------------------------------------------
// Helpers (small pure functions duplicated from FlightDeck / agents-columns)
// ---------------------------------------------------------------------------

function agentStatusColor(status: string): string {
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

function formatTokens(tokens: number): string {
  if (tokens >= 1_000_000) return `${(tokens / 1_000_000).toFixed(1)}M`;
  if (tokens >= 1_000) return `${(tokens / 1_000).toFixed(1)}k`;
  return `${tokens}`;
}

const totalTokens = (usage?: TokenUsage | null) =>
  (usage?.input_tokens ?? 0) + (usage?.output_tokens ?? 0);

// ---------------------------------------------------------------------------
// Props
// ---------------------------------------------------------------------------

export interface AgentsPanelProps {
  agents: Accessor<GlobalAgentEntry[]>;
  selectedAgentId: Accessor<string | null>;
  setSelectedAgentId: Setter<string | null>;
  busyAgentId: Accessor<string | null>;
  runtimeTick: Accessor<number>;
  agentTelemetryMap: Accessor<Map<string, TokenUsage>>;
  polledQuestions: Accessor<PendingQuestion[]>;
  polledApprovals: Accessor<PendingApproval[]>;
  pendingQuestions: Accessor<PendingQuestion[]> | undefined;
  setQuestionDialogQuestion: Setter<PendingQuestion | null>;
  setQuestionFreeText: Setter<string>;
  setQuestionSending: Setter<boolean>;
  setApprovalDialogItem: Setter<PendingApproval | null>;
  setApprovalSending: Setter<boolean>;
  personas: Accessor<Persona[]> | undefined;
  fdPromptPickerFor: Accessor<string | null>;
  setFdPromptPickerFor: Setter<string | null>;
  setFdActivePrompt: Setter<{ agent_id: string; persona: Persona; template: PromptTemplate } | null>;
  requestPauseAgent: (agent: GlobalAgentEntry) => void;
  requestResumeAgent: (agent: GlobalAgentEntry) => void;
  requestKillAgent: (agent: GlobalAgentEntry) => void;
  restartAgent: (agent: GlobalAgentEntry) => Promise<void>;
  openConfigDialog: (agent: GlobalAgentEntry) => void;
  toggleExpandAgent: (agent: GlobalAgentEntry) => void;
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

export default function AgentsPanel(props: AgentsPanelProps) {
  /** Return prompt templates scoped to a specific persona ID. */
  const promptsForPersona = (persona_id: string | null | undefined): { persona: Persona; template: PromptTemplate }[] => {
    if (!persona_id) return [];
    const personas = props.personas?.() ?? [];
    const persona = personas.find((p) => p.id === persona_id);
    if (!persona) return [];
    return (persona.prompts ?? []).map((t) => ({ persona, template: t }));
  };

  const getAgentQuestions = (agent_id: string) => {
    const fromProps = (props.pendingQuestions?.() ?? []).filter(
      (q) => q.agent_id === agent_id,
    );
    if (fromProps.length > 0) return fromProps;
    return props.polledQuestions().filter((q) => q.agent_id === agent_id);
  };

  const getAgentApprovals = (agent_id: string) => {
    const fromSse = pendingApprovalToasts().filter(
      (ap) => ap.agent_id === agent_id,
    );
    if (fromSse.length > 0) return fromSse;
    return props.polledApprovals().filter((ap) => ap.agent_id === agent_id);
  };

  const agentColumnCallbacks: AgentColumnCallbacks = {
    tick: props.runtimeTick,
    onQuestionClick: (agent_id) => {
      const questions = getAgentQuestions(agent_id);
      if (questions.length > 0) {
        props.setQuestionDialogQuestion(questions[0]);
        props.setQuestionFreeText('');
        props.setQuestionSending(false);
      }
    },
    onApprovalClick: (agent_id) => {
      const approvals = getAgentApprovals(agent_id);
      if (approvals.length > 0) {
        props.setApprovalDialogItem(approvals[0]);
        props.setApprovalSending(false);
      }
    },
  };

  const agentColumns = createAgentColumns(agentColumnCallbacks);

  const agentRows = createMemo<AgentRow[]>(() =>
    props.agents().map((agent) => {
      const usage = props.agentTelemetryMap().get(agent.agent_id);
      const qFromProps = (props.pendingQuestions?.() ?? []).filter(
        (q) => q.agent_id === agent.agent_id,
      );
      const questionCount = qFromProps.length > 0
        ? qFromProps.length
        : props.polledQuestions().filter((q) => q.agent_id === agent.agent_id).length;
      const aFromSse = pendingApprovalToasts().filter(
        (a) => a.agent_id === agent.agent_id,
      );
      const approvalCount = aFromSse.length > 0
        ? aFromSse.length
        : props.polledApprovals().filter((a) => a.agent_id === agent.agent_id).length;
      return { agent, usage, questionCount, approvalCount };
    }),
  );

  const selectedAgent = () => {
    const id = props.selectedAgentId();
    return id ? props.agents().find((a) => a.agent_id === id) ?? null : null;
  };

  const selectedRow = () => {
    const id = props.selectedAgentId();
    return id ? agentRows().find((r) => r.agent.agent_id === id) ?? null : null;
  };

  return (
    <>
      <DataTable
        columns={agentColumns}
        data={agentRows()}
        getRowId={(row: AgentRow) => row.agent.agent_id}
        selectedRowId={props.selectedAgentId()}
        onRowClick={(row: AgentRow) =>
          props.setSelectedAgentId(
            props.selectedAgentId() === row.agent.agent_id ? null : row.agent.agent_id,
          )
        }
        emptyMessage="No active agents"
      />
      <Dialog open={!!selectedAgent()} onOpenChange={(open) => { if (!open) props.setSelectedAgentId(null); }}>
        <DialogContent class="max-w-[520px] w-[90vw] p-0" onInteractOutside={(e) => e.preventDefault()}>
          <Show when={selectedAgent()}>
            {(agent) => {
              const usage = () => selectedRow()?.usage;
              const busy = () => props.busyAgentId() === agent().agent_id;
              const agentQuestions = () => getAgentQuestions(agent().agent_id);
              const agentApprovals = () => getAgentApprovals(agent().agent_id);

              return (
                <div class="fd-detail-panel" style="border: none; margin: 0; border-radius: 0;">
                  <div class="fd-detail-panel-header">
                    <span class="flight-deck-item-avatar">
                      {agent().spec.avatar || <Bot size={14} />}
                    </span>
                    <span class="fd-detail-panel-title" style="flex:1">
                      {agent().spec.friendly_name || agent().spec.name}
                    </span>
                    <span class={`flight-deck-status ${agentStatusColor(agent().status)}`}>
                      {agent().status}
                    </span>
                    <button
                      class="flight-deck-action-btn"
                      title="Close"
                      onClick={() => props.setSelectedAgentId(null)}
                      aria-label="Close"
                    >
                      <X size={14} />
                    </button>
                  </div>
                  <div class="fd-detail-panel-body">
                    <Show when={agent().active_model}>
                      <div>
                        <strong>Model:</strong> {agent().active_model}
                      </div>
                    </Show>
                    <Show when={usage()}>
                      <div class="fd-telemetry-row">
                        <span>{formatTokens(totalTokens(usage()))} tokens</span>
                        <span>{usage()!.model_calls} calls</span>
                        <span>{usage()!.tool_calls} tools</span>
                      </div>
                    </Show>
                    <Show when={agent().session_id}>
                      <div>
                        <strong>Session:</strong>{' '}
                        <code>{agent().session_id!.slice(0, 8)}…</code>
                      </div>
                    </Show>
                    <Show when={agent().parent_id}>
                      <div>
                        <strong>Parent:</strong>{' '}
                        <code>{agent().parent_id!.slice(0, 8)}…</code>
                      </div>
                    </Show>
                    <Show when={agent().last_error}>
                      <div class="text-destructive">
                        <strong>Error:</strong> {agent().last_error}
                      </div>
                    </Show>
                    {/* Question badges */}
                    <Show when={agentQuestions().length > 0}>
                      <div>
                        <span
                          class="fd-badge fd-badge-question"
                          style="cursor:pointer;"
                          title="Pending question — click to answer"
                          onClick={(e) => {
                            e.stopPropagation();
                            props.setQuestionDialogQuestion(agentQuestions()[0]);
                            props.setQuestionFreeText('');
                            props.setQuestionSending(false);
                          }}
                        >
                          <HelpCircle size={14} /> {agentQuestions().length} pending question(s)
                        </span>
                      </div>
                    </Show>
                    {/* Approval badges */}
                    <Show when={agentApprovals().length > 0}>
                      <div>
                        <span
                          class="fd-badge fd-badge-approval"
                          style="cursor:pointer;"
                          title="Pending approval — click to review"
                          onClick={(e) => {
                            e.stopPropagation();
                            props.setApprovalDialogItem(agentApprovals()[0]);
                            props.setApprovalSending(false);
                          }}
                        >
                          <Lock size={14} /> {agentApprovals().length} pending approval(s)
                        </span>
                      </div>
                    </Show>
                    {/* Action buttons */}
                    <div class="flight-deck-actions">
                      <Show
                        when={
                          agent().status === 'active' ||
                          agent().status === 'waiting' ||
                          agent().status === 'spawning'
                        }
                      >
                        <button
                          class="flight-deck-action-btn"
                          title="Pause"
                          disabled={busy()}
                          onClick={() => props.requestPauseAgent(agent())}
                        >
                          <Pause size={14} />
                        </button>
                      </Show>
                      <Show
                        when={
                          agent().status === 'paused' ||
                          agent().status === 'blocked'
                        }
                      >
                        <button
                          class="flight-deck-action-btn"
                          title="Resume"
                          disabled={busy()}
                          onClick={() => props.requestResumeAgent(agent())}
                        >
                          <Play size={14} />
                        </button>
                      </Show>
                      <button
                        class="flight-deck-action-btn"
                        title="Reconfigure"
                        disabled={busy()}
                        onClick={() => props.openConfigDialog(agent())}
                      >
                        <Settings size={14} />
                      </button>
                      <button
                        class="flight-deck-action-btn"
                        title="View event log"
                        onClick={() => props.toggleExpandAgent(agent())}
                      >
                        <ClipboardList size={14} />
                      </button>
                      <Show when={agent().session_id && agent().status !== 'done' && agent().status !== 'error'}>
                        <button
                          class="flight-deck-action-btn"
                          title="Restart agent"
                          disabled={busy()}
                          onClick={() => void props.restartAgent(agent())}
                        >
                          <RefreshCw size={14} />
                        </button>
                      </Show>
                      <Show when={promptsForPersona(agent().spec.persona_id).length > 0 && (agent().status === 'active' || agent().status === 'waiting')}>
                        <Popover
                          open={props.fdPromptPickerFor() === agent().agent_id}
                          onOpenChange={(open) => props.setFdPromptPickerFor(open ? agent().agent_id : null)}
                        >
                          <PopoverTrigger
                            as={(triggerProps: any) => (
                              <button
                                class="flight-deck-action-btn"
                                title="Send prompt template"
                                disabled={busy()}
                                {...triggerProps}
                                onClick={() => props.setFdPromptPickerFor(props.fdPromptPickerFor() === agent().agent_id ? null : agent().agent_id)}
                              >
                                <BookOpen size={14} />
                              </button>
                            )}
                          />
                          <PopoverContent class="prompt-picker-dropdown-portal">
                            <For each={promptsForPersona(agent().spec.persona_id)}>
                              {(item) => (
                                <div
                                  class="prompt-picker-item"
                                  onClick={() => {
                                    props.setFdPromptPickerFor(null);
                                    if (!item.template.input_schema?.properties || Object.keys(item.template.input_schema.properties as any).length === 0) {
                                      void invoke('send_prompt_to_bot', {
                                        agent_id: agent().agent_id,
                                        persona_id: item.persona.id,
                                        prompt_id: item.template.id,
                                        params: {},
                                      }).catch((err: any) => logError('FlightDeck', `Failed to send prompt: ${err}`));
                                    } else {
                                      props.setFdActivePrompt({ agent_id: agent().agent_id, persona: item.persona, template: item.template });
                                    }
                                  }}
                                >
                                  <span class="prompt-picker-item-name">{item.template.name || item.template.id}</span>
                                  <Show when={item.template.description}>
                                    <span class="prompt-picker-item-desc">{item.template.description}</span>
                                  </Show>
                                </div>
                              )}
                            </For>
                          </PopoverContent>
                        </Popover>
                      </Show>
                      <Show
                        when={
                          agent().status !== 'done' && agent().status !== 'error'
                        }
                      >
                        <button
                          class="flight-deck-action-btn"
                          title="Kill"
                          disabled={busy()}
                          onClick={() => props.requestKillAgent(agent())}
                        >
                          <Square size={14} />
                        </button>
                      </Show>
                    </div>
                  </div>
                </div>
              );
            }}
          </Show>
        </DialogContent>
      </Dialog>
    </>
  );
}
