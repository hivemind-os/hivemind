use axum::extract::State;
use axum::response::sse::{Event, Sse};
use axum::Json;
use serde::Serialize;
use std::collections::HashMap;
use std::convert::Infallible;

use crate::AppState;

// ── Unified Pending Interaction types ────────────────────────────────────

/// How to route the response for this interaction.
#[derive(Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum InteractionRouting {
    /// Agent lives on a real chat session supervisor.
    Session,
    /// Agent lives on the bot supervisor (no real session).
    Bot,
    /// Workflow feedback gate (route via workflow_respond_gate).
    Gate,
}

/// A single pending interaction — question, tool approval, or workflow gate.
/// Every interaction carries an `entity_id` (the entity that owns it).
#[derive(Clone, Serialize)]
pub(crate) struct PendingInteraction {
    pub request_id: String,
    /// Typed entity reference: "agent/<id>", "session/<id>", or "workflow/<id>"
    pub entity_id: String,
    /// Human-readable source name (agent name, workflow name, etc.)
    pub source_name: String,
    /// How to route the response — determined at the backend, not guessed by the frontend.
    pub routing: InteractionRouting,
    #[serde(flatten)]
    pub kind: PendingInteractionKind,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub(crate) enum PendingInteractionKind {
    #[serde(rename = "question")]
    Question {
        text: String,
        choices: Vec<String>,
        allow_freeform: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<String>,
        /// For routing: which session owns this agent (if any)
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        agent_id: String,
    },
    #[serde(rename = "tool_approval")]
    ToolApproval {
        tool_id: String,
        input: String,
        reason: String,
        /// For routing
        session_id: String,
        agent_id: String,
    },
    #[serde(rename = "workflow_gate")]
    WorkflowGate {
        instance_id: i64,
        step_id: String,
        prompt: String,
        choices: Vec<String>,
        allow_freeform: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
    },
}

/// Aggregated badge counts per entity (with ancestor propagation).
#[derive(Clone, Default, Serialize)]
pub(crate) struct InteractionCounts {
    pub questions: usize,
    pub approvals: usize,
    pub gates: usize,
}

impl InteractionCounts {
    fn total(&self) -> usize {
        self.questions + self.approvals + self.gates
    }
}

// ── Endpoints ────────────────────────────────────────────────────────────

/// GET /api/v1/pending-interactions
/// Returns all pending interactions across sessions, bots, and workflows.
pub(crate) async fn api_all_pending_interactions(
    State(state): State<AppState>,
) -> Json<Vec<PendingInteraction>> {
    let mut result = Vec::new();

    // 1. Agent questions from all sessions + bot supervisor
    let all_questions = state.chat.list_all_pending_questions().await;
    for (session_id, q) in all_questions {
        let entity_id = hive_core::agent_ref(&q.agent_id);
        let is_bot = session_id == "__bot__"
            || session_id == "__service__"
            || session_id.starts_with("trigger-");
        let routing = if is_bot { InteractionRouting::Bot } else { InteractionRouting::Session };
        let sid = if is_bot { None } else { Some(session_id) };
        result.push(PendingInteraction {
            request_id: q.request_id,
            entity_id,
            source_name: q.agent_name.clone(),
            routing,
            kind: PendingInteractionKind::Question {
                text: q.text,
                choices: q.choices,
                allow_freeform: q.allow_freeform,
                message: q.message,
                session_id: sid,
                agent_id: q.agent_id,
            },
        });
    }

    // 2. Agent approvals from all sessions + bot supervisor
    let all_approvals = state.chat.list_all_pending_approvals().await;
    for (session_id, a) in all_approvals {
        let entity_id = hive_core::agent_ref(&a.agent_id);
        let is_bot = session_id == "__bot__"
            || session_id == "__service__"
            || session_id.starts_with("trigger-");
        let routing = if is_bot { InteractionRouting::Bot } else { InteractionRouting::Session };
        result.push(PendingInteraction {
            request_id: a.request_id,
            entity_id,
            source_name: a.agent_name.clone(),
            routing,
            kind: PendingInteractionKind::ToolApproval {
                tool_id: a.tool_id,
                input: a.input,
                reason: a.reason,
                session_id,
                agent_id: a.agent_id,
            },
        });
    }

    // 3. Workflow feedback gates across all sessions
    if let Ok(all_gates) = state.workflows.list_all_waiting_feedback().await {
        for wf in all_gates {
            let entity_id = hive_core::workflow_ref(&wf.instance_id.to_string());
            result.push(PendingInteraction {
                request_id: format!("wf:{}:{}", wf.instance_id, wf.step_id),
                entity_id,
                source_name: format!("Workflow: {}", wf.definition_name),
                routing: InteractionRouting::Gate,
                kind: PendingInteractionKind::WorkflowGate {
                    instance_id: wf.instance_id,
                    step_id: wf.step_id,
                    prompt: wf.prompt,
                    choices: wf.choices,
                    allow_freeform: wf.allow_freeform,
                    session_id: Some(wf.parent_session_id),
                },
            });
        }
    }

    Json(result)
}
/// Returns badge counts per entity, propagated up the ancestor chain.
pub(crate) async fn api_pending_interaction_counts(
    State(state): State<AppState>,
) -> Json<HashMap<String, InteractionCounts>> {
    // First, collect all interactions to get per-entity direct counts
    let interactions = api_all_pending_interactions_inner(&state).await;

    let mut counts: HashMap<String, InteractionCounts> = HashMap::new();

    for interaction in &interactions {
        let entry = counts.entry(interaction.entity_id.clone()).or_default();
        match &interaction.kind {
            PendingInteractionKind::Question { .. } => entry.questions += 1,
            PendingInteractionKind::ToolApproval { .. } => entry.approvals += 1,
            PendingInteractionKind::WorkflowGate { .. } => entry.gates += 1,
        }
    }

    // Propagate counts up the ancestor chain
    let direct_entities: Vec<String> = counts.keys().cloned().collect();
    for entity_id in &direct_entities {
        let ancestors = state.entity_graph.ancestors(entity_id);
        let direct = counts.get(entity_id).cloned().unwrap_or_default();
        if direct.total() == 0 {
            continue;
        }
        for ancestor in ancestors {
            let entry = counts.entry(ancestor.entity_id).or_default();
            entry.questions += direct.questions;
            entry.approvals += direct.approvals;
            entry.gates += direct.gates;
        }
    }

    Json(counts)
}

/// Internal helper that collects all interactions without wrapping in Json.
async fn api_all_pending_interactions_inner(state: &AppState) -> Vec<PendingInteraction> {
    let mut result = Vec::new();

    let all_questions = state.chat.list_all_pending_questions().await;
    for (session_id, q) in all_questions {
        let entity_id = hive_core::agent_ref(&q.agent_id);
        let is_bot = session_id == "__bot__"
            || session_id == "__service__"
            || session_id.starts_with("trigger-");
        let routing = if is_bot { InteractionRouting::Bot } else { InteractionRouting::Session };
        let sid = if is_bot { None } else { Some(session_id) };
        result.push(PendingInteraction {
            request_id: q.request_id,
            entity_id,
            source_name: q.agent_name.clone(),
            routing,
            kind: PendingInteractionKind::Question {
                text: q.text,
                choices: q.choices,
                allow_freeform: q.allow_freeform,
                message: q.message,
                session_id: sid,
                agent_id: q.agent_id,
            },
        });
    }

    let all_approvals = state.chat.list_all_pending_approvals().await;
    for (session_id, a) in all_approvals {
        let entity_id = hive_core::agent_ref(&a.agent_id);
        let is_bot = session_id == "__bot__"
            || session_id == "__service__"
            || session_id.starts_with("trigger-");
        let routing = if is_bot { InteractionRouting::Bot } else { InteractionRouting::Session };
        result.push(PendingInteraction {
            request_id: a.request_id,
            entity_id,
            source_name: a.agent_name.clone(),
            routing,
            kind: PendingInteractionKind::ToolApproval {
                tool_id: a.tool_id,
                input: a.input,
                reason: a.reason,
                session_id,
                agent_id: a.agent_id,
            },
        });
    }

    if let Ok(all_gates) = state.workflows.list_all_waiting_feedback().await {
        for wf in all_gates {
            let entity_id = hive_core::workflow_ref(&wf.instance_id.to_string());
            result.push(PendingInteraction {
                request_id: format!("wf:{}:{}", wf.instance_id, wf.step_id),
                entity_id,
                source_name: format!("Workflow: {}", wf.definition_name),
                routing: InteractionRouting::Gate,
                kind: PendingInteractionKind::WorkflowGate {
                    instance_id: wf.instance_id,
                    step_id: wf.step_id,
                    prompt: wf.prompt,
                    choices: wf.choices,
                    allow_freeform: wf.allow_freeform,
                    session_id: Some(wf.parent_session_id),
                },
            });
        }
    }

    result
}

// ── SSE stream ───────────────────────────────────────────────────────────

/// Combined snapshot sent as a single SSE event.
#[derive(Serialize)]
struct InteractionSnapshot {
    interactions: Vec<PendingInteraction>,
    counts: HashMap<String, InteractionCounts>,
}

/// Build a full snapshot of interactions + propagated counts.
async fn build_snapshot(state: &AppState) -> InteractionSnapshot {
    let interactions = api_all_pending_interactions_inner(state).await;

    let mut counts: HashMap<String, InteractionCounts> = HashMap::new();
    for interaction in &interactions {
        let entry = counts.entry(interaction.entity_id.clone()).or_default();
        match &interaction.kind {
            PendingInteractionKind::Question { .. } => entry.questions += 1,
            PendingInteractionKind::ToolApproval { .. } => entry.approvals += 1,
            PendingInteractionKind::WorkflowGate { .. } => entry.gates += 1,
        }
    }
    let direct_entities: Vec<String> = counts.keys().cloned().collect();
    for entity_id in &direct_entities {
        let ancestors = state.entity_graph.ancestors(entity_id);
        let direct = counts.get(entity_id).cloned().unwrap_or_default();
        if direct.total() == 0 {
            continue;
        }
        for ancestor in ancestors {
            let entry = counts.entry(ancestor.entity_id).or_default();
            entry.questions += direct.questions;
            entry.approvals += direct.approvals;
            entry.gates += direct.gates;
        }
    }

    InteractionSnapshot { interactions, counts }
}

fn snapshot_event(snap: &InteractionSnapshot) -> Result<Event, Infallible> {
    let json = serde_json::to_string(snap).unwrap_or_default();
    Ok(Event::default().event("snapshot").data(json))
}

/// GET /api/v1/interactions/stream
/// SSE stream that pushes a full snapshot whenever interactions change.
pub(crate) async fn api_interactions_stream(
    State(state): State<AppState>,
) -> Sse<impl futures_core::Stream<Item = Result<Event, Infallible>>> {
    let mut approval_rx = state.chat.subscribe_approvals();
    let mut gate_rx = state.event_bus.subscribe_queued_bounded("interaction", 10_000);
    // Workflow gates publish to "workflow.step.waiting" and "workflow.interaction.*",
    // so subscribe to the "workflow" prefix to catch all gate-related changes.
    let mut wf_rx = state.event_bus.subscribe_queued_bounded("workflow", 10_000);

    let stream = async_stream::stream! {
        // Initial snapshot
        let snap = build_snapshot(&state).await;
        yield snapshot_event(&snap);

        // Periodic refresh interval — guarantees the frontend receives
        // pending questions even if push events were lost due to broadcast
        // lag, reconnection gaps, or other transient issues.
        let mut refresh_interval = tokio::time::interval(std::time::Duration::from_secs(10));
        // The first tick fires immediately; skip it since we just sent
        // the initial snapshot above.
        refresh_interval.tick().await;

        loop {
            // Wait for any source to signal a change, or the periodic refresh.
            tokio::select! {
                biased;
                _ = state.shutdown.cancelled() => break,
                approval = approval_rx.recv() => {
                    match approval {
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!(skipped = n, "interaction SSE: approval stream lagged");
                        }
                        Ok(_) => {}
                    }
                }
                gate = gate_rx.recv() => {
                    if gate.is_none() {
                        break;
                    }
                }
                wf = wf_rx.recv() => {
                    if wf.is_none() {
                        break;
                    }
                }
                _ = refresh_interval.tick() => {
                    // Periodic refresh — rebuild snapshot to catch any
                    // interactions that slipped through push channels.
                }
            }

            // Debounce: drain any additional events that arrive within 100ms.
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            while approval_rx.try_recv().is_ok() {}
            while gate_rx.try_recv().is_ok() {}
            while wf_rx.try_recv().is_ok() {}

            let snap = build_snapshot(&state).await;
            yield snapshot_event(&snap);
        }
    };

    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("keep-alive"),
    )
}
