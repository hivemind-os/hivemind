/**
 * Centralized interaction routing — single source of truth for answering
 * questions, approving tools, and responding to workflow gates.
 *
 * The backend sets a `routing` field ("session" | "bot" | "gate") on
 * every interaction, so the frontend never has to guess from sentinel
 * session IDs or field presence.
 */
import { invoke } from "@tauri-apps/api/core";

// ── Types matching the backend PendingInteraction ────────────────────────

/** Backend-determined routing kind — set at the source, not guessed. */
export type InteractionRoutingKind = "session" | "bot" | "gate";

export interface PendingInteraction {
  request_id: string;
  /** Typed entity reference: "agent/<id>", "session/<id>", "workflow/<id>" */
  entity_id: string;
  source_name: string;
  type: "question" | "tool_approval" | "workflow_gate";
  /** How to route the response — determined by the backend. */
  routing?: InteractionRoutingKind;
  // Question fields
  text?: string;
  choices?: string[];
  allow_freeform?: boolean;
  multi_select?: boolean;
  message?: string;
  session_id?: string;
  agent_id?: string;
  // Tool approval fields
  tool_id?: string;
  input?: string;
  reason?: string;
  // Workflow gate fields
  instance_id?: number;
  step_id?: string;
  prompt?: string;
}

export interface InteractionCounts {
  questions: number;
  approvals: number;
  gates: number;
}

// ── Response payloads ────────────────────────────────────────────────────

export interface QuestionResponse {
  selected_choice?: number;
  selected_choices?: number[];
  text?: string;
}

export interface ApprovalResponse {
  approved: boolean;
  allow_session?: boolean;
  allow_agent?: boolean;
}

export interface GateResponse {
  selected?: string;
  text?: string;
}

// ── Parse entity refs ────────────────────────────────────────────────────

export function parseEntityRef(ref: string): { type: string; id: string } | null {
  const slash = ref.indexOf("/");
  if (slash === -1) return null;
  return { type: ref.substring(0, slash), id: ref.substring(slash + 1) };
}

// ── Internal routing helpers ─────────────────────────────────────────────

async function routeAgentResponse(
  interaction: PendingInteraction,
  payload: { request_id: string; payload: Record<string, unknown> },
): Promise<void> {
  const { agent_id, session_id, routing } = interaction;

  // The backend stores agent_id = session_id for questions asked by the
  // session's own chat-loop agent (not a supervisor sub-agent).  Routing
  // to the agent-specific endpoint with that id would 404 because no such
  // agent exists in the supervisor.  Normalise this case: when agent_id
  // equals session_id treat it as if agent_id is absent so we always hit
  // the session-level endpoint.
  const effectiveAgentId = (agent_id && agent_id !== session_id) ? agent_id : undefined;

  if (routing === "session" && session_id) {
    if (effectiveAgentId) {
      await invoke("agent_respond_interaction", { session_id, agent_id: effectiveAgentId, response: payload });
    } else {
      await invoke("chat_respond_interaction", { session_id, response: payload });
    }
  } else if (routing === "bot" && agent_id) {
    await invoke("bot_interaction", { agent_id, response: payload });
  } else if (effectiveAgentId && session_id) {
    await invoke("agent_respond_interaction", { session_id, agent_id: effectiveAgentId, response: payload });
  } else if (agent_id && !session_id) {
    // agent_id present but no session — must be a standalone bot agent
    await invoke("bot_interaction", { agent_id, response: payload });
  } else if (session_id) {
    await invoke("chat_respond_interaction", { session_id, response: payload });
  } else {
    throw new Error("Cannot route interaction: no routing info");
  }
}

// ── Centralized routing ──────────────────────────────────────────────────

/**
 * Answer a question interaction.
 */
export async function answerQuestion(
  interaction: PendingInteraction,
  response: QuestionResponse,
): Promise<void> {
  const payload = {
    request_id: interaction.request_id,
    payload: {
      type: "answer" as const,
      ...(response.selected_choice !== undefined && { selected_choice: response.selected_choice }),
      ...(response.selected_choices !== undefined && { selected_choices: response.selected_choices }),
      ...(response.text !== undefined && { text: response.text }),
    },
  };
  await routeAgentResponse(interaction, payload);
}

/**
 * Respond to a tool approval interaction.
 */
export async function respondToApproval(
  interaction: PendingInteraction,
  response: ApprovalResponse,
): Promise<void> {
  const payload = {
    request_id: interaction.request_id,
    payload: {
      type: "tool_approval" as const,
      approved: response.approved,
      allow_session: response.allow_session ?? false,
      allow_agent: response.allow_agent ?? false,
    },
  };
  await routeAgentResponse(interaction, payload);
}

/**
 * Respond to a workflow feedback gate.
 */
export async function respondToGate(
  interaction: PendingInteraction,
  response: GateResponse,
): Promise<void> {
  const { instance_id, step_id } = interaction;
  if (instance_id == null || !step_id) {
    throw new Error("Cannot route gate response: missing instance_id/step_id");
  }
  await invoke("workflow_respond_gate", {
    instance_id: instance_id,
    step_id: step_id,
    response,
  });
}

/**
 * Universal response dispatcher — routes based on interaction.type.
 */
export async function respondToInteraction(
  interaction: PendingInteraction,
  response: QuestionResponse | ApprovalResponse | GateResponse,
): Promise<void> {
  switch (interaction.type) {
    case "question":
      return answerQuestion(interaction, response as QuestionResponse);
    case "tool_approval":
      return respondToApproval(interaction, response as ApprovalResponse);
    case "workflow_gate":
      return respondToGate(interaction, response as GateResponse);
    default:
      throw new Error(`Unknown interaction type: ${interaction.type}`);
  }
}
