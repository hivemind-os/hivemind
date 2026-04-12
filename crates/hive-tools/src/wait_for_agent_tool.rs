use crate::{BoxFuture, Tool, ToolError, ToolResult};
use hive_classification::ChannelClass;
use hive_contracts::{ToolAnnotations, ToolApproval, ToolDefinition};
use serde_json::{json, Value};

/// Built-in tool that blocks until a sub-agent completes (done/error) or times out.
/// Execution is handled by the loop.
pub struct WaitForAgentTool {
    definition: ToolDefinition,
}

impl Default for WaitForAgentTool {
    fn default() -> Self {
        Self {
            definition: ToolDefinition {
                id: "core.wait_for_agent".to_string(),
                name: "Wait For Agent".to_string(),
                description: "Block until a sub-agent finishes (done or error). \
                    Returns the agent's final status and result. \
                    This is much more efficient than polling with core.list_agents \
                    or core.get_agent_result in a loop — prefer this tool when you \
                    need to wait for an agent to complete."
                    .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "agent_id": {
                            "type": "string",
                            "description": "The ID of the agent to wait for"
                        },
                        "timeout_secs": {
                            "type": "number",
                            "description": "Maximum seconds to wait (default: 300)"
                        }
                    },
                    "required": ["agent_id"]
                }),
                output_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "agent_id": { "type": "string" },
                        "status": { "type": "string", "enum": ["done", "error", "timeout"] },
                        "result": { "type": "string" }
                    }
                })),
                channel_class: ChannelClass::Internal,
                side_effects: false,
                approval: ToolApproval::Auto,
                annotations: ToolAnnotations {
                    title: "Wait For Agent".to_string(),
                    read_only_hint: Some(true),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(true),
                    open_world_hint: Some(false),
                },
            },
        }
    }
}

impl Tool for WaitForAgentTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, _input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async {
            Err(ToolError::ExecutionFailed(
                "core.wait_for_agent is handled by the loop, not direct execution".to_string(),
            ))
        })
    }
}
