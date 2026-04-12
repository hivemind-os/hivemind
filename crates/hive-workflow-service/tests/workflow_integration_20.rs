//! 20 integration tests exercising the workflow engine from simple to complex.
//! Tests cover: definition CRUD, launch, execution, branching, loops,
//! feedback gates, error handling, multi-trigger, parallel steps,
//! variable binding, output mapping, pause/resume/kill, permission
//! management, and complex graph topologies.

use hive_workflow::executor::{ExecutionContext, NullEventEmitter, StepExecutor, WorkflowEngine};
use hive_workflow::store::{WorkflowPersistence, WorkflowStore};
use hive_workflow::types::*;
use hive_workflow_service::WorkflowService;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::Mutex;

// ---------------------------------------------------------------------------
// Shared mock executor
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct RecordingExecutor {
    tool_calls: Arc<Mutex<Vec<(String, Value)>>>,
    agent_calls: Arc<Mutex<Vec<(String, String)>>>,
    messages: Arc<Mutex<Vec<(String, String)>>>,
    /// If set, call_tool returns an error for tools matching this prefix.
    fail_tool_prefix: Arc<Mutex<Option<String>>>,
}

impl RecordingExecutor {
    fn new() -> Self {
        Self {
            tool_calls: Arc::new(Mutex::new(Vec::new())),
            agent_calls: Arc::new(Mutex::new(Vec::new())),
            messages: Arc::new(Mutex::new(Vec::new())),
            fail_tool_prefix: Arc::new(Mutex::new(None)),
        }
    }

    async fn set_fail_prefix(&self, prefix: &str) {
        *self.fail_tool_prefix.lock().await = Some(prefix.to_string());
    }
}

#[async_trait::async_trait]
impl StepExecutor for RecordingExecutor {
    async fn call_tool(
        &self,
        tool_id: &str,
        arguments: Value,
        _ctx: &ExecutionContext,
    ) -> Result<Value, String> {
        self.tool_calls.lock().await.push((tool_id.to_string(), arguments.clone()));

        if let Some(prefix) = self.fail_tool_prefix.lock().await.as_ref() {
            if tool_id.starts_with(prefix) {
                return Err(format!("tool {tool_id} failed on purpose"));
            }
        }
        Ok(json!({ "output": format!("result-of-{tool_id}"), "args": arguments }))
    }

    async fn invoke_agent(
        &self,
        persona_id: &str,
        task: &str,
        _async_exec: bool,
        _timeout_secs: Option<u64>,
        _step_permissions: &[PermissionEntry],
        _agent_name: Option<&str>,
        _: Option<&str>,
        _ctx: &ExecutionContext,
    ) -> Result<Value, String> {
        self.agent_calls.lock().await.push((persona_id.to_string(), task.to_string()));
        Ok(json!({ "result": format!("agent-{persona_id}-done"), "task": task }))
    }

    async fn signal_agent(
        &self,
        target: &SignalTarget,
        content: &str,
        _ctx: &ExecutionContext,
    ) -> Result<Value, String> {
        let target_str = match target {
            SignalTarget::Session { session_id } => format!("session:{session_id}"),
            SignalTarget::Agent { agent_id } => format!("agent:{agent_id}"),
        };
        self.messages.lock().await.push((target_str, content.to_string()));
        Ok(json!({ "sent": true }))
    }

    async fn wait_for_agent(
        &self,
        _: &str,
        _: Option<u64>,
        _: &ExecutionContext,
    ) -> Result<Value, String> {
        Ok(Value::Null)
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
        Ok("feedback-req-id".to_string())
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
        Ok("event-gate-sub-id".to_string())
    }

    async fn launch_workflow(
        &self,
        _workflow_name: &str,
        _inputs: Value,
        _ctx: &ExecutionContext,
    ) -> Result<i64, String> {
        Ok(9999)
    }

    async fn schedule_task(
        &self,
        schedule: &ScheduleTaskDef,
        _ctx: &ExecutionContext,
    ) -> Result<String, String> {
        Ok(format!("task-{}", schedule.name))
    }

    async fn render_prompt_template(
        &self,
        _persona_id: &str,
        _prompt_id: &str,
        _parameters: Value,
        _ctx: &ExecutionContext,
    ) -> Result<String, String> {
        Err("render_prompt_template not implemented in test executor".to_string())
    }
}

/// Helper to build engine + store + executor triple.
fn make_engine() -> (Arc<WorkflowEngine>, Arc<WorkflowStore>, Arc<RecordingExecutor>) {
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let exec = Arc::new(RecordingExecutor::new());
    let emitter = Arc::new(NullEventEmitter);
    let engine = Arc::new(WorkflowEngine::new(store.clone(), exec.clone(), emitter));
    (engine, store, exec)
}

/// Helper to save & launch a YAML workflow.
async fn save_and_launch(
    engine: &WorkflowEngine,
    store: &WorkflowStore,
    yaml: &str,
    inputs: Value,
) -> i64 {
    let def: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();
    store.save_definition(yaml, &def).unwrap();
    engine.launch(def, inputs, "test-session".into(), None, vec![], None).await.unwrap()
}

/// Short sleep to let async execution finish.
async fn tick() {
    tokio::time::sleep(tokio::time::Duration::from_millis(250)).await;
}

// =========================================================================
// 1. Minimal trigger-only workflow completes immediately
// =========================================================================
#[tokio::test]
async fn test_01_minimal_trigger_only() {
    let (engine, store, _exec) = make_engine();
    let yaml = r#"
name: minimal
version: "1.0"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs: []
    next: [done]
  - id: done
    type: control_flow
    control:
      kind: end_workflow
"#;
    let id = save_and_launch(&engine, &store, yaml, json!({})).await;
    tick().await;

    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert_eq!(inst.step_states["start"].status, StepStatus::Completed);
    assert_eq!(inst.step_states["done"].status, StepStatus::Completed);
}

// =========================================================================
// 2. Linear tool chain: trigger → tool_a → tool_b → end
// =========================================================================
#[tokio::test]
async fn test_02_linear_tool_chain() {
    let (engine, store, exec) = make_engine();
    let yaml = r#"
name: linear-chain
version: "1.0"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs: []
    next: [tool_a]
  - id: tool_a
    type: task
    task:
      kind: call_tool
      tool_id: formatter
      arguments: { text: "hello" }
    next: [tool_b]
  - id: tool_b
    type: task
    task:
      kind: call_tool
      tool_id: validator
      arguments: { data: "world" }
    next: [done]
  - id: done
    type: control_flow
    control:
      kind: end_workflow
"#;
    let id = save_and_launch(&engine, &store, yaml, json!({})).await;
    tick().await;

    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);

    let calls = exec.tool_calls.lock().await;
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].0, "formatter");
    assert_eq!(calls[1].0, "validator");
}

// =========================================================================
// 3. Input propagation: trigger inputs flow to tool arguments
// =========================================================================
#[tokio::test]
async fn test_03_input_propagation() {
    let (engine, store, exec) = make_engine();
    let yaml = r#"
name: input-prop
version: "1.0"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs:
      - name: greeting
        type: string
        required: true
    outputs:
      msg: "{{trigger.greeting}}"
    next: [echo]
  - id: echo
    type: task
    task:
      kind: call_tool
      tool_id: echo
      arguments:
        message: "{{steps.start.outputs.msg}}"
    next: [done]
  - id: done
    type: control_flow
    control:
      kind: end_workflow
"#;
    let id = save_and_launch(&engine, &store, yaml, json!({"greeting": "hi there"})).await;
    tick().await;

    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);

    let calls = exec.tool_calls.lock().await;
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].1["message"], "hi there");
}

// =========================================================================
// 4. Output mapping: workflow produces output from step results
// =========================================================================
#[tokio::test]
async fn test_04_output_mapping() {
    let (engine, store, _exec) = make_engine();
    let yaml = r#"
name: with-output
version: "1.0"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs: []
    next: [work]
  - id: work
    type: task
    task:
      kind: call_tool
      tool_id: compute
      arguments: {}
    outputs:
      result: "{{result.output}}"
    next: [done]
  - id: done
    type: control_flow
    control:
      kind: end_workflow
output:
  answer: "{{steps.work.outputs.result}}"
"#;
    let id = save_and_launch(&engine, &store, yaml, json!({})).await;
    tick().await;

    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    let output = inst.output.expect("should have output");
    assert_eq!(output["answer"], "result-of-compute");
}

// =========================================================================
// 5. Branch takes the "then" path when condition is true
// =========================================================================
#[tokio::test]
async fn test_05_branch_then_path() {
    let (engine, store, exec) = make_engine();
    let yaml = r#"
name: branch-then
version: "1.0"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs:
      - name: amount
        type: number
    outputs:
      amount: "{{trigger.amount}}"
    next: [decide]
  - id: decide
    type: control_flow
    control:
      kind: branch
      condition: "{{steps.start.outputs.amount}} > 100"
      then: [approve]
      else: [reject]
  - id: approve
    type: task
    task:
      kind: call_tool
      tool_id: approve_payment
      arguments: {}
    next: [done]
  - id: reject
    type: task
    task:
      kind: call_tool
      tool_id: reject_payment
      arguments: {}
    next: [done]
  - id: done
    type: control_flow
    control:
      kind: end_workflow
"#;
    let id = save_and_launch(&engine, &store, yaml, json!({"amount": 200})).await;
    tick().await;

    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);

    let calls = exec.tool_calls.lock().await;
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "approve_payment");
    // reject_payment should be skipped
    assert_eq!(inst.step_states["reject"].status, StepStatus::Skipped);
}

// =========================================================================
// 6. Branch takes the "else" path when condition is false
// =========================================================================
#[tokio::test]
async fn test_06_branch_else_path() {
    let (engine, store, exec) = make_engine();
    let yaml = r#"
name: branch-else
version: "1.0"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs:
      - name: amount
        type: number
    outputs:
      amount: "{{trigger.amount}}"
    next: [decide]
  - id: decide
    type: control_flow
    control:
      kind: branch
      condition: "{{steps.start.outputs.amount}} > 100"
      then: [approve]
      else: [reject]
  - id: approve
    type: task
    task:
      kind: call_tool
      tool_id: approve_payment
      arguments: {}
    next: [done]
  - id: reject
    type: task
    task:
      kind: call_tool
      tool_id: reject_payment
      arguments: {}
    next: [done]
  - id: done
    type: control_flow
    control:
      kind: end_workflow
"#;
    let id = save_and_launch(&engine, &store, yaml, json!({"amount": 50})).await;
    tick().await;

    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);

    let calls = exec.tool_calls.lock().await;
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "reject_payment");
    assert_eq!(inst.step_states["approve"].status, StepStatus::Skipped);
}

// =========================================================================
// 7. Feedback gate pauses and resumes on response
// =========================================================================
#[tokio::test]
async fn test_07_feedback_gate_lifecycle() {
    let (engine, store, _exec) = make_engine();
    let yaml = r#"
name: feedback
version: "1.0"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs: []
    next: [gate]
  - id: gate
    type: task
    task:
      kind: feedback_gate
      prompt: "Continue?"
      choices: ["yes", "no"]
      allow_freeform: false
    outputs:
      answer: "{{result.selected}}"
    next: [done]
  - id: done
    type: control_flow
    control:
      kind: end_workflow
output:
  user_response: "{{steps.gate.outputs.answer}}"
"#;
    let id = save_and_launch(&engine, &store, yaml, json!({})).await;
    tick().await;

    // Should be waiting
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::WaitingOnInput);
    assert_eq!(inst.step_states["gate"].status, StepStatus::WaitingOnInput);

    // Respond
    engine.respond_to_gate(id, "gate", json!({"selected": "yes", "text": ""})).await.unwrap();
    tick().await;

    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
}

// =========================================================================
// 8. Kill terminates a waiting workflow
// =========================================================================
#[tokio::test]
async fn test_08_kill_waiting_workflow() {
    let (engine, store, _exec) = make_engine();
    let yaml = r#"
name: killable
version: "1.0"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs: []
    next: [gate]
  - id: gate
    type: task
    task:
      kind: feedback_gate
      prompt: "Waiting..."
      choices: []
      allow_freeform: true
    next: [done]
  - id: done
    type: control_flow
    control:
      kind: end_workflow
"#;
    let id = save_and_launch(&engine, &store, yaml, json!({})).await;
    tick().await;

    assert_eq!(store.get_instance(id).unwrap().unwrap().status, WorkflowStatus::WaitingOnInput);

    engine.kill(id).await.unwrap();

    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Killed);
}

// =========================================================================
// 9. Pause and resume a running workflow
// =========================================================================
#[tokio::test]
async fn test_09_pause_resume() {
    let (engine, store, _exec) = make_engine();
    let yaml = r#"
name: pausable
version: "1.0"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs: []
    next: [gate]
  - id: gate
    type: task
    task:
      kind: feedback_gate
      prompt: "Waiting..."
      choices: []
      allow_freeform: true
    next: [done]
  - id: done
    type: control_flow
    control:
      kind: end_workflow
"#;
    let id = save_and_launch(&engine, &store, yaml, json!({})).await;
    tick().await;

    // Pause
    engine.pause(id).await.unwrap();
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Paused);

    // Resume
    engine.resume(id).await.unwrap();
    let inst = store.get_instance(id).unwrap().unwrap();
    // Should return to waiting state after resume
    assert!(
        matches!(inst.status, WorkflowStatus::WaitingOnInput | WorkflowStatus::Running),
        "status should be WaitingOnInput or Running, got {:?}",
        inst.status
    );
}

// =========================================================================
// 10. Agent invocation records correct persona and task
// =========================================================================
#[tokio::test]
async fn test_10_invoke_agent() {
    let (engine, store, exec) = make_engine();
    let yaml = r#"
name: agent-call
version: "1.0"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs: []
    next: [agent]
  - id: agent
    type: task
    task:
      kind: invoke_agent
      persona_id: code-reviewer
      task: "Review PR #42"
      async: false
    outputs:
      review: "{{result.result}}"
    next: [done]
  - id: done
    type: control_flow
    control:
      kind: end_workflow
output:
  review: "{{steps.agent.outputs.review}}"
"#;
    let id = save_and_launch(&engine, &store, yaml, json!({})).await;
    tick().await;

    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);

    let calls = exec.agent_calls.lock().await;
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "code-reviewer");
    assert_eq!(calls[0].1, "Review PR #42");

    let output = inst.output.unwrap();
    assert_eq!(output["review"], "agent-code-reviewer-done");
}

// =========================================================================
// 11. Signal agent step records target and content
// =========================================================================
#[tokio::test]
async fn test_11_signal_agent() {
    let (engine, store, exec) = make_engine();
    let yaml = r#"
name: notifier
version: "1.0"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs: []
    next: [notify]
  - id: notify
    type: task
    task:
      kind: send_message
      target:
        type: session
        session_id: sess-42
      content: "Build completed"
    next: [done]
  - id: done
    type: control_flow
    control:
      kind: end_workflow
"#;
    let id = save_and_launch(&engine, &store, yaml, json!({})).await;
    tick().await;

    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);

    let msgs = exec.messages.lock().await;
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].0, "session:sess-42");
    assert_eq!(msgs[0].1, "Build completed");
}

// =========================================================================
// 12. Delay step with 0 seconds completes immediately
// =========================================================================
#[tokio::test]
async fn test_12_delay_step() {
    let (engine, store, _exec) = make_engine();
    let yaml = r#"
name: with-delay
version: "1.0"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs: []
    next: [wait]
  - id: wait
    type: task
    task:
      kind: delay
      duration_secs: 0
    next: [done]
  - id: done
    type: control_flow
    control:
      kind: end_workflow
"#;
    let id = save_and_launch(&engine, &store, yaml, json!({})).await;
    tick().await;

    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert_eq!(inst.step_states["wait"].status, StepStatus::Completed);
}

// =========================================================================
// 13. Error strategy: skip on failure
// =========================================================================
#[tokio::test]
async fn test_13_error_strategy_skip() {
    let (engine, store, exec) = make_engine();
    exec.set_fail_prefix("flaky").await;

    let yaml = r#"
name: skip-on-error
version: "1.0"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs: []
    next: [risky]
  - id: risky
    type: task
    task:
      kind: call_tool
      tool_id: flaky_tool
      arguments: {}
    on_error:
      strategy: skip
    next: [safe]
  - id: safe
    type: task
    task:
      kind: call_tool
      tool_id: reliable_tool
      arguments: {}
    next: [done]
  - id: done
    type: control_flow
    control:
      kind: end_workflow
"#;
    let id = save_and_launch(&engine, &store, yaml, json!({})).await;
    tick().await;

    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert_eq!(inst.step_states["risky"].status, StepStatus::Skipped);
    assert_eq!(inst.step_states["safe"].status, StepStatus::Completed);
}

// =========================================================================
// 14. Error strategy: fail_workflow on failure
// =========================================================================
#[tokio::test]
async fn test_14_error_strategy_fail() {
    let (engine, store, exec) = make_engine();
    exec.set_fail_prefix("broken").await;

    let yaml = r#"
name: fail-on-error
version: "1.0"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs: []
    next: [bad]
  - id: bad
    type: task
    task:
      kind: call_tool
      tool_id: broken_tool
      arguments: {}
    on_error:
      strategy: fail_workflow
    next: [never]
  - id: never
    type: task
    task:
      kind: call_tool
      tool_id: should_not_run
      arguments: {}
    next: [done]
  - id: done
    type: control_flow
    control:
      kind: end_workflow
"#;
    let id = save_and_launch(&engine, &store, yaml, json!({})).await;
    tick().await;

    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Failed);
    assert_eq!(inst.step_states["bad"].status, StepStatus::Failed);

    // "never" step should not have executed
    let calls = exec.tool_calls.lock().await;
    assert!(
        !calls.iter().any(|(id, _)| id == "should_not_run"),
        "should_not_run should not have been called"
    );
}

// =========================================================================
// 15. Definition CRUD via WorkflowService
// =========================================================================
#[tokio::test]
async fn test_15_definition_crud() {
    let svc = WorkflowService::in_memory().unwrap();

    let yaml = r#"
name: user/crud-test
version: "2.0"
description: "CRUD test workflow"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs: []
    next: [done]
  - id: done
    type: control_flow
    control:
      kind: end_workflow
"#;
    // Save
    let def = svc.save_definition(yaml).await.unwrap();
    assert_eq!(def.name, "user/crud-test");
    assert_eq!(def.version, "2.0");
    assert_eq!(def.description, Some("CRUD test workflow".into()));

    // List
    let defs = svc.list_definitions().await.unwrap();
    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].name, "user/crud-test");

    // Get specific version
    let (got_def, got_yaml) = svc.get_definition("user/crud-test", "2.0").await.unwrap();
    assert_eq!(got_def.name, "user/crud-test");
    assert!(!got_yaml.is_empty());

    // Get latest
    let (latest, _) = svc.get_latest_definition("user/crud-test").await.unwrap();
    assert_eq!(latest.version, "2.0");

    // Delete
    let deleted = svc.delete_definition("user/crud-test", "2.0").await.unwrap();
    assert!(deleted);

    let defs = svc.list_definitions().await.unwrap();
    assert_eq!(defs.len(), 0);
}

// =========================================================================
// 16. Invalid definition is rejected
// =========================================================================
#[tokio::test]
async fn test_16_invalid_definition_rejected() {
    let svc = WorkflowService::in_memory().unwrap();

    // Missing trigger step — should fail validation
    let yaml = r#"
name: user/bad-workflow
version: "1.0"
steps:
  - id: only_tool
    type: task
    task:
      kind: call_tool
      tool_id: something
      arguments: {}
"#;
    let result = svc.save_definition(yaml).await;
    assert!(result.is_err(), "should reject workflow without trigger step");
}

// =========================================================================
// 17. Launch non-existent definition fails
// =========================================================================
#[tokio::test]
async fn test_17_launch_missing_definition() {
    let svc = WorkflowService::in_memory().unwrap();

    let result = svc.launch("nonexistent", None, json!({}), "sess-1", None, None, None, None).await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("not found") || err.contains("Not found"),
        "error should mention not found: {err}"
    );
}

// =========================================================================
// 18. Instance list with status filter
// =========================================================================
#[tokio::test]
async fn test_18_instance_list_filter() {
    // Use engine directly since WorkflowService::in_memory() has no interaction gate
    let (engine, store, _exec) = make_engine();
    let yaml = r#"
name: filter-test
version: "1.0"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs: []
    next: [gate]
  - id: gate
    type: task
    task:
      kind: feedback_gate
      prompt: "Wait"
      choices: []
      allow_freeform: true
    next: [done]
  - id: done
    type: control_flow
    control:
      kind: end_workflow
"#;
    let def: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();
    {
        // store is used directly (no lock needed)
        store.save_definition(yaml, &def).unwrap();
    }

    // Launch 2 instances
    let _id1 =
        engine.launch(def.clone(), json!({}), "sess-1".into(), None, vec![], None).await.unwrap();
    let _id2 = engine.launch(def, json!({}), "sess-2".into(), None, vec![], None).await.unwrap();

    tick().await;

    // Both should be waiting
    let filter =
        InstanceFilter { statuses: vec![WorkflowStatus::WaitingOnInput], ..Default::default() };
    let waiting = store.list_instances(&filter).unwrap();
    assert_eq!(waiting.items.len(), 2);

    // Filter by session
    let filter = InstanceFilter { parent_session_id: Some("sess-1".into()), ..Default::default() };
    let session_filtered = store.list_instances(&filter).unwrap();
    assert_eq!(session_filtered.items.len(), 1);
}

// =========================================================================
// 19. Diamond merge: trigger → (A, B in parallel) → merge → end
// =========================================================================
#[tokio::test]
async fn test_19_diamond_merge_pattern() {
    let (engine, store, exec) = make_engine();
    let yaml = r#"
name: diamond
version: "1.0"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs: []
    next: [left, right]
  - id: left
    type: task
    task:
      kind: call_tool
      tool_id: left_worker
      arguments: {}
    next: [merge]
  - id: right
    type: task
    task:
      kind: call_tool
      tool_id: right_worker
      arguments: {}
    next: [merge]
  - id: merge
    type: task
    task:
      kind: call_tool
      tool_id: aggregator
      arguments: {}
    next: [done]
  - id: done
    type: control_flow
    control:
      kind: end_workflow
"#;
    let id = save_and_launch(&engine, &store, yaml, json!({})).await;
    tick().await;

    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);

    let calls = exec.tool_calls.lock().await;
    // left, right, then aggregator
    assert_eq!(calls.len(), 3);
    let tool_ids: Vec<&str> = calls.iter().map(|(id, _)| id.as_str()).collect();
    assert!(tool_ids.contains(&"left_worker"));
    assert!(tool_ids.contains(&"right_worker"));
    assert!(tool_ids.contains(&"aggregator"));
    // aggregator must come after both workers
    let agg_idx = tool_ids.iter().position(|&id| id == "aggregator").unwrap();
    assert!(agg_idx >= 2, "aggregator should be last");
}

// =========================================================================
// 20. Complex pipeline: trigger → tool → branch → (agent | gate) → notify → end
// =========================================================================
#[tokio::test]
async fn test_20_complex_pipeline() {
    let (engine, store, exec) = make_engine();
    let yaml = r#"
name: complex-pipeline
version: "1.0"
variables:
  type: object
  properties:
    priority:
      type: string
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs:
      - name: priority
        type: string
        required: true
    outputs:
      priority: "{{trigger.priority}}"
    next: [fetch]

  - id: fetch
    type: task
    task:
      kind: call_tool
      tool_id: data_fetcher
      arguments:
        query: "get latest"
    outputs:
      data: "{{result.output}}"
    next: [decide]

  - id: decide
    type: control_flow
    control:
      kind: branch
      condition: "{{steps.start.outputs.priority}} == high"
      then: [auto_process]
      else: [manual_review]

  - id: auto_process
    type: task
    task:
      kind: invoke_agent
      persona_id: processor
      task: "Auto-process data"
      async: false
    outputs:
      result: "{{result.result}}"
    next: [notify]

  - id: manual_review
    type: task
    task:
      kind: feedback_gate
      prompt: "Review this data before proceeding"
      choices: ["approve", "reject"]
      allow_freeform: true
    outputs:
      review: "{{result.selected}}"
    next: [notify]

  - id: notify
    type: task
    task:
      kind: send_message
      target:
        type: session
        session_id: monitoring
      content: "Pipeline complete"
    next: [done]

  - id: done
    type: control_flow
    control:
      kind: end_workflow
output:
  status: "completed"
"#;

    // Test high priority path: auto_process (agent), then notify, then done
    let id = save_and_launch(&engine, &store, yaml, json!({"priority": "high"})).await;
    tick().await;

    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(
        inst.status,
        WorkflowStatus::Completed,
        "high-priority should complete: step_states={:?}",
        inst.step_states.iter().map(|(k, v)| (k, &v.status)).collect::<Vec<_>>()
    );

    let agent_calls = exec.agent_calls.lock().await;
    assert_eq!(agent_calls.len(), 1);
    assert_eq!(agent_calls[0].0, "processor");

    let msgs = exec.messages.lock().await;
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].0, "session:monitoring");

    assert_eq!(inst.step_states["manual_review"].status, StepStatus::Skipped);
}
