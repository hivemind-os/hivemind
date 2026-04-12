use std::sync::Arc;

use hive_classification::{ChannelClass, DataClass};
use hive_contracts::{ToolAnnotations, ToolApproval, ToolDefinition};
use hive_process::ProcessManager;
use serde_json::{json, Value};

use crate::{BoxFuture, Tool, ToolError, ToolResult};

/// Check the status and recent output of a background process.
pub struct ProcessStatusTool {
    definition: ToolDefinition,
    manager: Arc<ProcessManager>,
}

impl ProcessStatusTool {
    pub fn new(manager: Arc<ProcessManager>) -> Self {
        Self {
            definition: ToolDefinition {
                id: "process.status".to_string(),
                name: "Process status".to_string(),
                description: concat!(
                    "Get the status and recent output of a background process. ",
                    "Use `tail_lines` to limit the output to the last N lines."
                )
                .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "process_id": {
                            "type": "string",
                            "description": "The process ID returned by process.start."
                        },
                        "tail_lines": {
                            "type": "number",
                            "description": "Number of lines to return from the end of output. Returns all output if omitted."
                        }
                    },
                    "required": ["process_id"]
                }),
                output_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string" },
                        "pid": { "type": "number" },
                        "command": { "type": "string" },
                        "status": { "type": "object" },
                        "uptime_secs": { "type": "number" },
                        "output": { "type": "string" }
                    }
                })),
                channel_class: ChannelClass::LocalOnly,
                side_effects: false,
                approval: ToolApproval::Auto,
                annotations: ToolAnnotations {
                    title: "Process status".to_string(),
                    read_only_hint: Some(true),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(true),
                    open_world_hint: Some(false),
                },
            },
            manager,
        }
    }
}

impl Tool for ProcessStatusTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let process_id = input.get("process_id").and_then(|v| v.as_str()).ok_or_else(|| {
                ToolError::InvalidInput("missing required field `process_id`".to_string())
            })?;

            let tail_lines = input.get("tail_lines").and_then(|v| v.as_u64()).map(|v| v as usize);

            let (info, output) =
                self.manager.status(process_id, tail_lines).map_err(ToolError::ExecutionFailed)?;

            let status_json = serde_json::to_value(&info.status).unwrap_or(json!("unknown"));

            Ok(ToolResult {
                output: json!({
                    "id": info.id,
                    "pid": info.pid,
                    "command": info.command,
                    "working_dir": info.working_dir,
                    "status": status_json,
                    "uptime_secs": info.uptime_secs,
                    "output": output,
                }),
                data_class: DataClass::Internal,
            })
        })
    }
}
