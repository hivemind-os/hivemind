use std::sync::Arc;

use hive_classification::{ChannelClass, DataClass};
use hive_contracts::{ToolAnnotations, ToolApproval, ToolDefinition};
use hive_process::ProcessManager;
use serde_json::{json, Value};

use crate::{BoxFuture, Tool, ToolError, ToolResult};

/// List all managed background processes.
pub struct ProcessListTool {
    definition: ToolDefinition,
    manager: Arc<ProcessManager>,
}

impl ProcessListTool {
    pub fn new(manager: Arc<ProcessManager>) -> Self {
        Self {
            definition: ToolDefinition {
                id: "process.list".to_string(),
                name: "List processes".to_string(),
                description: "List all managed background processes with their status.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {}
                }),
                output_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "processes": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string" },
                                    "pid": { "type": "number" },
                                    "command": { "type": "string" },
                                    "status": { "type": "object" },
                                    "uptime_secs": { "type": "number" }
                                }
                            }
                        }
                    }
                })),
                channel_class: ChannelClass::LocalOnly,
                side_effects: false,
                approval: ToolApproval::Auto,
                annotations: ToolAnnotations {
                    title: "List processes".to_string(),
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

impl Tool for ProcessListTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, _input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let processes: Vec<Value> = self
                .manager
                .list()
                .into_iter()
                .map(|info| {
                    let status_json =
                        serde_json::to_value(&info.status).unwrap_or(json!("unknown"));
                    json!({
                        "id": info.id,
                        "pid": info.pid,
                        "command": info.command,
                        "working_dir": info.working_dir,
                        "status": status_json,
                        "uptime_secs": info.uptime_secs,
                    })
                })
                .collect();

            Ok(ToolResult {
                output: json!({ "processes": processes }),
                data_class: DataClass::Internal,
            })
        })
    }
}
