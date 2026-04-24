use crate::{BoxFuture, Tool, ToolError, ToolRegistry, ToolResult};
use hive_classification::{ChannelClass, DataClass};
use hive_connectors::ConnectorRegistry;
use hive_contracts::{ToolAnnotations, ToolApproval, ToolDefinition};
use hive_workflow_service::WorkflowService;
use serde_json::{json, Value};
use std::sync::Arc;

/// Derive a human-readable category from a tool ID prefix.
fn tool_category(tool_id: &str) -> &'static str {
    if let Some(prefix) = tool_id.split('.').next() {
        match prefix {
            "filesystem" | "fs" => "File System",
            "http" => "HTTP / Web Requests",
            "connector" => "Connectors / Messaging",
            "git" => "Git / Version Control",
            "shell" | "process" => "Shell / Process",
            "core" => "Core / System",
            "workflow" | "workflow_author" => "Workflow",
            "mcp" => "MCP Servers",
            "scheduler" => "Scheduling",
            "search" => "Search",
            "database" | "db" => "Database",
            _ => "Other",
        }
    } else {
        "Other"
    }
}

// ---------------------------------------------------------------------------
// workflow_author.list_available_tools — Discover tools for call_tool steps
// ---------------------------------------------------------------------------

pub struct WfAuthorListToolsTool {
    definition: ToolDefinition,
    registry: Arc<ToolRegistry>,
}

impl WfAuthorListToolsTool {
    pub fn new(registry: Arc<ToolRegistry>) -> Self {
        Self {
            definition: ToolDefinition {
                id: "workflow_author.list_available_tools".to_string(),
                name: "List Available Tools".to_string(),
                description: "List tools that can be used in call_tool workflow steps. \
                    Returns tool ID, name, description, and category. Use the filter parameter to search \
                    by keyword — don't list all tools unless you need to browse. Use get_tool_details for full schemas."
                    .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "filter": {
                            "type": "string",
                            "description": "Text filter to search tool names and descriptions (e.g., 'email', 'http', 'file'). Strongly recommended to narrow results."
                        },
                        "category": {
                            "type": "string",
                            "description": "Filter by category: 'File System', 'HTTP / Web Requests', 'Connectors / Messaging', 'Git / Version Control', 'Shell / Process', 'Core / System', 'MCP Servers', 'Scheduling', 'Search', 'Database', 'Other'"
                        },
                        "limit": {
                            "type": "number",
                            "description": "Maximum number of results to return (default: 50)"
                        }
                    }
                }),
                output_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "tools": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string" },
                                    "name": { "type": "string" },
                                    "description": { "type": "string" }
                                }
                            }
                        },
                        "total": { "type": "number" }
                    }
                })),
                channel_class: ChannelClass::Internal,
                side_effects: false,
                approval: ToolApproval::Auto,
                annotations: ToolAnnotations {
                    title: "List Available Tools".to_string(),
                    read_only_hint: Some(true),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(true),
                    open_world_hint: Some(false),
                },
            },
            registry,
        }
    }
}

impl Tool for WfAuthorListToolsTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let filter = input.get("filter").and_then(|v| v.as_str()).map(|s| s.to_lowercase());

            let category_filter =
                input.get("category").and_then(|v| v.as_str()).map(|s| s.to_lowercase());

            let limit = input.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;

            let mut scored_tools: Vec<(u8, Value)> = self
                .registry
                .list_definitions()
                .into_iter()
                // Exclude the workflow_author tools themselves
                .filter(|d| !d.id.starts_with("workflow_author."))
                .filter(|d| {
                    if let Some(ref cat) = category_filter {
                        tool_category(&d.id).to_lowercase().contains(cat)
                    } else {
                        true
                    }
                })
                .filter(|d| {
                    if let Some(ref f) = filter {
                        d.id.to_lowercase().contains(f)
                            || d.name.to_lowercase().contains(f)
                            || d.description.to_lowercase().contains(f)
                    } else {
                        true
                    }
                })
                .map(|d| {
                    // Compute a relevance score for sorting when a filter is applied
                    let score = if let Some(ref f) = filter {
                        let mut s = 0u8;
                        if d.id.to_lowercase().starts_with(f) {
                            s += 3;
                        } else if d.id.to_lowercase().contains(f) {
                            s += 2;
                        }
                        if d.name.to_lowercase().contains(f) {
                            s += 1;
                        }
                        s
                    } else {
                        0
                    };
                    (
                        score,
                        json!({
                            "id": d.id,
                            "name": d.name,
                            "description": d.description,
                            "category": tool_category(&d.id)
                        }),
                    )
                })
                .collect();

            // Sort by relevance (highest first) when filtering
            if filter.is_some() {
                scored_tools.sort_by(|a, b| b.0.cmp(&a.0));
            }

            let total = scored_tools.len();
            let tools: Vec<Value> = scored_tools.into_iter().take(limit).map(|(_, v)| v).collect();
            let shown = tools.len();

            Ok(ToolResult {
                output: json!({
                    "tools": tools,
                    "shown": shown,
                    "total": total,
                    "hint": if total > shown {
                        format!("Showing {} of {} tools. Use a more specific filter or increase limit.", shown, total)
                    } else {
                        format!("{} tools found.", total)
                    }
                }),
                data_class: DataClass::Internal,
            })
        })
    }
}

// ---------------------------------------------------------------------------
// workflow_author.get_tool_details — Full details about a specific tool
// ---------------------------------------------------------------------------

pub struct WfAuthorGetToolDetailsTool {
    definition: ToolDefinition,
    registry: Arc<ToolRegistry>,
}

impl WfAuthorGetToolDetailsTool {
    pub fn new(registry: Arc<ToolRegistry>) -> Self {
        Self {
            definition: ToolDefinition {
                id: "workflow_author.get_tool_details".to_string(),
                name: "Get Tool Details".to_string(),
                description: "Get full details about a specific tool including its input schema, \
                    output schema, and annotations. Use this to understand a tool's parameters \
                    before referencing it in a call_tool step."
                    .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "tool_id": {
                            "type": "string",
                            "description": "The ID of the tool to get details for"
                        }
                    },
                    "required": ["tool_id"]
                }),
                output_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string" },
                        "name": { "type": "string" },
                        "description": { "type": "string" },
                        "input_schema": { "type": "object" },
                        "output_schema": {},
                        "side_effects": { "type": "boolean" },
                        "annotations": { "type": "object" }
                    }
                })),
                channel_class: ChannelClass::Internal,
                side_effects: false,
                approval: ToolApproval::Auto,
                annotations: ToolAnnotations {
                    title: "Get Tool Details".to_string(),
                    read_only_hint: Some(true),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(true),
                    open_world_hint: Some(false),
                },
            },
            registry,
        }
    }
}

impl Tool for WfAuthorGetToolDetailsTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let tool_id = input
                .get("tool_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("tool_id is required".to_string()))?;

            let tool = self
                .registry
                .get(tool_id)
                .ok_or_else(|| ToolError::ExecutionFailed(format!("Tool '{tool_id}' not found")))?;

            let def = tool.definition();
            Ok(ToolResult {
                output: json!({
                    "id": def.id,
                    "name": def.name,
                    "description": def.description,
                    "input_schema": def.input_schema,
                    "output_schema": def.output_schema,
                    "side_effects": def.side_effects,
                    "channel_class": serde_json::to_value(def.channel_class).unwrap_or(json!("unknown")),
                    "annotations": {
                        "title": def.annotations.title,
                        "read_only": def.annotations.read_only_hint,
                        "destructive": def.annotations.destructive_hint,
                        "idempotent": def.annotations.idempotent_hint
                    }
                }),
                data_class: DataClass::Internal,
            })
        })
    }
}

// ---------------------------------------------------------------------------
// workflow_author.list_connectors — List configured connectors
// ---------------------------------------------------------------------------

pub struct WfAuthorListConnectorsTool {
    definition: ToolDefinition,
    registry: Option<Arc<ConnectorRegistry>>,
}

impl WfAuthorListConnectorsTool {
    pub fn new(registry: Option<Arc<ConnectorRegistry>>) -> Self {
        Self {
            definition: ToolDefinition {
                id: "workflow_author.list_connectors".to_string(),
                name: "List Connectors".to_string(),
                description:
                    "List configured communication connectors (email, Slack, Discord, etc.) \
                    and their capabilities. Use connector IDs for incoming_message triggers."
                        .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {}
                }),
                output_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "connectors": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string" },
                                    "display_name": { "type": "string" },
                                    "provider": { "type": "string" },
                                    "status": { "type": "string" },
                                    "has_communication": { "type": "boolean" },
                                    "has_calendar": { "type": "boolean" },
                                    "has_drive": { "type": "boolean" },
                                    "has_contacts": { "type": "boolean" },
                                    "channels": {
                                        "type": "array",
                                        "description": "Available channels/folders for communication connectors",
                                        "items": {
                                            "type": "object",
                                            "properties": {
                                                "id": { "type": "string" },
                                                "name": { "type": "string" },
                                                "channel_type": { "type": "string" },
                                                "group_name": { "type": "string" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                })),
                channel_class: ChannelClass::Internal,
                side_effects: false,
                approval: ToolApproval::Auto,
                annotations: ToolAnnotations {
                    title: "List Connectors".to_string(),
                    read_only_hint: Some(true),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(true),
                    open_world_hint: Some(false),
                },
            },
            registry,
        }
    }
}

impl Tool for WfAuthorListConnectorsTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, _input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let connectors: Vec<Value> = if let Some(ref registry) = self.registry {
                let mut results = Vec::new();
                for c in registry.list() {
                    let mut entry = json!({
                        "id": c.id(),
                        "display_name": c.display_name(),
                        "provider": format!("{:?}", c.provider()),
                        "status": serde_json::to_value(c.status()).unwrap_or(json!("unknown")),
                        "has_communication": c.communication().is_some(),
                        "has_calendar": c.calendar().is_some(),
                        "has_drive": c.drive().is_some(),
                        "has_contacts": c.contacts().is_some()
                    });
                    // Include channel details for communication-capable connectors
                    if let Some(comm) = c.communication() {
                        if let Ok(Ok(channels)) = tokio::time::timeout(
                            std::time::Duration::from_secs(5),
                            comm.list_channels(),
                        )
                        .await
                        {
                            let channel_list: Vec<Value> = channels
                                .iter()
                                .take(30)
                                .map(|ch| {
                                    let mut v = json!({
                                        "id": ch.id,
                                        "name": ch.name,
                                    });
                                    if let Some(ref t) = ch.channel_type {
                                        v["channel_type"] = json!(t);
                                    }
                                    if let Some(ref g) = ch.group_name {
                                        v["group_name"] = json!(g);
                                    }
                                    v
                                })
                                .collect();
                            if !channel_list.is_empty() {
                                entry["channels"] = json!(channel_list);
                            }
                        }
                    }
                    results.push(entry);
                }
                results
            } else {
                vec![]
            };

            Ok(ToolResult {
                output: json!({ "connectors": connectors }),
                data_class: DataClass::Internal,
            })
        })
    }
}

// ---------------------------------------------------------------------------
// workflow_author.list_personas — List available agent personas
// ---------------------------------------------------------------------------

pub struct WfAuthorListPersonasTool {
    definition: ToolDefinition,
    personas: Arc<parking_lot::Mutex<Vec<hive_contracts::Persona>>>,
}

impl WfAuthorListPersonasTool {
    pub fn new(personas: Arc<parking_lot::Mutex<Vec<hive_contracts::Persona>>>) -> Self {
        Self {
            definition: ToolDefinition {
                id: "workflow_author.list_personas".to_string(),
                name: "List Personas".to_string(),
                description: "List available agent personas that can be used in invoke_agent \
                    workflow steps. Returns persona ID, name, description, and capabilities."
                    .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {}
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
                                    "description": { "type": "string" },
                                    "loop_strategy": { "type": "string" },
                                    "allowed_tools": { "type": "array" }
                                }
                            }
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
            personas,
        }
    }
}

impl Tool for WfAuthorListPersonasTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, _input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let personas: Vec<Value> = self
                .personas
                .lock()
                .iter()
                .map(|p| {
                    json!({
                        "id": p.id,
                        "name": p.name,
                        "description": p.description,
                        "loop_strategy": serde_json::to_value(&p.loop_strategy).unwrap_or(json!("react")),
                        "allowed_tools": p.allowed_tools
                    })
                })
                .collect();

            Ok(ToolResult {
                output: json!({ "personas": personas }),
                data_class: DataClass::Internal,
            })
        })
    }
}

// ---------------------------------------------------------------------------
// workflow_author.list_event_topics — List event bus topics
// ---------------------------------------------------------------------------

/// An event topic with description and payload keys, injected at construction.
#[derive(Debug, Clone)]
pub struct EventTopicInfo {
    pub topic: String,
    pub description: String,
    pub payload_keys: Vec<String>,
}

pub struct WfAuthorListEventTopicsTool {
    definition: ToolDefinition,
    topics: Vec<EventTopicInfo>,
}

impl WfAuthorListEventTopicsTool {
    pub fn new(topics: Vec<EventTopicInfo>) -> Self {
        Self {
            definition: ToolDefinition {
                id: "workflow_author.list_event_topics".to_string(),
                name: "List Event Topics".to_string(),
                description: "List available event bus topics that can be used in event_pattern \
                    triggers and event_gate steps. Returns topic name, description, and payload fields."
                    .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {}
                }),
                output_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "topics": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "topic": { "type": "string" },
                                    "description": { "type": "string" },
                                    "payload_keys": { "type": "array", "items": { "type": "string" } }
                                }
                            }
                        }
                    }
                })),
                channel_class: ChannelClass::Internal,
                side_effects: false,
                approval: ToolApproval::Auto,
                annotations: ToolAnnotations {
                    title: "List Event Topics".to_string(),
                    read_only_hint: Some(true),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(true),
                    open_world_hint: Some(false),
                },
            },
            topics,
        }
    }
}

impl Tool for WfAuthorListEventTopicsTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, _input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let topics: Vec<Value> = self
                .topics
                .iter()
                .map(|t| {
                    json!({
                        "topic": t.topic,
                        "description": t.description,
                        "payload_keys": t.payload_keys
                    })
                })
                .collect();

            Ok(ToolResult { output: json!({ "topics": topics }), data_class: DataClass::Internal })
        })
    }
}

// ---------------------------------------------------------------------------
// workflow_author.list_workflows — List existing workflow definitions
// ---------------------------------------------------------------------------

pub struct WfAuthorListWorkflowsTool {
    definition: ToolDefinition,
    service: Arc<WorkflowService>,
}

impl WfAuthorListWorkflowsTool {
    pub fn new(service: Arc<WorkflowService>) -> Self {
        Self {
            definition: ToolDefinition {
                id: "workflow_author.list_workflows".to_string(),
                name: "List Workflow Definitions".to_string(),
                description: "List existing workflow definitions that can be referenced in \
                    launch_workflow steps. Returns namespaced name (e.g. \"user/my-workflow\"), version, description, and trigger types."
                    .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {}
                }),
                output_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "workflows": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string" },
                                    "name": { "type": "string" },
                                    "version": { "type": "string" },
                                    "description": { "type": "string" },
                                    "trigger_types": { "type": "array", "items": { "type": "string" } },
                                    "step_count": { "type": "number" }
                                }
                            }
                        }
                    }
                })),
                channel_class: ChannelClass::Internal,
                side_effects: false,
                approval: ToolApproval::Auto,
                annotations: ToolAnnotations {
                    title: "List Workflow Definitions".to_string(),
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

impl Tool for WfAuthorListWorkflowsTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, _input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let defs = self
                .service
                .list_definitions()
                .await
                .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

            let workflows: Vec<Value> = defs
                .into_iter()
                .map(|d| {
                    json!({
                        "id": d.id,
                        "name": d.name,
                        "version": d.version,
                        "description": d.description,
                        "trigger_types": d.trigger_types,
                        "step_count": d.step_count
                    })
                })
                .collect();

            Ok(ToolResult {
                output: json!({ "workflows": workflows }),
                data_class: DataClass::Internal,
            })
        })
    }
}

// ---------------------------------------------------------------------------
// workflow_author.submit_workflow — Final tool: submit YAML + message
// ---------------------------------------------------------------------------

pub struct WfAuthorSubmitWorkflowTool {
    definition: ToolDefinition,
}

impl Default for WfAuthorSubmitWorkflowTool {
    fn default() -> Self {
        Self {
            definition: ToolDefinition {
                id: "workflow_author.submit_workflow".to_string(),
                name: "Submit Workflow".to_string(),
                description: "Submit the authored or modified workflow YAML and a message to the user. \
                    The YAML must be a valid, complete workflow definition with a namespace-qualified name \
                    (e.g. \"user/my-workflow\"). The message should summarize what was created or changed. \
                    After this succeeds, proceed to submit tests and then run them."
                    .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "yaml": {
                            "type": "string",
                            "description": "The complete workflow definition in YAML format"
                        },
                        "message": {
                            "type": "string",
                            "description": "A summary message for the user describing what was created or changed"
                        }
                    },
                    "required": ["yaml", "message"]
                }),
                output_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "success": { "type": "boolean" },
                        "error": { "type": "string" }
                    }
                })),
                channel_class: ChannelClass::Internal,
                side_effects: true,
                approval: ToolApproval::Auto,
                annotations: ToolAnnotations {
                    title: "Submit Workflow".to_string(),
                    read_only_hint: Some(false),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(false),
                    open_world_hint: Some(false),
                },
            },
        }
    }
}

impl Tool for WfAuthorSubmitWorkflowTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let yaml = input
                .get("yaml")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("yaml is required".to_string()))?;

            let _message = input
                .get("message")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("message is required".to_string()))?;

            // Validate by parsing the YAML as a WorkflowDefinition
            let parsed: Result<hive_workflow_service::hive_workflow::WorkflowDefinition, _> =
                serde_yaml::from_str(yaml);

            match parsed {
                Ok(def) => {
                    // Also run structural validation
                    if let Err(e) = hive_workflow_service::hive_workflow::validate_definition(&def)
                    {
                        Ok(ToolResult {
                            output: json!({
                                "success": false,
                                "error": format!("Workflow validation error: {}", e)
                            }),
                            data_class: DataClass::Internal,
                        })
                    } else {
                        // Run quality lint checks and include warnings
                        let mut lint_warnings: Vec<Value> = Vec::new();
                        let yaml_lower = yaml.to_lowercase();

                        for step in &def.steps {
                            if let hive_workflow_service::hive_workflow::StepType::Task { task } =
                                &step.step_type
                            {
                                // Check for missing timeouts on agent steps
                                if matches!(task,
                                    hive_workflow_service::hive_workflow::TaskDef::InvokeAgent { timeout_secs: None, .. } |
                                    hive_workflow_service::hive_workflow::TaskDef::InvokePrompt { timeout_secs: None, .. }
                                ) {
                                    lint_warnings.push(json!({
                                        "step": step.id,
                                        "warning": "Agent step has no timeout_secs. Consider adding one (e.g., 300)."
                                    }));
                                }
                                // Check for missing error handling on external calls
                                let needs_handler = matches!(task,
                                    hive_workflow_service::hive_workflow::TaskDef::CallTool { .. } |
                                    hive_workflow_service::hive_workflow::TaskDef::InvokeAgent { .. } |
                                    hive_workflow_service::hive_workflow::TaskDef::InvokePrompt { .. }
                                );
                                if needs_handler && step.on_error.is_none() {
                                    lint_warnings.push(json!({
                                        "step": step.id,
                                        "warning": "Task step has no on_error. Consider adding retry or skip strategy."
                                    }));
                                }
                            }
                        }

                        // Check for missing end_workflow
                        let has_end = def.steps.iter().any(|s| matches!(
                            &s.step_type,
                            hive_workflow_service::hive_workflow::StepType::ControlFlow {
                                control: hive_workflow_service::hive_workflow::ControlFlowDef::EndWorkflow
                            }
                        ));
                        if !has_end {
                            lint_warnings.push(json!({
                                "step": "(workflow)",
                                "warning": "No end_workflow step. Consider adding one as a terminal node."
                            }));
                        }

                        let _ = yaml_lower; // used for lint checks

                        let mut success_msg = "Workflow submitted successfully. Now generate test cases and call workflow_author.submit_tests.".to_string();
                        if !lint_warnings.is_empty() {
                            success_msg.push_str("\n\nNote: There are some optional quality suggestions you can mention to the user (these are non-blocking and do NOT require resubmission):");
                            for w in &lint_warnings {
                                if let (Some(step), Some(warning)) = (
                                    w.get("step").and_then(|v| v.as_str()),
                                    w.get("warning").and_then(|v| v.as_str()),
                                ) {
                                    success_msg.push_str(&format!("\n- {}: {}", step, warning));
                                }
                            }
                        }
                        let output = json!({
                            "success": true,
                            "message": success_msg
                        });
                        Ok(ToolResult { output, data_class: DataClass::Internal })
                    }
                }
                Err(e) => Ok(ToolResult {
                    output: json!({
                        "success": false,
                        "error": format!("YAML parse error: {}", e)
                    }),
                    data_class: DataClass::Internal,
                }),
            }
        })
    }
}

// ---------------------------------------------------------------------------
// workflow_author.submit_tests — Submit test cases for a workflow
// ---------------------------------------------------------------------------

pub struct WfAuthorSubmitTestsTool {
    definition: ToolDefinition,
}

impl Default for WfAuthorSubmitTestsTool {
    fn default() -> Self {
        Self {
            definition: ToolDefinition {
                id: "workflow_author.submit_tests".to_string(),
                name: "Submit Workflow Tests".to_string(),
                description: "Submit test cases for the workflow. Each test case defines trigger inputs, \
                    expected outcomes, and optional mock outputs for side-effecting steps. \
                    Call this after submit_workflow, then call workflow_author.run_tests to execute them."
                    .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "tests": {
                            "type": "array",
                            "description": "Array of test cases to add to the workflow",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "name": {
                                        "type": "string",
                                        "description": "Short kebab-case test name (e.g. 'happy-path', 'empty-input')"
                                    },
                                    "description": {
                                        "type": "string",
                                        "description": "What this test verifies"
                                    },
                                    "inputs": {
                                        "type": "object",
                                        "description": "Trigger inputs for this test case"
                                    },
                                    "shadow_outputs": {
                                        "type": "object",
                                        "description": "Per-step output overrides (step_id → mock output object). Use to mock agent or external tool outputs."
                                    },
                                    "expectations": {
                                        "type": "object",
                                        "properties": {
                                            "status": {
                                                "type": "string",
                                                "enum": ["completed", "failed"],
                                                "description": "Expected final workflow status"
                                            },
                                            "output": {
                                                "type": "object",
                                                "description": "Expected workflow output (partial match — extra keys OK)"
                                            },
                                            "steps_completed": {
                                                "type": "array",
                                                "items": { "type": "string" },
                                                "description": "Step IDs that should have completed"
                                            },
                                            "steps_not_reached": {
                                                "type": "array",
                                                "items": { "type": "string" },
                                                "description": "Step IDs that should NOT have executed"
                                            },
                                            "intercepted_action_counts": {
                                                "type": "object",
                                                "description": "Expected counts of intercepted actions (e.g. {\"tool_calls\": 3})"
                                            }
                                        }
                                    }
                                },
                                "required": ["name", "inputs", "expectations"]
                            }
                        },
                        "message": {
                            "type": "string",
                            "description": "Summary message for the user about the generated tests"
                        }
                    },
                    "required": ["tests", "message"]
                }),
                output_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "success": { "type": "boolean" },
                        "count": { "type": "integer" },
                        "error": { "type": "string" }
                    }
                })),
                channel_class: ChannelClass::Internal,
                side_effects: true,
                approval: ToolApproval::Auto,
                annotations: ToolAnnotations {
                    title: "Submit Workflow Tests".to_string(),
                    read_only_hint: Some(false),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(false),
                    open_world_hint: Some(false),
                },
            },
        }
    }
}

impl Tool for WfAuthorSubmitTestsTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let tests = input
                .get("tests")
                .ok_or_else(|| ToolError::InvalidInput("tests array is required".to_string()))?;

            let tests_array = tests
                .as_array()
                .ok_or_else(|| ToolError::InvalidInput("tests must be an array".to_string()))?;

            // Validate each test case has required fields
            let mut errors: Vec<String> = Vec::new();
            for (i, test) in tests_array.iter().enumerate() {
                if test.get("name").and_then(|v| v.as_str()).is_none() {
                    errors.push(format!("Test case {} is missing 'name'", i));
                }
                if test.get("inputs").is_none() {
                    errors.push(format!("Test case {} is missing 'inputs'", i));
                }
                if test.get("expectations").is_none() {
                    errors.push(format!("Test case {} is missing 'expectations'", i));
                }
            }

            if !errors.is_empty() {
                return Ok(ToolResult {
                    output: json!({
                        "success": false,
                        "error": format!("Validation errors:\n{}", errors.join("\n"))
                    }),
                    data_class: DataClass::Internal,
                });
            }

            Ok(ToolResult {
                output: json!({
                    "success": true,
                    "count": tests_array.len(),
                    "message": format!("{} test case(s) submitted. Now call workflow_author.run_tests with the definition_name to execute them.", tests_array.len())
                }),
                data_class: DataClass::Internal,
            })
        })
    }
}

// ---------------------------------------------------------------------------
// workflow_author.run_tests — Execute submitted tests and return results
// ---------------------------------------------------------------------------

pub struct WfAuthorRunTestsTool {
    definition: ToolDefinition,
    workflow_service: Arc<WorkflowService>,
}

impl WfAuthorRunTestsTool {
    pub fn new(workflow_service: Arc<WorkflowService>) -> Self {
        Self {
            definition: ToolDefinition {
                id: "workflow_author.run_tests".to_string(),
                name: "Run Workflow Tests".to_string(),
                description: "Execute the test cases previously submitted via submit_tests. \
                    Returns pass/fail for each test with failure details. \
                    Call this after submit_tests to validate the workflow. \
                    If tests fail, analyze the failures and fix the workflow or tests, then run again (max 2 retries)."
                    .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "definition_name": {
                            "type": "string",
                            "description": "The workflow definition name (e.g. 'user/my-workflow')"
                        },
                        "test_names": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Optional list of specific test names to run. Omit to run all tests."
                        }
                    },
                    "required": ["definition_name"]
                }),
                output_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "all_passed": { "type": "boolean" },
                        "total": { "type": "integer" },
                        "passed": { "type": "integer" },
                        "failed": { "type": "integer" },
                        "results": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "test_name": { "type": "string" },
                                    "passed": { "type": "boolean" },
                                    "duration_ms": { "type": "integer" },
                                    "actual_status": { "type": "string" },
                                    "failures": { "type": "array" },
                                    "step_statuses": { "type": "array" }
                                }
                            }
                        },
                        "message": { "type": "string" }
                    }
                })),
                channel_class: ChannelClass::Internal,
                side_effects: false,
                approval: ToolApproval::Auto,
                annotations: ToolAnnotations {
                    title: "Run Workflow Tests".to_string(),
                    read_only_hint: Some(true),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(true),
                    open_world_hint: Some(false),
                },
            },
            workflow_service,
        }
    }
}

impl Tool for WfAuthorRunTestsTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let definition_name = input
                .get("definition_name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    ToolError::InvalidInput("definition_name is required".to_string())
                })?;

            let test_names: Option<Vec<String>> = input
                .get("test_names")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                });

            let test_names_ref = test_names.as_deref();

            let (results, _total_requested) = match self
                .workflow_service
                .run_tests(definition_name, None, test_names_ref, true, None)
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    return Ok(ToolResult {
                        output: json!({
                            "error": format!("Failed to run tests: {}", e),
                            "all_passed": false,
                        }),
                        data_class: DataClass::Internal,
                    });
                }
            };

            if results.is_empty() {
                let reason = if test_names_ref.is_some() {
                    "No tests matched the specified names."
                } else {
                    "No tests are defined on this workflow."
                };
                return Ok(ToolResult {
                    output: json!({
                        "all_passed": false,
                        "total": 0,
                        "passed": 0,
                        "failed": 0,
                        "results": [],
                        "message": reason,
                    }),
                    data_class: DataClass::Internal,
                });
            }

            let total = results.len();
            let passed_count = results.iter().filter(|r| r.passed).count();
            let failed_count = total - passed_count;
            let all_passed = failed_count == 0;

            let compact_results: Vec<Value> = results
                .iter()
                .map(|r| {
                    let mut obj = json!({
                        "test_name": r.test_name,
                        "passed": r.passed,
                        "duration_ms": r.duration_ms,
                    });
                    if let Some(status) = &r.actual_status {
                        obj["actual_status"] = json!(status);
                    }
                    if !r.failures.is_empty() {
                        obj["failures"] = json!(r.failures.iter().map(|f| {
                            json!({
                                "expectation": f.expectation,
                                "expected": f.expected,
                                "actual": f.actual,
                            })
                        }).collect::<Vec<_>>());
                    }
                    // Compact step statuses (id + status only, no outputs)
                    if !r.step_results.is_empty() {
                        obj["step_statuses"] = json!(r.step_results.iter().map(|s| {
                            let mut step = json!({
                                "step_id": s.step_id,
                                "status": s.status,
                            });
                            if let Some(err) = &s.error {
                                step["error"] = json!(err);
                            }
                            step
                        }).collect::<Vec<_>>());
                    }
                    obj
                })
                .collect();

            let message = if all_passed {
                format!(
                    "All {} test(s) passed. You are done — respond to the user with a summary.",
                    total
                )
            } else {
                format!(
                    "{}/{} test(s) failed. Analyze the failures below and fix the workflow or tests, then resubmit and run again.",
                    failed_count, total
                )
            };

            Ok(ToolResult {
                output: json!({
                    "all_passed": all_passed,
                    "total": total,
                    "passed": passed_count,
                    "failed": failed_count,
                    "results": compact_results,
                    "message": message,
                }),
                data_class: DataClass::Internal,
            })
        })
    }
}

// ---------------------------------------------------------------------------
// workflow_author.suggest_tools — Suggest relevant tools for a goal
// ---------------------------------------------------------------------------

pub struct WfAuthorSuggestToolsTool {
    definition: ToolDefinition,
    registry: Arc<ToolRegistry>,
}

impl WfAuthorSuggestToolsTool {
    pub fn new(registry: Arc<ToolRegistry>) -> Self {
        Self {
            definition: ToolDefinition {
                id: "workflow_author.suggest_tools".to_string(),
                name: "Suggest Tools for Goal".to_string(),
                description:
                    "Given a natural language description of what you want to accomplish, \
                    returns a curated list of relevant tools with usage hints. \
                    Use this instead of listing all tools when you know the goal."
                        .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "goal": {
                            "type": "string",
                            "description": "What do you want to accomplish? (e.g., 'send an email', 'read a file', 'make an HTTP request')"
                        }
                    },
                    "required": ["goal"]
                }),
                output_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "suggestions": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "tool_id": { "type": "string" },
                                    "name": { "type": "string" },
                                    "description": { "type": "string" },
                                    "category": { "type": "string" },
                                    "relevance": { "type": "string" },
                                    "usage_hint": { "type": "string" }
                                }
                            }
                        }
                    }
                })),
                channel_class: ChannelClass::Internal,
                side_effects: false,
                approval: ToolApproval::Auto,
                annotations: ToolAnnotations {
                    title: "Suggest Tools for Goal".to_string(),
                    read_only_hint: Some(true),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(true),
                    open_world_hint: Some(false),
                },
            },
            registry,
        }
    }
}

impl Tool for WfAuthorSuggestToolsTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let goal = input
                .get("goal")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("goal is required".to_string()))?
                .to_lowercase();

            // Extract keywords from the goal
            let keywords: Vec<&str> =
                goal.split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() > 2).collect();

            let mut scored_tools: Vec<(u32, Value)> = self
                .registry
                .list_definitions()
                .into_iter()
                .filter(|d| !d.id.starts_with("workflow_author."))
                .filter_map(|d| {
                    let id_lower = d.id.to_lowercase();
                    let name_lower = d.name.to_lowercase();
                    let desc_lower = d.description.to_lowercase();
                    let searchable = format!("{id_lower} {name_lower} {desc_lower}");

                    let mut score: u32 = 0;
                    for kw in &keywords {
                        if id_lower.contains(kw) {
                            score += 3;
                        }
                        if name_lower.contains(kw) {
                            score += 2;
                        }
                        if desc_lower.contains(kw) {
                            score += 1;
                        }
                    }

                    // Boost tools that match the overall goal context
                    let context_keywords = goal_context_keywords(&goal);
                    for ck in &context_keywords {
                        if searchable.contains(ck) {
                            score += 2;
                        }
                    }

                    if score > 0 {
                        let relevance = if score >= 6 {
                            "high"
                        } else if score >= 3 {
                            "medium"
                        } else {
                            "low"
                        };

                        let usage_hint = format!(
                            "Use in a call_tool step with tool_id: \"{}\". Check get_tool_details for required arguments.",
                            d.id
                        );

                        Some((score, json!({
                            "tool_id": d.id,
                            "name": d.name,
                            "description": d.description,
                            "category": tool_category(&d.id),
                            "relevance": relevance,
                            "usage_hint": usage_hint
                        })))
                    } else {
                        None
                    }
                })
                .collect();

            scored_tools.sort_by(|a, b| b.0.cmp(&a.0));
            let suggestions: Vec<Value> =
                scored_tools.into_iter().take(10).map(|(_, v)| v).collect();

            let hint = if suggestions.is_empty() {
                "No tools matched your goal. Try rephrasing or use list_available_tools with a filter."
            } else {
                "Use get_tool_details on a tool to see its full input/output schema before adding it to a workflow."
            };

            Ok(ToolResult {
                output: json!({
                    "suggestions": suggestions,
                    "hint": hint
                }),
                data_class: DataClass::Internal,
            })
        })
    }
}

/// Expand a goal description into additional context keywords for better matching.
fn goal_context_keywords(goal: &str) -> Vec<&'static str> {
    let mut keywords = Vec::new();
    let g = goal.to_lowercase();

    if g.contains("email") || g.contains("mail") || g.contains("message") || g.contains("send") {
        keywords.extend_from_slice(&["send", "message", "email", "connector", "communication"]);
    }
    if g.contains("file") || g.contains("read") || g.contains("write") || g.contains("save") {
        keywords.extend_from_slice(&["file", "filesystem", "read", "write", "path"]);
    }
    if g.contains("http")
        || g.contains("api")
        || g.contains("request")
        || g.contains("fetch")
        || g.contains("webhook")
    {
        keywords.extend_from_slice(&["http", "request", "url", "api", "get", "post"]);
    }
    if g.contains("git") || g.contains("commit") || g.contains("branch") || g.contains("repo") {
        keywords.extend_from_slice(&["git", "commit", "branch", "repository"]);
    }
    if g.contains("schedule") || g.contains("cron") || g.contains("timer") || g.contains("periodic")
    {
        keywords.extend_from_slice(&["schedule", "cron", "task", "periodic"]);
    }
    if g.contains("search") || g.contains("find") || g.contains("query") || g.contains("lookup") {
        keywords.extend_from_slice(&["search", "find", "query", "index"]);
    }
    if g.contains("notify") || g.contains("alert") || g.contains("slack") || g.contains("discord") {
        keywords.extend_from_slice(&["notification", "alert", "send", "message", "connector"]);
    }

    keywords
}

// ---------------------------------------------------------------------------
// workflow_author.get_template — Get a workflow template by pattern
// ---------------------------------------------------------------------------

/// Available workflow template patterns and their YAML.
fn get_template_yaml(pattern: &str) -> Option<(&'static str, &'static str)> {
    match pattern.to_lowercase().as_str() {
        "email-responder" | "email-autoresponder" | "auto-reply" => Some((
            "Email Auto-Responder: Monitor emails, draft AI response, get human approval, send reply",
            include_str!("../templates/email_responder.yaml"),
        )),
        "scheduled-report" | "weekly-report" | "scheduled-pipeline" => Some((
            "Scheduled Report: Fetch data on a schedule, analyze with AI, deliver the report",
            include_str!("../templates/scheduled_report.yaml"),
        )),
        "research-and-write" | "multi-agent" | "agent-collaboration" => Some((
            "Multi-Agent Research & Writing: Research → Write → Human Review → Revise loop",
            include_str!("../templates/research_and_write.yaml"),
        )),
        "event-processor" | "order-processor" | "event-driven" => Some((
            "Event-Driven Processor: React to events, validate, branch, process in parallel",
            include_str!("../templates/event_processor.yaml"),
        )),
        "batch-processor" | "for-each" | "collection-processor" => Some((
            "Batch Processor: Iterate over a collection, process each item, accumulate results",
            include_str!("../templates/batch_processor.yaml"),
        )),
        _ => None,
    }
}

pub struct WfAuthorGetTemplateTool {
    definition: ToolDefinition,
}

impl Default for WfAuthorGetTemplateTool {
    fn default() -> Self {
        Self {
            definition: ToolDefinition {
                id: "workflow_author.get_template".to_string(),
                name: "Get Workflow Template".to_string(),
                description: "Get a complete, working workflow YAML template for a common pattern. \
                    Available patterns: 'email-responder', 'scheduled-report', 'research-and-write', \
                    'event-processor', 'batch-processor'. Use as a starting point and customize."
                    .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "pattern": {
                            "type": "string",
                            "description": "Template pattern name: 'email-responder', 'scheduled-report', 'research-and-write', 'event-processor', 'batch-processor'"
                        }
                    },
                    "required": ["pattern"]
                }),
                output_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "pattern": { "type": "string" },
                        "description": { "type": "string" },
                        "yaml": { "type": "string" },
                        "customization_hints": { "type": "string" }
                    }
                })),
                channel_class: ChannelClass::Internal,
                side_effects: false,
                approval: ToolApproval::Auto,
                annotations: ToolAnnotations {
                    title: "Get Workflow Template".to_string(),
                    read_only_hint: Some(true),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(true),
                    open_world_hint: Some(false),
                },
            },
        }
    }
}

impl Tool for WfAuthorGetTemplateTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let pattern = input
                .get("pattern")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("pattern is required".to_string()))?;

            if let Some((description, yaml)) = get_template_yaml(pattern) {
                Ok(ToolResult {
                    output: json!({
                        "pattern": pattern,
                        "description": description,
                        "yaml": yaml,
                        "customization_hints": "Customize this template by: 1) Changing the workflow name, \
                            2) Replacing placeholder tool_ids and connector IDs with real ones from discovery, \
                            3) Adjusting trigger configuration, 4) Modifying agent tasks for your use case, \
                            5) Adding/removing steps as needed."
                    }),
                    data_class: DataClass::Internal,
                })
            } else {
                let patterns = [
                    "email-responder",
                    "scheduled-report",
                    "research-and-write",
                    "event-processor",
                    "batch-processor",
                ];
                Ok(ToolResult {
                    output: json!({
                        "error": format!("Unknown pattern '{}'. Available patterns: {}", pattern, patterns.join(", ")),
                        "available_patterns": patterns
                    }),
                    data_class: DataClass::Internal,
                })
            }
        })
    }
}

// ---------------------------------------------------------------------------
// workflow_author.lint_workflow — Check workflow quality
// ---------------------------------------------------------------------------

pub struct WfAuthorLintWorkflowTool {
    definition: ToolDefinition,
}

impl Default for WfAuthorLintWorkflowTool {
    fn default() -> Self {
        Self {
            definition: ToolDefinition {
                id: "workflow_author.lint_workflow".to_string(),
                name: "Lint Workflow".to_string(),
                description: "Check a workflow YAML for quality issues beyond basic validation. \
                    Reports warnings about missing error handling, unused variables, missing timeouts, \
                    and other best-practice violations. Use before submit_workflow to improve quality."
                    .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "yaml": {
                            "type": "string",
                            "description": "The workflow YAML to lint"
                        }
                    },
                    "required": ["yaml"]
                }),
                output_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "warnings": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "step_id": { "type": "string" },
                                    "severity": { "type": "string" },
                                    "message": { "type": "string" },
                                    "suggestion": { "type": "string" }
                                }
                            }
                        },
                        "score": { "type": "string" },
                        "summary": { "type": "string" }
                    }
                })),
                channel_class: ChannelClass::Internal,
                side_effects: false,
                approval: ToolApproval::Auto,
                annotations: ToolAnnotations {
                    title: "Lint Workflow".to_string(),
                    read_only_hint: Some(true),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(true),
                    open_world_hint: Some(false),
                },
            },
        }
    }
}

impl Tool for WfAuthorLintWorkflowTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let yaml = input
                .get("yaml")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("yaml is required".to_string()))?;

            let parsed: Result<hive_workflow_service::hive_workflow::WorkflowDefinition, _> =
                serde_yaml::from_str(yaml);

            let def = match parsed {
                Ok(d) => d,
                Err(e) => {
                    return Ok(ToolResult {
                        output: json!({
                            "warnings": [{
                                "step_id": "(parse)",
                                "severity": "error",
                                "message": format!("YAML parse error: {}", e),
                                "suggestion": "Fix the YAML syntax before linting."
                            }],
                            "score": "N/A",
                            "summary": "Cannot lint — YAML does not parse."
                        }),
                        data_class: DataClass::Internal,
                    });
                }
            };

            let mut warnings: Vec<Value> = Vec::new();

            // Collect all step IDs that appear in any step's `next` (i.e., they have predecessors)
            let yaml_text_lower = yaml.to_lowercase();

            for step in &def.steps {
                let step_type = &step.step_type;
                let step_id = &step.id;

                // Check: invoke_agent without timeout
                if let hive_workflow_service::hive_workflow::StepType::Task { task } = step_type {
                    match task {
                        hive_workflow_service::hive_workflow::TaskDef::InvokeAgent {
                            timeout_secs,
                            ..
                        } => {
                            if timeout_secs.is_none() {
                                warnings.push(json!({
                                    "step_id": step_id,
                                    "severity": "warning",
                                    "message": "invoke_agent step has no timeout_secs",
                                    "suggestion": "Add timeout_secs (e.g., 300) to prevent the agent from running indefinitely."
                                }));
                            }
                        }
                        hive_workflow_service::hive_workflow::TaskDef::InvokePrompt {
                            timeout_secs,
                            ..
                        } => {
                            if timeout_secs.is_none() {
                                warnings.push(json!({
                                    "step_id": step_id,
                                    "severity": "warning",
                                    "message": "invoke_prompt step has no timeout_secs",
                                    "suggestion": "Add timeout_secs (e.g., 300) to prevent the agent from running indefinitely."
                                }));
                            }
                        }
                        hive_workflow_service::hive_workflow::TaskDef::CallTool { .. } => {}
                        _ => {}
                    }
                }

                // Check: task steps without error handling (except set_variable and delay)
                if let hive_workflow_service::hive_workflow::StepType::Task { task } = step_type {
                    let needs_error_handling = matches!(
                        task,
                        hive_workflow_service::hive_workflow::TaskDef::CallTool { .. }
                            | hive_workflow_service::hive_workflow::TaskDef::InvokeAgent { .. }
                            | hive_workflow_service::hive_workflow::TaskDef::InvokePrompt { .. }
                            | hive_workflow_service::hive_workflow::TaskDef::LaunchWorkflow { .. }
                    );

                    if needs_error_handling && step.on_error.is_none() {
                        warnings.push(json!({
                            "step_id": step_id,
                            "severity": "warning",
                            "message": "Task step has no on_error strategy",
                            "suggestion": "Add on_error with strategy: retry for external calls, or strategy: skip with default_output for non-critical steps."
                        }));
                    }
                }

                // Check: generic step IDs
                if step_id.starts_with("step_") || step_id.starts_with("step") && step_id.len() <= 6
                {
                    warnings.push(json!({
                        "step_id": step_id,
                        "severity": "info",
                        "message": "Step has a generic ID",
                        "suggestion": "Use a descriptive ID like 'fetch_customer_data' instead of generic numbered IDs."
                    }));
                }
            }

            // Check: no end_workflow step
            let has_end = def.steps.iter().any(|s| {
                matches!(
                    &s.step_type,
                    hive_workflow_service::hive_workflow::StepType::ControlFlow {
                        control: hive_workflow_service::hive_workflow::ControlFlowDef::EndWorkflow
                    }
                )
            });
            if !has_end {
                warnings.push(json!({
                    "step_id": "(workflow)",
                    "severity": "warning",
                    "message": "Workflow has no end_workflow step",
                    "suggestion": "Add an end_workflow step as a terminal node for clarity."
                }));
            }

            // Check: scheduled workflow without manual trigger (for testing)
            let has_schedule = def.steps.iter().any(|s| {
                matches!(
                    &s.step_type,
                    hive_workflow_service::hive_workflow::StepType::Trigger { trigger }
                    if matches!(trigger.trigger_type, hive_workflow_service::hive_workflow::TriggerType::Schedule { .. })
                )
            });
            let has_manual = def.steps.iter().any(|s| {
                matches!(
                    &s.step_type,
                    hive_workflow_service::hive_workflow::StepType::Trigger { trigger }
                    if matches!(trigger.trigger_type, hive_workflow_service::hive_workflow::TriggerType::Manual { .. })
                )
            });
            if has_schedule && !has_manual {
                warnings.push(json!({
                    "step_id": "(workflow)",
                    "severity": "info",
                    "message": "Scheduled workflow has no manual trigger for testing",
                    "suggestion": "Add a manual trigger that points to the same first step, so the workflow can be tested without waiting for the schedule."
                }));
            }

            // Check: variables declared but never referenced in YAML text
            if let Some(props) = def.variables.get("properties").and_then(|v| v.as_object()) {
                for var_name in props.keys() {
                    let patterns = [format!("variables.{var_name}"), format!("{{{{{var_name}}}}}")];
                    let referenced =
                        patterns.iter().any(|p| yaml_text_lower.contains(&p.to_lowercase()));
                    if !referenced {
                        warnings.push(json!({
                            "step_id": "(variables)",
                            "severity": "info",
                            "message": format!("Variable '{}' is declared but never referenced", var_name),
                            "suggestion": "Remove unused variables or add references to them in step arguments/outputs."
                        }));
                    }
                }
            }

            // Check: missing description
            if def.description.as_ref().is_none_or(|d| d.trim().is_empty()) {
                warnings.push(json!({
                    "step_id": "(workflow)",
                    "severity": "info",
                    "message": "Workflow has no description",
                    "suggestion": "Add a description to explain what this workflow does."
                }));
            }

            let warning_count = warnings.iter().filter(|w| w["severity"] == "warning").count();
            let info_count = warnings.iter().filter(|w| w["severity"] == "info").count();

            let score = if warning_count == 0 && info_count == 0 {
                "excellent"
            } else if warning_count == 0 {
                "good"
            } else if warning_count <= 2 {
                "fair"
            } else {
                "needs improvement"
            };

            let summary = format!(
                "{} warnings, {} suggestions. Quality: {}.",
                warning_count, info_count, score
            );

            Ok(ToolResult {
                output: json!({
                    "warnings": warnings,
                    "score": score,
                    "summary": summary
                }),
                data_class: DataClass::Internal,
            })
        })
    }
}

// ---------------------------------------------------------------------------
// Factory: create all workflow author tools
// ---------------------------------------------------------------------------

/// All IDs of workflow author tools, for use in allowed_tools filtering.
pub const WORKFLOW_AUTHOR_TOOL_IDS: &[&str] = &[
    "workflow_author.list_available_tools",
    "workflow_author.get_tool_details",
    "workflow_author.suggest_tools",
    "workflow_author.list_connectors",
    "workflow_author.list_personas",
    "workflow_author.list_event_topics",
    "workflow_author.list_workflows",
    "workflow_author.get_template",
    "workflow_author.lint_workflow",
    "workflow_author.submit_workflow",
    "workflow_author.submit_tests",
    "workflow_author.run_tests",
];

/// Build a default set of well-known event topics.
/// Dynamic connector-specific topics can be discovered via the list_connectors tool.
pub fn default_event_topics() -> Vec<EventTopicInfo> {
    vec![
        EventTopicInfo {
            topic: "chat.session.created".into(),
            description: "A new chat session was created".into(),
            payload_keys: vec!["sessionId".into()],
        },
        EventTopicInfo {
            topic: "chat.session.activity".into(),
            description: "Activity detected in a chat session".into(),
            payload_keys: vec!["sessionId".into(), "stage".into(), "intent".into()],
        },
        EventTopicInfo {
            topic: "chat.session.resumed".into(),
            description: "A chat session was resumed".into(),
            payload_keys: vec!["sessionId".into()],
        },
        EventTopicInfo {
            topic: "chat.message.queued".into(),
            description: "A chat message was queued for processing".into(),
            payload_keys: vec!["sessionId".into(), "messageId".into()],
        },
        EventTopicInfo {
            topic: "chat.message.completed".into(),
            description: "A chat message finished processing".into(),
            payload_keys: vec!["sessionId".into(), "messageId".into()],
        },
        EventTopicInfo {
            topic: "chat.message.failed".into(),
            description: "A chat message processing failed".into(),
            payload_keys: vec!["sessionId".into(), "messageId".into(), "error".into()],
        },
        EventTopicInfo {
            topic: "tool.invoked".into(),
            description: "A tool was invoked by an agent".into(),
            payload_keys: vec!["toolId".into(), "dataClass".into()],
        },
        EventTopicInfo {
            topic: "workflow.definition.saved".into(),
            description: "A workflow definition was saved".into(),
            payload_keys: vec!["name".into(), "version".into()],
        },
        EventTopicInfo {
            topic: "workflow.definition.deleted".into(),
            description: "A workflow definition was deleted".into(),
            payload_keys: vec!["name".into(), "version".into()],
        },
        EventTopicInfo {
            topic: "workflow.interaction.requested".into(),
            description: "A workflow step requested user interaction".into(),
            payload_keys: vec![
                "instance_id".into(),
                "step_id".into(),
                "prompt".into(),
                "choices".into(),
            ],
        },
        EventTopicInfo {
            topic: "workflow.trigger.fired".into(),
            description: "A workflow trigger fired".into(),
            payload_keys: vec!["definition".into(), "instance_id".into()],
        },
        EventTopicInfo {
            topic: "config.reloaded".into(),
            description: "Application configuration was reloaded".into(),
            payload_keys: vec!["personas_dir".into(), "config_path".into()],
        },
        EventTopicInfo {
            topic: "scheduler.task.completed".into(),
            description: "A scheduled task completed".into(),
            payload_keys: vec![],
        },
        EventTopicInfo {
            topic: "mcp.notification".into(),
            description: "An MCP server sent a notification".into(),
            payload_keys: vec!["serverId".into()],
        },
        EventTopicInfo {
            topic: "mcp.server.connected".into(),
            description: "An MCP server connected".into(),
            payload_keys: vec!["serverId".into()],
        },
        EventTopicInfo {
            topic: "mcp.server.disconnected".into(),
            description: "An MCP server disconnected".into(),
            payload_keys: vec!["serverId".into()],
        },
        EventTopicInfo {
            topic: "daemon.shutdown_requested".into(),
            description: "Daemon shutdown was requested".into(),
            payload_keys: vec!["requested_by".into()],
        },
    ]
}

/// Create all workflow author tools. Returns a Vec of tools to be registered.
pub fn create_workflow_author_tools(
    tool_registry: Arc<ToolRegistry>,
    connector_registry: Option<Arc<ConnectorRegistry>>,
    workflow_service: Arc<WorkflowService>,
    personas: Arc<parking_lot::Mutex<Vec<hive_contracts::Persona>>>,
    event_topics: Vec<EventTopicInfo>,
) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(WfAuthorListToolsTool::new(tool_registry.clone())) as Arc<dyn Tool>,
        Arc::new(WfAuthorGetToolDetailsTool::new(tool_registry.clone())),
        Arc::new(WfAuthorSuggestToolsTool::new(tool_registry)),
        Arc::new(WfAuthorListConnectorsTool::new(connector_registry)),
        Arc::new(WfAuthorListPersonasTool::new(personas)),
        Arc::new(WfAuthorListEventTopicsTool::new(event_topics)),
        Arc::new(WfAuthorListWorkflowsTool::new(workflow_service.clone())),
        Arc::new(WfAuthorGetTemplateTool::default()),
        Arc::new(WfAuthorLintWorkflowTool::default()),
        Arc::new(WfAuthorSubmitWorkflowTool::default()),
        Arc::new(WfAuthorSubmitTestsTool::default()),
        Arc::new(WfAuthorRunTestsTool::new(workflow_service)),
    ]
}
