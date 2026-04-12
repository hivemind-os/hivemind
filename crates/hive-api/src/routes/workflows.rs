use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::sse::{Event, Sse},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::convert::Infallible;

use crate::{chat_error, workflow_error, AppState};

// ── Types ────────────────────────────────────────────────────────────────

#[derive(Deserialize, Default)]
pub(crate) struct WfDefinitionFilterQuery {
    mode: Option<String>,
    #[serde(default)]
    include_archived: bool,
}

#[derive(Deserialize)]
pub(crate) struct WfSaveDefinitionBody {
    yaml: String,
}

#[derive(Serialize)]
pub(crate) struct WfDefinitionResponse {
    definition: hive_workflow::WorkflowDefinition,
    yaml: String,
}

#[derive(Deserialize)]
pub(crate) struct WfLaunchRequest {
    #[serde(default)]
    definition: Option<String>,
    #[serde(default)]
    definition_id: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    inputs: serde_json::Value,
    parent_session_id: String,
    #[serde(default)]
    parent_agent_id: Option<String>,
    #[serde(default)]
    permissions: Option<Vec<hive_workflow::PermissionEntry>>,
    #[serde(default)]
    trigger_step_id: Option<String>,
    #[serde(default)]
    workspace_path: Option<String>,
}

#[derive(Deserialize, Default)]
pub(crate) struct WfInstanceFilterQuery {
    status: Option<String>,
    definition: Option<String>,
    session_id: Option<String>,
    agent_id: Option<String>,
    limit: Option<usize>,
    offset: Option<usize>,
    mode: Option<String>,
    include_archived: Option<bool>,
}

#[derive(Deserialize)]
pub(crate) struct WfGateResponse {
    response: serde_json::Value,
}

/// Archive (hide) or restore a workflow definition.
#[derive(Deserialize)]
pub(crate) struct WfArchiveBody {
    #[serde(default = "default_archive_true")]
    archived: bool,
}

fn default_archive_true() -> bool {
    true
}

/// Pause or resume auto-triggers for a workflow definition.
#[derive(Deserialize)]
pub(crate) struct WfTriggersPausedBody {
    #[serde(default = "default_archive_true")]
    paused: bool,
}

#[derive(Deserialize)]
pub(crate) struct WfAiAssistRequest {
    #[serde(default)]
    yaml: String,
    prompt: String,
    #[serde(default)]
    agent_id: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct WfAiAssistResponse {
    agent_id: String,
}

#[derive(Deserialize)]
pub(crate) struct WfUploadAttachmentBody {
    filename: String,
    description: String,
    #[serde(default)]
    media_type: Option<String>,
    content: String,
}

#[derive(Deserialize)]
pub(crate) struct WfAttachmentPath {
    workflow_id: String,
    version: String,
}

#[derive(Deserialize)]
pub(crate) struct WfAttachmentDeletePath {
    workflow_id: String,
    version: String,
    attachment_id: String,
}

#[derive(Deserialize)]
pub(crate) struct WfCopyAttachmentsPath {
    workflow_id: String,
    from_version: String,
    to_version: String,
}

#[derive(Deserialize)]
pub(crate) struct WfCopyDefinitionRequest {
    source_name: String,
    #[serde(default)]
    source_version: Option<String>,
    new_name: String,
}

// ── Definition handlers ──────────────────────────────────────────────────

pub(crate) async fn wf_list_definitions(
    State(state): State<AppState>,
    Query(filter): Query<WfDefinitionFilterQuery>,
) -> Result<Json<Vec<hive_workflow::WorkflowDefinitionSummary>>, (StatusCode, String)> {
    let mut defs = state.workflows.list_definitions().await.map_err(workflow_error)?;
    if let Some(ref mode_str) = filter.mode {
        let mode: hive_workflow::WorkflowMode =
            serde_json::from_value(serde_json::Value::String(mode_str.clone())).unwrap_or_default();
        defs.retain(|d| d.mode == mode);
    }
    if !filter.include_archived {
        defs.retain(|d| !d.archived);
    }
    Ok(Json(defs))
}

pub(crate) async fn wf_save_definition(
    State(state): State<AppState>,
    Json(body): Json<WfSaveDefinitionBody>,
) -> Result<(StatusCode, Json<hive_workflow::WorkflowDefinition>), (StatusCode, String)> {
    let def = state.workflows.save_definition(&body.yaml).await.map_err(workflow_error)?;

    state.trigger_manager.register_definition(&def).await;

    let _ = state.event_bus.publish(
        "workflow.definition.saved",
        "hive-workflow",
        json!({ "name": def.name, "version": def.version }),
    );

    Ok((StatusCode::CREATED, Json(def)))
}

pub(crate) async fn wf_copy_definition(
    State(state): State<AppState>,
    Json(body): Json<WfCopyDefinitionRequest>,
) -> Result<Json<hive_workflow::WorkflowDefinition>, (StatusCode, String)> {
    let def = state
        .workflows
        .copy_definition(&body.source_name, body.source_version.as_deref(), &body.new_name)
        .await
        .map_err(workflow_error)?;
    Ok(Json(def))
}

pub(crate) async fn wf_get_latest_definition(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<WfDefinitionResponse>, (StatusCode, String)> {
    let (def, yaml) = state.workflows.get_latest_definition(&name).await.map_err(workflow_error)?;
    Ok(Json(WfDefinitionResponse { definition: def, yaml }))
}

pub(crate) async fn wf_get_definition(
    State(state): State<AppState>,
    Path((name, version)): Path<(String, String)>,
) -> Result<Json<WfDefinitionResponse>, (StatusCode, String)> {
    let (def, yaml) =
        state.workflows.get_definition(&name, &version).await.map_err(workflow_error)?;
    Ok(Json(WfDefinitionResponse { definition: def, yaml }))
}

pub(crate) async fn wf_get_definition_by_id(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<WfDefinitionResponse>, (StatusCode, String)> {
    let (def, yaml) = state.workflows.get_definition_by_id(&id).await.map_err(workflow_error)?;
    Ok(Json(WfDefinitionResponse { definition: def, yaml }))
}

pub(crate) async fn wf_delete_definition(
    State(state): State<AppState>,
    Path((name, version)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    if hive_core::is_bundled_workflow(&name) {
        return Err((
            StatusCode::FORBIDDEN,
            format!("Cannot delete bundled workflow '{name}'. Use archive (hide) instead."),
        ));
    }

    let linked_tasks = state.scheduler.list_tasks_for_workflow(&name).unwrap_or_default();
    for task in &linked_tasks {
        let _ = state.scheduler.delete_task(&task.id);
    }

    let definition_id = match state.workflows.get_definition(&name, &version).await {
        Ok((def, _yaml)) => Some(def.id),
        Err(e) => {
            tracing::warn!(
                name = %name,
                version = %version,
                "failed to resolve definition id for trigger unregister: {e}"
            );
            None
        }
    };
    if let Some(definition_id) = definition_id.as_deref() {
        state.trigger_manager.unregister_definition(definition_id, Some(&version)).await;
    }

    let deleted =
        state.workflows.delete_definition(&name, &version).await.map_err(workflow_error)?;

    if deleted {
        let _ = state.event_bus.publish(
            "workflow.definition.deleted",
            "hive-workflow",
            json!({ "name": name, "version": version }),
        );
    }

    Ok(Json(json!({ "deleted": deleted, "deleted_tasks": linked_tasks.len() })))
}

/// Reset a bundled workflow to its factory YAML.
pub(crate) async fn wf_reset_definition(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<hive_workflow::WorkflowDefinition>, (StatusCode, String)> {
    let def =
        state.workflows.reset_bundled_workflow(&name).await.map_err(workflow_error)?.ok_or_else(
            || {
                (
                    StatusCode::NOT_FOUND,
                    format!("Workflow '{name}' is not a bundled workflow and cannot be reset."),
                )
            },
        )?;

    state.trigger_manager.register_definition(&def).await;

    Ok(Json(def))
}

pub(crate) async fn wf_archive_definition(
    State(state): State<AppState>,
    Path((name, version)): Path<(String, String)>,
    body: Option<Json<WfArchiveBody>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let archived = body.map(|b| b.archived).unwrap_or(true);
    let updated = state
        .workflows
        .archive_definition(&name, &version, archived)
        .await
        .map_err(workflow_error)?;

    if updated {
        match state.workflows.get_definition(&name, &version).await {
            Ok((def, _yaml)) => {
                if archived {
                    // Unregister triggers when archiving a definition.
                    state.trigger_manager.unregister_definition(&def.id, Some(&def.version)).await;
                } else {
                    // Re-register triggers when un-archiving.
                    state.trigger_manager.register_definition(&def).await;
                }
            }
            Err(e) => {
                tracing::warn!(
                    name = %name,
                    version = %version,
                    "failed to refresh trigger registration after archive toggle: {e}"
                );
            }
        }

        let _ = state.event_bus.publish(
            "workflow.definition.saved",
            "hive-workflow",
            json!({ "name": name, "version": version, "archived": archived }),
        );
    }

    Ok(Json(json!({ "name": name, "version": version, "archived": archived })))
}

/// Pause or resume auto-triggers for a workflow definition.
pub(crate) async fn wf_set_triggers_paused(
    State(state): State<AppState>,
    Path((name, version)): Path<(String, String)>,
    body: Option<Json<WfTriggersPausedBody>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let paused = body.map(|b| b.paused).unwrap_or(true);
    let updated = state
        .workflows
        .set_triggers_paused(&name, &version, paused)
        .await
        .map_err(workflow_error)?;

    if updated {
        match state.workflows.get_definition(&name, &version).await {
            Ok((def, _yaml)) => {
                if paused {
                    state.trigger_manager.unregister_definition(&def.id, Some(&def.version)).await;
                } else {
                    state.trigger_manager.register_definition(&def).await;
                }
            }
            Err(e) => {
                tracing::warn!(name = %name, version = %version,
                    "failed to refresh trigger registration after triggers_paused toggle: {e}");
            }
        }

        let _ = state.event_bus.publish(
            "workflow.definition.saved",
            "hive-workflow",
            json!({ "name": name, "version": version, "triggers_paused": paused }),
        );
    }

    Ok(Json(json!({ "name": name, "version": version, "triggers_paused": paused })))
}

/// Returns active triggers and scheduler tasks that depend on a workflow definition.
pub(crate) async fn wf_check_definition_dependents(
    State(state): State<AppState>,
    Path((name, version)): Path<(String, String)>,
) -> Json<serde_json::Value> {
    let all_triggers = state.trigger_manager.list_active().await;
    let triggers: Vec<_> = all_triggers
        .triggers
        .into_iter()
        .filter(|t| t.definition_name == name && t.definition_version == version)
        .collect();

    let tasks: Vec<_> = state
        .scheduler
        .list_tasks_for_workflow(&name)
        .unwrap_or_default()
        .into_iter()
        .map(|t| json!({ "id": t.id, "name": t.name, "status": t.status }))
        .collect();

    Json(json!({
        "triggers": triggers,
        "scheduled_tasks": tasks,
    }))
}

// ── Instance handlers ────────────────────────────────────────────────────

pub(crate) async fn wf_launch_instance(
    State(state): State<AppState>,
    Json(req): Json<WfLaunchRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, String)> {
    let def_name = if let Some(ref id) = req.definition_id {
        let (def, _yaml) =
            state.workflows.get_definition_by_id(id).await.map_err(workflow_error)?;
        def.name
    } else if let Some(ref name) = req.definition {
        name.clone()
    } else {
        return Err((
            StatusCode::BAD_REQUEST,
            "Either 'definition' (name) or 'definition_id' must be provided".to_string(),
        ));
    };

    let instance_id = state
        .workflows
        .launch(
            &def_name,
            req.version.as_deref(),
            req.inputs,
            &req.parent_session_id,
            req.parent_agent_id.as_deref(),
            req.permissions,
            req.trigger_step_id.as_deref(),
            req.workspace_path.as_deref(),
        )
        .await
        .map_err(workflow_error)?;
    Ok((StatusCode::CREATED, Json(json!({ "instance_id": instance_id }))))
}

pub(crate) async fn wf_list_instances(
    State(state): State<AppState>,
    Query(filter): Query<WfInstanceFilterQuery>,
) -> Result<Json<hive_workflow::InstanceListResult>, (StatusCode, String)> {
    let statuses: Vec<hive_workflow::WorkflowStatus> = filter
        .status
        .as_deref()
        .unwrap_or("")
        .split(',')
        .filter(|s| !s.is_empty())
        .filter_map(|s| serde_json::from_value(serde_json::Value::String(s.to_string())).ok())
        .collect();
    let definition_names: Vec<String> = filter
        .definition
        .as_deref()
        .unwrap_or("")
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();
    let f = hive_workflow::InstanceFilter {
        statuses,
        definition_names,
        definition_id: None,
        parent_session_id: filter.session_id,
        parent_agent_id: filter.agent_id,
        limit: filter.limit,
        offset: filter.offset,
        mode: filter
            .mode
            .as_deref()
            .and_then(|m| serde_json::from_value(serde_json::Value::String(m.to_string())).ok()),
        include_archived: filter.include_archived.unwrap_or(false),
    };
    let mut result = state.workflows.list_instances(&f).await.map_err(workflow_error)?;

    enrich_workflow_summaries(&state, &mut result.items).await;

    Ok(Json(result))
}

/// Enrich workflow instance summaries with pending agent approval/question
/// counts.
async fn enrich_workflow_summaries(
    state: &AppState,
    items: &mut [hive_workflow::WorkflowInstanceSummary],
) {
    if items.is_empty() {
        return;
    }

    let child_agents = state.workflows.list_child_agent_ids().await.unwrap_or_default();

    let mut agent_to_instance: std::collections::HashMap<&str, i64> =
        std::collections::HashMap::new();
    for (&instance_id, agent_ids) in &child_agents {
        for agent_id in agent_ids {
            agent_to_instance.insert(agent_id, instance_id);
        }
    }

    let pending_approvals = state.chat.list_all_pending_approvals().await;
    let pending_questions = state.chat.list_all_pending_questions().await;

    let mut approval_counts: std::collections::HashMap<i64, usize> =
        std::collections::HashMap::new();
    let mut question_counts: std::collections::HashMap<i64, usize> =
        std::collections::HashMap::new();

    for (_session_id, approval) in &pending_approvals {
        if let Some(&instance_id) = agent_to_instance.get(approval.agent_id.as_str()) {
            *approval_counts.entry(instance_id).or_default() += 1;
        }
    }
    for (_session_id, question) in &pending_questions {
        if let Some(&instance_id) = agent_to_instance.get(question.agent_id.as_str()) {
            *question_counts.entry(instance_id).or_default() += 1;
        }
    }

    for item in items.iter_mut() {
        if let Some(&count) = approval_counts.get(&item.id) {
            item.pending_agent_approvals = count;
        }
        if let Some(&count) = question_counts.get(&item.id) {
            item.pending_agent_questions = count;
        }
        if let Some(ids) = child_agents.get(&item.id) {
            item.child_agent_ids = ids.clone();
        }
    }
}

pub(crate) async fn wf_get_instance(
    State(state): State<AppState>,
    Path(instance_id): Path<i64>,
) -> Result<Json<hive_workflow::WorkflowInstance>, (StatusCode, String)> {
    state.workflows.get_instance(instance_id).await.map(Json).map_err(workflow_error)
}

pub(crate) async fn wf_delete_instance(
    State(state): State<AppState>,
    Path(instance_id): Path<i64>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let deleted = state.workflows.delete_instance(instance_id).await.map_err(workflow_error)?;
    Ok(Json(json!({ "deleted": deleted })))
}

pub(crate) async fn wf_archive_instance(
    State(state): State<AppState>,
    Path(instance_id): Path<i64>,
    body: Option<Json<WfArchiveBody>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let archived = body.map(|b| b.archived).unwrap_or(true);
    let updated =
        state.workflows.archive_instance(instance_id, archived).await.map_err(workflow_error)?;
    Ok(Json(json!({ "instance_id": instance_id, "archived": archived, "updated": updated })))
}

pub(crate) async fn wf_pause_instance(
    State(state): State<AppState>,
    Path(instance_id): Path<i64>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    state.workflows.pause(instance_id).await.map_err(workflow_error)?;
    Ok(Json(json!({ "ok": true })))
}

pub(crate) async fn wf_resume_instance(
    State(state): State<AppState>,
    Path(instance_id): Path<i64>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    state.workflows.resume(instance_id).await.map_err(workflow_error)?;
    Ok(Json(json!({ "ok": true })))
}

pub(crate) async fn wf_kill_instance(
    State(state): State<AppState>,
    Path(instance_id): Path<i64>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    state.workflows.kill(instance_id).await.map_err(workflow_error)?;
    Ok(Json(json!({ "ok": true })))
}

pub(crate) async fn wf_update_permissions(
    State(state): State<AppState>,
    Path(instance_id): Path<i64>,
    Json(permissions): Json<Vec<hive_workflow::PermissionEntry>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    state.workflows.update_permissions(instance_id, permissions).await.map_err(workflow_error)?;
    Ok(Json(json!({ "ok": true })))
}

pub(crate) async fn wf_respond_to_gate(
    State(state): State<AppState>,
    Path((instance_id, step_id)): Path<(i64, String)>,
    Json(body): Json<WfGateResponse>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    state
        .workflows
        .respond_to_gate(instance_id, &step_id, body.response)
        .await
        .map_err(workflow_error)?;
    Ok(Json(json!({ "ok": true })))
}

// ── Event stream / topics / triggers ─────────────────────────────────────

/// SSE stream for all workflow events.
pub(crate) async fn wf_event_stream(
    State(state): State<AppState>,
) -> Sse<impl futures_core::Stream<Item = Result<Event, Infallible>>> {
    let mut rx = state.event_bus.subscribe_queued_bounded("workflow", 10_000);

    let stream = async_stream::stream! {
        while let Some(envelope) = rx.recv().await {
            let data = serde_json::to_string(&serde_json::json!({
                "topic": envelope.topic,
                "payload": envelope.payload,
                "timestamp_ms": envelope.timestamp_ms,
            }))
            .unwrap_or_default();
            yield Ok(Event::default()
                .event(&envelope.topic)
                .data(data));
        }
    };

    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("keep-alive"),
    )
}

/// Returns the list of known event topics.
pub(crate) async fn wf_list_topics(State(state): State<AppState>) -> Json<serde_json::Value> {
    let mut topics: Vec<serde_json::Value> = vec![
        json!({ "topic": "chat.session.created", "description": "A new chat session was created", "payload_keys": ["sessionId"] }),
        json!({ "topic": "chat.session.activity", "description": "Activity detected in a chat session", "payload_keys": ["sessionId", "stage", "intent"] }),
        json!({ "topic": "chat.session.resumed", "description": "A chat session was resumed", "payload_keys": ["sessionId"] }),
        json!({ "topic": "chat.session.interrupt_requested", "description": "An interrupt was requested for a session", "payload_keys": ["sessionId", "mode"] }),
        json!({ "topic": "chat.session.interrupted", "description": "A chat session was interrupted", "payload_keys": ["sessionId", "messageId"] }),
        json!({ "topic": "chat.message.queued", "description": "A chat message was queued for processing", "payload_keys": ["sessionId", "messageId"] }),
        json!({ "topic": "chat.message.completed", "description": "A chat message finished processing", "payload_keys": ["sessionId", "messageId"] }),
        json!({ "topic": "chat.message.failed", "description": "A chat message processing failed", "payload_keys": ["sessionId", "messageId", "error"] }),
        json!({ "topic": "tool.invoked", "description": "A tool was invoked by an agent", "payload_keys": ["toolId", "dataClass"] }),
        json!({ "topic": "workflow.definition.saved", "description": "A workflow definition was saved", "payload_keys": ["name", "version"] }),
        json!({ "topic": "workflow.definition.deleted", "description": "A workflow definition was deleted", "payload_keys": ["name", "version"] }),
        json!({ "topic": "workflow.interaction.requested", "description": "A workflow step requested user interaction", "payload_keys": ["instance_id", "step_id", "prompt", "choices"] }),
        json!({ "topic": "config.reloaded", "description": "Application configuration was reloaded", "payload_keys": ["personas_dir", "config_path"] }),
        json!({ "topic": "config.validated", "description": "Configuration was validated", "payload_keys": ["bind"] }),
        json!({ "topic": "config.channels_reloaded", "description": "Communication connectors were reloaded", "payload_keys": ["connectors_path"] }),
        json!({ "topic": "scheduler.task.completed", "description": "A scheduled task completed (use scheduler.task.completed.* for specific)", "payload_keys": [] }),
        json!({ "topic": "mcp.notification", "description": "An MCP server sent a notification", "payload_keys": ["serverId"] }),
        json!({ "topic": "mcp.server.connected", "description": "An MCP server connected", "payload_keys": ["serverId"] }),
        json!({ "topic": "mcp.server.disconnected", "description": "An MCP server disconnected", "payload_keys": ["serverId"] }),
        json!({ "topic": "mcp.server.error", "description": "An MCP server encountered an error", "payload_keys": ["serverId", "error"] }),
        json!({ "topic": "mcp.server.removed", "description": "An MCP server was removed", "payload_keys": ["serverId"] }),
        json!({ "topic": "workflow.trigger.fired", "description": "A workflow trigger fired", "payload_keys": ["definition", "instance_id"] }),
        json!({ "topic": "daemon.shutdown_requested", "description": "Daemon shutdown was requested", "payload_keys": ["requested_by"] }),
    ];

    if let Some(ref connector_svc) = state.connectors {
        let registry = connector_svc.registry();
        for connector in registry.list() {
            topics.push(json!({
                "topic": format!("comm.message.received.{}", connector.id()),
                "description": format!("Incoming message on connector '{}'", connector.id()),
                "payload_keys": ["channel_id", "provider", "external_id", "from", "to", "subject", "body", "timestamp_ms", "metadata"],
                "dynamic": true
            }));
        }
    }

    Json(json!({ "topics": topics }))
}

/// Returns the list of active trigger registrations and event gates.
pub(crate) async fn wf_list_active_triggers(
    State(state): State<AppState>,
) -> Json<hive_workflow_service::ActiveTriggersResponse> {
    Json(state.trigger_manager.list_active().await)
}

/// Launch a one-shot workflow AI authoring assistant agent.
pub(crate) async fn wf_ai_assist(
    State(state): State<AppState>,
    Json(body): Json<WfAiAssistRequest>,
) -> Result<Json<WfAiAssistResponse>, (StatusCode, String)> {
    if let Some(ref existing_id) = body.agent_id {
        if let Ok(()) =
            state.chat.continue_workflow_ai_assist(existing_id, &body.yaml, &body.prompt).await
        {
            return Ok(Json(WfAiAssistResponse { agent_id: existing_id.clone() }));
        }
    }

    let agent_id =
        state.chat.launch_workflow_ai_assist(&body.yaml, &body.prompt).await.map_err(chat_error)?;
    Ok(Json(WfAiAssistResponse { agent_id }))
}

// ── Attachment handlers ──────────────────────────────────────────────────

pub(crate) async fn wf_upload_attachment(
    State(state): State<AppState>,
    Path(path): Path<WfAttachmentPath>,
    Json(body): Json<WfUploadAttachmentBody>,
) -> Result<(StatusCode, Json<hive_workflow::WorkflowAttachment>), (StatusCode, String)> {
    use base64::Engine;

    let data = base64::engine::general_purpose::STANDARD
        .decode(&body.content)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("invalid base64: {e}")))?;
    if data.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "file content cannot be empty".into()));
    }

    let attachment_id = uuid::Uuid::new_v4().to_string();
    let size_bytes = data.len() as u64;

    state
        .workflows
        .upload_attachment(&path.workflow_id, &path.version, &attachment_id, &body.filename, &data)
        .map_err(workflow_error)?;

    let attachment = hive_workflow::WorkflowAttachment {
        id: attachment_id,
        filename: body.filename,
        description: body.description,
        media_type: body.media_type,
        size_bytes: Some(size_bytes),
    };

    Ok((StatusCode::CREATED, Json(attachment)))
}

pub(crate) async fn wf_list_attachments(
    State(state): State<AppState>,
    Path(path): Path<WfAttachmentPath>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let dir = state.workflows.attachments_dir(&path.workflow_id, &path.version);
    let exists = dir.exists();
    Ok(Json(json!({
        "workflow_id": path.workflow_id,
        "version": path.version,
        "directory": dir.to_string_lossy(),
        "exists": exists,
    })))
}

pub(crate) async fn wf_delete_attachment(
    State(state): State<AppState>,
    Path(path): Path<WfAttachmentDeletePath>,
) -> Result<StatusCode, (StatusCode, String)> {
    let dir = state.workflows.attachments_dir(&path.workflow_id, &path.version);
    if dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with(&format!("{}_", path.attachment_id)) {
                    std::fs::remove_file(entry.path())
                        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
                    return Ok(StatusCode::NO_CONTENT);
                }
            }
        }
    }
    Err((StatusCode::NOT_FOUND, "attachment not found".into()))
}

pub(crate) async fn wf_copy_attachments(
    State(state): State<AppState>,
    Path(path): Path<WfCopyAttachmentsPath>,
) -> Result<StatusCode, (StatusCode, String)> {
    state
        .workflows
        .copy_attachments(&path.workflow_id, &path.from_version, &path.to_version)
        .map_err(workflow_error)?;
    Ok(StatusCode::OK)
}
