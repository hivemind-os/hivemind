use hive_workflow::*;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Wait for an instance to reach a terminal state (handles async delay steps).
async fn wait_for_terminal(store: &WorkflowStore, instance_id: i64) {
    for _ in 0..100 {
        let inst = store.get_instance(instance_id).unwrap().unwrap();
        if matches!(
            inst.status,
            WorkflowStatus::Completed | WorkflowStatus::Failed | WorkflowStatus::Killed
        ) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// End-to-end test: parse YAML → validate → launch → verify completion.
#[tokio::test]
async fn test_yaml_to_execution() {
    let yaml = r#"
name: e2e-test
version: "1.0"
description: "End-to-end YAML workflow test"

variables:
  type: object
  properties:
    processed:
      type: boolean
      default: false

steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs:
        - name: greeting
          input_type: string
          required: true
    next: [process]

  - id: process
    type: task
    task:
      kind: call_tool
      tool_id: echo_tool
      arguments:
        message: "{{trigger.greeting}}"
    outputs:
      echo_result: "{{result.echo}}"
    next: [decide]

  - id: decide
    type: control_flow
    control:
      kind: branch
      condition: "{{steps.process.outputs.echo_result}} == hello"
      then: [finish_ok]
      else: [finish_alt]

  - id: finish_ok
    type: task
    task:
      kind: delay
      duration_secs: 0
    outputs:
      path: "ok"
    next: [end]

  - id: finish_alt
    type: task
    task:
      kind: delay
      duration_secs: 0
    outputs:
      path: "alt"
    next: [end]

  - id: end
    type: control_flow
    control:
      kind: end_workflow

output:
  result_path: "{{steps.finish_ok.outputs.path}}"
"#;

    // Parse YAML
    let definition: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(definition.name, "e2e-test");
    assert_eq!(definition.steps.len(), 6);

    // Validate
    validate_definition(&definition).unwrap();

    // Execute
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let executor = Arc::new(EchoExecutor);
    let emitter = Arc::new(NullEventEmitter);
    let engine = WorkflowEngine::new(store.clone(), executor, emitter);

    let instance_id = engine
        .launch(
            definition,
            serde_json::json!({"greeting": "hello"}),
            "test-session".into(),
            None,
            vec![],
            None,
        )
        .await
        .unwrap();

    let store = &*store;
    wait_for_terminal(store, instance_id).await;
    let instance = store.get_instance(instance_id).unwrap().unwrap();

    assert_eq!(instance.status, WorkflowStatus::Completed);
    assert_eq!(instance.step_states["finish_ok"].status, StepStatus::Completed);
    assert_eq!(instance.step_states["finish_alt"].status, StepStatus::Skipped);

    // Check output
    let output = instance.output.as_ref().unwrap();
    assert_eq!(output["result_path"], "ok");
}

/// Test workflow with parallel steps (fork-join pattern).
#[tokio::test]
async fn test_parallel_steps() {
    let yaml = r#"
name: parallel-test
version: "1.0"
variables:
  type: object
  properties: {}

steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs: []
    next: [task_a, task_b]

  - id: task_a
    type: task
    task:
      kind: call_tool
      tool_id: echo_tool
      arguments:
        message: "a"
    outputs:
      val: "{{result.echo}}"
    next: [join]

  - id: task_b
    type: task
    task:
      kind: call_tool
      tool_id: echo_tool
      arguments:
        message: "b"
    outputs:
      val: "{{result.echo}}"
    next: [join]

  - id: join
    type: task
    task:
      kind: delay
      duration_secs: 0
    next: [end]

  - id: end
    type: control_flow
    control:
      kind: end_workflow
"#;

    let definition: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();
    validate_definition(&definition).unwrap();

    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let executor = Arc::new(EchoExecutor);
    let emitter = Arc::new(NullEventEmitter);
    let engine = WorkflowEngine::new(store.clone(), executor, emitter);

    let instance_id = engine
        .launch(definition, serde_json::json!({}), "test-session".into(), None, vec![], None)
        .await
        .unwrap();

    let store = &*store;
    wait_for_terminal(store, instance_id).await;
    let instance = store.get_instance(instance_id).unwrap().unwrap();

    assert_eq!(instance.status, WorkflowStatus::Completed);
    assert_eq!(instance.step_states["task_a"].status, StepStatus::Completed);
    assert_eq!(instance.step_states["task_b"].status, StepStatus::Completed);
    assert_eq!(instance.step_states["join"].status, StepStatus::Completed);
}

/// Test YAML definition round-trip through store.
#[test]
fn test_yaml_store_roundtrip() {
    let yaml = r#"
name: stored-workflow
version: "2.0"
description: "Test storage"
variables:
  type: object
  properties:
    count:
      type: number
      default: 0
steps:
  - id: start
    type: trigger
    trigger:
      type: schedule
      cron: "0 0 9 * * MON-FRI *"
    next: [end]
  - id: end
    type: control_flow
    control:
      kind: end_workflow
requested_tools:
  - tool_id: fs.read
    approval: auto
"#;

    let def: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();
    validate_definition(&def).unwrap();

    let store = WorkflowStore::in_memory().unwrap();
    store.save_definition(yaml, &def).unwrap();

    let (loaded, loaded_yaml) = store.get_definition("stored-workflow", "2.0").unwrap().unwrap();
    assert_eq!(loaded.name, "stored-workflow");
    assert_eq!(loaded.version, "2.0");
    assert_eq!(loaded.requested_tools.len(), 1);
    assert_eq!(loaded.requested_tools[0].tool_id, "fs.read");
    assert_eq!(loaded.requested_tools[0].approval, ToolApprovalLevel::Auto);
    assert!(!loaded_yaml.is_empty());

    let summaries = store.list_definitions().unwrap();
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].trigger_types, vec!["schedule"]);
}

// ---------------------------------------------------------------------------
// Test executor that echoes input as output
// ---------------------------------------------------------------------------

struct EchoExecutor;

#[async_trait::async_trait]
impl StepExecutor for EchoExecutor {
    async fn call_tool(
        &self,
        _tool_id: &str,
        args: serde_json::Value,
        _ctx: &ExecutionContext,
    ) -> Result<serde_json::Value, String> {
        let msg = args.get("message").and_then(|v| v.as_str()).unwrap_or("default");
        Ok(serde_json::json!({"echo": msg, "status": "ok"}))
    }

    async fn invoke_agent(
        &self,
        _: &str,
        _: &str,
        _: bool,
        _: Option<u64>,
        _: &[PermissionEntry],
        _: Option<&str>,
        _: Option<&str>,
        _: &ExecutionContext,
    ) -> Result<serde_json::Value, String> {
        Ok(serde_json::json!({"result": "agent_done"}))
    }

    async fn signal_agent(
        &self,
        _: &SignalTarget,
        _: &str,
        _: &ExecutionContext,
    ) -> Result<serde_json::Value, String> {
        Ok(serde_json::Value::Null)
    }

    async fn wait_for_agent(
        &self,
        _: &str,
        _: Option<u64>,
        _: &ExecutionContext,
    ) -> Result<serde_json::Value, String> {
        Ok(serde_json::Value::Null)
    }

    async fn create_feedback_request(
        &self,
        _: i64,
        _: &str,
        _: &str,
        _: Option<&[String]>,
        _: bool,
        _: &ExecutionContext,
    ) -> Result<String, String> {
        Ok("mock-req".to_string())
    }

    async fn register_event_gate(
        &self,
        _: i64,
        _: &str,
        _: &str,
        _: Option<&str>,
        _: Option<u64>,
        _: &ExecutionContext,
    ) -> Result<String, String> {
        Ok("mock-sub".to_string())
    }

    async fn launch_workflow(
        &self,
        _: &str,
        _: serde_json::Value,
        _: &ExecutionContext,
    ) -> Result<i64, String> {
        Ok(9999)
    }

    async fn schedule_task(
        &self,
        _: &ScheduleTaskDef,
        _: &ExecutionContext,
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
        Err("render_prompt_template not implemented in test executor".to_string())
    }
}

// ---------------------------------------------------------------------------
// Capturing executor for template resolution verification
// ---------------------------------------------------------------------------

struct CapturingExecutor {
    captured_calls: Arc<Mutex<Vec<(String, serde_json::Value)>>>,
}

#[async_trait::async_trait]
impl StepExecutor for CapturingExecutor {
    async fn call_tool(
        &self,
        tool_id: &str,
        args: serde_json::Value,
        _ctx: &ExecutionContext,
    ) -> Result<serde_json::Value, String> {
        self.captured_calls.lock().await.push((tool_id.to_string(), args));
        Ok(serde_json::json!({"status": "sent"}))
    }

    async fn invoke_agent(
        &self,
        _: &str,
        _: &str,
        _: bool,
        _: Option<u64>,
        _: &[PermissionEntry],
        _: Option<&str>,
        _: Option<&str>,
        _: &ExecutionContext,
    ) -> Result<serde_json::Value, String> {
        Ok(serde_json::json!({}))
    }
    async fn signal_agent(
        &self,
        _: &SignalTarget,
        _: &str,
        _: &ExecutionContext,
    ) -> Result<serde_json::Value, String> {
        Ok(serde_json::Value::Null)
    }
    async fn wait_for_agent(
        &self,
        _: &str,
        _: Option<u64>,
        _: &ExecutionContext,
    ) -> Result<serde_json::Value, String> {
        Ok(serde_json::Value::Null)
    }
    async fn create_feedback_request(
        &self,
        _: i64,
        _: &str,
        _: &str,
        _: Option<&[String]>,
        _: bool,
        _: &ExecutionContext,
    ) -> Result<String, String> {
        Ok("req".to_string())
    }
    async fn register_event_gate(
        &self,
        _: i64,
        _: &str,
        _: &str,
        _: Option<&str>,
        _: Option<u64>,
        _: &ExecutionContext,
    ) -> Result<String, String> {
        Ok("sub".to_string())
    }
    async fn launch_workflow(
        &self,
        _: &str,
        _: serde_json::Value,
        _: &ExecutionContext,
    ) -> Result<i64, String> {
        Ok(9999)
    }
    async fn schedule_task(
        &self,
        _: &ScheduleTaskDef,
        _: &ExecutionContext,
    ) -> Result<String, String> {
        Ok("task".to_string())
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

/// Verify that feedback gate responses flow through to downstream step templates.
/// This tests the complete pipeline: YAML → launch → feedback gate → respond →
/// downstream tool call receives resolved template values.
#[tokio::test]
async fn test_feedback_gate_output_flows_to_downstream_tool() {
    let yaml = r#"
name: feedback-flow-test
version: "1.0"
description: "Test feedback gate outputs in templates"
variables:
  type: object
  properties: {}
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
      prompt: 'Do you approve this?'
      choices:
        - "Yes"
        - "No"
      allow_freeform: true
    next: [notify]
  - id: notify
    type: task
    task:
      kind: call_tool
      tool_id: echo_tool
      arguments:
        choice: '{{steps.approval.outputs.selected}}'
        comment: '{{steps.approval.outputs.text}}'
        summary: 'User chose {{steps.approval.outputs.selected}} with comment: {{steps.approval.outputs.text}}'
    next: [done]
  - id: done
    type: control_flow
    control:
      kind: end_workflow
"#;

    let def: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();
    validate_definition(&def).unwrap();

    let store: Arc<dyn WorkflowPersistence> = Arc::new(WorkflowStore::in_memory().unwrap());
    let captured = Arc::new(Mutex::new(Vec::new()));
    let executor = Arc::new(CapturingExecutor { captured_calls: Arc::clone(&captured) });
    let engine = WorkflowEngine::new(Arc::clone(&store), executor, Arc::new(NullEventEmitter));

    let instance_id = engine
        .launch(def, serde_json::json!({}), "session-1".into(), None, vec![], None)
        .await
        .unwrap();

    // Should be waiting on feedback
    let instance = store.get_instance(instance_id).unwrap().unwrap();
    assert_eq!(
        instance.status,
        WorkflowStatus::WaitingOnInput,
        "expected WaitingOnInput, got {:?}",
        instance.status
    );
    assert_eq!(instance.step_states["approval"].status, StepStatus::WaitingOnInput);

    // Respond to the feedback gate (same shape as what the UI sends)
    engine
        .respond_to_gate(
            instance_id,
            "approval",
            serde_json::json!({"selected": "Yes", "text": "Looks great!"}),
        )
        .await
        .unwrap();

    // respond_to_gate spawns continuation in background — wait for it
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Verify workflow completed
    let instance = store.get_instance(instance_id).unwrap().unwrap();
    assert_eq!(
        instance.status,
        WorkflowStatus::Completed,
        "expected Completed, got {:?} error={:?}",
        instance.status,
        instance.error
    );

    // Verify the feedback gate outputs are stored
    let gate_outputs = instance.step_states["approval"]
        .outputs
        .as_ref()
        .expect("approval step should have outputs");
    assert_eq!(gate_outputs["selected"], "Yes", "gate outputs.selected");
    assert_eq!(gate_outputs["text"], "Looks great!", "gate outputs.text");

    // Verify the downstream tool was called with resolved template values
    let calls = captured.lock().await;
    assert_eq!(calls.len(), 1, "expected exactly one tool call");
    let (tool_id, args) = &calls[0];
    assert_eq!(tool_id, "echo_tool");
    assert_eq!(
        args["choice"], "Yes",
        "template {{steps.approval.outputs.selected}} should resolve"
    );
    assert_eq!(
        args["comment"], "Looks great!",
        "template {{steps.approval.outputs.text}} should resolve"
    );
    assert_eq!(
        args["summary"], "User chose Yes with comment: Looks great!",
        "mixed template should resolve"
    );
}

/// Proves that using `{{steps.gate.outputs.result}}` (old wrong path) resolves to empty,
/// while `{{steps.gate.outputs.selected}}` (correct path) resolves correctly.
/// This was the root cause: the expression helper was suggesting `result` as the
/// field name, but feedback gate responses have `selected` and `text` fields.
#[tokio::test]
async fn test_feedback_gate_wrong_reference_resolves_empty() {
    let yaml = r#"
name: feedback-wrong-ref
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
      prompt: "Choose"
      choices: ["A", "B"]
      allow_freeform: false
    next: [check]
  - id: check
    type: task
    task:
      kind: call_tool
      tool_id: test_tool
      arguments:
        wrong_ref: '{{steps.gate.outputs.result}}'
        correct_ref: '{{steps.gate.outputs.selected}}'
    next: [end]
  - id: end
    type: control_flow
    control:
      kind: end_workflow
"#;

    let def: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();
    let store: Arc<dyn WorkflowPersistence> = Arc::new(WorkflowStore::in_memory().unwrap());
    let captured = Arc::new(Mutex::new(Vec::new()));
    let executor = Arc::new(CapturingExecutor { captured_calls: Arc::clone(&captured) });
    let engine = WorkflowEngine::new(Arc::clone(&store), executor, Arc::new(NullEventEmitter));

    let id =
        engine.launch(def, serde_json::json!({}), "s".into(), None, vec![], None).await.unwrap();
    engine
        .respond_to_gate(id, "gate", serde_json::json!({"selected": "A", "text": ""}))
        .await
        .unwrap();

    // respond_to_gate spawns continuation in background — wait for it
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let calls = captured.lock().await;
    assert_eq!(calls.len(), 1);
    let (_, args) = &calls[0];
    // WRONG path: resolves to empty because there is no "result" key in {selected, text}
    assert_eq!(args["wrong_ref"], "",
        "{{steps.gate.outputs.result}} should resolve to empty — no 'result' key in feedback response");
    // CORRECT path: resolves to the actual selected choice
    assert_eq!(
        args["correct_ref"], "A",
        "{{steps.gate.outputs.selected}} should resolve to the chosen value"
    );
}

#[tokio::test]
async fn test_tool_call_template_resolution() {
    // This YAML matches what the UI designer would produce (single-quoted templates)
    let yaml = r#"
name: template-test
version: "1.0"
description: "Test template resolution in tool call args"
variables:
  type: object
  properties:
    greeting:
      type: string
      default: "Hello"
steps:
  - id: trigger
    type: trigger
    trigger:
      type: manual
      inputs:
        - name: recipient
          type: string
          required: true
        - name: message_body
          type: string
          required: true
    outputs:
      recipient: '{{result.recipient}}'
      message_body: '{{result.message_body}}'
    next: [send]
  - id: send
    type: task
    task:
      kind: call_tool
      tool_id: comm.send_external_message
      arguments:
        connector_id: test-connector
        to: '{{trigger.recipient}}'
        body: '{{variables.greeting}} {{trigger.recipient}}, {{trigger.message_body}}'
        subject: 'Notification for {{trigger.recipient}}'
    next: [done]
  - id: done
    type: control_flow
    control:
      kind: end_workflow
"#;

    let def: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();
    let store: Arc<dyn WorkflowPersistence> = Arc::new(WorkflowStore::in_memory().unwrap());
    let captured = Arc::new(Mutex::new(Vec::new()));
    let executor = Arc::new(CapturingExecutor { captured_calls: Arc::clone(&captured) });
    let engine = WorkflowEngine::new(Arc::clone(&store), executor, Arc::new(NullEventEmitter));

    let inputs = serde_json::json!({
        "recipient": "alice@example.com",
        "message_body": "Your order is ready"
    });

    let instance_id =
        engine.launch(def, inputs, "session-1".into(), None, vec![], None).await.unwrap();

    // Verify the workflow completed
    let instance = store.get_instance(instance_id).unwrap().unwrap();
    assert_eq!(
        instance.status,
        WorkflowStatus::Completed,
        "workflow should have completed, got {:?} error={:?}",
        instance.status,
        instance.error
    );

    // Verify the tool was called with resolved arguments
    let calls = captured.lock().await;
    assert_eq!(calls.len(), 1, "expected exactly one tool call");
    let (tool_id, args) = &calls[0];
    assert_eq!(tool_id, "comm.send_external_message");
    assert_eq!(args["connector_id"], "test-connector");
    assert_eq!(args["to"], "alice@example.com");
    assert_eq!(args["body"], "Hello alice@example.com, Your order is ready");
    assert_eq!(args["subject"], "Notification for alice@example.com");
}

// ===========================================================================
// ForEach / While loop tests
// ===========================================================================

/// ForEach over a 3-item array: body runs 3 times, accumulates results.
#[tokio::test]
async fn test_foreach_three_items() {
    let yaml = r#"
name: foreach-test
version: "1.0"
variables:
  type: object
  properties:
    results:
      type: array
      default: []

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
      body: [process]
    next: [end]

  - id: process
    type: task
    task:
      kind: set_variable
      assignments:
        - variable: results
          operation: append_list
          value: "processed-{{item}}"
    next: []

  - id: end
    type: control_flow
    control:
      kind: end_workflow

output:
  all_results: "{{variables.results}}"
"#;

    let definition: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();
    validate_definition(&definition).unwrap();

    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let executor = Arc::new(EchoExecutor);
    let emitter = Arc::new(NullEventEmitter);
    let engine = WorkflowEngine::new(store.clone(), executor, emitter);

    let instance_id = engine
        .launch(
            definition,
            serde_json::json!({"items": ["a", "b", "c"]}),
            "test-session".into(),
            None,
            vec![],
            None,
        )
        .await
        .unwrap();

    let instance = store.get_instance(instance_id).unwrap().unwrap();
    assert_eq!(instance.status, WorkflowStatus::Completed);
    assert_eq!(instance.step_states["loop"].status, StepStatus::Completed);
    assert_eq!(instance.step_states["process"].status, StepStatus::Completed);

    // Check accumulated results
    let results = instance.variables.get("results").unwrap().as_array().unwrap();
    assert_eq!(results.len(), 3);
    assert_eq!(results[0], "processed-a");
    assert_eq!(results[1], "processed-b");
    assert_eq!(results[2], "processed-c");
}

/// ForEach over an empty array: body never runs, next steps proceed.
#[tokio::test]
async fn test_foreach_empty_array() {
    let yaml = r#"
name: foreach-empty
version: "1.0"
variables:
  type: object
  properties:
    ran:
      type: boolean
      default: false

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
      body: [process]
    next: [end]

  - id: process
    type: task
    task:
      kind: set_variable
      assignments:
        - variable: ran
          value: "true"
    next: []

  - id: end
    type: control_flow
    control:
      kind: end_workflow

output:
  body_ran: "{{variables.ran}}"
"#;

    let definition: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();
    validate_definition(&definition).unwrap();

    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let executor = Arc::new(EchoExecutor);
    let emitter = Arc::new(NullEventEmitter);
    let engine = WorkflowEngine::new(store.clone(), executor, emitter);

    let instance_id = engine
        .launch(
            definition,
            serde_json::json!({"items": []}),
            "test-session".into(),
            None,
            vec![],
            None,
        )
        .await
        .unwrap();

    let instance = store.get_instance(instance_id).unwrap().unwrap();
    assert_eq!(instance.status, WorkflowStatus::Completed);
    assert_eq!(instance.step_states["loop"].status, StepStatus::Completed);
    // Body step should be Pending still (never ran) or Skipped
    let process_status = instance.step_states["process"].status;
    assert!(
        matches!(process_status, StepStatus::Pending | StepStatus::Skipped),
        "expected Pending or Skipped, got {:?}",
        process_status
    );
    // Variable should still be false
    assert_eq!(instance.variables["ran"], false);
}

/// ForEach sets item_var and item_var_index correctly each iteration.
#[tokio::test]
async fn test_foreach_item_var_and_index() {
    let yaml = r#"
name: foreach-index
version: "1.0"
variables:
  type: object
  properties:
    log:
      type: array
      default: []

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
      collection: "{{trigger.names}}"
      item_var: name
      body: [log_it]
    next: [end]

  - id: log_it
    type: task
    task:
      kind: set_variable
      assignments:
        - variable: log
          operation: append_list
          value: "{{name_index}}:{{name}}"
    next: []

  - id: end
    type: control_flow
    control:
      kind: end_workflow
"#;

    let definition: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();
    validate_definition(&definition).unwrap();

    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let executor = Arc::new(EchoExecutor);
    let emitter = Arc::new(NullEventEmitter);
    let engine = WorkflowEngine::new(store.clone(), executor, emitter);

    let instance_id = engine
        .launch(
            definition,
            serde_json::json!({"names": ["alice", "bob"]}),
            "test-session".into(),
            None,
            vec![],
            None,
        )
        .await
        .unwrap();

    let instance = store.get_instance(instance_id).unwrap().unwrap();
    assert_eq!(instance.status, WorkflowStatus::Completed);

    let log = instance.variables.get("log").unwrap().as_array().unwrap();
    assert_eq!(log.len(), 2);
    assert_eq!(log[0], "0:alice");
    assert_eq!(log[1], "1:bob");
}

/// While with counter: loops until a flag is set after enough iterations.
/// Uses append_list to track iterations and max_iterations as safety limit.
#[tokio::test]
async fn test_while_counter() {
    let yaml = r#"
name: while-counter
version: "1.0"
variables:
  type: object
  properties:
    keep_going:
      type: boolean
      default: true
    ticks:
      type: array
      default: []

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
      kind: while
      condition: "{{variables.keep_going}}"
      max_iterations: 3
      body: [tick]
    next: [end]

  - id: tick
    type: task
    task:
      kind: set_variable
      assignments:
        - variable: ticks
          operation: append_list
          value: "tick"
    next: []

  - id: end
    type: control_flow
    control:
      kind: end_workflow

output:
  tick_count: "{{variables.ticks}}"
"#;

    let definition: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();
    validate_definition(&definition).unwrap();

    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let executor = Arc::new(EchoExecutor);
    let emitter = Arc::new(NullEventEmitter);
    let engine = WorkflowEngine::new(store.clone(), executor, emitter);

    let instance_id = engine
        .launch(definition, serde_json::json!({}), "test-session".into(), None, vec![], None)
        .await
        .unwrap();

    let instance = store.get_instance(instance_id).unwrap().unwrap();
    assert_eq!(instance.status, WorkflowStatus::Completed);
    assert_eq!(instance.step_states["loop"].status, StepStatus::Completed);
    // Should have 3 ticks (max_iterations)
    let ticks = instance.variables["ticks"].as_array().unwrap();
    assert_eq!(ticks.len(), 3);
}

/// While with initially false condition: body never runs.
#[tokio::test]
async fn test_while_false_initial() {
    let yaml = r#"
name: while-false
version: "1.0"
variables:
  type: object
  properties:
    flag:
      type: boolean
      default: false
    ticks:
      type: array
      default: []

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
      kind: while
      condition: "{{variables.flag}}"
      body: [tick]
    next: [end]

  - id: tick
    type: task
    task:
      kind: set_variable
      assignments:
        - variable: ticks
          operation: append_list
          value: "tick"
    next: []

  - id: end
    type: control_flow
    control:
      kind: end_workflow
"#;

    let definition: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();
    validate_definition(&definition).unwrap();

    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let executor = Arc::new(EchoExecutor);
    let emitter = Arc::new(NullEventEmitter);
    let engine = WorkflowEngine::new(store.clone(), executor, emitter);

    let instance_id = engine
        .launch(definition, serde_json::json!({}), "test-session".into(), None, vec![], None)
        .await
        .unwrap();

    let instance = store.get_instance(instance_id).unwrap().unwrap();
    assert_eq!(instance.status, WorkflowStatus::Completed);
    assert_eq!(instance.step_states["loop"].status, StepStatus::Completed);
    // Body never ran
    let ticks = instance.variables["ticks"].as_array().unwrap();
    assert_eq!(ticks.len(), 0);
}

/// While with max_iterations safety limit.
#[tokio::test]
async fn test_while_max_iterations() {
    let yaml = r#"
name: while-max
version: "1.0"
variables:
  type: object
  properties:
    ticks:
      type: array
      default: []

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
      kind: while
      condition: "true"
      max_iterations: 5
      body: [tick]
    next: [end]

  - id: tick
    type: task
    task:
      kind: set_variable
      assignments:
        - variable: ticks
          operation: append_list
          value: "tick"
    next: []

  - id: end
    type: control_flow
    control:
      kind: end_workflow
"#;

    let definition: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();
    validate_definition(&definition).unwrap();

    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let executor = Arc::new(EchoExecutor);
    let emitter = Arc::new(NullEventEmitter);
    let engine = WorkflowEngine::new(store.clone(), executor, emitter);

    let instance_id = engine
        .launch(definition, serde_json::json!({}), "test-session".into(), None, vec![], None)
        .await
        .unwrap();

    let instance = store.get_instance(instance_id).unwrap().unwrap();
    assert_eq!(instance.status, WorkflowStatus::Completed);
    // Max 5 iterations, condition is always true
    let ticks = instance.variables["ticks"].as_array().unwrap();
    assert_eq!(ticks.len(), 5);
}

/// ForEach with multi-step body (sequential steps within the body).
#[tokio::test]
async fn test_foreach_multi_step_body() {
    let yaml = r#"
name: foreach-multi-body
version: "1.0"
variables:
  type: object
  properties:
    log:
      type: array
      default: []

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
      item_var: x
      body: [step_a, step_b]
    next: [end]

  - id: step_a
    type: task
    task:
      kind: call_tool
      tool_id: echo_tool
      arguments:
        message: "{{x}}"
    outputs:
      echoed: "{{result.echo}}"
    next: [step_b]

  - id: step_b
    type: task
    task:
      kind: set_variable
      assignments:
        - variable: log
          operation: append_list
          value: "echoed:{{steps.step_a.outputs.echoed}}"
    next: []

  - id: end
    type: control_flow
    control:
      kind: end_workflow
"#;

    let definition: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();
    validate_definition(&definition).unwrap();

    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let executor = Arc::new(EchoExecutor);
    let emitter = Arc::new(NullEventEmitter);
    let engine = WorkflowEngine::new(store.clone(), executor, emitter);

    let instance_id = engine
        .launch(
            definition,
            serde_json::json!({"items": ["hello", "world"]}),
            "test-session".into(),
            None,
            vec![],
            None,
        )
        .await
        .unwrap();

    let instance = store.get_instance(instance_id).unwrap().unwrap();
    assert_eq!(instance.status, WorkflowStatus::Completed);

    let log = instance.variables.get("log").unwrap().as_array().unwrap();
    assert_eq!(log.len(), 2);
    assert_eq!(log[0], "echoed:hello");
    assert_eq!(log[1], "echoed:world");
}

/// ForEach with non-array collection value produces error.
#[tokio::test]
async fn test_foreach_non_array_collection_fails() {
    let yaml = r#"
name: foreach-bad-coll
version: "1.0"
variables:
  type: object
  properties: {}

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
      collection: "{{trigger.value}}"
      item_var: item
      body: [process]
    next: [end]

  - id: process
    type: task
    task:
      kind: delay
      duration_secs: 0
    next: []

  - id: end
    type: control_flow
    control:
      kind: end_workflow
"#;

    let definition: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();
    validate_definition(&definition).unwrap();

    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let executor = Arc::new(EchoExecutor);
    let emitter = Arc::new(NullEventEmitter);
    let engine = WorkflowEngine::new(store.clone(), executor, emitter);

    let instance_id = engine
        .launch(
            definition,
            serde_json::json!({"value": "not-an-array"}),
            "test-session".into(),
            None,
            vec![],
            None,
        )
        .await
        .unwrap();

    let instance = store.get_instance(instance_id).unwrap().unwrap();
    assert_eq!(instance.status, WorkflowStatus::Failed);
    assert!(instance.error.as_ref().unwrap().contains("must be an array"));
}

/// Persistence: loop state survives save/load cycle.
#[tokio::test]
async fn test_loop_state_persistence() {
    // Verify that LoopState serializes/deserializes correctly
    let loop_state = LoopState {
        iteration: 2,
        collection: Some(vec![
            serde_json::json!("a"),
            serde_json::json!("b"),
            serde_json::json!("c"),
        ]),
        item_var: Some("item".to_string()),
        body_step_ids: vec!["body1".to_string(), "body2".to_string()],
        max_iterations: None,
        preview_paused: false,
        preview_results: None,
    };

    let json = serde_json::to_string(&loop_state).unwrap();
    let deserialized: LoopState = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.iteration, 2);
    assert_eq!(deserialized.collection.as_ref().unwrap().len(), 3);
    assert_eq!(deserialized.item_var.as_ref().unwrap(), "item");
    assert_eq!(deserialized.body_step_ids.len(), 2);
    assert!(deserialized.max_iterations.is_none());

    // Verify LoopWaiting status serializes correctly
    let status = StepStatus::LoopWaiting;
    let status_json = serde_json::to_value(&status).unwrap();
    assert_eq!(status_json, "loop_waiting");
    let deserialized_status: StepStatus = serde_json::from_value(status_json).unwrap();
    assert_eq!(deserialized_status, StepStatus::LoopWaiting);
}
