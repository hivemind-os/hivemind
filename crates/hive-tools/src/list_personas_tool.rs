use crate::{BoxFuture, Tool, ToolError, ToolResult};
use hive_classification::ChannelClass;
use hive_contracts::{ToolAnnotations, ToolApproval, ToolDefinition};
use serde_json::{json, Value};

/// Built-in tool that lists all available personas (agent types).
/// Execution is handled by the loop.
pub struct ListPersonasTool {
    definition: ToolDefinition,
}

impl Default for ListPersonasTool {
    fn default() -> Self {
        Self {
            definition: ToolDefinition {
                id: "core.list_personas".to_string(),
                name: "List Personas".to_string(),
                description: "List all available personas (agent types) with their ID, name, and description. Use this to discover what personas are available before spawning an agent.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                }),
                output_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "personas": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string" },
                                    "name": { "type": "string" },
                                    "description": { "type": "string" }
                                }
                            },
                            "description": "All available personas (agent types)"
                        }
                    }
                })),
                channel_class: ChannelClass::Internal,
                side_effects: false,
                approval: ToolApproval::Auto,
                annotations: ToolAnnotations {
                    title: "List Personas".to_string(),
                    read_only_hint: Some(true),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(true),
                    open_world_hint: Some(false),
                },
            },
        }
    }
}

impl Tool for ListPersonasTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, _input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async {
            Err(ToolError::ExecutionFailed(
                "core.list_personas is handled by the loop, not direct execution".to_string(),
            ))
        })
    }
}
