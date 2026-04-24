//! Integration tests for shadow (test-run) mode.
//!
//! These verify that the full engine pipeline correctly intercepts
//! side-effecting operations when `execution_mode == Shadow`.

use async_trait::async_trait;
use hive_contracts::tools::ToolDefinitionBuilder;
use hive_workflow::*;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

// ---------------------------------------------------------------------------
// Test helpers
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

// ---------------------------------------------------------------------------
// Mock ToolInfoProvider
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Recording executor — records real calls to verify pass-through vs intercept
// ---------------------------------------------------------------------------

struct RecordingExecutor {
    tool_calls: Mutex<Vec<(String, Value)>>,
    agent_calls: Mutex<Vec<(String, String)>>,
    workflow_launches: Mutex<Vec<(String, Value)>>,
}

impl RecordingExecutor {
    fn new() -> Self {
        Self {
            tool_calls: Mutex::new(Vec::new()),
            agent_calls: Mutex::new(Vec::new()),
            workflow_launches: Mutex::new(Vec::new()),
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
        Ok(json!({"echo": "real", "status": "ok", "count": 42}))
    }

    async fn invoke_agent(
        &self,
        persona_id: &str,
        task: &str,
        _async_exec: bool,
        _timeout_secs: Option<u64>,
        _permissions: &[PermissionEntry],
        _agent_name: Option<&str>,
        _existing_agent_id: Option<&str>,
        _ctx: &ExecutionContext,
    ) -> Result<Value, String> {
        self.agent_calls
            .lock()
            .await
            .push((persona_id.to_string(), task.to_string()));
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
        name: &str,
        inputs: Value,
        _ctx: &ExecutionContext,
    ) -> Result<i64, String> {
        self.workflow_launches
            .lock()
            .await
            .push((name.to_string(), inputs));
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

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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
// T1.1: Shadow mode intercepts dangerous tool calls
// ===================================================================

#[tokio::test]
async fn test_shadow_intercepts_dangerous_tool_call() {
    let yaml = r#"
name: shadow-tool-test
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
      tool_id: comm.send_email
      arguments:
        to: "user@example.com"
        body: "Hello"
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
        ToolDefinitionBuilder::new("comm.send_email", "Send Email")
            .side_effects(true)
            .build(),
    ]);

    let engine = build_engine(store.clone(), executor.clone(), tools);

    let instance_id = engine
        .launch_with_id(
            definition,
            json!({}),
            "test-session".into(),
            None,
            vec![],
            None,
            None,
            ExecutionMode::Shadow,
        )
        .await
        .unwrap();

    wait_for_terminal(&store, instance_id).await;

    let inst = store.get_instance(instance_id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert_eq!(inst.execution_mode, ExecutionMode::Shadow);

    // Real executor should NOT have been called
    assert_eq!(executor.tool_calls.lock().await.len(), 0);

    // Intercepted actions should be recorded
    let page = store
        .list_intercepted_actions(instance_id, 100, 0)
        .unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].kind, "tool_call");
    assert_eq!(page.items[0].details["tool_id"], "comm.send_email");

    let summary = store.get_shadow_summary(instance_id).unwrap();
    assert_eq!(summary.total_intercepted, 1);
    assert_eq!(summary.tool_calls_intercepted, 1);
}

// ===================================================================
// T1.2: Shadow mode intercepts agent invocations
// ===================================================================

#[tokio::test]
async fn test_shadow_intercepts_agent_invocation() {
    let yaml = r#"
name: shadow-agent-test
version: "1.0"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs: []
    next: [invoke]
  - id: invoke
    type: task
    task:
      kind: invoke_agent
      persona_id: writer-bot
      task: "Write a blog post"
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
            "test-session".into(),
            None,
            vec![],
            None,
            None,
            ExecutionMode::Shadow,
        )
        .await
        .unwrap();

    wait_for_terminal(&store, instance_id).await;

    let inst = store.get_instance(instance_id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);

    // Agent invocations are now delegated to inner (agents run in sandbox),
    // but an intercepted action is still recorded for visibility.
    assert_eq!(executor.agent_calls.lock().await.len(), 1);

    let page = store
        .list_intercepted_actions(instance_id, 100, 0)
        .unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].kind, "agent_invocation");

    let summary = store.get_shadow_summary(instance_id).unwrap();
    assert_eq!(summary.agent_invocations_intercepted, 1);
}

// ===================================================================
// T1.4: Shadow mode fails event gates (error handled by skip)
// ===================================================================

#[tokio::test]
async fn test_shadow_event_gate_fails() {
    let yaml = r#"
name: shadow-event-gate-test
version: "1.0"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs: []
    next: [wait_event]
  - id: wait_event
    type: task
    task:
      kind: event_gate
      topic: "order.shipped"
      timeout_secs: 30
    on_error:
      strategy: skip
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
            "test-session".into(),
            None,
            vec![],
            None,
            None,
            ExecutionMode::Shadow,
        )
        .await
        .unwrap();

    wait_for_terminal(&store, instance_id).await;

    let inst = store.get_instance(instance_id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);

    let page = store
        .list_intercepted_actions(instance_id, 100, 0)
        .unwrap();
    assert!(page.total >= 1);
    let gate_action = page.items.iter().find(|a| a.kind == "event_gate_blocked");
    assert!(
        gate_action.is_some(),
        "expected event_gate intercepted action"
    );
}

// ===================================================================
// T1.5: Shadow mode intercepts workflow launches
// ===================================================================

#[tokio::test]
async fn test_shadow_intercepts_workflow_launch() {
    let yaml = r#"
name: shadow-launch-test
version: "1.0"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs: []
    next: [launch_child]
  - id: launch_child
    type: task
    task:
      kind: launch_workflow
      workflow_name: child-workflow
      inputs:
        data: "test"
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
            "test-session".into(),
            None,
            vec![],
            None,
            None,
            ExecutionMode::Shadow,
        )
        .await
        .unwrap();

    wait_for_terminal(&store, instance_id).await;

    let inst = store.get_instance(instance_id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);

    assert_eq!(executor.workflow_launches.lock().await.len(), 0);

    let page = store
        .list_intercepted_actions(instance_id, 100, 0)
        .unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].kind, "workflow_launch");
    assert_eq!(page.items[0].details["workflow_name"], "child-workflow");

    let summary = store.get_shadow_summary(instance_id).unwrap();
    assert_eq!(summary.workflow_launches_intercepted, 1);
}

// ===================================================================
// T1.6: Shadow mode intercepts schedule_task
// ===================================================================

#[tokio::test]
async fn test_shadow_intercepts_schedule_task() {
    let yaml = r#"
name: shadow-schedule-test
version: "1.0"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs: []
    next: [schedule]
  - id: schedule
    type: task
    task:
      kind: schedule_task
      schedule:
        name: nightly-report
        schedule: "0 0 * * *"
        action:
          kind: call_tool
          tool_id: report.generate
          arguments: {}
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
            "test-session".into(),
            None,
            vec![],
            None,
            None,
            ExecutionMode::Shadow,
        )
        .await
        .unwrap();

    wait_for_terminal(&store, instance_id).await;

    let inst = store.get_instance(instance_id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);

    let page = store
        .list_intercepted_actions(instance_id, 100, 0)
        .unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].kind, "scheduled_task");

    let summary = store.get_shadow_summary(instance_id).unwrap();
    assert_eq!(summary.scheduled_tasks_intercepted, 1);
}

// ===================================================================
// T1.7: Read-only tools pass through in shadow mode
// ===================================================================

#[tokio::test]
async fn test_shadow_passthrough_readonly_tools() {
    let yaml = r#"
name: shadow-readonly-test
version: "1.0"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs: []
    next: [list_contacts]
  - id: list_contacts
    type: task
    task:
      kind: call_tool
      tool_id: contacts.list
      arguments:
        query: "test"
    next: [list_files]
  - id: list_files
    type: task
    task:
      kind: call_tool
      tool_id: drive.list_files
      arguments:
        folder: "root"
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
        ToolDefinitionBuilder::new("contacts.list", "List Contacts")
            .read_only()
            .build(),
        ToolDefinitionBuilder::new("drive.list_files", "List Files")
            .read_only()
            .build(),
    ]);

    let engine = build_engine(store.clone(), executor.clone(), tools);

    let instance_id = engine
        .launch_with_id(
            definition,
            json!({}),
            "test-session".into(),
            None,
            vec![],
            None,
            None,
            ExecutionMode::Shadow,
        )
        .await
        .unwrap();

    wait_for_terminal(&store, instance_id).await;

    let inst = store.get_instance(instance_id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);

    // Both tools SHOULD have been called (pass-through for read-only)
    let calls = executor.tool_calls.lock().await;
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].0, "contacts.list");
    assert_eq!(calls[1].0, "drive.list_files");

    let summary = store.get_shadow_summary(instance_id).unwrap();
    assert_eq!(summary.total_intercepted, 0);
}

// ===================================================================
// T1.8: Normal mode is completely unaffected
// ===================================================================

#[tokio::test]
async fn test_normal_mode_unaffected() {
    let yaml = r#"
name: normal-mode-test
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
      tool_id: comm.send_email
      arguments:
        to: "user@example.com"
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
        ToolDefinitionBuilder::new("comm.send_email", "Send Email")
            .side_effects(true)
            .build(),
    ]);

    let engine = build_engine(store.clone(), executor.clone(), tools);

    let instance_id = engine
        .launch_with_id(
            definition,
            json!({}),
            "test-session".into(),
            None,
            vec![],
            None,
            None,
            ExecutionMode::Normal,
        )
        .await
        .unwrap();

    wait_for_terminal(&store, instance_id).await;

    let inst = store.get_instance(instance_id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert_eq!(inst.execution_mode, ExecutionMode::Normal);

    // Real executor SHOULD have been called
    let calls = executor.tool_calls.lock().await;
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "comm.send_email");

    // No intercepted actions
    let page = store
        .list_intercepted_actions(instance_id, 100, 0)
        .unwrap();
    assert_eq!(page.total, 0);

    let summary = store.get_shadow_summary(instance_id).unwrap();
    assert_eq!(summary.total_intercepted, 0);
}

// ===================================================================
// T1.9: Shadow summary counters accurate for ForEach loops
// ===================================================================

#[tokio::test]
async fn test_shadow_foreach_summary_counters() {
    let yaml = r#"
name: shadow-foreach-test
version: "1.0"
variables:
  type: object
  properties:
    items:
      type: array

steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs:
        - name: items
          input_type: array
    next: [loop]

  - id: loop
    type: control_flow
    control:
      kind: for_each
      collection: "{{trigger.items}}"
      item_var: item
      body: [send_email]
    next: [end]

  - id: send_email
    type: task
    task:
      kind: call_tool
      tool_id: comm.send_email
      arguments:
        to: "{{item}}"
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
        ToolDefinitionBuilder::new("comm.send_email", "Send Email")
            .side_effects(true)
            .build(),
    ]);

    let engine = build_engine(store.clone(), executor.clone(), tools);

    let instance_id = engine
        .launch_with_id(
            definition,
            json!({"items": ["alice@ex.com", "bob@ex.com", "carol@ex.com"]}),
            "test-session".into(),
            None,
            vec![],
            None,
            None,
            ExecutionMode::Shadow,
        )
        .await
        .unwrap();

    wait_for_terminal(&store, instance_id).await;

    let inst = store.get_instance(instance_id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);

    assert_eq!(executor.tool_calls.lock().await.len(), 0);

    let page = store
        .list_intercepted_actions(instance_id, 100, 0)
        .unwrap();
    assert_eq!(page.total, 3);
    for action in &page.items {
        assert_eq!(action.kind, "tool_call");
        assert_eq!(action.details["tool_id"], "comm.send_email");
    }

    let summary = store.get_shadow_summary(instance_id).unwrap();
    assert_eq!(summary.total_intercepted, 3);
    assert_eq!(summary.tool_calls_intercepted, 3);
}

// ===================================================================
// T1.10: Synthetic outputs don't break downstream branches
// ===================================================================

#[tokio::test]
async fn test_shadow_synthetic_outputs_dont_break_branches() {
    let yaml = r#"
name: shadow-branch-test
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
      tool_id: http.request
      arguments:
        url: "https://api.example.com/data"
    outputs:
      response: "{{result}}"
    next: [decide]
  - id: decide
    type: control_flow
    control:
      kind: branch
      condition: "{{steps.fetch.outputs.response}} != null"
      then: [success]
      else: [fallback]
  - id: success
    type: task
    task:
      kind: delay
      duration_secs: 0
    outputs:
      path: "success"
    next: [end]
  - id: fallback
    type: task
    task:
      kind: delay
      duration_secs: 0
    outputs:
      path: "fallback"
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
            .open_world()
            .build(),
    ]);

    let engine = build_engine(store.clone(), executor.clone(), tools);

    let instance_id = engine
        .launch_with_id(
            definition,
            json!({}),
            "test-session".into(),
            None,
            vec![],
            None,
            None,
            ExecutionMode::Shadow,
        )
        .await
        .unwrap();

    wait_for_terminal(&store, instance_id).await;

    // Must COMPLETE, not fail with expression errors
    let inst = store.get_instance(instance_id).unwrap().unwrap();
    assert_eq!(
        inst.status,
        WorkflowStatus::Completed,
        "shadow mode should complete without expression errors, got: {:?}",
        inst.error
    );

    let page = store
        .list_intercepted_actions(instance_id, 100, 0)
        .unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].kind, "tool_call");
}

// ===================================================================
// T1.11: Mixed safe + dangerous tools in same workflow
// ===================================================================

#[tokio::test]
async fn test_shadow_mixed_safe_and_dangerous() {
    let yaml = r#"
name: shadow-mixed-test
version: "1.0"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs: []
    next: [read_data]
  - id: read_data
    type: task
    task:
      kind: call_tool
      tool_id: db.query
      arguments:
        sql: "SELECT * FROM users"
    next: [send_notifications]
  - id: send_notifications
    type: task
    task:
      kind: call_tool
      tool_id: comm.send_email
      arguments:
        to: "everyone"
        body: "Update"
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
        ToolDefinitionBuilder::new("db.query", "Database Query")
            .read_only()
            .build(),
        ToolDefinitionBuilder::new("comm.send_email", "Send Email")
            .side_effects(true)
            .build(),
    ]);

    let engine = build_engine(store.clone(), executor.clone(), tools);

    let instance_id = engine
        .launch_with_id(
            definition,
            json!({}),
            "test-session".into(),
            None,
            vec![],
            None,
            None,
            ExecutionMode::Shadow,
        )
        .await
        .unwrap();

    wait_for_terminal(&store, instance_id).await;

    let inst = store.get_instance(instance_id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);

    // db.query (read-only) should pass through
    let calls = executor.tool_calls.lock().await;
    assert_eq!(calls.len(), 1, "only read-only tool should pass through");
    assert_eq!(calls[0].0, "db.query");

    // comm.send_email (side-effecting) should be intercepted
    let page = store
        .list_intercepted_actions(instance_id, 100, 0)
        .unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].kind, "tool_call");
    assert_eq!(page.items[0].details["tool_id"], "comm.send_email");
}

// ===================================================================
// T1.12: No tool info provider = all tools intercepted (fail-closed)
// ===================================================================

#[tokio::test]
async fn test_shadow_no_tool_info_intercepts_all() {
    let yaml = r#"
name: shadow-no-info-test
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
      tool_id: some.unknown.tool
      arguments:
        data: "test"
    next: [step2]
  - id: step2
    type: task
    task:
      kind: call_tool
      tool_id: another.tool
      arguments:
        data: "test"
    next: [end]
  - id: end
    type: control_flow
    control:
      kind: end_workflow
"#;

    let definition: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let executor = Arc::new(RecordingExecutor::new());
    let emitter = Arc::new(NullEventEmitter);
    // No tool info provider — intercept everything
    let engine = WorkflowEngine::new(store.clone(), executor.clone(), emitter);

    let instance_id = engine
        .launch_with_id(
            definition,
            json!({}),
            "test-session".into(),
            None,
            vec![],
            None,
            None,
            ExecutionMode::Shadow,
        )
        .await
        .unwrap();

    wait_for_terminal(&store, instance_id).await;

    let inst = store.get_instance(instance_id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);

    assert_eq!(executor.tool_calls.lock().await.len(), 0);

    let summary = store.get_shadow_summary(instance_id).unwrap();
    assert_eq!(summary.total_intercepted, 2);
    assert_eq!(summary.tool_calls_intercepted, 2);
}

// ===================================================================
// T1.13: execution_mode persisted correctly
// ===================================================================

#[tokio::test]
async fn test_execution_mode_persisted() {
    let yaml = r#"
name: persist-mode-test
version: "1.0"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs: []
    next: [end]
  - id: end
    type: control_flow
    control:
      kind: end_workflow
"#;

    let definition: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let executor = Arc::new(RecordingExecutor::new());
    let engine = build_engine(store.clone(), executor, HashMap::new());

    let shadow_id = engine
        .launch_with_id(
            definition.clone(),
            json!({}),
            "test-session".into(),
            None,
            vec![],
            None,
            None,
            ExecutionMode::Shadow,
        )
        .await
        .unwrap();

    let normal_id = engine
        .launch_with_id(
            definition,
            json!({}),
            "test-session".into(),
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

    let shadow_inst = store.get_instance(shadow_id).unwrap().unwrap();
    assert_eq!(shadow_inst.execution_mode, ExecutionMode::Shadow);

    let normal_inst = store.get_instance(normal_id).unwrap().unwrap();
    assert_eq!(normal_inst.execution_mode, ExecutionMode::Normal);

    // List instances should include execution_mode
    let result = store
        .list_instances(&InstanceFilter::default())
        .unwrap();
    for inst in &result.items {
        if inst.id == shadow_id {
            assert_eq!(inst.execution_mode, ExecutionMode::Shadow);
        } else if inst.id == normal_id {
            assert_eq!(inst.execution_mode, ExecutionMode::Normal);
        }
    }
}

// ===================================================================
// T1.14: Intercepted actions pagination
// ===================================================================

#[tokio::test]
async fn test_intercepted_actions_pagination() {
    let yaml = r#"
name: shadow-pagination-test
version: "1.0"
variables:
  type: object
  properties:
    items:
      type: array

steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs:
        - name: items
          input_type: array
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
      tool_id: comm.send_email
      arguments:
        to: "{{item}}"
    next: []
  - id: end
    type: control_flow
    control:
      kind: end_workflow
"#;

    let items: Vec<String> = (0..10).map(|i| format!("user{}@example.com", i)).collect();
    let definition: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let executor = Arc::new(RecordingExecutor::new());
    let tools = make_tool_map(vec![
        ToolDefinitionBuilder::new("comm.send_email", "Send Email")
            .side_effects(true)
            .build(),
    ]);

    let engine = build_engine(store.clone(), executor, tools);

    let instance_id = engine
        .launch_with_id(
            definition,
            json!({"items": items}),
            "test-session".into(),
            None,
            vec![],
            None,
            None,
            ExecutionMode::Shadow,
        )
        .await
        .unwrap();

    wait_for_terminal(&store, instance_id).await;

    let page1 = store
        .list_intercepted_actions(instance_id, 3, 0)
        .unwrap();
    assert_eq!(page1.items.len(), 3);
    assert_eq!(page1.total, 10);

    let page2 = store
        .list_intercepted_actions(instance_id, 3, 3)
        .unwrap();
    assert_eq!(page2.items.len(), 3);
    assert_eq!(page2.total, 10);

    let page4 = store
        .list_intercepted_actions(instance_id, 3, 9)
        .unwrap();
    assert_eq!(page4.items.len(), 1);
    assert_eq!(page4.total, 10);
}
