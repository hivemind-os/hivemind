use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::sse::{Event, Sse},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::convert::Infallible;

use crate::{
    chat_error, clamp_limit, AppState, ChatMemoryItem, ChatSessionSnapshot, ChatSessionSummary,
    InterruptRequest, RiskScanRecord, SendMessageRequest, SendMessageResponse, SessionModality,
    ToolApprovalRequest, ToolApprovalResponse,
};
use hive_classification::DataClass;
use hive_contracts::{
    FileAuditRecord, FileAuditStatus, InteractionResponsePayload, SaveFileRequest,
    UserInteractionResponse, WorkspaceClassification, WorkspaceEntry, WorkspaceFileContent,
};

// ── Request / response types ─────────────────────────────────────────────

#[derive(Deserialize, Default)]
pub(crate) struct CreateSessionRequest {
    #[serde(default)]
    modality: SessionModality,
    title: Option<String>,
    persona_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct UploadFileRequest {
    filename: String,
    content: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct LinkWorkspaceRequest {
    path: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct FilePathQuery {
    path: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct MoveEntryRequest {
    from: String,
    to: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AuditFileRequest {
    model: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct AuditStatusResponse {
    pub(crate) record: Option<FileAuditRecord>,
    pub(crate) status: FileAuditStatus,
}

#[derive(Deserialize)]
pub(crate) struct SetDefaultClassificationRequest {
    default: DataClass,
}

#[derive(Deserialize)]
pub(crate) struct SetOverrideRequest {
    class: DataClass,
}

#[derive(Deserialize)]
pub(crate) struct DeleteSessionQuery {
    #[serde(default)]
    scrub_kb: bool,
}

#[derive(Deserialize)]
pub(crate) struct ProposeLayoutRequest {
    algorithm: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct MemoryQuery {
    query: Option<String>,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RenameSessionRequest {
    pub title: String,
}

// ── Session CRUD ─────────────────────────────────────────────────────────

pub(crate) async fn create_chat_session(
    State(state): State<AppState>,
    body: Option<Json<CreateSessionRequest>>,
) -> Result<Json<ChatSessionSnapshot>, (StatusCode, String)> {
    let req = body.map(|b| b.0).unwrap_or_default();
    state
        .chat
        .create_session(req.modality, req.title, req.persona_id)
        .await
        .map(Json)
        .map_err(chat_error)
}

pub(crate) async fn list_chat_sessions(
    State(state): State<AppState>,
) -> Json<Vec<ChatSessionSummary>> {
    Json(state.chat.list_sessions().await)
}

pub(crate) async fn get_chat_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<ChatSessionSnapshot>, (StatusCode, String)> {
    state.chat.get_session(&session_id).await.map(Json).map_err(chat_error)
}

pub(crate) async fn delete_chat_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<DeleteSessionQuery>,
) -> Result<StatusCode, (StatusCode, String)> {
    state.chat.delete_session(&session_id, query.scrub_kb).await.map_err(chat_error)?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn rename_chat_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(request): Json<RenameSessionRequest>,
) -> Result<Json<ChatSessionSnapshot>, (StatusCode, String)> {
    state.chat.rename_session(&session_id, request.title).await.map(Json).map_err(chat_error)
}

pub(crate) async fn send_chat_message(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(request): Json<SendMessageRequest>,
) -> Result<Json<SendMessageResponse>, (StatusCode, String)> {
    state.chat.enqueue_message(&session_id, request).await.map(Json).map_err(chat_error)
}

// ── Session persona ─────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub(crate) struct SetSessionPersonaRequest {
    persona_id: String,
}

pub(crate) async fn set_session_persona(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(request): Json<SetSessionPersonaRequest>,
) -> Result<Json<ChatSessionSnapshot>, (StatusCode, String)> {
    state
        .chat
        .set_session_persona(&session_id, &request.persona_id)
        .await
        .map(Json)
        .map_err(chat_error)
}

// ── Canvas ───────────────────────────────────────────────────────────────

pub(crate) async fn api_recluster_canvas(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let count = state.chat.recluster_canvas(&session_id).await.map_err(chat_error)?;
    Ok(Json(serde_json::json!({ "clusters_created": count })))
}

pub(crate) async fn api_propose_layout(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    body: Option<Json<ProposeLayoutRequest>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let algorithm = body.and_then(|b| b.algorithm.clone());
    let proposal_id =
        state.chat.propose_layout(&session_id, algorithm).await.map_err(chat_error)?;
    Ok(Json(serde_json::json!({ "proposal_id": proposal_id })))
}

// ── File upload / workspace ──────────────────────────────────────────────

pub(crate) async fn upload_file_to_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(request): Json<UploadFileRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    use base64::Engine;

    let content = base64::engine::general_purpose::STANDARD
        .decode(&request.content)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("invalid base64: {e}")))?;
    if content.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "file content cannot be empty".to_string()));
    }
    let path = state
        .chat
        .upload_file(&session_id, &request.filename, &content)
        .await
        .map_err(chat_error)?;
    Ok(Json(serde_json::json!({ "path": path })))
}

pub(crate) async fn link_session_workspace(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(request): Json<LinkWorkspaceRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    state.chat.link_workspace(&session_id, &request.path).await.map_err(chat_error)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub(crate) struct ListFilesQuery {
    /// Optional subdirectory path to list. When omitted, lists the workspace root.
    pub path: Option<String>,
}

pub(crate) async fn list_workspace_files(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<ListFilesQuery>,
) -> Result<Json<Vec<WorkspaceEntry>>, (StatusCode, String)> {
    state
        .chat
        .list_workspace_files(&session_id, query.path.as_deref())
        .await
        .map(Json)
        .map_err(chat_error)
}

pub(crate) async fn read_workspace_file(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<FilePathQuery>,
) -> Result<Json<WorkspaceFileContent>, (StatusCode, String)> {
    state.chat.read_workspace_file(&session_id, &query.path).await.map(Json).map_err(chat_error)
}

pub(crate) async fn save_workspace_file(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<FilePathQuery>,
    Json(body): Json<SaveFileRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    if let Some(ref b64) = body.content_base64 {
        state
            .chat
            .save_workspace_file_binary(&session_id, &query.path, b64)
            .await
            .map_err(chat_error)?;
    } else {
        state
            .chat
            .save_workspace_file(&session_id, &query.path, &body.content)
            .await
            .map_err(chat_error)?;
    }
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn create_workspace_directory(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<FilePathQuery>,
) -> Result<StatusCode, (StatusCode, String)> {
    state.chat.create_workspace_directory(&session_id, &query.path).await.map_err(chat_error)?;
    Ok(StatusCode::CREATED)
}

pub(crate) async fn delete_workspace_entry(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<FilePathQuery>,
) -> Result<StatusCode, (StatusCode, String)> {
    state.chat.delete_workspace_entry(&session_id, &query.path).await.map_err(chat_error)?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn move_workspace_entry(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<MoveEntryRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    state.chat.move_workspace_entry(&session_id, &body.from, &body.to).await.map_err(chat_error)?;
    Ok(StatusCode::NO_CONTENT)
}

// ── Workspace audit / classification ─────────────────────────────────────

pub(crate) async fn api_audit_workspace_file(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<FilePathQuery>,
    Json(body): Json<AuditFileRequest>,
) -> Result<Json<FileAuditRecord>, (StatusCode, String)> {
    state
        .chat
        .audit_workspace_file(&session_id, &query.path, &body.model)
        .await
        .map(Json)
        .map_err(chat_error)
}

pub(crate) async fn api_get_workspace_audit(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<FilePathQuery>,
) -> Result<Json<AuditStatusResponse>, (StatusCode, String)> {
    match state.chat.get_file_audit(&session_id, &query.path).await {
        Ok(Some((record, status))) => {
            Ok(Json(AuditStatusResponse { record: Some(record), status }))
        }
        Ok(None) => {
            Ok(Json(AuditStatusResponse { record: None, status: FileAuditStatus::Unaudited }))
        }
        Err(e) => Err(chat_error(e)),
    }
}

pub(crate) async fn api_get_classification(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Json<WorkspaceClassification> {
    Json(state.chat.get_workspace_classification(&session_id))
}

pub(crate) async fn api_set_classification_default(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<SetDefaultClassificationRequest>,
) -> StatusCode {
    state.chat.set_workspace_classification_default(&session_id, body.default);
    let _ = state.chat.persist_session_metadata(&session_id).await;
    StatusCode::NO_CONTENT
}

pub(crate) async fn api_set_classification_override(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<FilePathQuery>,
    Json(body): Json<SetOverrideRequest>,
) -> StatusCode {
    state.chat.set_classification_override(&session_id, &query.path, body.class);
    let _ = state.chat.persist_session_metadata(&session_id).await;
    StatusCode::NO_CONTENT
}

pub(crate) async fn api_clear_classification_override(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<FilePathQuery>,
) -> StatusCode {
    state.chat.clear_classification_override(&session_id, &query.path);
    let _ = state.chat.persist_session_metadata(&session_id).await;
    StatusCode::NO_CONTENT
}

// ── Workspace indexing ───────────────────────────────────────────────────

pub(crate) async fn api_workspace_index_status_stream(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Sse<impl futures_core::Stream<Item = Result<Event, Infallible>>>, (StatusCode, String)>
{
    let rx = state
        .chat
        .subscribe_index_status(&session_id)
        .await
        .ok_or((StatusCode::NOT_FOUND, "session not found".to_string()))?;

    let shutdown = state.shutdown.clone();
    let stream = async_stream::stream! {
        let mut rx = rx;
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

    Ok(Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("keep-alive"),
    ))
}

pub(crate) async fn api_workspace_indexed_files(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Json<Vec<String>> {
    Json(state.chat.indexed_files(&session_id).await)
}

pub(crate) async fn api_workspace_reindex_file(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<FilePathQuery>,
) -> StatusCode {
    state.chat.reindex_file(&session_id, &query.path).await;
    StatusCode::NO_CONTENT
}

// ── Tool approval / interaction ──────────────────────────────────────────

pub(crate) async fn api_chat_tool_approval(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(request): Json<ToolApprovalRequest>,
) -> Result<Json<ToolApprovalResponse>, (StatusCode, String)> {
    let response = UserInteractionResponse {
        request_id: request.request_id,
        payload: InteractionResponsePayload::ToolApproval {
            approved: request.approved,
            allow_session: request.allow_session,
            allow_agent: request.allow_agent,
        },
    };
    handle_chat_interaction_response(state, session_id, response).await
}

pub(crate) async fn api_chat_interaction_response(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(response): Json<UserInteractionResponse>,
) -> Result<Json<ToolApprovalResponse>, (StatusCode, String)> {
    handle_chat_interaction_response(state, session_id, response).await
}

async fn handle_chat_interaction_response(
    state: AppState,
    session_id: String,
    response: UserInteractionResponse,
) -> Result<Json<ToolApprovalResponse>, (StatusCode, String)> {
    // For the session-level route (no agent_id), both "allow for session"
    // and "allow for agent" grant a session-scoped permission rule.  The
    // old chat loop shares the session's permissions Arc directly, so the
    // rule is immediately visible to subsequent tool calls.
    let granted_scope = match &response.payload {
        InteractionResponsePayload::ToolApproval {
            approved, allow_session, allow_agent, ..
        } if *allow_session || *allow_agent => state
            .chat
            .grant_session_permission(&session_id, &response.request_id, *approved)
            .await
            .map_err(chat_error)?,
        _ => None,
    };

    let acknowledged =
        state.chat.respond_to_interaction(&session_id, response).await.map_err(chat_error)?;
    Ok(Json(ToolApprovalResponse { acknowledged, granted_scope }))
}

// ── Permissions ──────────────────────────────────────────────────────────

pub(crate) async fn get_session_permissions(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<hive_contracts::SessionPermissions>, (StatusCode, String)> {
    let perms = state.chat.get_permissions(&session_id).await.map_err(chat_error)?;
    Ok(Json(perms))
}

pub(crate) async fn update_session_permissions(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(permissions): Json<hive_contracts::SessionPermissions>,
) -> Result<StatusCode, (StatusCode, String)> {
    state.chat.set_permissions(&session_id, permissions).await.map_err(chat_error)?;
    Ok(StatusCode::NO_CONTENT)
}

// ── SSE stream ───────────────────────────────────────────────────────────

pub(crate) async fn api_chat_stream(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Sse<impl futures_core::Stream<Item = Result<Event, Infallible>>>, (StatusCode, String)>
{
    let rx = state.chat.subscribe_stream(&session_id).await.map_err(chat_error)?;

    let shutdown = state.shutdown.clone();
    let stream = async_stream::stream! {
        let mut rx = rx;
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

    Ok(Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("ping"),
    ))
}

// ── Interrupt / resume ───────────────────────────────────────────────────

pub(crate) async fn interrupt_chat_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(request): Json<InterruptRequest>,
) -> Result<Json<ChatSessionSnapshot>, (StatusCode, String)> {
    state.chat.interrupt_session(&session_id, request.mode).await.map(Json).map_err(chat_error)
}

pub(crate) async fn resume_chat_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<ChatSessionSnapshot>, (StatusCode, String)> {
    state.chat.resume_session(&session_id).await.map(Json).map_err(chat_error)
}

// ── Memory / risk scans ──────────────────────────────────────────────────

pub(crate) async fn get_chat_session_memory(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<MemoryQuery>,
) -> Result<Json<Vec<ChatMemoryItem>>, (StatusCode, String)> {
    let limit = clamp_limit(query.limit, 20);
    state.chat.get_session_memory(&session_id, limit).await.map(Json).map_err(chat_error)
}

pub(crate) async fn search_memory(
    State(state): State<AppState>,
    Query(query): Query<MemoryQuery>,
) -> Result<Json<Vec<ChatMemoryItem>>, (StatusCode, String)> {
    let limit = clamp_limit(query.limit, 10);
    state
        .chat
        .search_memory(query.query.as_deref().unwrap_or_default(), DataClass::Restricted, limit)
        .await
        .map(Json)
        .map_err(chat_error)
}

pub(crate) async fn list_risk_scans(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<MemoryQuery>,
) -> Result<Json<Vec<RiskScanRecord>>, (StatusCode, String)> {
    let limit = clamp_limit(query.limit, 20);
    state.chat.get_risk_scans(&session_id, limit).await.map(Json).map_err(chat_error)
}

// ── Pending approvals / questions ────────────────────────────────────────

#[derive(Serialize)]
pub(crate) struct PendingApprovalItem {
    session_id: String,
    agent_id: String,
    agent_name: String,
    request_id: String,
    tool_id: String,
    input: String,
    reason: String,
}

#[derive(Serialize)]
pub(crate) struct PendingQuestionItem {
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<String>,
    agent_id: String,
    agent_name: String,
    request_id: String,
    text: String,
    choices: Vec<String>,
    allow_freeform: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    workflow_instance_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    workflow_step_id: Option<String>,
    /// "session" or "bot" — tells the frontend which Tauri command to use.
    routing: String,
}

pub(crate) async fn api_pending_approvals(
    State(state): State<AppState>,
) -> Json<Vec<PendingApprovalItem>> {
    let raw = state.chat.list_all_pending_approvals().await;
    Json(
        raw.into_iter()
            .map(|(session_id, a)| PendingApprovalItem {
                session_id,
                agent_id: a.agent_id,
                agent_name: a.agent_name,
                request_id: a.request_id,
                tool_id: a.tool_id,
                input: a.input,
                reason: a.reason,
            })
            .collect(),
    )
}

pub(crate) async fn api_pending_questions(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Json<Vec<PendingQuestionItem>> {
    let is_bot_session = session_id == "__bot__"
        || session_id == "__service__"
        || session_id.starts_with("trigger-");
    let routing_label = if is_bot_session { "bot" } else { "session" };
    // Agent questions
    let raw = state.chat.list_pending_questions_for_session(&session_id).await;
    let mut items: Vec<PendingQuestionItem> = raw
        .into_iter()
        .map(|q| PendingQuestionItem {
            session_id: Some(session_id.clone()),
            agent_id: q.agent_id,
            agent_name: q.agent_name,
            request_id: q.request_id,
            text: q.text,
            choices: q.choices,
            allow_freeform: q.allow_freeform,
            message: q.message,
            workflow_instance_id: None,
            workflow_step_id: None,
            routing: routing_label.to_string(),
        })
        .collect();

    // Workflow feedback gates owned by this session
    if let Ok(wf_items) = state.workflows.list_waiting_feedback_for_session(&session_id).await {
        for wf in wf_items {
            items.push(PendingQuestionItem {
                session_id: Some(session_id.clone()),
                agent_id: String::new(),
                agent_name: format!("Workflow: {}", wf.definition_name),
                request_id: format!("wf:{}:{}", wf.instance_id, wf.step_id),
                text: wf.prompt,
                choices: wf.choices,
                allow_freeform: wf.allow_freeform,
                message: None,
                workflow_instance_id: Some(wf.instance_id),
                workflow_step_id: Some(wf.step_id),
                routing: "gate".to_string(),
            });
        }
    }

    Json(items)
}

/// Global pending questions across all sessions and bots (for FlightDeck polling).
pub(crate) async fn api_all_pending_questions(
    State(state): State<AppState>,
) -> Json<Vec<PendingQuestionItem>> {
    let raw = state.chat.list_all_pending_questions().await;
    Json(
        raw.into_iter()
            .map(|(session_id, q)| {
                let is_bot = session_id == "__bot__"
                    || session_id == "__service__"
                    || session_id.starts_with("trigger-");
                PendingQuestionItem {
                    session_id: Some(session_id),
                    agent_id: q.agent_id,
                    agent_name: q.agent_name,
                    request_id: q.request_id,
                    text: q.text,
                    choices: q.choices,
                    allow_freeform: q.allow_freeform,
                    message: q.message,
                    workflow_instance_id: None,
                    workflow_step_id: None,
                    routing: if is_bot { "bot".to_string() } else { "session".to_string() },
                }
            })
            .collect(),
    )
}

// ── User status (AFK) ───────────────────────────────────────────────────

pub(crate) async fn api_get_user_status(State(state): State<AppState>) -> Json<Value> {
    let status = state.user_status.get();
    Json(json!({ "status": status }))
}

pub(crate) async fn api_set_user_status(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let status: hive_contracts::config::UserStatus =
        serde_json::from_value(body.get("status").cloned().unwrap_or(Value::Null))
            .map_err(|e| (StatusCode::BAD_REQUEST, format!("invalid status: {e}")))?;
    state.user_status.set(status);
    Ok(Json(json!({ "status": status })))
}

pub(crate) async fn api_status_heartbeat(State(state): State<AppState>) -> Json<Value> {
    let status = state.user_status.heartbeat();
    // Also run auto-transition check based on config
    let config = state.config.load();
    state.user_status.check_auto_transitions(&config.afk);
    Json(json!({ "status": status }))
}

pub(crate) async fn api_status_event_stream(
    State(state): State<AppState>,
) -> Sse<impl futures_core::Stream<Item = Result<Event, Infallible>>> {
    let mut rx = state.user_status.subscribe();
    let current = state.user_status.get();
    let shutdown = state.shutdown.clone();
    let stream = async_stream::stream! {
        // Send current status as initial event.
        yield Ok(Event::default().data(
            serde_json::to_string(&json!({ "status": current })).unwrap_or_default()
        ));
        loop {
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => break,
                result = rx.recv() => {
                    match result {
                        Ok(status) => {
                            yield Ok(Event::default().data(
                                serde_json::to_string(&json!({ "status": status })).unwrap_or_default()
                            ));
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

pub(crate) async fn api_approval_event_stream(
    State(state): State<AppState>,
) -> Sse<impl futures_core::Stream<Item = Result<Event, Infallible>>> {
    let mut rx = state.chat.subscribe_approvals();
    let initial = state.chat.list_all_pending_approvals().await;
    tracing::debug!(count = initial.len(), "approval SSE: sending initial batch");
    let shutdown = state.shutdown.clone();
    let stream = async_stream::stream! {
        for (session_id, a) in initial {
            let ev = crate::chat::ApprovalStreamEvent::Added {
                session_id,
                agent_id: a.agent_id,
                agent_name: a.agent_name,
                request_id: a.request_id,
                tool_id: a.tool_id,
                input: a.input,
                reason: a.reason,
            };
            let data = serde_json::to_string(&ev).unwrap_or_default();
            yield Ok(Event::default().data(data));
        }
        tracing::debug!("approval SSE: streaming live events");
        loop {
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => break,
                result = rx.recv() => {
                    match result {
                        Ok(event) => {
                            tracing::debug!(?event, "approval SSE: forwarding live event");
                            let data = serde_json::to_string(&event).unwrap_or_default();
                            yield Ok(Event::default().data(data));
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            tracing::debug!("approval SSE: broadcast channel closed");
                            break;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!(skipped = n, "approval SSE: lagged behind");
                            continue;
                        }
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
