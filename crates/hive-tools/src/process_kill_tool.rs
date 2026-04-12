use std::sync::Arc;

use hive_classification::{ChannelClass, DataClass};
use hive_contracts::{ToolAnnotations, ToolApproval, ToolDefinition};
use hive_process::ProcessManager;
use serde_json::{json, Value};

use crate::{BoxFuture, Tool, ToolError, ToolResult};

/// Terminate a background process by sending a signal.
pub struct ProcessKillTool {
    definition: ToolDefinition,
    manager: Arc<ProcessManager>,
}

impl ProcessKillTool {
    pub fn new(manager: Arc<ProcessManager>) -> Self {
        Self {
            definition: ToolDefinition {
                id: "process.kill".to_string(),
                name: "Kill process".to_string(),
                description: concat!(
                    "Terminate a background process. Sends SIGTERM by default. ",
                    "Use signal parameter for SIGKILL, SIGINT, or SIGHUP."
                )
                .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "process_id": {
                            "type": "string",
                            "description": "The process ID returned by process.start."
                        },
                        "signal": {
                            "type": "string",
                            "description": "Signal to send: SIGTERM (default), SIGKILL, SIGINT, SIGHUP.",
                            "enum": ["SIGTERM", "SIGKILL", "SIGINT", "SIGHUP"]
                        }
                    },
                    "required": ["process_id"]
                }),
                output_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "success": { "type": "boolean" },
                        "status": { "type": "object" }
                    }
                })),
                channel_class: ChannelClass::LocalOnly,
                side_effects: true,
                approval: ToolApproval::Ask,
                annotations: ToolAnnotations {
                    title: "Kill process".to_string(),
                    read_only_hint: Some(false),
                    destructive_hint: Some(true),
                    idempotent_hint: Some(true),
                    open_world_hint: Some(false),
                },
            },
            manager,
        }
    }
}

impl Tool for ProcessKillTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let process_id = input.get("process_id").and_then(|v| v.as_str()).ok_or_else(|| {
                ToolError::InvalidInput("missing required field `process_id`".to_string())
            })?;

            let signal = input.get("signal").and_then(|v| v.as_str());

            let info = self.manager.kill(process_id, signal).map_err(ToolError::ExecutionFailed)?;

            let status_json = serde_json::to_value(&info.status).unwrap_or(json!("unknown"));

            Ok(ToolResult {
                output: json!({ "success": true, "status": status_json }),
                data_class: DataClass::Internal,
            })
        })
    }
}
