use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

// ---------------------------------------------------------------------------
// Workflow mode
// ---------------------------------------------------------------------------

/// Controls how a workflow integrates with the user experience.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowMode {
    /// Runs independently in the background. Managed from the Workflows page.
    #[default]
    Background,
    /// Attached to a chat session. Shares the session's workspace, surfaces
    /// interactions in the chat thread, and displays a result widget on
    /// completion.
    Chat,
}

// ---------------------------------------------------------------------------
// Workflow Definition (immutable after creation)
// ---------------------------------------------------------------------------

pub fn generate_workflow_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// A file attachment associated with a workflow definition.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkflowAttachment {
    pub id: String,
    pub filename: String,
    /// Description of what this file should be used for by an AI agent.
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowDefinition {
    /// Immutable identifier for this workflow definition. Auto-generated if not
    /// provided.  Remains stable across renames and version bumps.
    #[serde(default = "generate_workflow_id")]
    pub id: String,
    pub name: String,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub description: Option<String>,
    /// Whether this workflow runs as a background process or attached to a chat
    /// session.
    #[serde(default)]
    pub mode: WorkflowMode,
    /// JSON Schema describing the internal variable bag
    #[serde(default = "default_variables_schema")]
    pub variables: serde_json::Value,
    pub steps: Vec<StepDef>,
    /// Maps output field names to expressions that build the workflow result
    #[serde(default)]
    pub output: Option<HashMap<String, String>>,
    /// Optional template resolved at completion and displayed as a human-readable
    /// result summary in the chat UI (only meaningful for `mode: Chat`).
    /// Example: `"{{steps.final.outputs.summary}}"`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_message: Option<String>,
    #[serde(default)]
    pub requested_tools: Vec<RequestedTool>,
    /// Default permission rules inherited by agents spawned from this workflow.
    #[serde(default)]
    pub permissions: Vec<PermissionEntry>,
    /// File attachments associated with this workflow definition.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<WorkflowAttachment>,
    /// `true` for factory-shipped workflows seeded from bundled YAML embedded
    /// in the binary.  Bundled workflows cannot be deleted, only archived
    /// (hidden).  Users can edit them and later reset to defaults.
    #[serde(default, skip)]
    pub bundled: bool,
    /// When `true` the workflow is hidden from normal listings but remains
    /// launchable so that existing triggers and references keep working.
    #[serde(default, skip)]
    pub archived: bool,
    /// When `true` automatic triggers (schedule, event, etc.) are suspended.
    /// Manual triggers remain unaffected.
    #[serde(default, skip)]
    pub triggers_paused: bool,
}

impl WorkflowDefinition {
    /// Returns an iterator over all trigger definitions embedded in trigger steps.
    pub fn trigger_defs(&self) -> impl Iterator<Item = &TriggerDef> {
        self.steps.iter().filter_map(|s| match &s.step_type {
            StepType::Trigger { trigger } => Some(trigger),
            _ => None,
        })
    }

    /// Validate the workflow name using the shared namespaced-ID rules.
    pub fn validate_name(name: &str) -> Result<(), String> {
        hive_contracts::validate_namespaced_id(name, "Workflow name")
    }

    /// The first segment of the name (e.g. `"system"` from `"system/code-review"`).
    pub fn namespace_root(&self) -> &str {
        self.name.split('/').next().unwrap_or(&self.name)
    }

    /// Everything after the first `/` (e.g. `"code-review"` from `"system/code-review"`).
    pub fn short_name(&self) -> &str {
        self.name.find('/').map(|i| &self.name[i + 1..]).unwrap_or(&self.name)
    }

    /// Whether this workflow belongs to the `system` namespace.
    pub fn is_system(&self) -> bool {
        self.namespace_root() == "system"
    }

    /// Whether this workflow belongs to the `user` namespace.
    pub fn is_user(&self) -> bool {
        self.namespace_root() == "user"
    }
}

fn default_version() -> String {
    "1.0".to_string()
}

fn default_variables_schema() -> serde_json::Value {
    serde_json::json!({ "type": "object", "properties": {} })
}

// ---------------------------------------------------------------------------
// Steps
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepDef {
    pub id: String,
    #[serde(flatten)]
    pub step_type: StepType,
    /// Output variable mappings: name -> expression
    #[serde(default)]
    pub outputs: HashMap<String, String>,
    #[serde(default)]
    pub on_error: Option<ErrorStrategy>,
    /// IDs of successor steps
    #[serde(default)]
    pub next: Vec<String>,
    /// Maximum execution time in seconds for this step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

// SPEC-GAP: Stage type taxonomy differs from spec. See DESIGN_NOTES.md.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StepType {
    Trigger { trigger: TriggerDef },
    Task { task: TaskDef },
    ControlFlow { control: ControlFlowDef },
}

// ---------------------------------------------------------------------------
// Task definitions
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TaskDef {
    CallTool {
        tool_id: String,
        #[serde(default)]
        arguments: HashMap<String, String>,
    },
    ScheduleTask {
        schedule: ScheduleTaskDef,
    },
    InvokeAgent {
        persona_id: String,
        task: String,
        #[serde(default)]
        async_exec: bool,
        /// Maximum execution time in seconds. When omitted the agent runs
        /// without a timeout.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_secs: Option<u64>,
        /// Per-step permission rules that override workflow-level defaults.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        permissions: Vec<PermissionEntry>,
        /// IDs of workflow-level attachments to expose to this agent.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<String>,
        /// Human-readable display name for the spawned agent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_name: Option<String>,
    },
    #[serde(alias = "send_message")]
    SignalAgent {
        target: SignalTarget,
        content: String,
    },
    FeedbackGate {
        prompt: String,
        #[serde(default)]
        choices: Option<Vec<String>>,
        #[serde(default = "default_true")]
        allow_freeform: bool,
    },
    EventGate {
        topic: String,
        #[serde(default)]
        filter: Option<String>,
        #[serde(default)]
        timeout_secs: Option<u64>,
    },
    LaunchWorkflow {
        workflow_name: String,
        #[serde(default)]
        inputs: HashMap<String, String>,
    },
    Delay {
        duration_secs: u64,
    },
    SetVariable {
        assignments: Vec<VariableAssignment>,
    },
    /// Resolve a persona's prompt template, render it with the given
    /// parameters, and invoke it as an agent task.
    InvokePrompt {
        persona_id: String,
        prompt_id: String,
        /// Handlebars template parameter values. Keys are template expressions
        /// that will be resolved against the workflow expression context.
        #[serde(default)]
        parameters: HashMap<String, String>,
        #[serde(default)]
        async_exec: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_secs: Option<u64>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        permissions: Vec<PermissionEntry>,
        /// Optional: send the rendered prompt to an existing agent instead of
        /// spawning a new one.  Supports template expressions, e.g.
        /// `{{steps.spawn_step.agent_id}}`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_agent_id: Option<String>,
        /// When `target_agent_id` is set and the target agent is not found,
        /// automatically spawn a new agent instead of failing.
        #[serde(default)]
        auto_create: bool,
        /// Human-readable display name for the spawned agent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_name: Option<String>,
    },
}

fn default_true() -> bool {
    true
}

fn default_assign_op() -> AssignOp {
    AssignOp::Set
}

/// How a variable assignment is applied.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AssignOp {
    /// Overwrite the variable with the resolved value.
    Set,
    /// Append the resolved value to an existing list (creates the list if missing).
    AppendList,
    /// Shallow-merge the resolved value (must be an object) into an existing map.
    MergeMap,
}

/// A single variable assignment within a `SetVariable` step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VariableAssignment {
    pub variable: String,
    pub value: String,
    #[serde(default = "default_assign_op")]
    pub operation: AssignOp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleTaskDef {
    pub name: String,
    pub schedule: String, // cron expression
    pub action: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SignalTarget {
    Agent { agent_id: String },
    Session { session_id: String },
}

// ---------------------------------------------------------------------------
// Control flow definitions
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ControlFlowDef {
    Branch {
        condition: String,
        #[serde(default)]
        then: Vec<String>,
        #[serde(default, rename = "else")]
        else_branch: Vec<String>,
    },
    ForEach {
        collection: String,
        item_var: String,
        #[serde(default)]
        body: Vec<String>,
    },
    While {
        condition: String,
        #[serde(default)]
        max_iterations: Option<u32>,
        #[serde(default)]
        body: Vec<String>,
    },
    EndWorkflow,
}

impl ControlFlowDef {
    /// Return the step IDs reachable through this control flow node's own
    /// edges (branch targets, loop bodies) given its current execution state.
    pub fn reachable_targets(&self, state: Option<&StepState>) -> Vec<&str> {
        match self {
            ControlFlowDef::Branch { then, else_branch, .. } => {
                if let Some(s) = state {
                    if s.status == StepStatus::Completed {
                        // Branch decided — only the selected targets are reachable.
                        // Match target strings from `self` against the branch_targets
                        // recorded in the step's outputs.
                        if let Some(ref outputs) = s.outputs {
                            if let Some(targets) = outputs.get("branch_targets") {
                                if let Some(arr) = targets.as_array() {
                                    return then
                                        .iter()
                                        .chain(else_branch.iter())
                                        .filter(|t| {
                                            arr.iter().any(|v| v.as_str() == Some(t.as_str()))
                                        })
                                        .map(|s| s.as_str())
                                        .collect();
                                }
                            }
                        }
                        // Completed but no branch_targets recorded — nothing reachable
                        return vec![];
                    }
                    if s.status == StepStatus::Skipped {
                        return vec![];
                    }
                }
                // Not yet decided — both paths are potentially reachable
                then.iter().chain(else_branch.iter()).map(|s| s.as_str()).collect()
            }
            ControlFlowDef::ForEach { body, .. } | ControlFlowDef::While { body, .. } => {
                // Body steps are reachable while the loop is active (not completed)
                let loop_active = state.is_none_or(|s| {
                    !matches!(s.status, StepStatus::Completed | StepStatus::Skipped)
                });
                if loop_active {
                    body.iter().map(|s| s.as_str()).collect()
                } else {
                    vec![]
                }
            }
            ControlFlowDef::EndWorkflow => vec![],
        }
    }
}

impl StepDef {
    /// Return all step IDs reachable from this step given its current state.
    ///
    /// Combines the universal `next` edges with any type-specific edges
    /// (branch targets, loop bodies) so each step type co-locates its own
    /// reachability rules.
    pub fn reachable_successors(&self, state: Option<&StepState>) -> Vec<&str> {
        let mut result: Vec<&str> = self.next.iter().map(|s| s.as_str()).collect();
        if let StepType::ControlFlow { control } = &self.step_type {
            result.extend(control.reachable_targets(state));
        }
        result
    }
}

// ---------------------------------------------------------------------------
// Error strategies
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "strategy", rename_all = "snake_case")]
pub enum ErrorStrategy {
    FailWorkflow {
        #[serde(default)]
        message: Option<String>,
    },
    Retry {
        max_retries: u32,
        #[serde(default = "default_retry_delay")]
        delay_secs: u64,
    },
    Skip {
        #[serde(default)]
        default_output: Option<serde_json::Value>,
    },
    GoTo {
        step_id: String,
    },
}

fn default_retry_delay() -> u64 {
    5
}

// ---------------------------------------------------------------------------
// Triggers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerDef {
    #[serde(flatten)]
    pub trigger_type: TriggerType,
}

impl TriggerDef {
    /// Returns the JSON Schema for this trigger's inputs.
    ///
    /// If `input_schema` is set on a Manual trigger, returns it directly.
    /// Otherwise, converts the legacy `inputs: Vec<TriggerInput>` to JSON Schema.
    /// For non-Manual triggers, returns an empty object schema.
    pub fn effective_input_schema(&self) -> serde_json::Value {
        match &self.trigger_type {
            TriggerType::Manual { inputs, input_schema } => {
                // Prefer explicit input_schema
                if let Some(schema) = input_schema {
                    if schema.get("properties").is_some() {
                        return schema.clone();
                    }
                }
                // Fall back to converting legacy inputs
                if inputs.is_empty() {
                    return serde_json::json!({ "type": "object", "properties": {} });
                }
                let mut properties = serde_json::Map::new();
                let mut required = Vec::new();
                for inp in inputs {
                    let mut prop = serde_json::Map::new();
                    prop.insert("type".into(), serde_json::Value::String(inp.input_type.clone()));
                    if let Some(ref default) = inp.default {
                        prop.insert("default".into(), default.clone());
                    }
                    if inp.required {
                        required.push(serde_json::Value::String(inp.name.clone()));
                    }
                    properties.insert(inp.name.clone(), serde_json::Value::Object(prop));
                }
                let mut schema = serde_json::Map::new();
                schema.insert("type".into(), serde_json::Value::String("object".into()));
                schema.insert("properties".into(), serde_json::Value::Object(properties));
                if !required.is_empty() {
                    schema.insert("required".into(), serde_json::Value::Array(required));
                }
                serde_json::Value::Object(schema)
            }
            _ => serde_json::json!({ "type": "object", "properties": {} }),
        }
    }

    /// Returns true if this trigger has an explicit `input_schema` (not legacy `inputs`).
    pub fn has_explicit_schema(&self) -> bool {
        matches!(
            &self.trigger_type,
            TriggerType::Manual { input_schema: Some(s), .. } if s.get("properties").is_some()
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TriggerType {
    Manual {
        /// Legacy simple input list (kept for backward compatibility).
        #[serde(default)]
        inputs: Vec<TriggerInput>,
        /// JSON Schema describing the trigger's input parameters.
        /// Takes precedence over `inputs` when present.
        #[serde(default)]
        input_schema: Option<serde_json::Value>,
    },
    IncomingMessage {
        channel_id: String,
        /// Optional specific channel within the connector to listen on (e.g. a Slack/Discord channel ID).
        #[serde(default)]
        listen_channel_id: Option<String>,
        #[serde(default)]
        filter: Option<String>,
        #[serde(default)]
        from_filter: Option<String>,
        #[serde(default)]
        subject_filter: Option<String>,
        #[serde(default)]
        body_filter: Option<String>,
        /// Whether to mark messages as read after processing (default: false).
        #[serde(default)]
        mark_as_read: bool,
        /// When true, reply messages are ignored (only new/original messages trigger).
        /// Detected via provider-specific metadata: Discord `referenced_message_id`,
        /// Slack `thread_ts`, Email `in_reply_to`/`references`.
        #[serde(default)]
        ignore_replies: bool,
    },
    EventPattern {
        topic: String,
        #[serde(default)]
        filter: Option<String>,
    },
    McpNotification {
        server_id: String,
        #[serde(default)]
        kind: Option<String>,
    },
    Schedule {
        cron: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerInput {
    pub name: String,
    #[serde(default = "default_input_type")]
    pub input_type: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub default: Option<serde_json::Value>,
}

fn default_input_type() -> String {
    "string".to_string()
}

// ---------------------------------------------------------------------------
// Requested tools (pre-flight approval)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestedTool {
    pub tool_id: String,
    #[serde(default = "default_approval")]
    pub approval: ToolApprovalLevel,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolApprovalLevel {
    Auto,
    Ask,
    Deny,
}

fn default_approval() -> ToolApprovalLevel {
    ToolApprovalLevel::Ask
}

// ---------------------------------------------------------------------------
// Instance types (mutable runtime state)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowInstance {
    pub id: i64,
    pub definition: WorkflowDefinition,
    pub status: WorkflowStatus,
    pub variables: serde_json::Value,
    pub step_states: HashMap<String, StepState>,
    pub parent_session_id: String,
    pub parent_agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger_step_id: Option<String>,
    pub permissions: Vec<PermissionEntry>,
    /// Shared workspace directory. For chat-mode workflows this is inherited
    /// from the parent chat session so all agents operate in the same directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_path: Option<String>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub completed_at_ms: Option<u64>,
    pub output: Option<serde_json::Value>,
    pub error: Option<String>,
    /// Resolved result message (from definition.result_message template).
    /// Set when the workflow completes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_result_message: Option<String>,
    /// Steps activated by GoTo error strategy. These steps bypass normal
    /// predecessor checks in `compute_ready_steps`.
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    pub goto_activated_steps: HashSet<String>,
    /// Steps that failed but had their error handled via GoTo. Treated as
    /// "done" for predecessor checks so their `next` successors can proceed.
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    pub goto_source_steps: HashSet<String>,
    /// Active loop iterations for ForEach/While control flow steps.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub active_loops: HashMap<String, LoopState>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowStatus {
    Pending,
    Running,
    Paused,
    WaitingOnInput,
    WaitingOnEvent,
    Completed,
    Failed,
    Killed,
}

impl std::fmt::Display for WorkflowStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::WaitingOnInput => "waiting_on_input",
            Self::WaitingOnEvent => "waiting_on_event",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Killed => "killed",
        };
        write!(f, "{s}")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepState {
    pub step_id: String,
    pub status: StepStatus,
    pub started_at_ms: Option<u64>,
    pub completed_at_ms: Option<u64>,
    pub outputs: Option<serde_json::Value>,
    pub error: Option<String>,
    pub retry_count: u32,
    /// Delay in seconds to wait before the next retry attempt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_delay_secs: Option<u64>,
    pub child_workflow_id: Option<i64>,
    pub child_agent_id: Option<String>,
    pub interaction_request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interaction_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interaction_choices: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interaction_allow_freeform: Option<bool>,
    /// Epoch milliseconds at which a Delay step should resume.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Skipped,
    WaitingOnInput,
    WaitingOnEvent,
    /// Delay step is waiting for a scheduled time to resume.
    WaitingForDelay,
    /// Loop control step is waiting for its body steps to complete before
    /// re-evaluating the next iteration.
    LoopWaiting,
}

/// Runtime state for an active ForEach or While loop iteration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopState {
    /// Current iteration index (0-based).
    pub iteration: usize,
    /// Resolved collection for ForEach loops.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collection: Option<Vec<serde_json::Value>>,
    /// Variable name for the current item (ForEach only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_var: Option<String>,
    /// Step IDs that form the loop body.
    pub body_step_ids: Vec<String>,
    /// Safety limit for While loops.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_iterations: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionEntry {
    pub tool_id: String,
    #[serde(default)]
    pub resource: Option<String>,
    pub approval: ToolApprovalLevel,
}

// ---------------------------------------------------------------------------
// Workflow events (emitted during execution)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkflowEvent {
    InstanceCreated {
        instance_id: i64,
        definition_name: String,
        parent_session_id: String,
        mode: WorkflowMode,
    },
    InstanceStarted {
        instance_id: i64,
    },
    InstancePaused {
        instance_id: i64,
    },
    InstanceResumed {
        instance_id: i64,
    },
    InstanceCompleted {
        instance_id: i64,
        output: Option<serde_json::Value>,
        /// Resolved human-readable result summary (from definition.result_message).
        result_message: Option<String>,
    },
    InstanceFailed {
        instance_id: i64,
        error: String,
    },
    InstanceKilled {
        instance_id: i64,
    },
    StepStarted {
        instance_id: i64,
        step_id: String,
    },
    StepCompleted {
        instance_id: i64,
        step_id: String,
        outputs: Option<serde_json::Value>,
    },
    StepFailed {
        instance_id: i64,
        step_id: String,
        error: String,
    },
    StepWaiting {
        instance_id: i64,
        step_id: String,
        waiting_type: String,
    },
    InteractionRequested {
        instance_id: i64,
        step_id: String,
        prompt: String,
        choices: Option<Vec<String>>,
    },
    InteractionResponded {
        instance_id: i64,
        step_id: String,
    },
    EventGateResolved {
        instance_id: i64,
        step_id: String,
    },
}

// ---------------------------------------------------------------------------
// Summary types (for list views)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowDefinitionSummary {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub mode: WorkflowMode,
    pub trigger_types: Vec<String>,
    pub step_count: usize,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    /// `true` for factory-shipped (bundled) definitions.
    #[serde(default)]
    pub bundled: bool,
    /// `true` when the user has hidden this definition.
    #[serde(default)]
    pub archived: bool,
    /// `true` when auto-triggers are suspended for this definition.
    #[serde(default)]
    pub triggers_paused: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowInstanceSummary {
    pub id: i64,
    pub definition_name: String,
    pub definition_version: String,
    pub status: WorkflowStatus,
    pub parent_session_id: String,
    pub parent_agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger_step_id: Option<String>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub completed_at_ms: Option<u64>,
    pub step_count: usize,
    pub steps_completed: usize,
    pub steps_failed: usize,
    pub steps_running: usize,
    pub has_pending_interaction: bool,
    /// Number of pending tool approvals on child agents spawned by this workflow.
    #[serde(default)]
    pub pending_agent_approvals: usize,
    /// Number of pending user questions on child agents spawned by this workflow.
    #[serde(default)]
    pub pending_agent_questions: usize,
    /// IDs of child agents spawned by this workflow (for UI cross-referencing).
    #[serde(default)]
    pub child_agent_ids: Vec<String>,
    /// Whether this instance has been archived (hidden from default listings).
    #[serde(default)]
    pub archived: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InstanceFilter {
    pub statuses: Vec<WorkflowStatus>,
    pub definition_names: Vec<String>,
    pub definition_id: Option<String>,
    pub parent_session_id: Option<String>,
    pub parent_agent_id: Option<String>,
    pub mode: Option<WorkflowMode>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    /// When `false` (the default), archived instances are excluded from results.
    pub include_archived: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceListResult {
    pub items: Vec<WorkflowInstanceSummary>,
    pub total: usize,
}
