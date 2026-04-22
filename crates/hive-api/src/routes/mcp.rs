use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::sse::{Event, Sse},
    Json,
};
use serde::Deserialize;

use crate::clamp_limit;
use crate::{mcp_error, AppState};
use hive_mcp::{
    McpCatalogEntry, McpNotificationEvent, McpPromptInfo, McpResourceInfo, McpServerLog,
    McpServerSnapshot, McpToolInfo,
};
use std::sync::Arc;

// ── MCP SSE event stream ────────────────────────────────────────────

/// Push-based SSE stream for all `mcp.*` EventBus topics.
pub(crate) async fn api_mcp_event_stream(
    State(state): State<AppState>,
) -> Sse<impl futures_core::Stream<Item = Result<Event, std::convert::Infallible>>> {
    let mut rx = state.event_bus.subscribe_queued_bounded("mcp", 10_000);
    let stream = async_stream::stream! {
        while let Some(envelope) = rx.recv().await {
            if let Ok(json) = serde_json::to_string(&envelope) {
                yield Ok(Event::default().event(&envelope.topic).data(json));
            }
        }
    };
    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new().interval(std::time::Duration::from_secs(15)),
    )
}

// ── Session-scoped MCP endpoints ────────────────────────────────────

/// List MCP servers for a specific session with per-session connection status.
pub(crate) async fn list_session_mcp_servers(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<Vec<McpServerSnapshot>>, (StatusCode, String)> {
    let mcp = state
        .chat
        .get_session_mcp(&session_id)
        .await
        .map_err(|_| (StatusCode::NOT_FOUND, format!("session {session_id} not found")))?;
    match mcp {
        Some(m) => Ok(Json(m.list_servers().await)),
        None => Ok(Json(Vec::new())),
    }
}

/// Connect an MCP server in a specific session.
pub(crate) async fn connect_session_mcp_server(
    State(state): State<AppState>,
    Path((session_id, server_id)): Path<(String, String)>,
) -> Result<Json<McpServerSnapshot>, (StatusCode, String)> {
    let mcp = state
        .chat
        .get_session_mcp(&session_id)
        .await
        .map_err(|_| (StatusCode::NOT_FOUND, format!("session {session_id} not found")))?
        .ok_or_else(|| {
            (StatusCode::BAD_REQUEST, "MCP not available for this session".to_string())
        })?;
    mcp.connect(&server_id).await.map(Json).map_err(mcp_error)
}

/// Disconnect an MCP server in a specific session.
pub(crate) async fn disconnect_session_mcp_server(
    State(state): State<AppState>,
    Path((session_id, server_id)): Path<(String, String)>,
) -> Result<Json<McpServerSnapshot>, (StatusCode, String)> {
    let mcp = state
        .chat
        .get_session_mcp(&session_id)
        .await
        .map_err(|_| (StatusCode::NOT_FOUND, format!("session {session_id} not found")))?
        .ok_or_else(|| {
            (StatusCode::BAD_REQUEST, "MCP not available for this session".to_string())
        })?;
    mcp.disconnect(&server_id).await.map(Json).map_err(mcp_error)
}

/// Get logs for an MCP server in a specific session.
pub(crate) async fn get_session_mcp_server_logs(
    State(state): State<AppState>,
    Path((session_id, server_id)): Path<(String, String)>,
) -> Result<Json<Vec<McpServerLog>>, (StatusCode, String)> {
    let mcp = state
        .chat
        .get_session_mcp(&session_id)
        .await
        .map_err(|_| (StatusCode::NOT_FOUND, format!("session {session_id} not found")))?
        .ok_or_else(|| {
            (StatusCode::BAD_REQUEST, "MCP not available for this session".to_string())
        })?;
    mcp.get_server_logs(&server_id).await.map(Json).map_err(mcp_error)
}

// ── Global MCP endpoints ────────────────────────────────────────────

pub(crate) async fn list_mcp_servers(
    State(state): State<AppState>,
) -> Json<Vec<McpServerSnapshot>> {
    Json(state.mcp.list_servers().await)
}

pub(crate) async fn connect_mcp_server(
    State(state): State<AppState>,
    Path(server_id): Path<String>,
) -> Result<Json<McpServerSnapshot>, (StatusCode, String)> {
    // Use the current working directory as workspace fallback so the sandbox
    // policy can grant read access to a meaningful directory.
    let cwd = std::env::current_dir().ok();
    state.mcp.connect_with_workspace(&server_id, cwd.as_deref()).await.map(Json).map_err(mcp_error)
}

pub(crate) async fn disconnect_mcp_server(
    State(state): State<AppState>,
    Path(server_id): Path<String>,
) -> Result<Json<McpServerSnapshot>, (StatusCode, String)> {
    state.mcp.disconnect(&server_id).await.map(Json).map_err(mcp_error)
}

pub(crate) async fn list_mcp_tools(
    State(state): State<AppState>,
    Path(server_id): Path<String>,
) -> Result<Json<Vec<McpToolInfo>>, (StatusCode, String)> {
    // Try live connection first; fall back to catalog cache for known servers.
    match state.mcp.list_tools(&server_id).await {
        Ok(tools) => Ok(Json(tools)),
        Err(_) => {
            let configs = state.mcp.server_configs().await;
            if !configs.iter().any(|c| c.id == server_id) {
                return Err((StatusCode::NOT_FOUND, format!("server `{server_id}` not found")));
            }
            let cached = state.mcp_catalog.tools_for_server(&server_id).await;
            Ok(Json(cached))
        }
    }
}

pub(crate) async fn list_mcp_resources(
    State(state): State<AppState>,
    Path(server_id): Path<String>,
) -> Result<Json<Vec<McpResourceInfo>>, (StatusCode, String)> {
    match state.mcp.list_resources(&server_id).await {
        Ok(resources) => Ok(Json(resources)),
        Err(_) => {
            let configs = state.mcp.server_configs().await;
            if !configs.iter().any(|c| c.id == server_id) {
                return Err((StatusCode::NOT_FOUND, format!("server `{server_id}` not found")));
            }
            let cached = state.mcp_catalog.resources_for_server(&server_id).await;
            Ok(Json(cached))
        }
    }
}

pub(crate) async fn list_mcp_prompts(
    State(state): State<AppState>,
    Path(server_id): Path<String>,
) -> Result<Json<Vec<McpPromptInfo>>, (StatusCode, String)> {
    match state.mcp.list_prompts(&server_id).await {
        Ok(prompts) => Ok(Json(prompts)),
        Err(_) => {
            let configs = state.mcp.server_configs().await;
            if !configs.iter().any(|c| c.id == server_id) {
                return Err((StatusCode::NOT_FOUND, format!("server `{server_id}` not found")));
            }
            let cached = state.mcp_catalog.prompts_for_server(&server_id).await;
            Ok(Json(cached))
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct McpNotificationsQuery {
    limit: Option<usize>,
}

pub(crate) async fn list_mcp_notifications(
    State(state): State<AppState>,
    Query(query): Query<McpNotificationsQuery>,
) -> Json<Vec<McpNotificationEvent>> {
    let limit = clamp_limit(query.limit, 25);
    Json(state.mcp.list_notifications(limit).await)
}

pub(crate) async fn get_mcp_server_logs(
    State(state): State<AppState>,
    Path(server_id): Path<String>,
) -> Result<Json<Vec<McpServerLog>>, (StatusCode, String)> {
    state.mcp.get_server_logs(&server_id).await.map(Json).map_err(mcp_error)
}

// ── Catalog endpoints ───────────────────────────────────────────────

/// Return the full MCP tool catalog (all servers).
pub(crate) async fn get_mcp_catalog(State(state): State<AppState>) -> Json<Vec<McpCatalogEntry>> {
    Json(state.mcp_catalog.all().await)
}

/// Refresh the catalog for a single server (connect, discover, disconnect).
pub(crate) async fn refresh_mcp_catalog_server(
    State(state): State<AppState>,
    Path(server_id): Path<String>,
) -> Result<Json<McpCatalogEntry>, (StatusCode, String)> {
    state
        .mcp
        .discover_and_catalog(&server_id, &state.mcp_catalog)
        .await
        .map(Json)
        .map_err(mcp_error)
}

/// Refresh the catalog for all enabled servers.
pub(crate) async fn refresh_mcp_catalog_all(
    State(state): State<AppState>,
) -> Json<Vec<McpCatalogEntry>> {
    state.mcp.refresh_catalog(&state.mcp_catalog).await;
    Json(state.mcp_catalog.all().await)
}

/// Test-connect to an MCP server without persisting config.
/// Used by the wizard to verify connectivity and discover tools.
#[derive(Deserialize)]
pub(crate) struct TestConnectRequest {
    pub config: hive_core::McpServerConfig,
}

#[derive(serde::Serialize)]
pub(crate) struct TestConnectResponse {
    pub tools: Vec<McpToolInfo>,
    pub resources: Vec<McpResourceInfo>,
    pub prompts: Vec<McpPromptInfo>,
}

pub(crate) async fn test_connect_mcp(
    State(state): State<AppState>,
    Json(req): Json<TestConnectRequest>,
) -> Result<Json<TestConnectResponse>, (StatusCode, String)> {
    use hive_mcp::McpService;

    // Create a temporary McpService with just this server.
    let mut tmp_svc = McpService::from_configs(
        std::slice::from_ref(&req.config),
        state.event_bus.clone(),
        Arc::clone(&state.sandbox_config),
    );
    if let Some(ne) = state.mcp.node_env() {
        tmp_svc = tmp_svc.with_node_env(ne);
    }
    if let Some(pe) = state.mcp.python_env() {
        tmp_svc = tmp_svc.with_python_env(pe);
    }

    // Connect, discover, disconnect.
    let entry = tmp_svc
        .discover_and_catalog(&req.config.id, &state.mcp_catalog)
        .await
        .map_err(mcp_error)?;

    Ok(Json(TestConnectResponse {
        tools: entry.tools,
        resources: entry.resources,
        prompts: entry.prompts,
    }))
}

/// POST /api/v1/mcp/servers/{server_id}/install-runtime
///
/// Install the required runtime for a specific MCP server and retry connection.
/// Returns the server snapshot on success, or an error if installation fails.
pub(crate) async fn install_mcp_runtime(
    State(state): State<AppState>,
    Path(server_id): Path<String>,
) -> Result<Json<McpServerSnapshot>, (StatusCode, String)> {
    // Detect what runtime this server needs.
    let configs = state.mcp.server_configs().await;
    let config = configs
        .iter()
        .find(|c| c.id == server_id)
        .ok_or_else(|| (StatusCode::NOT_FOUND, format!("server `{server_id}` not found")))?;

    let command = config
        .command
        .as_deref()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "server has no command configured".to_string()))?;

    let parts = hive_mcp::runtime::detect_runtime(command);
    match parts {
        hive_mcp::runtime::McpRuntime::Node => {
            state.node_env.ensure_node().await.map_err(|e| {
                (StatusCode::INTERNAL_SERVER_ERROR, format!("Node.js install failed: {e}"))
            })?;
        }
        hive_mcp::runtime::McpRuntime::Python => {
            state.python_env.ensure_default_env().await.map_err(|e| {
                (StatusCode::INTERNAL_SERVER_ERROR, format!("Python install failed: {e}"))
            })?;
        }
        other => {
            return Err((
                StatusCode::BAD_REQUEST,
                format!(
                    "{} cannot be auto-installed. {}",
                    other,
                    hive_mcp::runtime::install_hint(other)
                ),
            ));
        }
    }

    // Retry connection now that the runtime is installed.
    let snapshot = state.mcp.connect(&server_id).await.map_err(mcp_error)?;
    Ok(Json(snapshot))
}

// ── MCP App endpoints ───────────────────────────────────────────────

/// POST /api/v1/mcp/servers/{server_id}/call-tool
///
/// Call a tool on an MCP server. Used by MCP Apps to proxy tool calls.
#[derive(Deserialize)]
pub(crate) struct CallToolRequest {
    pub name: String,
    pub arguments: Option<serde_json::Value>,
}

#[derive(serde::Serialize)]
pub(crate) struct CallToolResponse {
    pub content: String,
    pub is_error: bool,
}

pub(crate) async fn call_mcp_tool(
    State(state): State<AppState>,
    Path(server_id): Path<String>,
    Json(req): Json<CallToolRequest>,
) -> Result<Json<CallToolResponse>, (StatusCode, String)> {
    let args: serde_json::Map<String, serde_json::Value> = match req.arguments {
        Some(serde_json::Value::Object(map)) => map,
        Some(_) => return Err((StatusCode::BAD_REQUEST, "arguments must be an object".into())),
        None => serde_json::Map::new(),
    };
    let result = state
        .mcp
        .call_tool(&server_id, &req.name, args)
        .await
        .map_err(mcp_error)?;
    Ok(Json(CallToolResponse {
        content: result.content,
        is_error: result.is_error,
    }))
}

/// POST /api/v1/mcp/servers/{server_id}/read-resource
///
/// Read a resource from an MCP server. Used by MCP Apps to proxy resource reads.
#[derive(Deserialize)]
pub(crate) struct ReadResourceRequest {
    pub uri: String,
}

pub(crate) async fn read_mcp_resource(
    State(state): State<AppState>,
    Path(server_id): Path<String>,
    Json(req): Json<ReadResourceRequest>,
) -> Result<Json<String>, (StatusCode, String)> {
    let content = state
        .mcp
        .read_resource(&server_id, &req.uri)
        .await
        .map_err(mcp_error)?;
    Ok(Json(content))
}

/// POST /api/v1/mcp/servers/{server_id}/fetch-ui-resource
///
/// Fetch an MCP App UI resource (ui:// scheme) with caching.
#[derive(Deserialize)]
pub(crate) struct FetchUiResourceRequest {
    pub uri: String,
}

pub(crate) async fn fetch_mcp_ui_resource(
    State(state): State<AppState>,
    Path(server_id): Path<String>,
    Json(req): Json<FetchUiResourceRequest>,
) -> Result<Json<hive_mcp::McpAppResource>, (StatusCode, String)> {
    // Ensure the server is connected — UI resources require a live session
    // because they're fetched via the MCP resources/read protocol call.
    state.mcp.ensure_connected(&server_id).await.map_err(mcp_error)?;
    let resource = state
        .mcp
        .fetch_ui_resource(&server_id, &req.uri, None)
        .await
        .map_err(mcp_error)?;
    Ok(Json(resource))
}
