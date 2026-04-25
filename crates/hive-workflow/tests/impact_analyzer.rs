//! Integration tests for the workflow impact analyzer.
//!
//! These verify that `analyze_workflow()` correctly classifies step risks,
//! computes loop multipliers, categorizes actions, and sets confidence levels
//! for various workflow shapes.

use hive_contracts::tools::ToolDefinitionBuilder;
use hive_workflow::*;
use serde_json::json;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn trigger_step(id: &str) -> StepDef {
    StepDef {
        id: id.to_string(),
        step_type: StepType::Trigger {
            trigger: TriggerDef {
                trigger_type: TriggerType::Manual {
                    inputs: vec![],
                    input_schema: None,
                },
            },
        },
        outputs: HashMap::new(),
        on_error: None,
        next: vec![],
        timeout_secs: None,
        designer_x: None,
        designer_y: None,
    }
}

fn task_step(id: &str, task: TaskDef) -> StepDef {
    StepDef {
        id: id.to_string(),
        step_type: StepType::Task { task },
        outputs: HashMap::new(),
        on_error: None,
        next: vec![],
        timeout_secs: None,
        designer_x: None,
        designer_y: None,
    }
}

fn cf_step(id: &str, control: ControlFlowDef) -> StepDef {
    StepDef {
        id: id.to_string(),
        step_type: StepType::ControlFlow { control },
        outputs: HashMap::new(),
        on_error: None,
        next: vec![],
        timeout_secs: None,
        designer_x: None,
        designer_y: None,
    }
}

fn make_def(steps: Vec<StepDef>) -> WorkflowDefinition {
    WorkflowDefinition {
        id: "test-id".to_string(),
        name: "test/analyzer".to_string(),
        version: "1.0".to_string(),
        description: None,
        mode: WorkflowMode::Background,
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn step_risk_levels_correct() {
    let mut tools: HashMap<String, hive_contracts::tools::ToolDefinition> = HashMap::new();
    tools.insert(
        "fs.read".into(),
        ToolDefinitionBuilder::new("fs.read", "Read File")
            .read_only()
            .build(),
    );
    tools.insert(
        "connector.send_message".into(),
        ToolDefinitionBuilder::new("connector.send_message", "Send Message")
            .side_effects(true)
            .build(),
    );

    let def = make_def(vec![
        trigger_step("trigger"),
        task_step(
            "read",
            TaskDef::CallTool {
                tool_id: "fs.read".into(),
                arguments: Default::default(),
            },
        ),
        task_step(
            "send",
            TaskDef::CallTool {
                tool_id: "connector.send_message".into(),
                arguments: Default::default(),
            },
        ),
        task_step(
            "agent",
            TaskDef::InvokeAgent {
                persona_id: "drafter".into(),
                task: "Draft email".into(),
                async_exec: false,
                timeout_secs: None,
                permissions: vec![],
                attachments: vec![],
                agent_name: None,
            },
        ),
        task_step(
            "unknown",
            TaskDef::CallTool {
                tool_id: "nonexistent.tool".into(),
                arguments: Default::default(),
            },
        ),
    ]);

    let est = analyze_workflow(&def, &tools);

    // Check individual step risk levels
    let find = |id: &str| est.steps.iter().find(|s| s.step_id == id).unwrap();

    assert_eq!(find("trigger").risk_level, RiskLevel::Safe);
    assert_eq!(find("read").risk_level, RiskLevel::Safe);
    assert_eq!(find("send").risk_level, RiskLevel::Danger);
    assert_eq!(find("agent").risk_level, RiskLevel::Caution);
    assert_eq!(find("unknown").risk_level, RiskLevel::Unknown);

    // Has unknowns → not High
    assert_ne!(est.confidence, Confidence::High);
}

#[test]
fn foreach_multiplier_in_expression() {
    let mut tools: HashMap<String, hive_contracts::tools::ToolDefinition> = HashMap::new();
    tools.insert(
        "connector.send_message".into(),
        ToolDefinitionBuilder::new("connector.send_message", "Send")
            .side_effects(true)
            .build(),
    );

    let def = make_def(vec![
        trigger_step("trigger"),
        cf_step(
            "loop",
            ControlFlowDef::ForEach {
                collection: "trigger.contacts".into(),
                item_var: "contact".into(),
                body: vec!["send_email".into()],
                preview_count: None,
            },
        ),
        task_step(
            "send_email",
            TaskDef::CallTool {
                tool_id: "connector.send_message".into(),
                arguments: Default::default(),
            },
        ),
    ]);

    let est = analyze_workflow(&def, &tools);

    // Messages have loop multiplier → max is None (unknown dynamic size)
    assert_eq!(est.totals.external_messages.min, 1);
    assert_eq!(est.totals.external_messages.max, None);

    let send = est.steps.iter().find(|s| s.step_id == "send_email").unwrap();
    let mult = send.multiplier.as_ref().expect("should have multiplier");
    assert!(
        mult.contains("ForEach(trigger.contacts)"),
        "multiplier should contain ForEach expression, got: {mult}"
    );
}

#[test]
fn nested_loops_both_multipliers() {
    let tools: HashMap<String, hive_contracts::tools::ToolDefinition> = [(
        "http.request".into(),
        ToolDefinitionBuilder::new("http.request", "HTTP Request")
            .side_effects(true)
            .open_world()
            .build(),
    )]
    .into_iter()
    .collect();

    let def = make_def(vec![
        trigger_step("trigger"),
        cf_step(
            "outer",
            ControlFlowDef::ForEach {
                collection: "trigger.depts".into(),
                item_var: "dept".into(),
                body: vec!["inner".into()],
                preview_count: None,
            },
        ),
        cf_step(
            "inner",
            ControlFlowDef::ForEach {
                collection: "dept.members".into(),
                item_var: "member".into(),
                body: vec!["fetch".into()],
                preview_count: None,
            },
        ),
        task_step(
            "fetch",
            TaskDef::CallTool {
                tool_id: "http.request".into(),
                arguments: Default::default(),
            },
        ),
    ]);

    let est = analyze_workflow(&def, &tools);

    let fetch = est.steps.iter().find(|s| s.step_id == "fetch").unwrap();
    let mult = fetch.multiplier.as_ref().expect("should have multiplier");
    assert!(mult.contains("ForEach(dept.members)"), "mult: {mult}");
    assert!(mult.contains("ForEach(trigger.depts)"), "mult: {mult}");
}

#[test]
fn all_unknown_tools_low_confidence() {
    let tools: HashMap<String, hive_contracts::tools::ToolDefinition> = HashMap::new();

    let def = make_def(vec![
        trigger_step("trigger"),
        task_step("a", TaskDef::CallTool { tool_id: "x.y".into(), arguments: Default::default() }),
        task_step("b", TaskDef::CallTool { tool_id: "z.w".into(), arguments: Default::default() }),
    ]);

    let est = analyze_workflow(&def, &tools);
    assert_eq!(est.confidence, Confidence::Low);
}

#[test]
fn all_known_tools_high_confidence() {
    let tools: HashMap<String, hive_contracts::tools::ToolDefinition> = [(
        "fs.read".into(),
        ToolDefinitionBuilder::new("fs.read", "Read").read_only().build(),
    )]
    .into_iter()
    .collect();

    let def = make_def(vec![
        trigger_step("trigger"),
        task_step("r", TaskDef::CallTool { tool_id: "fs.read".into(), arguments: Default::default() }),
        task_step("v", TaskDef::SetVariable { assignments: vec![] }),
    ]);

    let est = analyze_workflow(&def, &tools);
    assert_eq!(est.confidence, Confidence::High);
    // No side-effecting actions
    assert_eq!(est.totals.external_messages.min, 0);
    assert_eq!(est.totals.http_calls.min, 0);
}

#[test]
fn agent_invocations_counted() {
    let tools = HashMap::new();

    let def = make_def(vec![
        trigger_step("trigger"),
        task_step(
            "agent1",
            TaskDef::InvokeAgent {
                persona_id: "drafter".into(),
                task: "Write email".into(),
                async_exec: false,
                timeout_secs: None,
                permissions: vec![],
                attachments: vec![],
                agent_name: None,
            },
        ),
        task_step(
            "agent2",
            TaskDef::InvokePrompt {
                persona_id: "summarizer".into(),
                prompt_id: "p1".into(),
                parameters: Default::default(),
                async_exec: false,
                timeout_secs: None,
                permissions: vec![],
                target_agent_id: None,
                auto_create: false,
                agent_name: None,
            },
        ),
    ]);

    let est = analyze_workflow(&def, &tools);
    assert_eq!(est.totals.agent_invocations.min, 2);
    assert_eq!(est.totals.agent_invocations.max, Some(2));
}

#[test]
fn scheduled_tasks_counted_and_danger() {
    let tools = HashMap::new();

    let def = make_def(vec![
        trigger_step("trigger"),
        task_step(
            "sched",
            TaskDef::ScheduleTask {
                schedule: ScheduleTaskDef {
                    name: "daily".into(),
                    schedule: "0 9 * * *".into(),
                    action: json!({}),
                },
            },
        ),
    ]);

    let est = analyze_workflow(&def, &tools);
    assert_eq!(est.totals.scheduled_tasks.min, 1);
    let sched = est.steps.iter().find(|s| s.step_id == "sched").unwrap();
    assert_eq!(sched.risk_level, RiskLevel::Danger);
}

#[test]
fn while_loop_shows_max_iterations() {
    let tools: HashMap<String, hive_contracts::tools::ToolDefinition> = [(
        "http.request".into(),
        ToolDefinitionBuilder::new("http.request", "HTTP")
            .side_effects(true)
            .open_world()
            .build(),
    )]
    .into_iter()
    .collect();

    let def = make_def(vec![
        trigger_step("trigger"),
        cf_step(
            "poll",
            ControlFlowDef::While {
                condition: "{{vars.more}}".into(),
                max_iterations: Some(25),
                body: vec!["req".into()],
            },
        ),
        task_step(
            "req",
            TaskDef::CallTool {
                tool_id: "http.request".into(),
                arguments: Default::default(),
            },
        ),
    ]);

    let est = analyze_workflow(&def, &tools);

    let req = est.steps.iter().find(|s| s.step_id == "req").unwrap();
    let mult = req.multiplier.as_ref().expect("should have multiplier");
    assert!(mult.contains("While(max=25)"), "mult: {mult}");

    // Has loop → max is None
    assert_eq!(est.totals.http_calls.max, None);
}

#[test]
fn pure_safe_workflow_zero_impact() {
    let tools: HashMap<String, hive_contracts::tools::ToolDefinition> = [(
        "fs.list".into(),
        ToolDefinitionBuilder::new("fs.list", "List Files").read_only().build(),
    )]
    .into_iter()
    .collect();

    let def = make_def(vec![
        trigger_step("trigger"),
        task_step("list", TaskDef::CallTool { tool_id: "fs.list".into(), arguments: Default::default() }),
        task_step("var", TaskDef::SetVariable { assignments: vec![] }),
        task_step("wait", TaskDef::Delay { duration_secs: 1 }),
    ]);

    let est = analyze_workflow(&def, &tools);
    assert_eq!(est.confidence, Confidence::High);
    assert_eq!(est.totals.external_messages.min, 0);
    assert_eq!(est.totals.http_calls.min, 0);
    assert_eq!(est.totals.agent_invocations.min, 0);
    assert_eq!(est.totals.destructive_ops.min, 0);
    assert_eq!(est.totals.scheduled_tasks.min, 0);
}

#[test]
fn destructive_tool_counted() {
    let tools: HashMap<String, hive_contracts::tools::ToolDefinition> = [(
        "db.delete_all".into(),
        ToolDefinitionBuilder::new("db.delete_all", "Delete All")
            .destructive()
            .build(),
    )]
    .into_iter()
    .collect();

    let def = make_def(vec![
        trigger_step("trigger"),
        task_step(
            "destroy",
            TaskDef::CallTool {
                tool_id: "db.delete_all".into(),
                arguments: Default::default(),
            },
        ),
    ]);

    let est = analyze_workflow(&def, &tools);
    assert_eq!(est.totals.destructive_ops.min, 1);
    assert_eq!(est.totals.destructive_ops.max, Some(1));
    assert_eq!(
        est.steps.iter().find(|s| s.step_id == "destroy").unwrap().risk_level,
        RiskLevel::Danger,
    );
}

#[test]
fn no_multiplier_for_top_level_steps() {
    let tools: HashMap<String, hive_contracts::tools::ToolDefinition> = [(
        "connector.send_message".into(),
        ToolDefinitionBuilder::new("connector.send_message", "Send")
            .side_effects(true)
            .build(),
    )]
    .into_iter()
    .collect();

    let def = make_def(vec![
        trigger_step("trigger"),
        task_step(
            "send",
            TaskDef::CallTool {
                tool_id: "connector.send_message".into(),
                arguments: Default::default(),
            },
        ),
    ]);

    let est = analyze_workflow(&def, &tools);
    let send = est.steps.iter().find(|s| s.step_id == "send").unwrap();
    assert!(send.multiplier.is_none(), "top-level step should have no multiplier");
    assert_eq!(est.totals.external_messages.min, 1);
    assert_eq!(est.totals.external_messages.max, Some(1));
}
