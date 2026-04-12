use crate::{BoxFuture, Tool, ToolError, ToolResult};
use hive_classification::ChannelClass;
use hive_contracts::{ToolAnnotations, ToolApproval, ToolDefinition};
use serde_json::{json, Value};

/// Built-in tool that retrieves the full final result of a completed agent.
/// Execution is handled by the loop.
pub struct GetAgentResultTool {
    definition: ToolDefinition,
}

impl Default for GetAgentResultTool {
    fn default() -> Self {
        Self {
            definition: ToolDefinition {
                id: "core.get_agent_result".to_string(),
                name: "Get Agent Result".to_string(),
                description: "Retrieve the full final result of a completed agent by its ID. \
                    Use core.list_agents to find agent IDs and their status."
                    .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "agent_id": {
                            "type": "string",
                            "description": "The ID of the agent whose result to retrieve"
                        }
                    },
                    "required": ["agent_id"]
                }),
                output_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "agent_id": { "type": "string" },
                        "status": { "type": "string" },
                        "result": { "type": "string" }
                    }
                })),
                channel_class: ChannelClass::Internal,
                side_effects: false,
                approval: ToolApproval::Auto,
                annotations: ToolAnnotations {
                    title: "Get Agent Result".to_string(),
                    read_only_hint: Some(true),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(true),
                    open_world_hint: Some(false),
                },
            },
        }
    }
}

impl Tool for GetAgentResultTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, _input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async {
            Err(ToolError::ExecutionFailed(
                "core.get_agent_result is handled by the loop, not direct execution".to_string(),
            ))
        })
    }
}
