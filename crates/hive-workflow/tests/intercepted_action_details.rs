//! Integration tests for Phase 6 — Rich intercepted action details.
//!
//! These verify that the shadow executor captures rich, structured details
//! for each intercepted action kind, suitable for the mock sandbox viewers
//! (email inbox, HTTP log, agent log).

use async_trait::async_trait;
use hive_contracts::tools::ToolDefinitionBuilder;
use hive_workflow::*;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

// ---------------------------------------------------------------------------
// Test helpers (shared pattern from shadow_integration.rs)
// ---------------------------------------------------------------------------

async fn wait_for_terminal(store: &WorkflowStore, instance_id: i64) {
    for _ in 0..200 {
        let inst = store.get_instance(instance_id).unwrap().unwrap();
        if matches!(
            inst.status,
            WorkflowStatus::Completed | WorkflowStatus::Failed | WorkflowStatus::Killed
        ) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!(
        "instance {} did not reach terminal state in time",
        instance_id
    );
}

struct MockToolInfo {
    tools: HashMap<String, hive_contracts::tools::ToolDefinition>,
}

impl ToolInfoProvider for MockToolInfo {
    fn get_tool_definition(
        &self,
        tool_id: &str,
    ) -> Option<hive_contracts::tools::ToolDefinition> {
        self.tools.get(tool_id).cloned()
    }
}

struct RecordingExecutor {
    tool_calls: Mutex<Vec<(String, Value)>>,
}

impl RecordingExecutor {
    fn new() -> Self {
        Self {
            tool_calls: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl StepExecutor for RecordingExecutor {
    async fn call_tool(
        &self,
        tool_id: &str,
        arguments: Value,
        _ctx: &ExecutionContext,
    ) -> Result<Value, String> {
        self.tool_calls
            .lock()
            .await
            .push((tool_id.to_string(), arguments.clone()));
        Ok(json!({"echo": "real", "status": "ok"}))
    }

    async fn invoke_agent(
        &self,
        _persona_id: &str,
        _task: &str,
        _async_exec: bool,
        _timeout_secs: Option<u64>,
        _permissions: &[PermissionEntry],
        _agent_name: Option<&str>,
        _existing_agent_id: Option<&str>,
        _ctx: &ExecutionContext,
    ) -> Result<Value, String> {
        Ok(json!({"result": "agent_done"}))
    }

    async fn signal_agent(
        &self,
        _target: &SignalTarget,
        _content: &str,
        _ctx: &ExecutionContext,
    ) -> Result<Value, String> {
        Ok(json!({}))
    }

    async fn wait_for_agent(
        &self,
        _agent_id: &str,
        _timeout_secs: Option<u64>,
        _ctx: &ExecutionContext,
    ) -> Result<Value, String> {
        Ok(json!({}))
    }

    async fn create_feedback_request(
        &self,
        _instance_id: i64,
        _step_id: &str,
        _prompt: &str,
        _choices: Option<&[String]>,
        _allow_freeform: bool,
        _ctx: &ExecutionContext,
    ) -> Result<String, String> {
        Ok("mock-req".to_string())
    }

    async fn register_event_gate(
        &self,
        _instance_id: i64,
        _step_id: &str,
        _topic: &str,
        _filter: Option<&str>,
        _timeout_secs: Option<u64>,
        _ctx: &ExecutionContext,
    ) -> Result<String, String> {
        Ok("mock-sub".to_string())
    }

    async fn launch_workflow(
        &self,
        _name: &str,
        _inputs: Value,
        _ctx: &ExecutionContext,
    ) -> Result<i64, String> {
        Ok(9999)
    }

    async fn schedule_task(
        &self,
        _schedule: &ScheduleTaskDef,
        _ctx: &ExecutionContext,
    ) -> Result<String, String> {
        Ok("task-id".to_string())
    }

    async fn render_prompt_template(
        &self,
        _persona_id: &str,
        _prompt_id: &str,
        _parameters: Value,
        _ctx: &ExecutionContext,
    ) -> Result<String, String> {
        Ok("rendered prompt".to_string())
    }
}

fn build_engine(
    store: Arc<WorkflowStore>,
    executor: Arc<dyn StepExecutor>,
    tools: HashMap<String, hive_contracts::tools::ToolDefinition>,
) -> WorkflowEngine {
    let emitter = Arc::new(NullEventEmitter);
    let mut engine = WorkflowEngine::new(store, executor, emitter);
    engine.set_tool_info_provider(Arc::new(MockToolInfo { tools }));
    engine
}

fn make_tool_map(
    defs: Vec<hive_contracts::tools::ToolDefinition>,
) -> HashMap<String, hive_contracts::tools::ToolDefinition> {
    defs.into_iter().map(|d| (d.id.clone(), d)).collect()
}

// ===================================================================
// T6.1: Email interception captures rich details (tool_id, args, risk)
// ===================================================================

#[tokio::test]
async fn test_email_interception_captures_rich_details() {
    let yaml = r#"
name: email-rich-test
version: "1.0"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs: []
    next: [send]
  - id: send
    type: task
    task:
      kind: call_tool
      tool_id: connector.send_message
      arguments:
        to: "alice@co.com"
        subject: "Weekly Report"
        body: "Hi Alice, here is your weekly report."
        channel_id: "outlook-main"
    next: [end]
  - id: end
    type: control_flow
    control:
      kind: end_workflow
"#;

    let definition: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let executor = Arc::new(RecordingExecutor::new());
    let tools = make_tool_map(vec![
        ToolDefinitionBuilder::new("connector.send_message", "Send Message")
            .side_effects(true)
            .build(),
    ]);

    let engine = build_engine(store.clone(), executor.clone(), tools);
    let instance_id = engine
        .launch_with_id(
            definition,
            json!({}),
            "test".into(),
            None,
            vec![],
            None,
            None,
            ExecutionMode::Shadow,
        )
        .await
        .unwrap();

    wait_for_terminal(&store, instance_id).await;

    let page = store.list_intercepted_actions(instance_id, 100, 0).unwrap();
    assert_eq!(page.total, 1);

    let action = &page.items[0];
    assert_eq!(action.kind, "tool_call");
    assert_eq!(action.step_id, "send");
    assert_eq!(action.details["tool_id"], "connector.send_message");

    // Arguments are preserved for rich viewer
    let args = &action.details["arguments"];
    assert_eq!(args["to"], "alice@co.com");
    assert_eq!(args["subject"], "Weekly Report");
    assert_eq!(args["body"], "Hi Alice, here is your weekly report.");
    assert_eq!(args["channel_id"], "outlook-main");

    // Risk level captured
    assert!(action.details.get("risk_level").is_some());
}

// ===================================================================
// T6.2: HTTP interception captures method, URL, body
// ===================================================================

#[tokio::test]
async fn test_http_interception_captures_method_url_body() {
    let yaml = r#"
name: http-rich-test
version: "1.0"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs: []
    next: [api_call]
  - id: api_call
    type: task
    task:
      kind: call_tool
      tool_id: http.request
      arguments:
        method: "POST"
        url: "https://api.stripe.com/v1/charges"
        body: '{"amount":2000,"currency":"usd"}'
    next: [end]
  - id: end
    type: control_flow
    control:
      kind: end_workflow
"#;

    let definition: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let executor = Arc::new(RecordingExecutor::new());
    let tools = make_tool_map(vec![
        ToolDefinitionBuilder::new("http.request", "HTTP Request")
            .side_effects(true)
            .build(),
    ]);

    let engine = build_engine(store.clone(), executor.clone(), tools);
    let instance_id = engine
        .launch_with_id(
            definition,
            json!({}),
            "test".into(),
            None,
            vec![],
            None,
            None,
            ExecutionMode::Shadow,
        )
        .await
        .unwrap();

    wait_for_terminal(&store, instance_id).await;

    let page = store.list_intercepted_actions(instance_id, 100, 0).unwrap();
    assert_eq!(page.total, 1);

    let action = &page.items[0];
    assert_eq!(action.kind, "tool_call");
    let args = &action.details["arguments"];
    assert_eq!(args["method"], "POST");
    assert_eq!(args["url"], "https://api.stripe.com/v1/charges");
    // Body is parsed from JSON string into a Value object
    assert_eq!(args["body"]["amount"], 2000);
    assert_eq!(args["body"]["currency"], "usd");
}

// ===================================================================
// T6.3: Agent interception captures persona, task
// ===================================================================

#[tokio::test]
async fn test_agent_interception_captures_persona_and_task() {
    let yaml = r#"
name: agent-rich-test
version: "1.0"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs: []
    next: [research]
  - id: research
    type: task
    task:
      kind: invoke_agent
      persona_id: researcher
      task: "Find data about Q1 sales performance and summarize"
      async_exec: false
    next: [end]
  - id: end
    type: control_flow
    control:
      kind: end_workflow
"#;

    let definition: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let executor = Arc::new(RecordingExecutor::new());
    let engine = build_engine(store.clone(), executor.clone(), HashMap::new());

    let instance_id = engine
        .launch_with_id(
            definition,
            json!({}),
            "test".into(),
            None,
            vec![],
            None,
            None,
            ExecutionMode::Shadow,
        )
        .await
        .unwrap();

    wait_for_terminal(&store, instance_id).await;

    let page = store.list_intercepted_actions(instance_id, 100, 0).unwrap();
    assert_eq!(page.total, 1);

    let action = &page.items[0];
    assert_eq!(action.kind, "agent_invocation");
    assert_eq!(action.details["persona_id"], "researcher");
    assert_eq!(
        action.details["task"],
        "Find data about Q1 sales performance and summarize"
    );
    assert_eq!(action.details["async"], false);
}

// ===================================================================
// T6.4: Large batch — intercepted actions paginate correctly
// ===================================================================

#[tokio::test]
async fn test_large_batch_pagination() {
    // Create a ForEach workflow that processes 60 items
    let yaml = r#"
name: batch-pagination-test
version: "1.0"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs: []
    next: [loop]
  - id: loop
    type: control_flow
    control:
      kind: for_each
      collection: "{{trigger.items}}"
      item_var: item
      body: [send]
    next: [end]
  - id: send
    type: task
    task:
      kind: call_tool
      tool_id: connector.send_message
      arguments:
        to: "{{variables.item}}"
        body: "Hello"
    next: []
  - id: end
    type: control_flow
    control:
      kind: end_workflow
"#;

    let definition: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let executor = Arc::new(RecordingExecutor::new());
    let tools = make_tool_map(vec![
        ToolDefinitionBuilder::new("connector.send_message", "Send Message")
            .side_effects(true)
            .build(),
    ]);

    let engine = build_engine(store.clone(), executor.clone(), tools);

    // Create 60 items
    let items: Vec<String> = (0..60).map(|i| format!("user{}@co.com", i)).collect();
    let instance_id = engine
        .launch_with_id(
            definition,
            json!({"items": items}),
            "test".into(),
            None,
            vec![],
            None,
            None,
            ExecutionMode::Shadow,
        )
        .await
        .unwrap();

    wait_for_terminal(&store, instance_id).await;

    // Page 1: first 25
    let page1 = store.list_intercepted_actions(instance_id, 25, 0).unwrap();
    assert_eq!(page1.total, 60);
    assert_eq!(page1.items.len(), 25);

    // Page 2: next 25
    let page2 = store.list_intercepted_actions(instance_id, 25, 25).unwrap();
    assert_eq!(page2.total, 60);
    assert_eq!(page2.items.len(), 25);

    // Page 3: remaining 10
    let page3 = store.list_intercepted_actions(instance_id, 25, 50).unwrap();
    assert_eq!(page3.total, 60);
    assert_eq!(page3.items.len(), 10);

    // Page 4: empty
    let page4 = store.list_intercepted_actions(instance_id, 25, 75).unwrap();
    assert_eq!(page4.total, 60);
    assert_eq!(page4.items.len(), 0);

    // Shadow summary counts
    let summary = store.get_shadow_summary(instance_id).unwrap();
    assert_eq!(summary.total_intercepted, 60);
    assert_eq!(summary.tool_calls_intercepted, 60);
}

// ===================================================================
// T6.5: Workflow launch interception captures name and inputs
// ===================================================================

#[tokio::test]
async fn test_workflow_launch_interception_details() {
    let yaml = r#"
name: launch-child-test
version: "1.0"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs: []
    next: [launch]
  - id: launch
    type: task
    task:
      kind: launch_workflow
      workflow_name: "child-report-generator"
      inputs:
        report_type: "weekly"
        recipients: '["team@co.com"]'
    next: [end]
  - id: end
    type: control_flow
    control:
      kind: end_workflow
"#;

    let definition: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let executor = Arc::new(RecordingExecutor::new());
    let engine = build_engine(store.clone(), executor.clone(), HashMap::new());

    let instance_id = engine
        .launch_with_id(
            definition,
            json!({}),
            "test".into(),
            None,
            vec![],
            None,
            None,
            ExecutionMode::Shadow,
        )
        .await
        .unwrap();

    wait_for_terminal(&store, instance_id).await;

    let page = store.list_intercepted_actions(instance_id, 100, 0).unwrap();
    assert_eq!(page.total, 1);

    let action = &page.items[0];
    assert_eq!(action.kind, "workflow_launch");
    assert_eq!(action.details["workflow_name"], "child-report-generator");
    assert_eq!(action.details["inputs"]["report_type"], "weekly");
}

// ===================================================================
// T6.6: Multiple action kinds in same workflow — all properly categorized
// ===================================================================

#[tokio::test]
async fn test_mixed_action_kinds() {
    let yaml = r#"
name: mixed-actions-test
version: "1.0"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs: []
    next: [fetch]
  - id: fetch
    type: task
    task:
      kind: call_tool
      tool_id: db.query
      arguments:
        query: "SELECT * FROM users"
    next: [invoke]
  - id: invoke
    type: task
    task:
      kind: invoke_agent
      persona_id: analyzer
      task: "Analyze user data"
      async_exec: false
    next: [send]
  - id: send
    type: task
    task:
      kind: call_tool
      tool_id: connector.send_message
      arguments:
        to: "manager@co.com"
        body: "Analysis complete"
    next: [child]
  - id: child
    type: task
    task:
      kind: launch_workflow
      workflow_name: "archive-results"
      inputs: {}
    next: [end]
  - id: end
    type: control_flow
    control:
      kind: end_workflow
"#;

    let definition: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let executor = Arc::new(RecordingExecutor::new());

    // db.query is read-only, connector.send_message has side effects
    let tools = make_tool_map(vec![
        ToolDefinitionBuilder::new("db.query", "Database Query")
            .read_only()
            .build(),
        ToolDefinitionBuilder::new("connector.send_message", "Send Message")
            .side_effects(true)
            .build(),
    ]);

    let engine = build_engine(store.clone(), executor.clone(), tools);
    let instance_id = engine
        .launch_with_id(
            definition,
            json!({}),
            "test".into(),
            None,
            vec![],
            None,
            None,
            ExecutionMode::Shadow,
        )
        .await
        .unwrap();

    wait_for_terminal(&store, instance_id).await;

    let page = store.list_intercepted_actions(instance_id, 100, 0).unwrap();

    // db.query is read-only → passes through → NOT intercepted
    // invoke_agent → intercepted
    // connector.send_message → intercepted
    // launch_workflow → intercepted
    assert_eq!(page.total, 3);

    let kinds: Vec<&str> = page.items.iter().map(|a| a.kind.as_str()).collect();
    assert!(kinds.contains(&"agent_invocation"));
    assert!(kinds.contains(&"tool_call"));
    assert!(kinds.contains(&"workflow_launch"));

    // db.query should have been passed through to real executor
    let real_calls = executor.tool_calls.lock().await;
    assert_eq!(real_calls.len(), 1);
    assert_eq!(real_calls[0].0, "db.query");

    // Summary counts
    let summary = store.get_shadow_summary(instance_id).unwrap();
    assert_eq!(summary.total_intercepted, 3);
    assert_eq!(summary.tool_calls_intercepted, 1); // only connector.send_message
    assert_eq!(summary.agent_invocations_intercepted, 1);
    assert_eq!(summary.workflow_launches_intercepted, 1);
}

// ===================================================================
// T6.7: Timestamp ordering — actions are ordered by timestamp
// ===================================================================

#[tokio::test]
async fn test_intercepted_actions_ordered_by_timestamp() {
    let yaml = r#"
name: timestamp-order-test
version: "1.0"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs: []
    next: [step1]
  - id: step1
    type: task
    task:
      kind: call_tool
      tool_id: email.send
      arguments:
        to: "first@co.com"
    next: [step2]
  - id: step2
    type: task
    task:
      kind: call_tool
      tool_id: email.send
      arguments:
        to: "second@co.com"
    next: [step3]
  - id: step3
    type: task
    task:
      kind: call_tool
      tool_id: email.send
      arguments:
        to: "third@co.com"
    next: [end]
  - id: end
    type: control_flow
    control:
      kind: end_workflow
"#;

    let definition: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let executor = Arc::new(RecordingExecutor::new());
    let tools = make_tool_map(vec![
        ToolDefinitionBuilder::new("email.send", "Send Email")
            .side_effects(true)
            .build(),
    ]);

    let engine = build_engine(store.clone(), executor.clone(), tools);
    let instance_id = engine
        .launch_with_id(
            definition,
            json!({}),
            "test".into(),
            None,
            vec![],
            None,
            None,
            ExecutionMode::Shadow,
        )
        .await
        .unwrap();

    wait_for_terminal(&store, instance_id).await;

    let page = store.list_intercepted_actions(instance_id, 100, 0).unwrap();
    assert_eq!(page.total, 3);

    // Verify timestamps are non-decreasing (ordered)
    for i in 1..page.items.len() {
        assert!(
            page.items[i].timestamp_ms >= page.items[i - 1].timestamp_ms,
            "Actions should be ordered by timestamp"
        );
    }

    // Verify step IDs match expected order
    assert_eq!(page.items[0].step_id, "step1");
    assert_eq!(page.items[1].step_id, "step2");
    assert_eq!(page.items[2].step_id, "step3");
}

// ===================================================================
// T6.8: Normal mode instance has no intercepted actions
// ===================================================================

#[tokio::test]
async fn test_normal_mode_no_intercepted_actions() {
    let yaml = r#"
name: normal-no-intercepts
version: "1.0"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs: []
    next: [send]
  - id: send
    type: task
    task:
      kind: call_tool
      tool_id: email.send
      arguments:
        to: "user@co.com"
    next: [end]
  - id: end
    type: control_flow
    control:
      kind: end_workflow
"#;

    let definition: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let executor = Arc::new(RecordingExecutor::new());
    let tools = make_tool_map(vec![
        ToolDefinitionBuilder::new("email.send", "Send Email")
            .side_effects(true)
            .build(),
    ]);

    let engine = build_engine(store.clone(), executor.clone(), tools);
    let instance_id = engine
        .launch_with_id(
            definition,
            json!({}),
            "test".into(),
            None,
            vec![],
            None,
            None,
            ExecutionMode::Normal,
        )
        .await
        .unwrap();

    wait_for_terminal(&store, instance_id).await;

    // Normal mode — tool was executed for real
    assert_eq!(executor.tool_calls.lock().await.len(), 1);

    // No intercepted actions
    let page = store.list_intercepted_actions(instance_id, 100, 0).unwrap();
    assert_eq!(page.total, 0);
    assert_eq!(page.items.len(), 0);

    let summary = store.get_shadow_summary(instance_id).unwrap();
    assert_eq!(summary.total_intercepted, 0);
}

// ===================================================================
// T6.9: Concurrent shadow and normal instances don't cross-contaminate
// ===================================================================

#[tokio::test]
async fn test_concurrent_shadow_normal_no_contamination() {
    let yaml = r#"
name: concurrent-test
version: "1.0"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs: []
    next: [send]
  - id: send
    type: task
    task:
      kind: call_tool
      tool_id: email.send
      arguments:
        to: "user@co.com"
    next: [end]
  - id: end
    type: control_flow
    control:
      kind: end_workflow
"#;

    let definition: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let executor = Arc::new(RecordingExecutor::new());
    let tools = make_tool_map(vec![
        ToolDefinitionBuilder::new("email.send", "Send Email")
            .side_effects(true)
            .build(),
    ]);

    let engine = build_engine(store.clone(), executor.clone(), tools);

    // Launch shadow instance
    let shadow_id = engine
        .launch_with_id(
            definition.clone(),
            json!({}),
            "test".into(),
            None,
            vec![],
            None,
            None,
            ExecutionMode::Shadow,
        )
        .await
        .unwrap();

    // Launch normal instance
    let normal_id = engine
        .launch_with_id(
            definition,
            json!({}),
            "test".into(),
            None,
            vec![],
            None,
            None,
            ExecutionMode::Normal,
        )
        .await
        .unwrap();

    wait_for_terminal(&store, shadow_id).await;
    wait_for_terminal(&store, normal_id).await;

    // Shadow has intercepted actions
    let shadow_page = store
        .list_intercepted_actions(shadow_id, 100, 0)
        .unwrap();
    assert_eq!(shadow_page.total, 1);
    assert_eq!(shadow_page.items[0].instance_id, shadow_id);

    // Normal has no intercepted actions
    let normal_page = store
        .list_intercepted_actions(normal_id, 100, 0)
        .unwrap();
    assert_eq!(normal_page.total, 0);

    // Normal executed the real tool
    let real_calls = executor.tool_calls.lock().await;
    assert!(real_calls.len() >= 1);
}
