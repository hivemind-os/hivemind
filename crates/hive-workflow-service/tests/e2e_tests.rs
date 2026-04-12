//! End-to-end tests for the workflow engine: launch → execute steps → complete.
//! Uses mock step executors to verify the full lifecycle without real tools/agents.

use hive_workflow::executor::{ExecutionContext, NullEventEmitter, StepExecutor, WorkflowEngine};
use hive_workflow::store::{WorkflowPersistence, WorkflowStore};
use hive_workflow::types::*;
use hive_workflow_service::WorkflowService;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::Mutex;

// ---------------------------------------------------------------------------
// Mock step executor that records calls and returns configurable results
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct MockStepExecutor {
    tool_calls: Arc<Mutex<Vec<(String, Value)>>>,
    agent_calls: Arc<Mutex<Vec<(String, String)>>>,
}

impl MockStepExecutor {
    fn new() -> Self {
        Self {
            tool_calls: Arc::new(Mutex::new(Vec::new())),
            agent_calls: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[async_trait::async_trait]
impl StepExecutor for MockStepExecutor {
    async fn call_tool(
        &self,
        tool_id: &str,
        arguments: Value,
        _ctx: &ExecutionContext,
    ) -> Result<Value, String> {
        self.tool_calls.lock().await.push((tool_id.to_string(), arguments.clone()));
        // Return a mock result based on the tool
        Ok(json!({ "output": format!("mock-{tool_id}"), "args": arguments }))
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
        Ok(json!({ "result": format!("agent-{persona_id} done") }))
    }

    async fn signal_agent(
        &self,
        _target: &SignalTarget,
        _content: &str,
        _ctx: &ExecutionContext,
    ) -> Result<Value, String> {
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
        Ok("mock-request-id".to_string())
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
        Ok("mock-subscription-id".to_string())
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
        Ok(format!("scheduled-{}", schedule.name))
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

// ---------------------------------------------------------------------------
// E2E: Sequential workflow (trigger → tool → end)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_e2e_sequential_workflow() {
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let mock = Arc::new(MockStepExecutor::new());
    let emitter = Arc::new(NullEventEmitter);

    let engine = WorkflowEngine::new(store.clone(), mock.clone(), emitter);

    let yaml = r#"
name: sequential
version: "1.0"
variables:
  type: object
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs:
      - name: msg
        type: string
        required: true
    outputs:
      msg: "{{trigger.msg}}"
    next: [process]

  - id: process
    type: task
    task:
      kind: call_tool
      tool_id: echo
      arguments:
        text: "{{steps.start.outputs.msg}}"
    outputs:
      reply: "{{result.output}}"
    next: [done]

  - id: done
    type: control_flow
    control:
      kind: end_workflow
output:
  reply: "{{steps.process.outputs.reply}}"
"#;

    let def: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();

    // Save definition
    {
        store.save_definition(yaml, &def).unwrap();
    }

    // Launch
    let instance_id = engine
        .launch(def, json!({"msg": "hello world"}), "session-1".to_string(), None, vec![], None)
        .await
        .unwrap();

    // Give async execution a moment
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    // Check instance completed
    let instance = { store.get_instance(instance_id).unwrap().unwrap() };

    assert_eq!(instance.status, WorkflowStatus::Completed);
    assert!(instance.output.is_some(), "workflow should have output");

    // Verify the tool was called
    let calls = mock.tool_calls.lock().await;
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "echo");
}

// ---------------------------------------------------------------------------
// E2E: Branching workflow
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_e2e_branching_workflow() {
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let mock = Arc::new(MockStepExecutor::new());
    let emitter = Arc::new(NullEventEmitter);

    let engine = WorkflowEngine::new(store.clone(), mock.clone(), emitter);

    let yaml = r#"
name: branching
version: "1.0"
variables:
  type: object
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs:
      - name: value
        type: number
        required: true
    outputs:
      value: "{{trigger.value}}"
    next: [decide]

  - id: decide
    type: control_flow
    control:
      kind: branch
      condition: "{{steps.start.outputs.value}} > 50"
      then: [high_path]
      else: [low_path]

  - id: high_path
    type: task
    task:
      kind: call_tool
      tool_id: high_handler
      arguments:
        val: "{{steps.start.outputs.value}}"
    next: [end]

  - id: low_path
    type: task
    task:
      kind: call_tool
      tool_id: low_handler
      arguments:
        val: "{{steps.start.outputs.value}}"
    next: [end]

  - id: end
    type: control_flow
    control:
      kind: end_workflow
"#;

    let def: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();
    {
        store.save_definition(yaml, &def).unwrap();
    }

    // Launch with value=75 → should take high_path
    let id = engine
        .launch(def, json!({"value": 75}), "session-1".to_string(), None, vec![], None)
        .await
        .unwrap();

    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    let instance = { store.get_instance(id).unwrap().unwrap() };

    assert_eq!(instance.status, WorkflowStatus::Completed);

    let calls = mock.tool_calls.lock().await;
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "high_handler", "should have taken high_path");
}

// ---------------------------------------------------------------------------
// E2E: Agent invocation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_e2e_agent_invocation() {
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let mock = Arc::new(MockStepExecutor::new());
    let emitter = Arc::new(NullEventEmitter);

    let engine = WorkflowEngine::new(store.clone(), mock.clone(), emitter);

    let yaml = r#"
name: agent-workflow
version: "1.0"
variables:
  type: object
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs: []
    next: [run_agent]

  - id: run_agent
    type: task
    task:
      kind: invoke_agent
      persona_id: summarizer
      task: "Summarize the project"
      async: false
    outputs:
      summary: "{{result.result}}"
    next: [end]

  - id: end
    type: control_flow
    control:
      kind: end_workflow
output:
  summary: "{{steps.run_agent.outputs.summary}}"
"#;

    let def: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();
    {
        store.save_definition(yaml, &def).unwrap();
    }

    let id =
        engine.launch(def, json!({}), "session-1".to_string(), None, vec![], None).await.unwrap();

    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    let instance = { store.get_instance(id).unwrap().unwrap() };

    assert_eq!(instance.status, WorkflowStatus::Completed);

    let calls = mock.agent_calls.lock().await;
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "summarizer");
}

// ---------------------------------------------------------------------------
// E2E: Feedback gate → respond → complete
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_e2e_feedback_gate_lifecycle() {
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let mock = Arc::new(MockStepExecutor::new());
    let emitter = Arc::new(NullEventEmitter);

    let engine = Arc::new(WorkflowEngine::new(store.clone(), mock.clone(), emitter));

    let yaml = r#"
name: feedback-flow
version: "1.0"
variables:
  type: object
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs: []
    next: [approval]

  - id: approval
    type: task
    task:
      kind: feedback_gate
      prompt: "Approve deployment?"
      choices: ["Yes", "No"]
      allow_freeform: false
    outputs:
      decision: "{{result.selected}}"
    next: [end]

  - id: end
    type: control_flow
    control:
      kind: end_workflow
output:
  decision: "{{steps.approval.outputs.decision}}"
"#;

    let def: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();
    {
        store.save_definition(yaml, &def).unwrap();
    }

    let id =
        engine.launch(def, json!({}), "session-1".to_string(), None, vec![], None).await.unwrap();

    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    // Should be waiting on input
    let instance = { store.get_instance(id).unwrap().unwrap() };
    assert_eq!(
        instance.status,
        WorkflowStatus::WaitingOnInput,
        "workflow should be waiting for feedback"
    );

    // Respond to the gate
    engine.respond_to_gate(id, "approval", json!({"selected": "Yes", "text": ""})).await.unwrap();

    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    // Should be completed now
    let instance = { store.get_instance(id).unwrap().unwrap() };
    assert_eq!(
        instance.status,
        WorkflowStatus::Completed,
        "workflow should complete after gate response"
    );

    // Verify the feedback gate outputs and mapped workflow output
    let gate_state = &instance.step_states["approval"];
    let gate_outputs = gate_state.outputs.as_ref().expect("gate should have outputs");
    assert_eq!(
        gate_outputs["decision"], "Yes",
        "output mapping {{{{result.selected}}}} should resolve. Got: {gate_outputs}"
    );
    let wf_output = instance.output.as_ref().expect("workflow should have output");
    assert_eq!(wf_output["decision"], "Yes", "workflow output should resolve from step outputs");
}

// ---------------------------------------------------------------------------
// E2E: Feedback gate outputs flow into downstream tool call templates
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_e2e_feedback_gate_outputs_in_downstream_tool_call() {
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let mock = Arc::new(MockStepExecutor::new());
    let emitter = Arc::new(NullEventEmitter);

    let engine = Arc::new(WorkflowEngine::new(store.clone(), mock.clone(), emitter));

    // This is the scenario: feedback gate (no output mappings) → tool call using gate outputs
    // This matches what the UI generates (no output bindings UI)
    let yaml = r#"
name: feedback-tool-test
version: "1.0"
variables:
  type: object
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
      prompt: "What should we do?"
      choices: ["approve", "reject"]
      allow_freeform: true
    next: [send]

  - id: send
    type: task
    task:
      kind: call_tool
      tool_id: comm.send_external_message
      arguments:
        connector_id: 'my-connector'
        body: 'Decision: {{steps.gate.outputs.selected}}, Comment: {{steps.gate.outputs.text}}'
        to: 'team-channel'
    next: [end]

  - id: end
    type: control_flow
    control:
      kind: end_workflow
"#;

    let def: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();
    store.save_definition(yaml, &def).unwrap();

    let id =
        engine.launch(def, json!({}), "session-1".to_string(), None, vec![], None).await.unwrap();

    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    // Should be waiting on feedback
    let instance = store.get_instance(id).unwrap().unwrap();
    assert_eq!(
        instance.status,
        WorkflowStatus::WaitingOnInput,
        "expected WaitingOnInput, got {:?}",
        instance.status
    );

    // Respond to the gate (same shape as what the UI sends)
    engine
        .respond_to_gate(id, "gate", json!({"selected": "approve", "text": "Looks good to me"}))
        .await
        .unwrap();

    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    // Should be completed
    let instance = store.get_instance(id).unwrap().unwrap();
    assert_eq!(
        instance.status,
        WorkflowStatus::Completed,
        "expected Completed, got {:?} error={:?}",
        instance.status,
        instance.error
    );

    // Verify gate step outputs (raw response, no output mappings)
    let gate_outputs =
        instance.step_states["gate"].outputs.as_ref().expect("gate step should have outputs");
    assert_eq!(gate_outputs["selected"], "approve");
    assert_eq!(gate_outputs["text"], "Looks good to me");

    // Verify the downstream tool call received resolved template values
    let calls = mock.tool_calls.lock().await;
    assert_eq!(calls.len(), 1, "expected exactly one tool call");
    let (tool_id, args) = &calls[0];
    assert_eq!(tool_id, "comm.send_external_message");
    assert_eq!(args["connector_id"], "my-connector", "literal arg should pass through");
    assert_eq!(
        args["body"], "Decision: approve, Comment: Looks good to me",
        "template args with feedback gate outputs should be resolved"
    );
    assert_eq!(args["to"], "team-channel");
}

// ---------------------------------------------------------------------------
// E2E: Feedback gate with output mappings + downstream templates
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_e2e_feedback_gate_with_output_mappings() {
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let mock = Arc::new(MockStepExecutor::new());
    let emitter = Arc::new(NullEventEmitter);

    let engine = Arc::new(WorkflowEngine::new(store.clone(), mock.clone(), emitter));

    // This tests the case where old workflows still have output mappings
    let yaml = r#"
name: feedback-mapped-test
version: "1.0"
variables:
  type: object
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
      prompt: "Approve?"
      choices: ["Yes", "No"]
      allow_freeform: false
    outputs:
      decision: '{{result.selected}}'
    next: [notify]

  - id: notify
    type: task
    task:
      kind: call_tool
      tool_id: echo
      arguments:
        msg: 'The decision was: {{steps.gate.outputs.decision}}'
    next: [end]

  - id: end
    type: control_flow
    control:
      kind: end_workflow
"#;

    let def: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();
    store.save_definition(yaml, &def).unwrap();

    let id =
        engine.launch(def, json!({}), "session-1".to_string(), None, vec![], None).await.unwrap();

    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    let instance = store.get_instance(id).unwrap().unwrap();
    assert_eq!(instance.status, WorkflowStatus::WaitingOnInput);

    engine.respond_to_gate(id, "gate", json!({"selected": "Yes", "text": ""})).await.unwrap();

    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    let instance = store.get_instance(id).unwrap().unwrap();
    assert_eq!(
        instance.status,
        WorkflowStatus::Completed,
        "expected Completed, got {:?} error={:?}",
        instance.status,
        instance.error
    );

    // With output mappings, gate outputs should be the mapped values
    let gate_outputs =
        instance.step_states["gate"].outputs.as_ref().expect("gate should have mapped outputs");
    assert_eq!(
        gate_outputs["decision"], "Yes",
        "output mapping {{result.selected}} should resolve to 'Yes'"
    );

    // Downstream step should see the mapped output
    let calls = mock.tool_calls.lock().await;
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].1["msg"], "The decision was: Yes",
        "downstream template should resolve mapped gate output"
    );
}

// ---------------------------------------------------------------------------
// E2E: Kill cancels running workflow
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_e2e_kill_workflow() {
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let mock = Arc::new(MockStepExecutor::new());
    let emitter = Arc::new(NullEventEmitter);

    let engine = Arc::new(WorkflowEngine::new(store.clone(), mock.clone(), emitter));

    let yaml = r#"
name: killable
version: "1.0"
variables:
  type: object
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
      kind: feedback_gate
      prompt: "Waiting..."
      choices: []
      allow_freeform: true
    next: [end]

  - id: end
    type: control_flow
    control:
      kind: end_workflow
"#;

    let def: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();
    {
        store.save_definition(yaml, &def).unwrap();
    }

    let id =
        engine.launch(def, json!({}), "session-1".to_string(), None, vec![], None).await.unwrap();

    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    // Kill while waiting
    engine.kill(id).await.unwrap();

    let instance = { store.get_instance(id).unwrap().unwrap() };
    assert_eq!(instance.status, WorkflowStatus::Killed);
}

// ---------------------------------------------------------------------------
// E2E: Full service workflow through WorkflowService
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_e2e_service_full_lifecycle() {
    let svc = WorkflowService::in_memory().unwrap();

    let yaml = r#"
name: user/service-test
version: "1.0"
variables:
  type: object
  properties:
    greeting:
      type: string
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs:
      - name: name
        type: string
        required: true
    outputs:
      name: "{{trigger.name}}"
    next: [greet]

  - id: greet
    type: task
    task:
      kind: delay
      duration_secs: 0
    outputs:
      msg: "hello"
    next: [end]

  - id: end
    type: control_flow
    control:
      kind: end_workflow
output:
  greeting: "{{steps.greet.outputs.msg}}"
"#;

    // Save definition
    svc.save_definition(yaml).await.unwrap();

    // Launch
    let id = svc
        .launch(
            "user/service-test",
            Some("1.0"),
            json!({"name": "World"}),
            "sess-1",
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();

    // Give async execution time
    tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;

    // Should be completed (delay of 0 seconds)
    let instance = svc.get_instance(id).await.unwrap();
    assert_eq!(
        instance.status,
        WorkflowStatus::Completed,
        "workflow should complete: {:?}",
        instance.status
    );
}
