use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::sse::{Event, Sse},
    Json,
};
use serde::Deserialize;
use std::convert::Infallible;

use crate::{
    chat_error, AppState, BotConfig, BotSummary, ToolApprovalResponse, WorkspaceEntry,
    WorkspaceFileContent,
};
use hive_agents::TelemetrySnapshot;
use hive_contracts::UserInteractionResponse;
use hive_core::{find_prompt_template, load_personas, render_prompt_template};
use serde_json::Value;

// Re-use the paged events response from the agents module.
use super::agents::{AgentEventsPagination, PagedEventsResponse};

pub(crate) async fn api_list_bots(State(state): State<AppState>) -> Json<Vec<BotSummary>> {
    Json(state.chat.list_bots().await)
}

pub(crate) async fn api_launch_bot(
    State(state): State<AppState>,
    Json(config): Json<BotConfig>,
) -> Result<Json<BotSummary>, (StatusCode, String)> {
    state.chat.launch_bot(config).await.map(Json).map_err(chat_error)
}

#[derive(Deserialize)]
pub(crate) struct MessageBody {
    content: String,
}

pub(crate) async fn api_message_bot(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(body): Json<MessageBody>,
) -> Result<StatusCode, (StatusCode, String)> {
    state
        .chat
        .message_bot(&agent_id, body.content)
        .await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(chat_error)
}

pub(crate) async fn api_deactivate_bot(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    state.chat.deactivate_bot(&agent_id).await.map(|_| StatusCode::NO_CONTENT).map_err(chat_error)
}

pub(crate) async fn api_activate_bot(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    state.chat.activate_bot(&agent_id).await.map(|_| StatusCode::NO_CONTENT).map_err(chat_error)
}

pub(crate) async fn api_delete_bot(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    state.chat.delete_bot(&agent_id).await.map(|_| StatusCode::NO_CONTENT).map_err(chat_error)
}

pub(crate) async fn api_bots_stream(
    State(state): State<AppState>,
) -> Sse<impl futures_core::Stream<Item = Result<Event, Infallible>>> {
    // Get initial snapshot so late-joining clients see all current bots.
    let agents = state.chat.list_bots().await;
    let telemetry = state.chat.bot_telemetry().await.ok();
    let mut rx = state.chat.subscribe_bot_events();
    let shutdown = state.shutdown.clone();
    let stream = async_stream::stream! {
        let snapshot = serde_json::json!({
            "type": "snapshot",
            "agents": agents,
            "telemetry": telemetry,
        });
        yield Ok(Event::default().data(serde_json::to_string(&snapshot).unwrap_or_default()));

        loop {
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => break,
                result = rx.recv() => {
                    match result {
                        Ok(event) => {
                            let data = serde_json::to_string(&event).unwrap_or_default();
                            yield Ok(Event::default().data(data));
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    }
                }
            }
        }
    };
    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("keep-alive"),
    )
}

pub(crate) async fn api_bot_telemetry(
    State(state): State<AppState>,
) -> Result<Json<TelemetrySnapshot>, (StatusCode, String)> {
    state.chat.bot_telemetry().await.map(Json).map_err(chat_error)
}

pub(crate) async fn api_bot_events(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Query(pagination): Query<AgentEventsPagination>,
) -> Result<Json<PagedEventsResponse>, (StatusCode, String)> {
    let (events, total) = state
        .chat
        .get_bot_events_paged(
            &agent_id,
            pagination.offset.unwrap_or(0),
            pagination.limit.unwrap_or(50),
        )
        .await
        .map_err(chat_error)?;
    Ok(Json(PagedEventsResponse { events, total }))
}

pub(crate) async fn api_bot_interaction(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(response): Json<UserInteractionResponse>,
) -> Result<Json<ToolApprovalResponse>, (StatusCode, String)> {
    let acknowledged =
        state.chat.respond_to_bot_interaction(&agent_id, response).await.map_err(chat_error)?;
    Ok(Json(ToolApprovalResponse { acknowledged, granted_scope: None }))
}

pub(crate) async fn api_get_bot_permissions(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> Result<Json<hive_contracts::SessionPermissions>, (StatusCode, String)> {
    let perms = state.chat.get_bot_permissions(&agent_id).await.map_err(chat_error)?;
    Ok(Json(perms))
}

pub(crate) async fn api_set_bot_permissions(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(permissions): Json<hive_contracts::SessionPermissions>,
) -> Result<StatusCode, (StatusCode, String)> {
    state.chat.set_bot_permissions(&agent_id, permissions).await.map_err(chat_error)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub(crate) struct WorkspaceFileQuery {
    path: String,
}

#[derive(Deserialize)]
pub(crate) struct WorkspaceListQuery {
    #[serde(default)]
    path: Option<String>,
}

pub(crate) async fn api_bot_workspace_files(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Query(query): Query<WorkspaceListQuery>,
) -> Result<Json<Vec<WorkspaceEntry>>, (StatusCode, String)> {
    state
        .chat
        .list_bot_workspace_files(&agent_id, query.path.as_deref())
        .map(Json)
        .map_err(chat_error)
}

pub(crate) async fn api_bot_workspace_file(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Query(query): Query<WorkspaceFileQuery>,
) -> Result<Json<WorkspaceFileContent>, (StatusCode, String)> {
    state.chat.read_bot_workspace_file(&agent_id, &query.path).map(Json).map_err(chat_error)
}

// ── Prompt-based bot launch / message ────────────────────────────────────

#[derive(Deserialize)]
pub(crate) struct SendPromptBody {
    persona_id: String,
    prompt_id: String,
    #[serde(default)]
    params: Value,
}

pub(crate) async fn api_send_prompt_to_bot(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(body): Json<SendPromptBody>,
) -> Result<StatusCode, (StatusCode, String)> {
    let personas = load_personas(&state.personas_dir).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to load personas: {e}"))
    })?;
    let persona = personas.iter().find(|p| p.id == body.persona_id).ok_or_else(|| {
        (StatusCode::NOT_FOUND, format!("Persona '{}' not found", body.persona_id))
    })?;
    let template = find_prompt_template(&persona.prompts, &body.prompt_id).ok_or_else(|| {
        (StatusCode::NOT_FOUND, format!("Prompt template '{}' not found", body.prompt_id))
    })?;
    let rendered = render_prompt_template(template, &body.params)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    state
        .chat
        .message_bot(&agent_id, rendered)
        .await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(chat_error)
}

#[derive(Deserialize)]
pub(crate) struct LaunchBotWithPromptBody {
    persona_id: String,
    prompt_id: String,
    #[serde(default)]
    params: Value,
    #[serde(default)]
    friendly_name: Option<String>,
    #[serde(default)]
    model: Option<String>,
}

pub(crate) async fn api_launch_bot_with_prompt(
    State(state): State<AppState>,
    Json(body): Json<LaunchBotWithPromptBody>,
) -> Result<Json<BotSummary>, (StatusCode, String)> {
    let personas = load_personas(&state.personas_dir).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to load personas: {e}"))
    })?;
    let persona = personas.iter().find(|p| p.id == body.persona_id).ok_or_else(|| {
        (StatusCode::NOT_FOUND, format!("Persona '{}' not found", body.persona_id))
    })?;
    let template = find_prompt_template(&persona.prompts, &body.prompt_id).ok_or_else(|| {
        (StatusCode::NOT_FOUND, format!("Prompt template '{}' not found", body.prompt_id))
    })?;
    let rendered = render_prompt_template(template, &body.params)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    let config = BotConfig {
        id: String::new(),
        friendly_name: body
            .friendly_name
            .unwrap_or_else(|| format!("{} — {}", persona.name, template.name)),
        description: template.description.clone(),
        system_prompt: persona.system_prompt.clone(),
        launch_prompt: rendered,
        model: body.model,
        preferred_models: persona.preferred_models.clone(),
        loop_strategy: Some(persona.loop_strategy.clone()),
        tool_execution_mode: Some(persona.tool_execution_mode),
        allowed_tools: persona.allowed_tools.clone(),
        avatar: persona.avatar.clone(),
        color: persona.color.clone(),
        data_class: Default::default(),
        role: Default::default(),
        mode: Default::default(),
        active: false,
        created_at: String::new(),
        timeout_secs: None,
        permission_rules: Vec::new(),
        tool_limits: None,
        persona_id: Some(body.persona_id.clone()),
        shadow_mode: false,
    };

    state.chat.launch_bot(config).await.map(Json).map_err(chat_error)
}
