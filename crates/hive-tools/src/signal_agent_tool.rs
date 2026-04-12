use crate::{BoxFuture, Tool, ToolError, ToolResult};
use hive_classification::ChannelClass;
use hive_contracts::{ToolAnnotations, ToolApproval, ToolDefinition};
use serde_json::{json, Value};

/// Built-in tool that lets one AI agent signal another AI agent in the same
/// session.  This is strictly an internal agent-to-agent coordination
/// mechanism — it does **not** send emails, chat messages, or any external
/// communications.
///
/// Execution is handled by the loop — the tool's `execute()` is never called
/// directly; only the definition is used.
pub struct SignalAgentTool {
    definition: ToolDefinition,
}

impl Default for SignalAgentTool {
    fn default() -> Self {
        Self {
            definition: ToolDefinition {
                id: "core.signal_agent".to_string(),
                name: "Signal Agent".to_string(),
                description: "Signal a peer AI agent running in this session. \
                    This is an internal agent-to-agent coordination tool — \
                    it does NOT send emails, chat messages, or any external communications. \
                    Use it only to pass results or coordinate work between spawned AI agents."
                    .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "agent_id": {
                            "type": "string",
                            "description": "ID of the target AI agent, or 'parent' to signal the spawning agent"
                        },
                        "content": {
                            "type": "string",
                            "description": "The payload to deliver to the target AI agent"
                        }
                    },
                    "required": ["agent_id", "content"]
                }),
                output_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "agent_id": {
                            "type": "string",
                            "description": "The AI agent that received the signal."
                        },
                        "delivered": {
                            "type": "boolean",
                            "description": "Whether the signal was accepted for delivery."
                        }
                    }
                })),
                channel_class: ChannelClass::Internal,
                side_effects: true,
                approval: ToolApproval::Auto,
                annotations: ToolAnnotations {
                    title: "Signal Agent".to_string(),
                    read_only_hint: Some(false),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(false),
                    open_world_hint: Some(false),
                },
            },
        }
    }
}

impl Tool for SignalAgentTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, _input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async {
            Err(ToolError::ExecutionFailed(
                "core.signal_agent is handled by the loop's agent orchestrator, not direct execution"
                    .to_string(),
            ))
        })
    }
}
