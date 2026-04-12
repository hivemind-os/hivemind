use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::sse::{Event, Sse},
    Json,
};
use serde::{Deserialize, Serialize};
use std::convert::Infallible;

use crate::{chat_error, AppState, ToolApprovalResponse};
use hive_agents::{AgentSummary, TelemetrySnapshot};
use hive_contracts::{InteractionResponsePayload, UserInteractionResponse};

#[derive(Deserialize)]
pub(crate) struct AgentEventsPagination {
    pub(crate) offset: Option<usize>,
    pub(crate) limit: Option<usize>,
}

#[derive(Serialize)]
pub(crate) struct PagedEventsResponse {
    pub(crate) events: Vec<hive_agents::SupervisorEvent>,
    pub(crate) total: usize,
}

#[derive(Serialize)]
pub(crate) struct PagedSessionEventsResponse {
    pub(crate) events: Vec<hive_chat::SessionEvent>,
    pub(crate) total: usize,
}

#[derive(Deserialize)]
pub(crate) struct RestartAgentRequest {
    model: Option<String>,
    allowed_tools: Option<Vec<String>>,
}

pub(crate) async fn api_list_session_agents(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<Vec<AgentSummary>>, (StatusCode, String)> {
    let agents = state.chat.list_session_agents(&session_id).await.map_err(chat_error)?;
    Ok(Json(agents))
}

pub(crate) async fn api_pause_agent(
    State(state): State<AppState>,
    Path((session_id, agent_id)): Path<(String, String)>,
) -> Result<StatusCode, (StatusCode, String)> {
    state.chat.pause_session_agent(&session_id, &agent_id).await.map_err(chat_error)?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn api_resume_agent(
    State(state): State<AppState>,
    Path((session_id, agent_id)): Path<(String, String)>,
) -> Result<StatusCode, (StatusCode, String)> {
    state.chat.resume_session_agent(&session_id, &agent_id).await.map_err(chat_error)?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn api_kill_agent(
    State(state): State<AppState>,
    Path((session_id, agent_id)): Path<(String, String)>,
) -> Result<StatusCode, (StatusCode, String)> {
    state.chat.kill_session_agent(&session_id, &agent_id).await.map_err(chat_error)?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn api_restart_agent(
    State(state): State<AppState>,
    Path((session_id, agent_id)): Path<(String, String)>,
    body: Option<Json<RestartAgentRequest>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let (new_model, new_allowed_tools) =
        body.map(|b| (b.0.model, b.0.allowed_tools)).unwrap_or((None, None));
    let new_id = state
        .chat
        .restart_session_agent(&session_id, &agent_id, new_model, new_allowed_tools)
        .await
        .map_err(chat_error)?;
    Ok(Json(serde_json::json!({ "new_agent_id": new_id })))
}

/// SSE stream of agent stage events. On connect, emits the full agent list
/// and telemetry snapshot as initial state, then streams SupervisorEvents.
pub(crate) async fn api_agent_stage_stream(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Sse<impl futures_core::Stream<Item = Result<Event, Infallible>>>, (StatusCode, String)>
{
    // Subscribe BEFORE snapshotting so that any events emitted between
    // snapshot and the first recv() are buffered in the receiver rather
    // than lost.  Duplicates (event + snapshot) are harmless — the
    // frontend deduplicates by request_id.
    let rx = state.chat.subscribe_supervisor_events(&session_id).await.map_err(chat_error)?;

    // Get initial snapshot.
    let agents = state.chat.list_session_agents(&session_id).await.map_err(chat_error)?;
    let telemetry = state.chat.session_agent_telemetry(&session_id).await.map_err(chat_error)?;
    let pending_questions = state.chat.list_pending_questions_for_session(&session_id).await;
    let sid = session_id.clone();

    let stream = async_stream::stream! {
        // Emit initial snapshot so late-joining clients see all current agents.
        let snapshot = serde_json::json!({
            "type": "snapshot",
            "agents": agents,
            "telemetry": telemetry,
            "pending_questions": pending_questions,
        });
        yield Ok(Event::default().data(serde_json::to_string(&snapshot).unwrap_or_default()));

        let mut rx = rx;
        loop {
            match rx.recv().await {
                Ok(event) => {
                    let data = serde_json::to_string(&event).unwrap_or_default();
                    yield Ok(Event::default().data(data));
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(session_id = %sid, skipped, "agent stage SSE lagged — sending recovery snapshot");
                    // Re-emit a full snapshot so the frontend catches up on
                    // any missed events (questions, status changes, etc.).
                    if let Ok(agents) = state.chat.list_session_agents(&sid).await {
                        let telemetry = state.chat.session_agent_telemetry(&sid).await.ok();
                        let pending_questions = state.chat.list_pending_questions_for_session(&sid).await;
                        let snap = serde_json::json!({
                            "type": "snapshot",
                            "agents": agents,
                            "telemetry": telemetry,
                            "pending_questions": pending_questions,
                        });
                        yield Ok(Event::default().data(serde_json::to_string(&snap).unwrap_or_default()));
                    }
                }
            }
        }
    };

    Ok(Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("keep-alive"),
    ))
}

pub(crate) async fn api_agent_events(
    State(state): State<AppState>,
    Path((session_id, agent_id)): Path<(String, String)>,
    Query(pagination): Query<AgentEventsPagination>,
) -> Result<Json<PagedEventsResponse>, (StatusCode, String)> {
    let (events, total) = state
        .chat
        .get_agent_events_paged(
            &session_id,
            &agent_id,
            pagination.offset.unwrap_or(0),
            pagination.limit.unwrap_or(50),
        )
        .await
        .map_err(chat_error)?;
    Ok(Json(PagedEventsResponse { events, total }))
}

pub(crate) async fn api_session_events(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(pagination): Query<AgentEventsPagination>,
) -> Result<Json<PagedSessionEventsResponse>, (StatusCode, String)> {
    let (events, total) = state
        .chat
        .get_session_events_paged(
            &session_id,
            pagination.offset.unwrap_or(0),
            pagination.limit.unwrap_or(500),
        )
        .await
        .map_err(chat_error)?;
    Ok(Json(PagedSessionEventsResponse { events, total }))
}

pub(crate) async fn api_agent_interaction_response(
    State(state): State<AppState>,
    Path((session_id, agent_id)): Path<(String, String)>,
    Json(response): Json<UserInteractionResponse>,
) -> Result<Json<ToolApprovalResponse>, (StatusCode, String)> {
    // Handle permission granting before resolving the interaction.
    let granted_scope = match &response.payload {
        InteractionResponsePayload::ToolApproval { approved, allow_session: true, .. } => {
            // "Allow for Session" — grant to all agents in this session.
            state
                .chat
                .grant_all_agents_permission(
                    &session_id,
                    &agent_id,
                    &response.request_id,
                    *approved,
                )
                .await
                .map_err(chat_error)?
        }
        InteractionResponsePayload::ToolApproval { approved, allow_agent: true, .. } => {
            // "Allow for Agent" — grant to this specific agent only.
            state
                .chat
                .grant_agent_permission(&session_id, &agent_id, &response.request_id, *approved)
                .await
                .map_err(chat_error)?
        }
        _ => None,
    };

    let acknowledged = state
        .chat
        .respond_to_agent_interaction(&session_id, &agent_id, response)
        .await
        .map_err(chat_error)?;
    Ok(Json(ToolApprovalResponse { acknowledged, granted_scope }))
}

pub(crate) async fn api_agent_telemetry(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<TelemetrySnapshot>, (StatusCode, String)> {
    state.chat.session_agent_telemetry(&session_id).await.map(Json).map_err(chat_error)
}
