use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;

use crate::{tool_error, AppState};
use hive_classification::DataClass;
use hive_tools::{ToolDefinition, ToolResult};

pub(crate) async fn list_tools(State(state): State<AppState>) -> Json<Vec<ToolDefinition>> {
    let mut tools = state.chat.list_tools();

    // Include MCP tools from the catalog (enabled servers only).
    // The catalog holds tools discovered during startup / config refresh,
    // so they are available even when servers are not actively connected.
    let enabled_ids: std::collections::HashSet<String> =
        state.mcp.server_configs().await.into_iter().filter(|c| c.enabled).map(|c| c.id).collect();
    for ct in state.mcp_catalog.all_cataloged_tools().await {
        if !enabled_ids.contains(&ct.server_id) {
            continue;
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

    // Include tools from enabled plugins.
    for plugin in state.plugin_registry.list() {
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
