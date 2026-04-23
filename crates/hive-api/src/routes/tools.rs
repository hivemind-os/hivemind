use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;

use crate::{tool_error, AppState};
use hive_classification::DataClass;
use hive_tools::{ToolDefinition, ToolResult};

#[derive(Debug, Deserialize)]
pub(crate) struct ListToolsQuery {
    pub persona_id: Option<String>,
}

pub(crate) async fn list_tools(
    State(state): State<AppState>,
    Query(query): Query<ListToolsQuery>,
) -> Json<Vec<ToolDefinition>> {
    let mut tools = state.chat.list_tools();

    // Resolve which MCP server IDs belong to the requested persona.
    // When no persona is specified, include all enabled servers.
    let persona_mcp_ids: Option<std::collections::HashSet<String>> = query
        .persona_id
        .as_deref()
        .map(|pid| state.chat.mcp_configs_for_persona(pid).into_iter().map(|c| c.id).collect());

    // Include MCP tools from the catalog (enabled servers only).
    // The catalog holds tools discovered during startup / config refresh,
    // so they are available even when servers are not actively connected.
    let enabled_ids: std::collections::HashSet<String> =
        state.mcp.server_configs().await.into_iter().filter(|c| c.enabled).map(|c| c.id).collect();
    for ct in state.mcp_catalog.all_cataloged_tools().await {
        if !enabled_ids.contains(&ct.server_id) {
            continue;
        }
        // When scoped to a persona, only include servers assigned to it.
        if let Some(ref allowed) = persona_mcp_ids {
            if !allowed.contains(&ct.server_id) {
                continue;
            }
        }
        tools.push(ToolDefinition {
            id: format!("mcp.{}.{}", ct.server_id, ct.tool.name),
            name: format!("MCP: {} ({})", ct.tool.name, ct.server_id),
            description: ct.tool.description,
            input_schema: ct.tool.input_schema,
            output_schema: None,
            channel_class: ct.channel_class,
            side_effects: true,
            approval: hive_contracts::ToolApproval::Ask,
            annotations: hive_contracts::ToolAnnotations {
                title: format!("MCP: {} ({})", ct.tool.name, ct.server_id),
                read_only_hint: None,
                destructive_hint: None,
                idempotent_hint: None,
                open_world_hint: Some(true),
            },
        });
    }

    // Include tools from enabled plugins, filtered by persona when specified.
    let plugins = match query.persona_id.as_deref() {
        Some(pid) => state.plugin_registry.list_for_persona(pid),
        None => state.plugin_registry.list().into_iter().filter(|p| p.enabled).collect(),
    };
    for plugin in plugins {
        if !plugin.enabled {
            continue;
        }
        let plugin_id = plugin.manifest.plugin_id();
        if state.plugin_host.get(&plugin_id).is_none() {
            continue;
        }
        if let Ok(bridge_tools) =
            hive_plugins::bridge::create_bridge_tools(&state.plugin_host, &plugin_id).await
        {
            for bt in bridge_tools {
                tools.push(bt.definition);
            }
        }
    }

    // Include session-only tools that are registered per-session in
    // build_session_tools() but not in the static ChatService registry.
    // We surface their definitions here so the UI (e.g. bot wizard tool
    // override, settings) can display the full set of available tools.

    // Data store is always available per-session.
    tools.push(hive_tools::DataStoreTool::tool_definition());

    // Workflow tools are always available (workflow service is always present).
    tools.extend(hive_tools::all_workflow_tool_definitions());

    // Web search is conditional on provider + API key configuration.
    if let Some(ws_def) = state.chat.web_search_tool_definition() {
        tools.push(ws_def);
    }

    // Deduplicate by tool ID (safety net in case any tool is already
    // present from the static registry or plugins).
    {
        let mut seen = std::collections::HashSet::new();
        tools.retain(|t| seen.insert(t.id.clone()));
    }

    Json(tools)
}

#[derive(Debug, Deserialize)]
pub(crate) struct ToolInvokeRequest {
    input: serde_json::Value,
    data_class: Option<DataClass>,
}

pub(crate) async fn invoke_tool(
    State(state): State<AppState>,
    Path(tool_id): Path<String>,
    Json(request): Json<ToolInvokeRequest>,
) -> Result<Json<ToolResult>, (StatusCode, String)> {
    let data_class = request.data_class.unwrap_or(DataClass::Internal);
    state.chat.invoke_tool(&tool_id, request.input, data_class).await.map(Json).map_err(tool_error)
}
