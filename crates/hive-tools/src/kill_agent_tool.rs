use crate::{BoxFuture, Tool, ToolError, ToolResult};
use hive_classification::ChannelClass;
use hive_contracts::{ToolAnnotations, ToolApproval, ToolDefinition};
use serde_json::{json, Value};

/// Built-in tool that lets an agent kill/deactivate another agent in the session.
/// Execution is handled by the loop.
pub struct KillAgentTool {
    definition: ToolDefinition,
}

impl Default for KillAgentTool {
    fn default() -> Self {
        Self {
            definition: ToolDefinition {
                id: "core.kill_agent".to_string(),
                name: "Kill Agent".to_string(),
                description: "Deactivate and remove an active agent from the session. The agent is immediately stopped and its resources are released.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "agent_id": {
                            "type": "string",
                            "description": "The runtime ID of the agent to kill"
                        }
                    },
                    "required": ["agent_id"]
                }),
                output_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "killed": {
                            "type": "boolean",
                            "description": "Whether the agent was successfully killed"
                        }
                    }
                })),
                channel_class: ChannelClass::Internal,
                side_effects: true,
                approval: ToolApproval::Auto,
                annotations: ToolAnnotations {
                    title: "Kill Agent".to_string(),
                    read_only_hint: Some(false),
                    destructive_hint: Some(true),
                    idempotent_hint: Some(true),
                    open_world_hint: Some(false),
                },
            },
        }
    }
}

impl Tool for KillAgentTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, _input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async {
            Err(ToolError::ExecutionFailed(
                "core.kill_agent is handled by the loop, not direct execution".to_string(),
            ))
        })
    }
}
