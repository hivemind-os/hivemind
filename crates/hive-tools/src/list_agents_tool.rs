use crate::{BoxFuture, Tool, ToolError, ToolResult};
use hive_classification::ChannelClass;
use hive_contracts::{ToolAnnotations, ToolApproval, ToolDefinition};
use serde_json::{json, Value};

/// Built-in tool that lets an agent list all active agent instances in the session.
/// Execution is handled by the loop.
pub struct ListAgentsTool {
    definition: ToolDefinition,
}

impl Default for ListAgentsTool {
    fn default() -> Self {
        Self {
            definition: ToolDefinition {
                id: "core.list_agents".to_string(),
                name: "List Agents".to_string(),
                description: "List all active agent instances in the current session with their ID, name, description, status, and result preview (if completed). Use core.get_agent_result to retrieve the full result.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                }),
                output_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "agents": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string" },
                                    "name": { "type": "string" },
                                    "description": { "type": "string" },
                                    "status": { "type": "string" },
                                    "result_preview": { "type": "string", "description": "Truncated result (first 200 chars) if agent has completed" }
                                }
                            }
                        }
                    }
                })),
                channel_class: ChannelClass::Internal,
                side_effects: false,
                approval: ToolApproval::Auto,
                annotations: ToolAnnotations {
                    title: "List Agents".to_string(),
                    read_only_hint: Some(true),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(true),
                    open_world_hint: Some(false),
                },
            },
        }
    }
}

impl Tool for ListAgentsTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, _input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async {
            Err(ToolError::ExecutionFailed(
                "core.list_agents is handled by the loop, not direct execution".to_string(),
            ))
        })
    }
}
