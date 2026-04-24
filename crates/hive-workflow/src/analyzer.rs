use hive_contracts::tools::ToolDefinition;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::types::{ControlFlowDef, StepDef, StepType, TaskDef, WorkflowDefinition};

// ---------------------------------------------------------------------------
// Risk levels
// ---------------------------------------------------------------------------

/// Risk classification for workflow steps and tools.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    /// Read-only tools, pure logic steps (SetVariable, Delay, Branch).
    Safe,
    /// Side-effecting but internal (launch_workflow, signal, agents).
    Caution,
    /// External side-effects (email, HTTP, destructive ops, scheduling).
    Danger,
    /// Tool not found in registry — treated as Caution for execution decisions.
    Unknown,
}

impl RiskLevel {
    /// Whether this risk level should be intercepted in shadow mode.
    pub fn should_intercept(&self) -> bool {
        !matches!(self, RiskLevel::Safe)
    }
}

// ---------------------------------------------------------------------------
// Tool-level classification
// ---------------------------------------------------------------------------

/// Known tool ID prefixes that produce external / destructive side-effects.
const DANGER_TOOL_PREFIXES: &[&str] = &[
    "connector.send_message",
    "http.request",
    "process.start",
];

/// Classify a single tool by its definition metadata.
///
/// Precedence:
///   1. `read_only_hint == true` → **Safe** (even if open_world, because
///      read-only external fetches are acceptable in shadow mode)
///   2. `destructive_hint == true` OR `open_world_hint == true` → **Danger**
///   3. Known external tool prefixes → **Danger**
///   4. `side_effects == true` → **Caution**
///   5. Otherwise → **Safe**
pub fn classify_tool(tool_def: &ToolDefinition) -> RiskLevel {
    if tool_def.annotations.read_only_hint == Some(true) {
        return RiskLevel::Safe;
    }
    if tool_def.annotations.destructive_hint == Some(true)
        || tool_def.annotations.open_world_hint == Some(true)
    {
        return RiskLevel::Danger;
    }
    if DANGER_TOOL_PREFIXES
        .iter()
        .any(|p| tool_def.id.starts_with(p))
    {
        return RiskLevel::Danger;
    }
    if tool_def.side_effects {
        return RiskLevel::Caution;
    }
    RiskLevel::Safe
}

// ---------------------------------------------------------------------------
// Step-level classification
// ---------------------------------------------------------------------------

/// Classify a task step.
///
/// For `CallTool` steps, uses the provided `tool_lookup` to resolve tool
/// metadata.  If the tool is not found, returns `Unknown` (which
/// `should_intercept()` treats the same as `Caution`).
pub fn classify_task(
    task: &TaskDef,
    tool_lookup: &dyn Fn(&str) -> Option<RiskLevel>,
) -> RiskLevel {
    match task {
        TaskDef::CallTool { tool_id, .. } => tool_lookup(tool_id).unwrap_or(RiskLevel::Unknown),
        TaskDef::InvokeAgent { .. } | TaskDef::InvokePrompt { .. } => RiskLevel::Caution,
        TaskDef::SignalAgent { .. } => RiskLevel::Caution,
        TaskDef::LaunchWorkflow { .. } => RiskLevel::Caution,
        TaskDef::ScheduleTask { .. } => RiskLevel::Danger,
        TaskDef::SetVariable { .. } => RiskLevel::Safe,
        TaskDef::Delay { .. } => RiskLevel::Safe,
        TaskDef::FeedbackGate { .. } => RiskLevel::Safe,
        TaskDef::EventGate { .. } => RiskLevel::Safe,
    }
}

/// Classify a full step type (trigger, task, or control flow).
pub fn classify_step_type(
    step_type: &StepType,
    tool_lookup: &dyn Fn(&str) -> Option<RiskLevel>,
) -> RiskLevel {
    match step_type {
        StepType::Trigger { .. } => RiskLevel::Safe,
        StepType::Task { task } => classify_task(task, tool_lookup),
        // Control flow nodes themselves are safe; the body steps are
        // classified individually when the engine visits them.
        StepType::ControlFlow { control } => match control {
            ControlFlowDef::Branch { .. }
            | ControlFlowDef::ForEach { .. }
            | ControlFlowDef::While { .. }
            | ControlFlowDef::EndWorkflow => RiskLevel::Safe,
        },
    }
}

// ---------------------------------------------------------------------------
// Schema-aware synthetic output generation
// ---------------------------------------------------------------------------

/// Generate a plausible zero-value from a JSON Schema.
///
/// Handles `type`, `properties`, `required`, `enum`, `const`, and basic
/// `oneOf`/`anyOf` (picks the first variant).  Produces empty/zero values
/// that are structurally valid enough for downstream expression evaluation.
pub fn generate_from_schema(schema: &serde_json::Value) -> serde_json::Value {
    use serde_json::{json, Value};

    // const takes absolute precedence
    if let Some(c) = schema.get("const") {
        return c.clone();
    }
    // enum → first variant
    if let Some(Value::Array(variants)) = schema.get("enum") {
        if let Some(first) = variants.first() {
            return first.clone();
        }
    }
    // oneOf / anyOf → recurse into first
    for key in &["oneOf", "anyOf"] {
        if let Some(Value::Array(variants)) = schema.get(key) {
            if let Some(first) = variants.first() {
                return generate_from_schema(first);
            }
        }
    }

    match schema.get("type").and_then(|t| t.as_str()) {
        Some("object") => {
            let mut obj = serde_json::Map::new();
            if let Some(props) = schema.get("properties").and_then(|p| p.as_object()) {
                for (key, sub_schema) in props {
                    obj.insert(key.clone(), generate_from_schema(sub_schema));
                }
            }
            Value::Object(obj)
        }
        Some("array") => {
            // If items schema is present, produce a single-element array
            // so downstream `.length` / indexing won't break unexpectedly.
            if let Some(items) = schema.get("items") {
                json!([generate_from_schema(items)])
            } else {
                json!([])
            }
        }
        Some("string") => json!(""),
        Some("number") => json!(0.0),
        Some("integer") => json!(0),
        Some("boolean") => json!(false),
        Some("null") => Value::Null,
        _ => Value::Null,
    }
}

// ---------------------------------------------------------------------------
// Workflow Impact Analysis
// ---------------------------------------------------------------------------

/// Confidence level for an impact estimate.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    /// All tools known, no unresolvable dynamic collections.
    High,
    /// Some unknowns or unresolvable collections.
    Medium,
    /// Many unknowns.
    Low,
}

/// Range estimate for an impact category.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EstimateRange {
    /// Known minimum count (1 per step occurrence outside loops).
    pub min: u64,
    /// Known maximum count if collection sizes are determinable.
    pub max: Option<u64>,
    /// Human-readable expression, e.g. "3 × ForEach(trigger.contacts)".
    pub expression: String,
}

/// Aggregated impact totals across the workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImpactTotals {
    pub external_messages: EstimateRange,
    pub http_calls: EstimateRange,
    pub agent_invocations: EstimateRange,
    pub destructive_ops: EstimateRange,
    pub scheduled_tasks: EstimateRange,
}

/// Risk information for a single step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepRiskInfo {
    pub step_id: String,
    pub risk_level: RiskLevel,
    pub action_summary: String,
    /// Enclosing loop multiplier expression, e.g. "× ForEach(trigger.contacts)".
    pub multiplier: Option<String>,
}

/// Full impact estimate for a workflow definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowImpactEstimate {
    pub steps: Vec<StepRiskInfo>,
    pub totals: ImpactTotals,
    pub confidence: Confidence,
}

/// Categorize a task step for impact counting purposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImpactCategory {
    ExternalMessage,
    HttpCall,
    AgentInvocation,
    DestructiveOp,
    ScheduledTask,
    None,
}

fn categorize_task(task: &TaskDef, tool_defs: &HashMap<String, ToolDefinition>) -> ImpactCategory {
    match task {
        TaskDef::CallTool { tool_id, .. } => {
            if tool_id.starts_with("connector.send_message") {
                return ImpactCategory::ExternalMessage;
            }
            if tool_id.starts_with("http.request") || tool_id.starts_with("http.") {
                return ImpactCategory::HttpCall;
            }
            if let Some(def) = tool_defs.get(tool_id.as_str()) {
                if def.annotations.destructive_hint == Some(true) {
                    return ImpactCategory::DestructiveOp;
                }
                if def.annotations.open_world_hint == Some(true) {
                    return ImpactCategory::HttpCall;
                }
            }
            ImpactCategory::None
        }
        TaskDef::InvokeAgent { .. } | TaskDef::InvokePrompt { .. } => ImpactCategory::AgentInvocation,
        TaskDef::ScheduleTask { .. } => ImpactCategory::ScheduledTask,
        _ => ImpactCategory::None,
    }
}

fn action_summary(task: &TaskDef) -> String {
    match task {
        TaskDef::CallTool { tool_id, .. } => {
            if tool_id.starts_with("connector.send_message") {
                format!("Sends message via {tool_id}")
            } else if tool_id.starts_with("http.") {
                format!("HTTP request via {tool_id}")
            } else {
                format!("Calls {tool_id}")
            }
        }
        TaskDef::InvokeAgent { persona_id, .. } => format!("Invokes agent '{persona_id}'"),
        TaskDef::InvokePrompt { persona_id, .. } => format!("Invokes prompt '{persona_id}'"),
        TaskDef::LaunchWorkflow { workflow_name, .. } => format!("Launches workflow '{workflow_name}'"),
        TaskDef::ScheduleTask { schedule } => format!("Schedules task '{}'", schedule.name),
        TaskDef::SignalAgent { .. } => "Signals agent".to_string(),
        TaskDef::SetVariable { .. } => "Sets variable".to_string(),
        TaskDef::Delay { .. } => "Delay".to_string(),
        TaskDef::FeedbackGate { .. } => "Feedback gate".to_string(),
        TaskDef::EventGate { .. } => "Event gate".to_string(),
    }
}

/// Compute the loop multiplier expression for a step by walking the
/// step graph upward to find enclosing ForEach/While loops.
fn compute_multiplier(
    step_id: &str,
    _steps: &[StepDef],
    loop_bodies: &HashMap<String, (String, &ControlFlowDef)>,
) -> Option<String> {
    // loop_bodies maps body_step_id → (loop_step_id, control_flow_def)
    let mut multipliers = Vec::new();
    let mut current = step_id.to_string();

    // Walk up through enclosing loops
    for _ in 0..10 {
        // Safety limit to prevent infinite loops
        if let Some((loop_id, cf)) = loop_bodies.get(&current) {
            match cf {
                ControlFlowDef::ForEach { collection, .. } => {
                    multipliers.push(format!("ForEach({collection})"));
                }
                ControlFlowDef::While { max_iterations, .. } => {
                    let max = max_iterations.unwrap_or(100);
                    multipliers.push(format!("While(max={max})"));
                }
                _ => {}
            }
            current = loop_id.clone();
        } else {
            break;
        }
    }

    if multipliers.is_empty() {
        None
    } else {
        Some(format!("× {}", multipliers.join(" × ")))
    }
}

/// Analyze a workflow definition to produce an impact estimate.
///
/// `tool_defs` maps tool_id → ToolDefinition for classification.
pub fn analyze_workflow(
    definition: &WorkflowDefinition,
    tool_defs: &HashMap<String, ToolDefinition>,
) -> WorkflowImpactEstimate {
    let tool_lookup_risk = |id: &str| -> Option<RiskLevel> {
        tool_defs.get(id).map(classify_tool)
    };

    // Build loop body → (loop_step_id, control_flow_def) mapping
    let mut loop_bodies: HashMap<String, (String, &ControlFlowDef)> = HashMap::new();
    for step in &definition.steps {
        if let StepType::ControlFlow { control } = &step.step_type {
            let body_ids = match control {
                ControlFlowDef::ForEach { body, .. } => body.as_slice(),
                ControlFlowDef::While { body, .. } => body.as_slice(),
                _ => continue,
            };
            for body_id in body_ids {
                loop_bodies.insert(body_id.clone(), (step.id.clone(), control));
            }
        }
    }

    let mut step_risks = Vec::new();
    let mut unknowns = 0u32;
    let mut total_steps = 0u32;

    // Impact counters: (min_count, has_loop_multiplier, multiplier_expression)
    struct Counter {
        min: u64,
        has_loop: bool,
        expressions: Vec<String>,
    }
    impl Counter {
        fn new() -> Self {
            Self { min: 0, has_loop: false, expressions: Vec::new() }
        }
        fn add(&mut self, multiplier: Option<&str>) {
            self.min += 1;
            if let Some(m) = multiplier {
                self.has_loop = true;
                self.expressions.push(format!("1 {m}"));
            } else {
                self.expressions.push("1".to_string());
            }
        }
        fn to_estimate(&self) -> EstimateRange {
            EstimateRange {
                min: self.min,
                max: if self.has_loop { None } else { Some(self.min) },
                expression: if self.expressions.is_empty() {
                    "0".to_string()
                } else {
                    self.expressions.join(" + ")
                },
            }
        }
    }

    let mut ext_messages = Counter::new();
    let mut http_calls = Counter::new();
    let mut agent_invocations = Counter::new();
    let mut destructive_ops = Counter::new();
    let mut scheduled_tasks = Counter::new();

    for step in &definition.steps {
        let risk = classify_step_type(&step.step_type, &tool_lookup_risk);
        if risk == RiskLevel::Unknown {
            unknowns += 1;
        }
        total_steps += 1;

        let (summary, category) = match &step.step_type {
            StepType::Task { task } => {
                (action_summary(task), categorize_task(task, tool_defs))
            }
            StepType::Trigger { .. } => ("Trigger".to_string(), ImpactCategory::None),
            StepType::ControlFlow { .. } => ("Control flow".to_string(), ImpactCategory::None),
        };

        let multiplier = compute_multiplier(&step.id, &definition.steps, &loop_bodies);

        // Count impacts
        let mult_ref = multiplier.as_deref();
        match category {
            ImpactCategory::ExternalMessage => ext_messages.add(mult_ref),
            ImpactCategory::HttpCall => http_calls.add(mult_ref),
            ImpactCategory::AgentInvocation => agent_invocations.add(mult_ref),
            ImpactCategory::DestructiveOp => destructive_ops.add(mult_ref),
            ImpactCategory::ScheduledTask => scheduled_tasks.add(mult_ref),
            ImpactCategory::None => {}
        }

        step_risks.push(StepRiskInfo {
            step_id: step.id.clone(),
            risk_level: risk,
            action_summary: summary,
            multiplier,
        });
    }

    let confidence = if unknowns == 0 {
        Confidence::High
    } else if unknowns * 2 <= total_steps {
        Confidence::Medium
    } else {
        Confidence::Low
    };

    WorkflowImpactEstimate {
        steps: step_risks,
        totals: ImpactTotals {
            external_messages: ext_messages.to_estimate(),
            http_calls: http_calls.to_estimate(),
            agent_invocations: agent_invocations.to_estimate(),
            destructive_ops: destructive_ops.to_estimate(),
            scheduled_tasks: scheduled_tasks.to_estimate(),
        },
        confidence,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TriggerType;
    use hive_contracts::ChannelClass;
    use hive_contracts::tools::{ToolAnnotations, ToolApproval};
    use serde_json::json;

    fn make_tool(id: &str) -> ToolDefinition {
        ToolDefinition {
            id: id.to_string(),
            name: id.to_string(),
            description: String::new(),
            input_schema: json!({}),
            output_schema: None,
            channel_class: ChannelClass::Internal,
            side_effects: false,
            approval: ToolApproval::Auto,
            annotations: ToolAnnotations {
                title: id.to_string(),
                read_only_hint: None,
                destructive_hint: None,
                idempotent_hint: None,
                open_world_hint: None,
            },
        }
    }

    fn make_step(id: &str, step_type: StepType) -> StepDef {
        StepDef {
            id: id.to_string(),
            step_type,
            outputs: HashMap::new(),
            on_error: None,
            next: vec![],
            timeout_secs: None,
            designer_x: None,
            designer_y: None,
        }
    }

    fn make_definition(steps: Vec<StepDef>) -> WorkflowDefinition {
        WorkflowDefinition {
            id: "test".to_string(),
            name: "test/wf".to_string(),
            version: "1.0".to_string(),
            description: None,
            mode: crate::types::WorkflowMode::Background,
            variables: json!({}),
            steps,
            output: None,
            result_message: None,
            requested_tools: vec![],
            permissions: vec![],
            attachments: vec![],
            tests: vec![],
            bundled: false,
            archived: false,
            triggers_paused: false,
        }
    }

    fn make_trigger_step(id: &str) -> StepDef {
        make_step(id, StepType::Trigger {
            trigger: crate::types::TriggerDef {
                trigger_type: TriggerType::Manual {
                    inputs: vec![],
                    input_schema: None,
                },
            },
        })
    }

    #[test]
    fn read_only_is_safe() {
        let mut tool = make_tool("some.tool");
        tool.annotations.read_only_hint = Some(true);
        assert_eq!(classify_tool(&tool), RiskLevel::Safe);
    }

    #[test]
    fn read_only_overrides_open_world() {
        let mut tool = make_tool("web.fetch");
        tool.annotations.read_only_hint = Some(true);
        tool.annotations.open_world_hint = Some(true);
        assert_eq!(classify_tool(&tool), RiskLevel::Safe);
    }

    #[test]
    fn destructive_is_danger() {
        let mut tool = make_tool("db.drop_table");
        tool.annotations.destructive_hint = Some(true);
        assert_eq!(classify_tool(&tool), RiskLevel::Danger);
    }

    #[test]
    fn open_world_is_danger() {
        let mut tool = make_tool("ext.api");
        tool.annotations.open_world_hint = Some(true);
        assert_eq!(classify_tool(&tool), RiskLevel::Danger);
    }

    #[test]
    fn known_dangerous_prefix() {
        let tool = {
            let mut t = make_tool("connector.send_message.email");
            t.side_effects = true;
            t
        };
        assert_eq!(classify_tool(&tool), RiskLevel::Danger);
    }

    #[test]
    fn side_effects_is_caution() {
        let mut tool = make_tool("some.internal.tool");
        tool.side_effects = true;
        assert_eq!(classify_tool(&tool), RiskLevel::Caution);
    }

    #[test]
    fn no_side_effects_no_hints_is_safe() {
        let tool = make_tool("pure.read.tool");
        assert_eq!(classify_tool(&tool), RiskLevel::Safe);
    }

    #[test]
    fn unknown_tool_from_task() {
        let task = TaskDef::CallTool {
            tool_id: "nonexistent.tool".into(),
            arguments: Default::default(),
        };
        let lookup = |_: &str| -> Option<RiskLevel> { None };
        assert_eq!(classify_task(&task, &lookup), RiskLevel::Unknown);
        assert!(RiskLevel::Unknown.should_intercept());
    }

    #[test]
    fn invoke_agent_is_caution() {
        let task = TaskDef::InvokeAgent {
            persona_id: "p".into(),
            task: "t".into(),
            async_exec: false,
            timeout_secs: None,
            permissions: vec![],
            attachments: vec![],
            agent_name: None,
        };
        assert_eq!(classify_task(&task, &|_| None), RiskLevel::Caution);
    }

    #[test]
    fn schedule_task_is_danger() {
        let task = TaskDef::ScheduleTask {
            schedule: crate::types::ScheduleTaskDef {
                name: "w".into(),
                schedule: "* * * * *".into(),
                action: serde_json::json!({}),
            },
        };
        assert_eq!(classify_task(&task, &|_| None), RiskLevel::Danger);
    }

    #[test]
    fn set_variable_is_safe() {
        let task = TaskDef::SetVariable {
            assignments: vec![],
        };
        assert_eq!(classify_task(&task, &|_| None), RiskLevel::Safe);
    }

    #[test]
    fn generate_schema_object() {
        let schema = json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "count": { "type": "integer" },
                "active": { "type": "boolean" }
            }
        });
        let result = generate_from_schema(&schema);
        assert_eq!(result["name"], json!(""));
        assert_eq!(result["count"], json!(0));
        assert_eq!(result["active"], json!(false));
    }

    #[test]
    fn generate_schema_enum() {
        let schema = json!({ "type": "string", "enum": ["a", "b", "c"] });
        assert_eq!(generate_from_schema(&schema), json!("a"));
    }

    #[test]
    fn generate_schema_const() {
        let schema = json!({ "const": 42 });
        assert_eq!(generate_from_schema(&schema), json!(42));
    }

    #[test]
    fn generate_schema_array_with_items() {
        let schema = json!({ "type": "array", "items": { "type": "string" } });
        assert_eq!(generate_from_schema(&schema), json!([""]));
    }

    #[test]
    fn safe_does_not_intercept() {
        assert!(!RiskLevel::Safe.should_intercept());
    }

    #[test]
    fn caution_danger_unknown_intercept() {
        assert!(RiskLevel::Caution.should_intercept());
        assert!(RiskLevel::Danger.should_intercept());
        assert!(RiskLevel::Unknown.should_intercept());
    }

    // -----------------------------------------------------------------------
    // Impact analyzer tests
    // -----------------------------------------------------------------------

    #[test]
    fn impact_simple_workflow_no_loops() {
        let mut read_tool = make_tool("fs.read");
        read_tool.annotations.read_only_hint = Some(true);
        let mut send_tool = make_tool("connector.send_message");
        send_tool.side_effects = true;

        let tool_defs: HashMap<String, ToolDefinition> = [
            ("fs.read".to_string(), read_tool),
            ("connector.send_message".to_string(), send_tool),
        ]
        .into_iter()
        .collect();

        let def = make_definition(vec![
            make_trigger_step("trigger"),
            make_step("read_step", StepType::Task {
                task: TaskDef::CallTool {
                    tool_id: "fs.read".into(),
                    arguments: Default::default(),
                },
            }),
            make_step("send_step", StepType::Task {
                task: TaskDef::CallTool {
                    tool_id: "connector.send_message".into(),
                    arguments: Default::default(),
                },
            }),
        ]);

        let est = analyze_workflow(&def, &tool_defs);
        assert_eq!(est.confidence, Confidence::High);
        assert_eq!(est.totals.external_messages.min, 1);
        assert_eq!(est.totals.external_messages.max, Some(1));
        assert_eq!(est.totals.http_calls.min, 0);

        // Check individual step risks
        let read_risk = est.steps.iter().find(|s| s.step_id == "read_step").unwrap();
        assert_eq!(read_risk.risk_level, RiskLevel::Safe);
        assert!(read_risk.multiplier.is_none());

        let send_risk = est.steps.iter().find(|s| s.step_id == "send_step").unwrap();
        assert_eq!(send_risk.risk_level, RiskLevel::Danger);
    }

    #[test]
    fn impact_foreach_multiplier() {
        let mut send_tool = make_tool("connector.send_message");
        send_tool.side_effects = true;

        let tool_defs: HashMap<String, ToolDefinition> = [
            ("connector.send_message".to_string(), send_tool),
        ]
        .into_iter()
        .collect();

        let def = make_definition(vec![
            make_trigger_step("trigger"),
            make_step("loop", StepType::ControlFlow {
                control: ControlFlowDef::ForEach {
                    collection: "trigger.contacts".into(),
                    item_var: "contact".into(),
                    body: vec!["send_email".into()],
                    preview_count: None,
                },
            }),
            make_step("send_email", StepType::Task {
                task: TaskDef::CallTool {
                    tool_id: "connector.send_message".into(),
                    arguments: Default::default(),
                },
            }),
        ]);

        let est = analyze_workflow(&def, &tool_defs);
        assert_eq!(est.totals.external_messages.min, 1);
        // Has a loop multiplier, so max is unknown
        assert_eq!(est.totals.external_messages.max, None);

        let send_risk = est.steps.iter().find(|s| s.step_id == "send_email").unwrap();
        assert!(send_risk.multiplier.is_some());
        assert!(send_risk.multiplier.as_ref().unwrap().contains("ForEach"));
    }

    #[test]
    fn impact_nested_loops() {
        let mut send_tool = make_tool("connector.send_message");
        send_tool.side_effects = true;
        let tool_defs: HashMap<String, ToolDefinition> = [
            ("connector.send_message".to_string(), send_tool),
        ]
        .into_iter()
        .collect();

        let def = make_definition(vec![
            make_trigger_step("trigger"),
            make_step("outer_loop", StepType::ControlFlow {
                control: ControlFlowDef::ForEach {
                    collection: "trigger.departments".into(),
                    item_var: "dept".into(),
                    body: vec!["inner_loop".into()],
                    preview_count: None,
                },
            }),
            make_step("inner_loop", StepType::ControlFlow {
                control: ControlFlowDef::ForEach {
                    collection: "dept.members".into(),
                    item_var: "member".into(),
                    body: vec!["send_email".into()],
                    preview_count: None,
                },
            }),
            make_step("send_email", StepType::Task {
                task: TaskDef::CallTool {
                    tool_id: "connector.send_message".into(),
                    arguments: Default::default(),
                },
            }),
        ]);

        let est = analyze_workflow(&def, &tool_defs);

        let send_risk = est.steps.iter().find(|s| s.step_id == "send_email").unwrap();
        let mult = send_risk.multiplier.as_ref().unwrap();
        // Should reference both loops
        assert!(mult.contains("ForEach(dept.members)"));
        assert!(mult.contains("ForEach(trigger.departments)"));
    }

    #[test]
    fn impact_unknown_tools_lower_confidence() {
        let tool_defs: HashMap<String, ToolDefinition> = HashMap::new();

        let def = make_definition(vec![
            make_trigger_step("trigger"),
            make_step("unknown1", StepType::Task {
                task: TaskDef::CallTool {
                    tool_id: "nonexistent.tool".into(),
                    arguments: Default::default(),
                },
            }),
            make_step("unknown2", StepType::Task {
                task: TaskDef::CallTool {
                    tool_id: "another.unknown".into(),
                    arguments: Default::default(),
                },
            }),
        ]);

        let est = analyze_workflow(&def, &tool_defs);
        // All CallTool steps are unknown → Low confidence since 2/3 are unknown
        assert_eq!(est.confidence, Confidence::Low);
    }

    #[test]
    fn impact_agent_invocations_counted() {
        let tool_defs: HashMap<String, ToolDefinition> = HashMap::new();

        let def = make_definition(vec![
            make_trigger_step("trigger"),
            make_step("agent_step", StepType::Task {
                task: TaskDef::InvokeAgent {
                    persona_id: "email-drafter".into(),
                    task: "Draft a reply".into(),
                    async_exec: false,
                    timeout_secs: None,
                    permissions: vec![],
                    attachments: vec![],
                    agent_name: None,
                },
            }),
        ]);

        let est = analyze_workflow(&def, &tool_defs);
        assert_eq!(est.totals.agent_invocations.min, 1);
        assert_eq!(est.totals.agent_invocations.max, Some(1));
    }

    #[test]
    fn impact_schedule_tasks_counted() {
        let tool_defs: HashMap<String, ToolDefinition> = HashMap::new();

        let def = make_definition(vec![
            make_trigger_step("trigger"),
            make_step("sched_step", StepType::Task {
                task: TaskDef::ScheduleTask {
                    schedule: crate::types::ScheduleTaskDef {
                        name: "daily-check".into(),
                        schedule: "0 9 * * *".into(),
                        action: json!({}),
                    },
                },
            }),
        ]);

        let est = analyze_workflow(&def, &tool_defs);
        assert_eq!(est.totals.scheduled_tasks.min, 1);
        assert_eq!(est.totals.scheduled_tasks.max, Some(1));
    }

    #[test]
    fn impact_while_loop_multiplier() {
        let mut http_tool = make_tool("http.request");
        http_tool.side_effects = true;
        http_tool.annotations.open_world_hint = Some(true);
        let tool_defs: HashMap<String, ToolDefinition> = [
            ("http.request".to_string(), http_tool),
        ]
        .into_iter()
        .collect();

        let def = make_definition(vec![
            make_trigger_step("trigger"),
            make_step("poll_loop", StepType::ControlFlow {
                control: ControlFlowDef::While {
                    condition: "{{vars.has_more}}".into(),
                    max_iterations: Some(50),
                    body: vec!["fetch_page".into()],
                },
            }),
            make_step("fetch_page", StepType::Task {
                task: TaskDef::CallTool {
                    tool_id: "http.request".into(),
                    arguments: Default::default(),
                },
            }),
        ]);

        let est = analyze_workflow(&def, &tool_defs);
        assert_eq!(est.totals.http_calls.min, 1);
        assert_eq!(est.totals.http_calls.max, None); // Has loop multiplier

        let fetch_risk = est.steps.iter().find(|s| s.step_id == "fetch_page").unwrap();
        assert!(fetch_risk.multiplier.as_ref().unwrap().contains("While(max=50)"));
    }

    #[test]
    fn impact_mixed_high_confidence() {
        let mut read_tool = make_tool("fs.read");
        read_tool.annotations.read_only_hint = Some(true);
        let tool_defs: HashMap<String, ToolDefinition> = [
            ("fs.read".to_string(), read_tool),
        ]
        .into_iter()
        .collect();

        let def = make_definition(vec![
            make_trigger_step("trigger"),
            make_step("read", StepType::Task {
                task: TaskDef::CallTool {
                    tool_id: "fs.read".into(),
                    arguments: Default::default(),
                },
            }),
            make_step("set_var", StepType::Task {
                task: TaskDef::SetVariable { assignments: vec![] },
            }),
        ]);

        let est = analyze_workflow(&def, &tool_defs);
        assert_eq!(est.confidence, Confidence::High);
        // No side-effecting actions
        assert_eq!(est.totals.external_messages.min, 0);
        assert_eq!(est.totals.http_calls.min, 0);
        assert_eq!(est.totals.agent_invocations.min, 0);
    }
}
