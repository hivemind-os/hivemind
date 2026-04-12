use std::sync::Arc;

use hive_classification::{ChannelClass, DataClass};
use hive_contracts::{ToolAnnotations, ToolApproval, ToolDefinition};
use hive_process::ProcessManager;
use serde_json::{json, Value};

use crate::{BoxFuture, Tool, ToolError, ToolResult};

/// Write data to the stdin of a background process.
pub struct ProcessWriteTool {
    definition: ToolDefinition,
    manager: Arc<ProcessManager>,
}

impl ProcessWriteTool {
    pub fn new(manager: Arc<ProcessManager>) -> Self {
        Self {
            definition: ToolDefinition {
                id: "process.write".to_string(),
                name: "Write to process".to_string(),
                description: concat!(
                    "Write text to the stdin of a running background process (identified by process_id from process.start). ",
                    "This is for sending keyboard/terminal input to interactive CLI programs — ",
                    "NOT for writing files (use filesystem.write) or sending messages."
                )
                .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "process_id": {
                            "type": "string",
                            "description": "The process ID returned by process.start."
                        },
                        "input": {
                            "type": "string",
                            "description": "Text to write to the process stdin. Include \\n for newlines."
                        }
                    },
                    "required": ["process_id", "input"]
                }),
                output_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "success": { "type": "boolean" }
                    }
                })),
                channel_class: ChannelClass::LocalOnly,
                side_effects: true,
                approval: ToolApproval::Ask,
                annotations: ToolAnnotations {
                    title: "Write to process".to_string(),
                    read_only_hint: Some(false),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(false),
                    open_world_hint: Some(false),
                },
            },
            manager,
        }
    }
}

impl Tool for ProcessWriteTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let process_id = input.get("process_id").and_then(|v| v.as_str()).ok_or_else(|| {
                ToolError::InvalidInput("missing required field `process_id`".to_string())
            })?;

            let text = input.get("input").and_then(|v| v.as_str()).ok_or_else(|| {
                ToolError::InvalidInput("missing required field `input`".to_string())
            })?;

            self.manager.write_stdin(process_id, text).map_err(ToolError::ExecutionFailed)?;

            Ok(ToolResult { output: json!({ "success": true }), data_class: DataClass::Internal })
        })
    }
}
