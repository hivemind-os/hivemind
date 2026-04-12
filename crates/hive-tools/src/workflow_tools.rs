use crate::{BoxFuture, Tool, ToolError, ToolResult};
use hive_classification::{ChannelClass, DataClass};
use hive_contracts::{ToolAnnotations, ToolApproval, ToolDefinition};
use hive_workflow_service::WorkflowService;
use serde_json::{json, Value};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// workflow.launch — Launch a new workflow instance
// ---------------------------------------------------------------------------

pub struct WorkflowLaunchTool {
    definition: ToolDefinition,
    service: Arc<WorkflowService>,
    session_id: Option<String>,
    workspace_path: Option<String>,
}

impl WorkflowLaunchTool {
    pub fn new(
        service: Arc<WorkflowService>,
        session_id: Option<String>,
        workspace_path: Option<String>,
    ) -> Self {
        Self {
            definition: ToolDefinition {
                id: "workflow.launch".to_string(),
                name: "Launch Workflow".to_string(),
                description: "Launch a new workflow instance from a saved definition. \
                    Specify the definition by name or by id. Provide trigger_step_id to select \
                    which trigger to fire when the workflow has multiple triggers."
                    .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "definition": {
                            "type": "string",
                            "description": "Namespace-qualified name of the workflow definition to launch, e.g. \"user/my-workflow\" (use this or `id`)"
                        },
                        "id": {
                            "type": "string",
                            "description": "Immutable ID of the workflow definition (alternative to `definition` namespaced name)"
                        },
                        "version": {
                            "type": "string",
                            "description": "Version of the definition (uses latest if omitted, only used with `definition`)"
                        },
                        "trigger_step_id": {
                            "type": "string",
                            "description": "Step ID of the trigger to fire (required when the workflow has multiple triggers)"
                        },
                        "inputs": {
                            "type": "object",
                            "description": "Trigger input values for the workflow"
                        }
                    }
                }),
                output_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "instance_id": { "type": "integer" },
                        "status": { "type": "string" }
                    }
                })),
                channel_class: ChannelClass::Internal,
                side_effects: true,
                approval: ToolApproval::Ask,
                annotations: ToolAnnotations {
                    title: "Launch Workflow".to_string(),
                    read_only_hint: Some(false),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(false),
                    open_world_hint: Some(false),
                },
            },
            service,
            session_id,
            workspace_path,
        }
    }
}

impl Tool for WorkflowLaunchTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let def_name = input.get("definition").and_then(|v| v.as_str());
            let def_id = input.get("id").and_then(|v| v.as_str());
            let version = input.get("version").and_then(|v| v.as_str());
            let trigger_step_id = input.get("trigger_step_id").and_then(|v| v.as_str());
            let inputs = input.get("inputs").cloned().unwrap_or(Value::Object(Default::default()));
            let session_id = self.session_id.as_deref().unwrap_or("unknown");

            // Resolve definition: by id (preferred) or by name
            let (def, _yaml) = if let Some(id) = def_id {
                self.service
                    .get_definition_by_id(id)
                    .await
                    .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?
            } else if let Some(name) = def_name {
                if let Some(v) = version {
                    self.service
                        .get_definition(name, v)
                        .await
                        .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?
                } else {
                    self.service
                        .get_latest_definition(name)
                        .await
                        .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?
                }
            } else {
                return Err(ToolError::InvalidInput(
                    "must provide either `definition` (name) or `id`".into(),
                ));
            };

            let instance_id = self
                .service
                .launch(
                    &def.name,
                    Some(def.version.as_str()),
                    inputs,
                    session_id,
                    None,
                    None,
                    trigger_step_id,
                    self.workspace_path.as_deref(),
                )
                .await
                .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

            Ok(ToolResult {
                output: json!({ "instance_id": instance_id, "status": "running" }),
                data_class: DataClass::Internal,
            })
        })
    }
}

// ---------------------------------------------------------------------------
// workflow.status — Get workflow instance status
// ---------------------------------------------------------------------------

pub struct WorkflowStatusTool {
    definition: ToolDefinition,
    service: Arc<WorkflowService>,
}

impl WorkflowStatusTool {
    pub fn new(service: Arc<WorkflowService>) -> Self {
        Self {
            definition: ToolDefinition {
                id: "workflow.status".to_string(),
                name: "Workflow Status".to_string(),
                description: "Get the current status and state of a workflow instance.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "instance_id": {
                            "type": "integer",
                            "description": "The workflow instance ID"
                        }
                    },
                    "required": ["instance_id"]
                }),
                output_schema: None,
                channel_class: ChannelClass::Internal,
                side_effects: false,
                approval: ToolApproval::Auto,
                annotations: ToolAnnotations {
                    title: "Workflow Status".to_string(),
                    read_only_hint: Some(true),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(true),
                    open_world_hint: Some(false),
                },
            },
            service,
        }
    }
}

impl Tool for WorkflowStatusTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let instance_id = input
                .get("instance_id")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| ToolError::InvalidInput("missing `instance_id`".into()))?;

            let instance = self
                .service
                .get_instance(instance_id)
                .await
                .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

            let output = serde_json::to_value(&instance)
                .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

            Ok(ToolResult { output, data_class: DataClass::Internal })
        })
    }
}

// ---------------------------------------------------------------------------
// workflow.list — List workflow instances
// ---------------------------------------------------------------------------

pub struct WorkflowListTool {
    definition: ToolDefinition,
    service: Arc<WorkflowService>,
}

impl WorkflowListTool {
    pub fn new(service: Arc<WorkflowService>) -> Self {
        Self {
            definition: ToolDefinition {
                id: "workflow.list".to_string(),
                name: "List Workflows".to_string(),
                description: "List workflow instances, optionally filtered by status, definition name (namespaced, e.g. \"user/my-workflow\"), definition ID, or session."
                    .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "status": {
                            "type": "string",
                            "description": "Filter by status (pending, running, paused, completed, failed, killed)"
                        },
                        "definition": {
                            "type": "string",
                            "description": "Filter by definition name (namespaced, e.g. \"user/my-workflow\")"
                        },
                        "id": {
                            "type": "string",
                            "description": "Filter by workflow definition ID"
                        },
                        "session_id": {
                            "type": "string",
                            "description": "Filter by parent session ID"
                        }
                    }
                }),
                output_schema: None,
                channel_class: ChannelClass::Internal,
                side_effects: false,
                approval: ToolApproval::Auto,
                annotations: ToolAnnotations {
                    title: "List Workflows".to_string(),
                    read_only_hint: Some(true),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(true),
                    open_world_hint: Some(false),
                },
            },
            service,
        }
    }
}

impl Tool for WorkflowListTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let filter = hive_workflow_service::hive_workflow::InstanceFilter {
                statuses: input
                    .get("status")
                    .and_then(|v| v.as_str())
                    .and_then(|s| {
                        serde_json::from_value::<
                            hive_workflow_service::hive_workflow::WorkflowStatus,
                        >(Value::String(s.to_string()))
                        .ok()
                    })
                    .into_iter()
                    .collect(),
                definition_names: input
                    .get("definition")
                    .and_then(|v| v.as_str())
                    .map(String::from)
                    .into_iter()
                    .collect(),
                definition_id: input.get("id").and_then(|v| v.as_str()).map(String::from),
                parent_session_id: input
                    .get("session_id")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                parent_agent_id: None,
                ..Default::default()
            };

            let result = self
                .service
                .list_instances(&filter)
                .await
                .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

            let output = serde_json::to_value(&result.items)
                .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

            Ok(ToolResult { output, data_class: DataClass::Internal })
        })
    }
}

// ---------------------------------------------------------------------------
// workflow.pause — Pause a running workflow
// ---------------------------------------------------------------------------

pub struct WorkflowPauseTool {
    definition: ToolDefinition,
    service: Arc<WorkflowService>,
}

impl WorkflowPauseTool {
    pub fn new(service: Arc<WorkflowService>) -> Self {
        Self {
            definition: ToolDefinition {
                id: "workflow.pause".to_string(),
                name: "Pause Workflow".to_string(),
                description: "Pause a running workflow instance.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "instance_id": {
                            "type": "integer",
                            "description": "The workflow instance ID to pause"
                        }
                    },
                    "required": ["instance_id"]
                }),
                output_schema: None,
                channel_class: ChannelClass::Internal,
                side_effects: true,
                approval: ToolApproval::Ask,
                annotations: ToolAnnotations {
                    title: "Pause Workflow".to_string(),
                    read_only_hint: Some(false),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(true),
                    open_world_hint: Some(false),
                },
            },
            service,
        }
    }
}

impl Tool for WorkflowPauseTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let id = input
                .get("instance_id")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| ToolError::InvalidInput("missing `instance_id`".into()))?;
            self.service.pause(id).await.map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
            Ok(ToolResult {
                output: json!({ "ok": true, "instance_id": id }),
                data_class: DataClass::Internal,
            })
        })
    }
}

// ---------------------------------------------------------------------------
// workflow.resume — Resume a paused workflow
// ---------------------------------------------------------------------------

pub struct WorkflowResumeTool {
    definition: ToolDefinition,
    service: Arc<WorkflowService>,
}

impl WorkflowResumeTool {
    pub fn new(service: Arc<WorkflowService>) -> Self {
        Self {
            definition: ToolDefinition {
                id: "workflow.resume".to_string(),
                name: "Resume Workflow".to_string(),
                description: "Resume a paused workflow instance.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "instance_id": {
                            "type": "integer",
                            "description": "The workflow instance ID to resume"
                        }
                    },
                    "required": ["instance_id"]
                }),
                output_schema: None,
                channel_class: ChannelClass::Internal,
                side_effects: true,
                approval: ToolApproval::Ask,
                annotations: ToolAnnotations {
                    title: "Resume Workflow".to_string(),
                    read_only_hint: Some(false),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(true),
                    open_world_hint: Some(false),
                },
            },
            service,
        }
    }
}

impl Tool for WorkflowResumeTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let id = input
                .get("instance_id")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| ToolError::InvalidInput("missing `instance_id`".into()))?;
            self.service.resume(id).await.map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
            Ok(ToolResult {
                output: json!({ "ok": true, "instance_id": id }),
                data_class: DataClass::Internal,
            })
        })
    }
}

// ---------------------------------------------------------------------------
// workflow.kill — Kill a running workflow
// ---------------------------------------------------------------------------

pub struct WorkflowKillTool {
    definition: ToolDefinition,
    service: Arc<WorkflowService>,
}

impl WorkflowKillTool {
    pub fn new(service: Arc<WorkflowService>) -> Self {
        Self {
            definition: ToolDefinition {
                id: "workflow.kill".to_string(),
                name: "Kill Workflow".to_string(),
                description: "Forcefully kill a running or paused workflow instance.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "instance_id": {
                            "type": "integer",
                            "description": "The workflow instance ID to kill"
                        }
                    },
                    "required": ["instance_id"]
                }),
                output_schema: None,
                channel_class: ChannelClass::Internal,
                side_effects: true,
                approval: ToolApproval::Ask,
                annotations: ToolAnnotations {
                    title: "Kill Workflow".to_string(),
                    read_only_hint: Some(false),
                    destructive_hint: Some(true),
                    idempotent_hint: Some(true),
                    open_world_hint: Some(false),
                },
            },
            service,
        }
    }
}

impl Tool for WorkflowKillTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let id = input
                .get("instance_id")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| ToolError::InvalidInput("missing `instance_id`".into()))?;
            self.service.kill(id).await.map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
            Ok(ToolResult {
                output: json!({ "ok": true, "instance_id": id }),
                data_class: DataClass::Internal,
            })
        })
    }
}

// ---------------------------------------------------------------------------
// workflow.respond — Respond to a feedback gate
// ---------------------------------------------------------------------------

pub struct WorkflowRespondTool {
    definition: ToolDefinition,
    service: Arc<WorkflowService>,
}

impl WorkflowRespondTool {
    pub fn new(service: Arc<WorkflowService>) -> Self {
        Self {
            definition: ToolDefinition {
                id: "workflow.respond".to_string(),
                name: "Respond to Workflow Gate".to_string(),
                description: "Respond to a feedback gate or approval step in a workflow. \
                    Provide the instance ID, step ID, and the response value."
                    .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "instance_id": {
                            "type": "integer",
                            "description": "The workflow instance ID"
                        },
                        "step_id": {
                            "type": "string",
                            "description": "The step ID waiting for a response"
                        },
                        "response": {
                            "description": "The response value to provide to the gate"
                        }
                    },
                    "required": ["instance_id", "step_id", "response"]
                }),
                output_schema: None,
                channel_class: ChannelClass::Internal,
                side_effects: true,
                approval: ToolApproval::Ask,
                annotations: ToolAnnotations {
                    title: "Respond to Workflow Gate".to_string(),
                    read_only_hint: Some(false),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(false),
                    open_world_hint: Some(false),
                },
            },
            service,
        }
    }
}

impl Tool for WorkflowRespondTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let instance_id = input
                .get("instance_id")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| ToolError::InvalidInput("missing `instance_id`".into()))?;
            let step_id = input
                .get("step_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("missing `step_id`".into()))?;
            let response = input
                .get("response")
                .cloned()
                .ok_or_else(|| ToolError::InvalidInput("missing `response`".into()))?;

            self.service
                .respond_to_gate(instance_id, step_id, response)
                .await
                .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

            Ok(ToolResult { output: json!({ "ok": true }), data_class: DataClass::Internal })
        })
    }
}
