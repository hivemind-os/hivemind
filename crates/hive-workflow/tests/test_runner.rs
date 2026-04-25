//! Integration tests for workflow unit-test runner (Phase 5).
//!
//! Validates that `run_test_case` launches in shadow mode with overrides,
//! waits for completion, and compares expectations correctly.

use async_trait::async_trait;
use hive_contracts::tools::ToolDefinitionBuilder;
use hive_workflow::*;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

// ---------------------------------------------------------------------------
// Shared test helpers (same patterns as shadow_integration.rs)
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
        Ok(json!({"echo": "real", "status": "ok", "count": 42}))
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
// T5.1: Test case with matching expectations passes
// ===================================================================

#[tokio::test]
async fn test_passing_test_case() {
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let executor = Arc::new(RecordingExecutor::new());
    let tools = make_tool_map(vec![
        ToolDefinitionBuilder::new("data.fetch", "Fetch data")
            .read_only()
            .build(),
    ]);
    let engine = build_engine(store, executor, tools);

    let yaml = r#"
name: test/simple
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
      tool_id: data.fetch
      arguments:
        query: "{{trigger.msg}}"
    next: []
output:
  result: "{{steps.fetch.outputs.echo}}"
tests:
  - name: happy-path
    inputs:
      msg: hello
    expectations:
      status: completed
      output:
        result: real
      steps_completed:
        - start
        - fetch
"#;
    let def: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();

    let results = run_all_tests(&engine, &def, false, None).await.unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].passed, "Test should pass: {:?}", results[0].failures);
    assert!(results[0].failures.is_empty());
    assert!(results[0].instance_id > 0);
    assert!(results[0].duration_ms < 10_000);
}

// ===================================================================
// T5.2: Test case with wrong status fails
// ===================================================================

#[tokio::test]
async fn test_wrong_status_fails() {
    let store = Arc::new(WorkflowStore::in_memory().unwrap());

    // Executor that always errors on tool calls
    struct FailingExecutor;
    #[async_trait]
    impl StepExecutor for FailingExecutor {
        async fn call_tool(&self, _: &str, _: Value, _: &ExecutionContext) -> Result<Value, String> {
            Err("boom".into())
        }
        async fn invoke_agent(&self, _: &str, _: &str, _: bool, _: Option<u64>, _: &[PermissionEntry], _: Option<&str>, _: Option<&str>, _: &ExecutionContext) -> Result<Value, String> { Ok(json!({})) }
        async fn signal_agent(&self, _: &SignalTarget, _: &str, _: &ExecutionContext) -> Result<Value, String> { Ok(json!({})) }
        async fn wait_for_agent(&self, _: &str, _: Option<u64>, _: &ExecutionContext) -> Result<Value, String> { Ok(json!({})) }
        async fn create_feedback_request(&self, _: i64, _: &str, _: &str, _: Option<&[String]>, _: bool, _: &ExecutionContext) -> Result<String, String> { Ok("r".into()) }
        async fn register_event_gate(&self, _: i64, _: &str, _: &str, _: Option<&str>, _: Option<u64>, _: &ExecutionContext) -> Result<String, String> { Ok("s".into()) }
        async fn launch_workflow(&self, _: &str, _: Value, _: &ExecutionContext) -> Result<i64, String> { Ok(1) }
        async fn schedule_task(&self, _: &ScheduleTaskDef, _: &ExecutionContext) -> Result<String, String> { Ok("t".into()) }
        async fn render_prompt_template(&self, _: &str, _: &str, _: Value, _: &ExecutionContext) -> Result<String, String> { Ok("p".into()) }
    }

    let tools = make_tool_map(vec![
        ToolDefinitionBuilder::new("broken.tool", "Broken")
            .side_effects(true)
            .build(),
    ]);
    let engine = build_engine(store, Arc::new(FailingExecutor), tools);

    let yaml = r#"
name: test/failing
version: "1.0"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs: []
    next: [do_thing]
  - id: do_thing
    type: task
    task:
      kind: call_tool
      tool_id: broken.tool
      arguments: {}
    next: []
tests:
  - name: should-complete
    inputs: {}
    expectations:
      status: completed
"#;
    let def: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();

    let results = run_all_tests(&engine, &def, false, None).await.unwrap();
    assert_eq!(results.len(), 1);
    // Shadow mode intercepts side-effecting tool with synthetic success output,
    // so workflow completes. The test expected "completed" and that's what happens.
    // This validates that shadow interception makes side-effecting tools succeed.
    assert!(results[0].passed, "Shadow mode intercepts side-effecting tool → workflow completes");
}

// ===================================================================
// T5.2 (revised): Test expects "completed" but workflow fails
// ===================================================================

#[tokio::test]
async fn test_wrong_status_reports_failure() {
    let store = Arc::new(WorkflowStore::in_memory().unwrap());

    // Executor that errors on read-only tool calls (pass-through in shadow)
    struct FailingReadOnlyExecutor;
    #[async_trait]
    impl StepExecutor for FailingReadOnlyExecutor {
        async fn call_tool(&self, _: &str, _: Value, _: &ExecutionContext) -> Result<Value, String> {
            Err("database connection failed".into())
        }
        async fn invoke_agent(&self, _: &str, _: &str, _: bool, _: Option<u64>, _: &[PermissionEntry], _: Option<&str>, _: Option<&str>, _: &ExecutionContext) -> Result<Value, String> { Ok(json!({})) }
        async fn signal_agent(&self, _: &SignalTarget, _: &str, _: &ExecutionContext) -> Result<Value, String> { Ok(json!({})) }
        async fn wait_for_agent(&self, _: &str, _: Option<u64>, _: &ExecutionContext) -> Result<Value, String> { Ok(json!({})) }
        async fn create_feedback_request(&self, _: i64, _: &str, _: &str, _: Option<&[String]>, _: bool, _: &ExecutionContext) -> Result<String, String> { Ok("r".into()) }
        async fn register_event_gate(&self, _: i64, _: &str, _: &str, _: Option<&str>, _: Option<u64>, _: &ExecutionContext) -> Result<String, String> { Ok("s".into()) }
        async fn launch_workflow(&self, _: &str, _: Value, _: &ExecutionContext) -> Result<i64, String> { Ok(1) }
        async fn schedule_task(&self, _: &ScheduleTaskDef, _: &ExecutionContext) -> Result<String, String> { Ok("t".into()) }
        async fn render_prompt_template(&self, _: &str, _: &str, _: Value, _: &ExecutionContext) -> Result<String, String> { Ok("p".into()) }
    }

    let tools = make_tool_map(vec![
        ToolDefinitionBuilder::new("db.query", "Query DB")
            .read_only()
            .build(),
    ]);
    let engine = build_engine(store, Arc::new(FailingReadOnlyExecutor), tools);

    let yaml = r#"
name: test/fail-readonly
version: "1.0"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs: []
    next: [query]
  - id: query
    type: task
    task:
      kind: call_tool
      tool_id: db.query
      arguments: {}
    next: []
tests:
  - name: expects-completion
    inputs: {}
    expectations:
      status: completed
"#;
    let def: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();

    let results = run_all_tests(&engine, &def, false, None).await.unwrap();
    assert_eq!(results.len(), 1);
    assert!(!results[0].passed);
    assert_eq!(results[0].failures[0].expectation, "status");
    assert_eq!(results[0].failures[0].expected, "completed");
    assert_eq!(results[0].failures[0].actual, "failed");
}

// ===================================================================
// T5.3: Test case with wrong output fails with diff
// ===================================================================

#[tokio::test]
async fn test_wrong_output_reports_diff() {
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let executor = Arc::new(RecordingExecutor::new());
    let tools = make_tool_map(vec![
        ToolDefinitionBuilder::new("data.fetch", "Fetch")
            .read_only()
            .build(),
    ]);
    let engine = build_engine(store, executor, tools);

    let yaml = r#"
name: test/output-mismatch
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
      tool_id: data.fetch
      arguments: {}
    next: []
output:
  echo_val: "{{steps.fetch.outputs.echo}}"
tests:
  - name: wrong-output
    inputs: {}
    expectations:
      status: completed
      output:
        echo_val: "expected_but_wrong"
"#;
    let def: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();

    let results = run_all_tests(&engine, &def, false, None).await.unwrap();
    assert_eq!(results.len(), 1);
    assert!(!results[0].passed);
    let out_fail = results[0].failures.iter().find(|f| f.expectation == "output").unwrap();
    assert!(out_fail.expected.contains("expected_but_wrong"));
    assert!(out_fail.actual.contains("real")); // RecordingExecutor returns {"echo": "real", ...}
}

// ===================================================================
// T5.4: shadow_outputs overrides skip execution for that step
// ===================================================================

#[tokio::test]
async fn test_shadow_outputs_override_step() {
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let executor = Arc::new(RecordingExecutor::new());
    let tools = make_tool_map(vec![
        ToolDefinitionBuilder::new("llm.invoke", "LLM")
            .side_effects(true)
            .build(),
    ]);
    let engine = build_engine(store, executor.clone(), tools);

    let yaml = r#"
name: test/override
version: "1.0"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs: []
    next: [agent_step]
  - id: agent_step
    type: task
    task:
      kind: call_tool
      tool_id: llm.invoke
      arguments:
        prompt: "summarize data"
    next: [use_result]
  - id: use_result
    type: task
    task:
      kind: call_tool
      tool_id: llm.invoke
      arguments:
        data: "{{steps.agent_step.outputs.summary}}"
    next: []
output:
  final: "{{steps.agent_step.outputs.summary}}"
tests:
  - name: with-override
    inputs: {}
    shadow_outputs:
      agent_step:
        summary: "mocked summary text"
    expectations:
      status: completed
      output:
        final: "mocked summary text"
      steps_completed:
        - agent_step
        - use_result
"#;
    let def: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();

    let results = run_all_tests(&engine, &def, false, None).await.unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].passed, "Failures: {:?}", results[0].failures);

    // Verify the overridden step was NOT executed through the real executor
    let calls = executor.tool_calls.lock().await;
    // agent_step should NOT have been called (overridden), but use_result
    // IS a side-effecting tool and should be intercepted by shadow mode.
    // Neither should hit the real executor.
    assert!(
        !calls.iter().any(|(id, _)| id == "llm.invoke"),
        "Shadow mode + override means no real executor calls for llm.invoke"
    );
}

// ===================================================================
// T5.5: steps_completed and steps_not_reached assertions
// ===================================================================

#[tokio::test]
async fn test_steps_completed_and_not_reached() {
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let executor = Arc::new(RecordingExecutor::new());
    let tools = make_tool_map(vec![
        ToolDefinitionBuilder::new("data.fetch", "Fetch")
            .read_only()
            .build(),
    ]);
    let engine = build_engine(store, executor, tools);

    let yaml = r#"
name: test/branching
version: "1.0"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs: []
    next: [check]
  - id: check
    type: control_flow
    control:
      kind: branch
      condition: "{{trigger.path}} == 'a'"
      then: [path_a]
      else: [path_b]
  - id: path_a
    type: task
    task:
      kind: call_tool
      tool_id: data.fetch
      arguments: {}
    next: []
  - id: path_b
    type: task
    task:
      kind: call_tool
      tool_id: data.fetch
      arguments: {}
    next: []
tests:
  - name: takes-path-a
    inputs:
      path: a
    expectations:
      status: completed
      steps_completed:
        - start
        - check
        - path_a
      steps_not_reached:
        - path_b
"#;
    let def: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();

    let results = run_all_tests(&engine, &def, false, None).await.unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].passed, "Failures: {:?}", results[0].failures);
}

// ===================================================================
// T5.6: intercepted_action_counts assertion
// ===================================================================

#[tokio::test]
async fn test_intercepted_action_counts() {
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let executor = Arc::new(RecordingExecutor::new());
    let tools = make_tool_map(vec![
        ToolDefinitionBuilder::new("comm.send_email", "Send email")
            .side_effects(true)
            .build(),
    ]);
    let engine = build_engine(store, executor, tools);

    let yaml = r#"
name: test/foreach-emails
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
    next: []
  - id: send
    type: task
    task:
      kind: call_tool
      tool_id: comm.send_email
      arguments:
        to: "{{variables.item}}"
    next: []
tests:
  - name: sends-3-emails
    inputs:
      items:
        - alice@co.com
        - bob@co.com
        - carol@co.com
    expectations:
      status: completed
      intercepted_action_counts:
        tool_calls: 3
        total: 3
"#;
    let def: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();

    let results = run_all_tests(&engine, &def, false, None).await.unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].passed, "Failures: {:?}", results[0].failures);
}

// ===================================================================
// T5.7: Multiple test cases — run all, report independently
// ===================================================================

#[tokio::test]
async fn test_multiple_test_cases_reported_independently() {
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let executor = Arc::new(RecordingExecutor::new());
    let tools = make_tool_map(vec![
        ToolDefinitionBuilder::new("data.fetch", "Fetch")
            .read_only()
            .build(),
    ]);
    let engine = build_engine(store, executor, tools);

    let yaml = r#"
name: test/multi
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
      tool_id: data.fetch
      arguments: {}
    next: []
output:
  val: "{{steps.fetch.outputs.echo}}"
tests:
  - name: pass-case
    inputs: {}
    expectations:
      status: completed
      output:
        val: real
  - name: fail-case
    inputs: {}
    expectations:
      status: completed
      output:
        val: "wrong_value"
  - name: status-check
    inputs: {}
    expectations:
      status: completed
"#;
    let def: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();

    let results = run_all_tests(&engine, &def, false, None).await.unwrap();
    assert_eq!(results.len(), 3);

    // pass-case should pass
    let pass = results.iter().find(|r| r.test_name == "pass-case").unwrap();
    assert!(pass.passed);

    // fail-case should fail (wrong output)
    let fail = results.iter().find(|r| r.test_name == "fail-case").unwrap();
    assert!(!fail.passed);
    assert_eq!(fail.failures.len(), 1);

    // status-check should pass
    let status = results.iter().find(|r| r.test_name == "status-check").unwrap();
    assert!(status.passed);

    // Each gets its own instance_id
    let ids: Vec<i64> = results.iter().map(|r| r.instance_id).collect();
    assert_ne!(ids[0], ids[1]);
    assert_ne!(ids[1], ids[2]);
}

// ===================================================================
// T5.8: Test case YAML round-trip — tests survive serialization
// ===================================================================

#[test]
fn test_yaml_roundtrip_preserves_tests() {
    let yaml = r#"
name: test/roundtrip
version: "1.0"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs: []
    next: []
tests:
  - name: tc1
    description: A test case
    trigger_step_id: start
    inputs:
      key: value
    shadow_outputs:
      some_step:
        mock: data
    expectations:
      status: completed
      output:
        key: expected
      steps_completed:
        - start
      steps_not_reached:
        - never
      intercepted_action_counts:
        tool_calls: 5
  - name: tc2
    inputs: {}
    expectations:
      status: failed
"#;
    let def: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(def.tests.len(), 2);

    // Serialize back
    let reserialized = serde_yaml::to_string(&def).unwrap();
    let roundtripped: WorkflowDefinition = serde_yaml::from_str(&reserialized).unwrap();

    assert_eq!(roundtripped.tests.len(), 2);
    assert_eq!(roundtripped.tests[0].name, "tc1");
    assert_eq!(roundtripped.tests[0].description.as_deref(), Some("A test case"));
    assert_eq!(roundtripped.tests[0].trigger_step_id.as_deref(), Some("start"));
    assert_eq!(roundtripped.tests[0].inputs["key"], "value");
    assert_eq!(roundtripped.tests[0].shadow_outputs["some_step"]["mock"], "data");
    assert_eq!(roundtripped.tests[0].expectations.status.as_deref(), Some("completed"));
    assert_eq!(roundtripped.tests[0].expectations.steps_completed, vec!["start"]);
    assert_eq!(roundtripped.tests[0].expectations.steps_not_reached, vec!["never"]);
    assert_eq!(*roundtripped.tests[0].expectations.intercepted_action_counts.get("tool_calls").unwrap(), 5);

    assert_eq!(roundtripped.tests[1].name, "tc2");
    assert_eq!(roundtripped.tests[1].expectations.status.as_deref(), Some("failed"));
    assert!(roundtripped.tests[1].shadow_outputs.is_empty());
}

// ===================================================================
// T5.9: Empty tests field — run_all_tests returns empty vec
// ===================================================================

#[tokio::test]
async fn test_no_tests_returns_empty() {
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let executor = Arc::new(RecordingExecutor::new());
    let engine = build_engine(store, executor, HashMap::new());

    let yaml = r#"
name: test/no-tests
version: "1.0"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs: []
    next: []
"#;
    let def: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();
    assert!(def.tests.is_empty());

    let results = run_all_tests(&engine, &def, false, None).await.unwrap();
    assert!(results.is_empty());
}

// ===================================================================
// T5.10: Test case with steps_not_reached fails when step was reached
// ===================================================================

#[tokio::test]
async fn test_steps_not_reached_fails_when_reached() {
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let executor = Arc::new(RecordingExecutor::new());
    let tools = make_tool_map(vec![
        ToolDefinitionBuilder::new("data.fetch", "Fetch")
            .read_only()
            .build(),
    ]);
    let engine = build_engine(store, executor, tools);

    let yaml = r#"
name: test/not-reached-fail
version: "1.0"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs: []
    next: [step_a]
  - id: step_a
    type: task
    task:
      kind: call_tool
      tool_id: data.fetch
      arguments: {}
    next: []
tests:
  - name: wrong-not-reached
    inputs: {}
    expectations:
      status: completed
      steps_not_reached:
        - step_a
"#;
    let def: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();

    let results = run_all_tests(&engine, &def, false, None).await.unwrap();
    assert_eq!(results.len(), 1);
    assert!(!results[0].passed);
    let fail = &results[0].failures[0];
    assert_eq!(fail.expectation, "steps_not_reached: step_a");
    assert_eq!(fail.expected, "Pending or Skipped");
    assert_eq!(fail.actual, "Completed");
}
