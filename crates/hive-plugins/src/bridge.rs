//! Plugin bridge tool — wraps plugin tools for the agent ToolRegistry.
//!
//! Creates a `hive_contracts::ToolDefinition` for each tool exposed by a
//! plugin, so they appear in the agent's tool set alongside built-in
//! and MCP tools.

use crate::host::PluginHost;
use crate::registry::PluginRegistry;
use hive_classification::DataClass;
use hive_contracts::tools::{ToolAnnotations, ToolApproval, ToolDefinition};
use hive_contracts::ChannelClass;
use hive_tools::{BoxFuture, Tool, ToolError, ToolRegistry, ToolResult};
use serde_json::Value;
use std::sync::Arc;

/// A bridge tool that maps a plugin tool into the agent's tool registry.
pub struct PluginBridgeTool {
    /// Full tool ID as registered (e.g., "plugin.github-issues.list_issues").
    pub tool_id: String,
    /// The plugin ID this tool belongs to.
    pub plugin_id: String,
    /// The tool name within the plugin.
    pub tool_name: String,
    /// Tool definition for the agent.
    pub definition: ToolDefinition,
    /// Reference to the plugin host for executing calls.
    pub host: Arc<PluginHost>,
}

impl Tool for PluginBridgeTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let output = self
                .host
                .call_tool(&self.plugin_id, &self.tool_name, input)
                .await
                .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
            Ok(ToolResult { output, data_class: DataClass::Internal })
        })
    }
}

/// Register all tools from a plugin into a collection of bridge tools.
pub async fn create_bridge_tools(
    host: &Arc<PluginHost>,
    plugin_id: &str,
) -> anyhow::Result<Vec<PluginBridgeTool>> {
    let tool_defs = host.list_tools(plugin_id).await?;
    let mut bridges = Vec::new();

    for tool_def in tool_defs {
        let tool_id = format!("plugin.{}.{}", plugin_id, tool_def.name);

        let has_side_effects = tool_def
            .annotations
            .as_object()
            .and_then(|obj| obj.get("sideEffects"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let read_only = tool_def
            .annotations
            .as_object()
            .and_then(|obj| obj.get("readOnlyHint"))
            .and_then(|v| v.as_bool());

        let destructive = tool_def
            .annotations
            .as_object()
            .and_then(|obj| obj.get("destructiveHint"))
            .and_then(|v| v.as_bool());

        let annotations = ToolAnnotations {
            title: tool_def.description.clone(),
            read_only_hint: read_only,
            destructive_hint: destructive,
            idempotent_hint: None,
            open_world_hint: None,
        };

        let approval = if has_side_effects { ToolApproval::Ask } else { ToolApproval::Auto };

        let definition = ToolDefinition {
            id: tool_id.clone(),
            name: tool_def.name.clone(),
            description: tool_def.description.clone(),
            input_schema: tool_def.input_schema.clone(),
            output_schema: None,
            channel_class: ChannelClass::Internal,
            side_effects: has_side_effects,
            approval,
            annotations,
        };

        bridges.push(PluginBridgeTool {
            tool_id,
            plugin_id: plugin_id.into(),
            tool_name: tool_def.name,
            definition,
            host: host.clone(),
        });
    }

    Ok(bridges)
}

/// Register tools from all enabled (and running) plugins into a `ToolRegistry`.
///
/// When `persona_id` is `Some`, only plugins whose `allowed_personas` includes
/// that persona (or is empty, meaning "all personas") are considered.
/// When `None`, all enabled plugins are included.
pub async fn register_plugin_tools(
    tool_registry: &mut ToolRegistry,
    host: &Arc<PluginHost>,
    plugin_registry: &PluginRegistry,
    persona_id: Option<&str>,
) {
    let plugins = match persona_id {
        Some(pid) => plugin_registry.list_for_persona(pid),
        None => plugin_registry.list(),
    };

    for plugin in plugins {
        if !plugin.enabled {
            continue;
        }
        let plugin_id = plugin.manifest.plugin_id();
        // Only register tools for plugins that have a running process
        if host.get(&plugin_id).is_none() {
            continue;
        }
        match create_bridge_tools(host, &plugin_id).await {
            Ok(bridges) => {
                for bridge in bridges {
                    if let Err(e) = tool_registry.register(Arc::new(bridge)) {
                        tracing::debug!(
                            %plugin_id,
                            error = %e,
                            "skipping duplicate plugin tool"
                        );
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    %plugin_id,
                    error = %e,
                    "failed to create bridge tools for plugin"
                );
            }
        }
    }
}
