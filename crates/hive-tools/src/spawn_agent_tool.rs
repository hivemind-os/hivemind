use crate::{BoxFuture, Tool, ToolError, ToolResult};
use hive_classification::ChannelClass;
use hive_contracts::{ToolAnnotations, ToolApproval, ToolDefinition};
use serde_json::{json, Value};

/// Built-in tool that lets an agent spawn a sub-agent for parallel work.
/// Execution is handled by the loop — the tool's `execute()` is never called
/// directly; only the definition is used.
pub struct SpawnAgentTool {
    definition: ToolDefinition,
}

impl Default for SpawnAgentTool {
    fn default() -> Self {
        Self {
            definition: ToolDefinition {
                id: "core.spawn_agent".to_string(),
                name: "Spawn Agent".to_string(),
                description: "Spawn an agent instance to work on a task in parallel. The agent runs independently. Use core.wait_for_agent to block until completion and get the result, rather than polling with core.list_agents. Use core.list_personas to see available personas. If persona is omitted or no match is found, the General Agent persona is used.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "persona": {
                            "type": "string",
                            "description": "Optional name or ID of a persona (agent type) to use. Must match an existing persona. If omitted or not found, the General Agent persona is used as fallback."
                        },
                        "friendly_name": {
                            "type": "string",
                            "description": "Optional friendly display name for the spawned agent instance. If omitted, a random name is generated (e.g. 'eager_turing')."
                        },
                        "task": {
                            "type": "string",
                            "description": "The task description for the spawned agent to work on"
                        },
                        "mode": {
                            "type": "string",
                            "enum": ["one_shot", "idle_after_task", "continuous"],
                            "description": "Agent lifecycle mode. 'one_shot' (default): complete the task and terminate. 'idle_after_task': complete, then wait for more messages. 'continuous': run continuously with standing orders."
                        },
                        "timeout_secs": {
                            "type": "integer",
                            "description": "Maximum execution time in seconds for one-shot agents. Ignored for other modes."
                        }
                    },
                    "required": ["task"]
                }),
                output_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "agent_id": {
                            "type": "string",
                            "description": "The spawned agent's runtime ID."
                        }
                    }
                })),
                channel_class: ChannelClass::Internal,
                side_effects: true,
                approval: ToolApproval::Auto,
                annotations: ToolAnnotations {
                    title: "Spawn Agent".to_string(),
                    read_only_hint: Some(false),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(false),
                    open_world_hint: Some(false),
                },
            },
        }
    }
}

impl Tool for SpawnAgentTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, _input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async {
            Err(ToolError::ExecutionFailed(
                "core.spawn_agent is handled by the loop's agent orchestrator, not direct execution"
                    .to_string(),
            ))
        })
    }
}
