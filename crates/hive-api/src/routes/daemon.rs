use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::sse::{Event, Sse},
    Json,
};
use serde::Deserialize;
use serde_json::json;

use crate::{
    open_kg, AppState, HealthResponse, SessionTelemetryEntry, ShutdownResponse, StatusResponse,
    SystemHealthSnapshot,
};
use hive_agents::{AgentSummary, TelemetrySnapshot};
use hive_classification::DataClass;
use hive_core::NewAuditEntry;
use std::collections::HashMap;
use std::sync::Arc;

// ── Health / status / shutdown ────────────────────────────────────────────

pub(crate) async fn healthz() -> Json<HealthResponse> {
    Json(HealthResponse { ok: true })
}

pub(crate) async fn status(State(state): State<AppState>) -> Json<StatusResponse> {
    let cfg = state.config.load();
    Json(StatusResponse {
        version: env!("CARGO_PKG_VERSION").to_string(),
        uptime_secs: state.start_time.elapsed().as_secs_f64(),
        pid: std::process::id(),
        platform: std::env::consts::OS.to_string(),
        bind: cfg.api.bind.clone(),
    })
}

pub(crate) async fn shutdown(State(state): State<AppState>) -> Json<ShutdownResponse> {
    let _ = state.audit.append(NewAuditEntry::new(
        "api",
        "daemon.shutdown",
        "daemon",
        DataClass::Internal,
        "shutdown requested through local api",
        "accepted",
    ));

    if let Err(e) = state.event_bus.publish(
        "daemon.shutdown_requested",
        "hive-api",
        json!({ "requested_by": "local-api" }),
    ) {
        tracing::warn!(error = %e, "failed to publish shutdown event");
    }

    state.shutdown.cancel();
    Json(ShutdownResponse { message: "Daemon shutting down".to_string() })
}

// ── Flight Deck ──────────────────────────────────────────────────────────

pub(crate) async fn api_system_health(
    State(state): State<AppState>,
) -> Result<Json<SystemHealthSnapshot>, (StatusCode, String)> {
    let sessions = state.chat.list_sessions().await;
    let active_session_count = sessions.len();

    let all_agents = state.chat.list_all_agents().await;
    let active_agent_count = all_agents
        .iter()
        .filter(|a| {
            !matches!(a.status, hive_agents::AgentStatus::Done | hive_agents::AgentStatus::Error)
        })
        .count();

    let mcp_servers = state.mcp.list_servers().await;
    let mcp_total_count = mcp_servers.len();
    let mcp_connected_count = mcp_servers
        .iter()
        .filter(|s| s.status == hive_contracts::McpConnectionStatus::Connected)
        .count();

    let active_workflow_count = state
        .workflows
        .list_instances(&hive_workflow::InstanceFilter {
            statuses: vec![
                hive_workflow::WorkflowStatus::Running,
                hive_workflow::WorkflowStatus::Paused,
            ],
            ..Default::default()
        })
        .await
        .map(|r| r.total)
        .unwrap_or(0);

    // Aggregate telemetry across all sessions + bots.
    let mut total_llm_calls: u32 = 0;
    let mut total_input_tokens: u64 = 0;
    let mut total_output_tokens: u64 = 0;
    for (_, telem) in state.chat.all_sessions_telemetry().await {
        total_llm_calls = total_llm_calls.saturating_add(telem.total.model_calls);
        total_input_tokens = total_input_tokens.saturating_add(telem.total.input_tokens);
        total_output_tokens = total_output_tokens.saturating_add(telem.total.output_tokens);
    }
    if let Ok(bot_telem) = state.chat.bot_telemetry().await {
        total_llm_calls = total_llm_calls.saturating_add(bot_telem.total.model_calls);
        total_input_tokens = total_input_tokens.saturating_add(bot_telem.total.input_tokens);
        total_output_tokens = total_output_tokens.saturating_add(bot_telem.total.output_tokens);
    }

    // Knowledge graph stats (blocking because it opens SQLite).
    let kg_path = Arc::clone(&state.knowledge_graph_path);
    let (knowledge_node_count, knowledge_edge_count) = tokio::task::spawn_blocking(move || {
        if let Ok(graph) = open_kg(&kg_path) {
            let n = graph.node_count().unwrap_or(0);
            let e = graph.edge_count().unwrap_or(0);
            (n, e)
        } else {
            (0, 0)
        }
    })
    .await
    .unwrap_or((0, 0));

    let local_model_count =
        state.local_models.as_ref().map(|lm| lm.list_models().len()).unwrap_or(0);
    let loaded_model_count =
        state.runtime_manager.as_ref().map(|rm| rm.loaded_model_statuses().len()).unwrap_or(0);

    Ok(Json(SystemHealthSnapshot {
        version: env!("CARGO_PKG_VERSION").to_string(),
        uptime_secs: state.start_time.elapsed().as_secs_f64(),
        pid: std::process::id(),
        platform: std::env::consts::OS.to_string(),
        active_session_count,
        active_agent_count,
        active_workflow_count,
        mcp_connected_count,
        mcp_total_count,
        total_llm_calls,
        total_input_tokens,
        total_output_tokens,
        knowledge_node_count,
        knowledge_edge_count,
        local_model_count,
        loaded_model_count,
    }))
}

// ── Services dashboard ───────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub(crate) struct ServiceLogQueryParams {
    since_ms: Option<u64>,
    limit: Option<usize>,
    level: Option<String>,
    search: Option<String>,
}

pub(crate) async fn api_list_services(
    State(state): State<AppState>,
) -> Json<Vec<hive_contracts::ServiceSnapshot>> {
    Json(state.service_registry.list())
}

pub(crate) async fn api_service_logs(
    State(state): State<AppState>,
    Path(service_id): Path<String>,
    Query(params): Query<ServiceLogQueryParams>,
) -> Json<Vec<hive_core::LogEntry>> {
    let query = hive_core::LogQuery {
        since_ms: params.since_ms,
        limit: params.limit,
        level: params.level,
        search: params.search,
    };
    Json(state.service_log_collector.get_logs(&service_id, &query))
}

pub(crate) async fn api_restart_service(
    State(state): State<AppState>,
    Path(service_id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    state
        .service_registry
        .restart(&service_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn api_services_events(
    State(state): State<AppState>,
) -> Sse<impl futures_core::Stream<Item = Result<Event, std::convert::Infallible>>> {
    let mut rx = state.service_registry.subscribe();
    let stream = async_stream::stream! {
        // Send initial snapshot so late-joiners see current state.
        let snapshot = state.service_registry.list();
        if let Ok(json) = serde_json::to_string(&snapshot) {
            yield Ok(Event::default().event("snapshot").data(json));
        }
        loop {
            tokio::select! {
                biased;
                _ = state.shutdown.cancelled() => break,
                result = rx.recv() => {
                    match result {
                        Ok(event) => {
                            if let Ok(json) = serde_json::to_string(&event) {
                                yield Ok(Event::default().event("status").data(json));
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!("services SSE lagged {n} events");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        }
    };
    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new().interval(std::time::Duration::from_secs(15)),
    )
}

// ── All agents / telemetry ───────────────────────────────────────────────

pub(crate) async fn api_all_agents(State(state): State<AppState>) -> Json<Vec<AgentSummary>> {
    Json(state.chat.list_all_agents().await)
}

pub(crate) async fn api_all_sessions_telemetry(
    State(state): State<AppState>,
) -> Json<Vec<SessionTelemetryEntry>> {
    let sessions = state.chat.list_sessions().await;
    let telem_pairs = state.chat.all_sessions_telemetry().await;
    let telem_map: HashMap<String, TelemetrySnapshot> = telem_pairs.into_iter().collect();

    let entries = sessions
        .into_iter()
        .map(|s| {
            let telemetry = telem_map.get(&s.id).cloned().unwrap_or_else(|| TelemetrySnapshot {
                per_agent: Vec::new(),
                total: Default::default(),
            });
            SessionTelemetryEntry { session_id: s.id, title: s.title, state: s.state, telemetry }
        })
        .collect();

    Json(entries)
}

// ── Local model load/unload (Flight Deck) ────────────────────────────────

pub(crate) async fn api_local_model_load(
    State(state): State<AppState>,
    Path(model_id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    let lm = state
        .local_models
        .as_ref()
        .ok_or_else(|| (StatusCode::NOT_FOUND, "local models not available".to_string()))?;
    let rm = state
        .runtime_manager
        .clone()
        .ok_or_else(|| (StatusCode::NOT_FOUND, "runtime manager not available".to_string()))?;
    let model = lm
        .get_model(&model_id)
        .map_err(|e| (StatusCode::NOT_FOUND, format!("model not found: {e}")))?;
    let model_path = model.local_path.clone();
    let runtime = model.runtime;
    tokio::task::spawn_blocking(move || {
        rm.load_model(&model_id, &model_path, runtime)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("failed to load model: {e}")))
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("task panicked: {e}")))?
    .map(|()| StatusCode::NO_CONTENT)
}

pub(crate) async fn api_local_model_unload(
    State(state): State<AppState>,
    Path(model_id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    let rm = state
        .runtime_manager
        .clone()
        .ok_or_else(|| (StatusCode::NOT_FOUND, "runtime manager not available".to_string()))?;
    tokio::task::spawn_blocking(move || {
        rm.unload_model(&model_id).map_err(|e| {
            (StatusCode::INTERNAL_SERVER_ERROR, format!("failed to unload model: {e}"))
        })
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("task panicked: {e}")))?
    .map(|()| StatusCode::NO_CONTENT)
}
