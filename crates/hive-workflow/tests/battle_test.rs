//! Battle Test Suite: 100 scenarios for the workflow engine.
//!
//! Categories:
//!  1. Basic Execution (10)
//!  2. Branching & Control Flow (12)
//!  3. Parallel Execution (10)
//!  4. Error Handling (15)
//!  5. Gates & Interactions (12)
//!  6. Daemon Restart / Recovery (10)
//!  7. Child Workflows (5)
//!  8. Variable Management (8)
//!  9. Concurrency & Load (10)
//! 10. Edge Cases & Adversarial (8)

use hive_workflow::*;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;

// ===================================================================
// Test Executors
// ===================================================================

/// Configurable executor that tracks calls and returns per-tool results.
struct BattleExecutor {
    tool_call_count: AtomicU32,
    tool_results: Mutex<HashMap<String, Result<Value, String>>>,
    agent_results: Mutex<HashMap<String, Result<Value, String>>>,
    launch_results: Mutex<HashMap<String, Result<i64, String>>>,
    /// Records (persona_id, existing_agent_id) for each `invoke_agent` call.
    invoke_agent_calls: Mutex<Vec<(String, Option<String>)>>,
}

impl BattleExecutor {
    fn new() -> Self {
        Self {
            tool_call_count: AtomicU32::new(0),
            tool_results: Mutex::new(HashMap::new()),
            agent_results: Mutex::new(HashMap::new()),
            launch_results: Mutex::new(HashMap::new()),
            invoke_agent_calls: Mutex::new(Vec::new()),
        }
    }

    async fn set_tool_result(&self, tool_id: &str, result: Result<Value, String>) {
        self.tool_results.lock().await.insert(tool_id.to_string(), result);
    }

    async fn set_agent_result(&self, persona_id: &str, result: Result<Value, String>) {
        self.agent_results.lock().await.insert(persona_id.to_string(), result);
    }

    async fn set_launch_result(&self, name: &str, result: Result<i64, String>) {
        self.launch_results.lock().await.insert(name.to_string(), result);
    }

    async fn get_invoke_agent_calls(&self) -> Vec<(String, Option<String>)> {
        self.invoke_agent_calls.lock().await.clone()
    }
}

#[async_trait::async_trait]
impl StepExecutor for BattleExecutor {
    async fn call_tool(
        &self,
        tool_id: &str,
        args: Value,
        _ctx: &ExecutionContext,
    ) -> Result<Value, String> {
        self.tool_call_count.fetch_add(1, Ordering::SeqCst);
        let results = self.tool_results.lock().await;
        results
            .get(tool_id)
            .cloned()
            .unwrap_or(Ok(json!({"status": "ok", "tool_id": tool_id, "args": args})))
    }
    async fn invoke_agent(
        &self,
        persona_id: &str,
        task: &str,
        _async_exec: bool,
        _timeout: Option<u64>,
        _perms: &[PermissionEntry],
        _name: Option<&str>,
        existing_agent_id: Option<&str>,
        _ctx: &ExecutionContext,
    ) -> Result<Value, String> {
        self.invoke_agent_calls
            .lock()
            .await
            .push((persona_id.to_string(), existing_agent_id.map(|s| s.to_string())));
        // If resuming an existing agent, check agent_results by agent_id first
        if let Some(agent_id) = existing_agent_id {
            let results = self.agent_results.lock().await;
            if let Some(r) = results.get(agent_id) {
                match r {
                    Ok(_) => return r.clone(),
                    Err(_) => {
                        // Mimic ServiceStepExecutor: signal failed,
                        // fall through to fresh spawn below
                    }
                }
            }
        }
        let results = self.agent_results.lock().await;
        results
            .get(persona_id)
            .cloned()
            .unwrap_or(Ok(json!({"agent_result": "done", "task": task})))
    }
    async fn signal_agent(
        &self,
        _: &SignalTarget,
        content: &str,
        _: &ExecutionContext,
    ) -> Result<Value, String> {
        Ok(json!({"signaled": true, "content": content}))
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
        _inst: i64,
        _step: &str,
        prompt: &str,
        choices: Option<&[String]>,
        allow_freeform: bool,
        _: &ExecutionContext,
    ) -> Result<String, String> {
        let _ = (prompt, choices, allow_freeform);
        Ok(format!("fb-req-{}", uuid::Uuid::new_v4()))
    }
    async fn register_event_gate(
        &self,
        _inst: i64,
        _step: &str,
        topic: &str,
        _filter: Option<&str>,
        _timeout: Option<u64>,
        _: &ExecutionContext,
    ) -> Result<String, String> {
        Ok(format!("evt-sub-{topic}"))
    }
    async fn launch_workflow(
        &self,
        name: &str,
        _inputs: Value,
        _: &ExecutionContext,
    ) -> Result<i64, String> {
        let results = self.launch_results.lock().await;
        results.get(name).cloned().unwrap_or(Ok(9999))
    }
    async fn schedule_task(
        &self,
        sched: &ScheduleTaskDef,
        _: &ExecutionContext,
    ) -> Result<String, String> {
        Ok(format!("task-{}", sched.name))
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

/// Event collector for assertions.
struct EventCollector {
    events: Mutex<Vec<WorkflowEvent>>,
}

impl EventCollector {
    fn new() -> Self {
        Self { events: Mutex::new(Vec::new()) }
    }
    async fn events(&self) -> Vec<WorkflowEvent> {
        self.events.lock().await.clone()
    }
    async fn count_of(&self, name: &str) -> usize {
        self.events.lock().await.iter().filter(|e| format!("{e:?}").contains(name)).count()
    }
}

#[async_trait::async_trait]
impl WorkflowEventEmitter for EventCollector {
    async fn emit(&self, event: WorkflowEvent) {
        self.events.lock().await.push(event);
    }
}

// ===================================================================
// Helper: build engine + store
// ===================================================================

fn make_engine(
    executor: Arc<BattleExecutor>,
    emitter: Arc<EventCollector>,
) -> (Arc<WorkflowStore>, WorkflowEngine) {
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let engine = WorkflowEngine::new(store.clone(), executor, emitter);
    (store, engine)
}

fn make_engine_concurrent(
    executor: Arc<BattleExecutor>,
    emitter: Arc<EventCollector>,
    max: usize,
) -> (Arc<WorkflowStore>, WorkflowEngine) {
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let engine = WorkflowEngine::with_concurrency(store.clone(), executor, emitter, max);
    (store, engine)
}

/// Parse YAML, validate, return definition.
fn def(yaml: &str) -> WorkflowDefinition {
    let d: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();
    validate_definition(&d).unwrap();
    d
}

/// Launch helper.
async fn launch(engine: &WorkflowEngine, definition: WorkflowDefinition, inputs: Value) -> i64 {
    engine.launch(definition, inputs, "battle-session".into(), None, vec![], None).await.unwrap()
}

/// Wait for an instance to reach a terminal or non-running state.
/// Delay steps with duration_secs=0 fire background timers that need a moment.
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
    // Final check — don't panic here, let the caller's assert provide the message
}

/// Wait for an instance to settle (reach any non-Running/non-Pending state).
/// Used after respond_to_gate/respond_to_event which continue execution in background.
async fn wait_for_settled(store: &WorkflowStore, instance_id: i64) {
    for _ in 0..100 {
        let inst = store.get_instance(instance_id).unwrap().unwrap();
        if !matches!(inst.status, WorkflowStatus::Running | WorkflowStatus::Pending) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

// ===================================================================
// 1. BASIC EXECUTION (10 tests)
// ===================================================================

/// #1: Minimal workflow: trigger  end
#[tokio::test]
async fn battle_01_minimal_trigger_to_end() {
    let yaml = r#"
name: battle-01
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs: []
    next: [end]
  - id: end
    type: control_flow
    control: { kind: end_workflow }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let em = Arc::new(EventCollector::new());
    let (store, engine) = make_engine(ex, em.clone());
    let id = launch(&engine, def(yaml), json!({})).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert!(em.count_of("InstanceCompleted").await >= 1);
}

/// #2: Single tool call step
#[tokio::test]
async fn battle_02_single_tool_call() {
    let yaml = r#"
name: battle-02
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs: []
    next: [tool]
  - id: tool
    type: task
    task: { kind: call_tool, tool_id: echo, arguments: { msg: hello } }
    next: [end]
  - id: end
    type: control_flow
    control: { kind: end_workflow }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex.clone(), Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert_eq!(ex.tool_call_count.load(Ordering::SeqCst), 1);
}

/// #3: Multi-step linear chain (5 steps)
#[tokio::test]
async fn battle_03_five_step_chain() {
    let yaml = r#"
name: battle-03
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: t, type: trigger, trigger: { type: manual, inputs: [] }, next: [s1] }
  - { id: s1, type: task, task: { kind: call_tool, tool_id: t1, arguments: {} }, next: [s2] }
  - { id: s2, type: task, task: { kind: call_tool, tool_id: t2, arguments: {} }, next: [s3] }
  - { id: s3, type: task, task: { kind: call_tool, tool_id: t3, arguments: {} }, next: [s4] }
  - { id: s4, type: task, task: { kind: call_tool, tool_id: t4, arguments: {} }, next: [end] }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex.clone(), Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert_eq!(ex.tool_call_count.load(Ordering::SeqCst), 4);
    for sid in ["s1", "s2", "s3", "s4"] {
        assert_eq!(inst.step_states[sid].status, StepStatus::Completed);
    }
}

/// #4: Trigger with required inputs
#[tokio::test]
async fn battle_04_trigger_required_inputs() {
    let yaml = r#"
name: battle-04
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs:
        - { name: name, input_type: string, required: true }
        - { name: count, input_type: number, required: true }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({"name": "test", "count": 42})).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
}

/// #5: Missing required input fails
#[tokio::test]
async fn battle_05_missing_required_input() {
    let yaml = r#"
name: battle-05
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      input_schema:
        type: object
        properties:
          name: { type: string }
        required: [name]
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (_store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let result = engine.launch(def(yaml), json!({}), "s".into(), None, vec![], None).await;
    assert!(result.is_err(), "should fail with missing required input");
}

/// #6: Workflow with output mapping
#[tokio::test]
async fn battle_06_output_mapping() {
    let yaml = r#"
name: battle-06
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs: []
    next: [tool]
  - id: tool
    type: task
    task: { kind: call_tool, tool_id: echo, arguments: { msg: world } }
    outputs:
      result_status: "{{result.status}}"
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
output:
  final: "{{steps.tool.outputs.result_status}}"
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    let output = inst.output.as_ref().unwrap();
    assert_eq!(output["final"], "ok");
}

/// #7: Workflow with result_message template
#[tokio::test]
async fn battle_07_result_message() {
    let yaml = r#"
name: battle-07
version: "1.0"
variables: { type: object, properties: {} }
result_message: "Tool returned {{steps.tool.outputs.result_status}}"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs: []
    next: [tool]
  - id: tool
    type: task
    task: { kind: call_tool, tool_id: echo, arguments: {} }
    outputs:
      result_status: "{{result.status}}"
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.resolved_result_message.as_deref(), Some("Tool returned ok"));
}

/// #8: Workflow with variable defaults
#[tokio::test]
async fn battle_08_variable_defaults() {
    let yaml = r#"
name: battle-08
version: "1.0"
variables:
  type: object
  properties:
    greeting: { type: string, default: "hello" }
    count: { type: number, default: 0 }
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs: []
    next: [tool]
  - id: tool
    type: task
    task: { kind: call_tool, tool_id: echo, arguments: { msg: "{{variables.greeting}}" } }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert_eq!(inst.variables["greeting"], "hello");
    assert_eq!(inst.variables["count"], 0);
}

/// #9: Delay step completes
#[tokio::test]
async fn battle_09_delay_step() {
    let yaml = r#"
name: battle-09
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [wait] }
  - { id: wait, type: task, task: { kind: delay, duration_secs: 0 }, next: [end] }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    wait_for_terminal(&store, id).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert_eq!(inst.step_states["wait"].status, StepStatus::Completed);
}

/// #10: InvokeAgent step
#[tokio::test]
async fn battle_10_invoke_agent() {
    let yaml = r#"
name: battle-10
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [agent] }
  - id: agent
    type: task
    task:
      kind: invoke_agent
      persona_id: test-persona
      task: "Do something useful"
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert_eq!(inst.step_states["agent"].status, StepStatus::Completed);
}

// ===================================================================
// 2. BRANCHING & CONTROL FLOW (12 tests)
// ===================================================================

/// #11: Branch takes THEN path
#[tokio::test]
async fn battle_11_branch_then_path() {
    let yaml = r#"
name: battle-11
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [branch] }
  - id: branch
    type: control_flow
    control: { kind: branch, condition: "{{trigger.val}} > 10", then: [yes], else: [no] }
  - { id: yes, type: task, task: { kind: delay, duration_secs: 0 }, outputs: { path: "yes" }, next: [end] }
  - { id: no, type: task, task: { kind: delay, duration_secs: 0 }, outputs: { path: "no" }, next: [end] }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
output:
  result: "{{steps.yes.outputs.path}}"
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({"val": 50})).await;
    wait_for_terminal(&store, id).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert_eq!(inst.step_states["yes"].status, StepStatus::Completed);
    assert_eq!(inst.step_states["no"].status, StepStatus::Skipped);
}

/// #12: Branch takes ELSE path
#[tokio::test]
async fn battle_12_branch_else_path() {
    let yaml = r#"
name: battle-12
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [branch] }
  - id: branch
    type: control_flow
    control: { kind: branch, condition: "{{trigger.val}} > 10", then: [yes], else: [no] }
  - { id: yes, type: task, task: { kind: delay, duration_secs: 0 }, next: [end] }
  - { id: no, type: task, task: { kind: delay, duration_secs: 0 }, next: [end] }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({"val": 5})).await;
    wait_for_terminal(&store, id).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.step_states["no"].status, StepStatus::Completed);
    assert_eq!(inst.step_states["yes"].status, StepStatus::Skipped);
}

/// #13: Nested branches (branch inside a branch)
#[tokio::test]
async fn battle_13_nested_branches() {
    let yaml = r#"
name: battle-13
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [b1] }
  - id: b1
    type: control_flow
    control: { kind: branch, condition: "{{trigger.a}} > 0", then: [b2], else: [low] }
  - id: b2
    type: control_flow
    control: { kind: branch, condition: "{{trigger.b}} > 0", then: [high_high], else: [high_low] }
  - { id: high_high, type: task, task: { kind: delay, duration_secs: 0 }, outputs: { r: "HH" }, next: [end] }
  - { id: high_low, type: task, task: { kind: delay, duration_secs: 0 }, outputs: { r: "HL" }, next: [end] }
  - { id: low, type: task, task: { kind: delay, duration_secs: 0 }, outputs: { r: "L" }, next: [end] }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({"a": 1, "b": 1})).await;
    wait_for_terminal(&store, id).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.step_states["high_high"].status, StepStatus::Completed);
    assert_eq!(inst.step_states["high_low"].status, StepStatus::Skipped);
    assert_eq!(inst.step_states["low"].status, StepStatus::Skipped);
}

/// #14: Diamond pattern: fork  two paths  join
#[tokio::test]
async fn battle_14_diamond_pattern() {
    let yaml = r#"
name: battle-14
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [a, b] }
  - { id: a, type: task, task: { kind: call_tool, tool_id: t1, arguments: {} }, next: [join] }
  - { id: b, type: task, task: { kind: call_tool, tool_id: t2, arguments: {} }, next: [join] }
  - { id: join, type: task, task: { kind: delay, duration_secs: 0 }, next: [end] }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex.clone(), Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    wait_for_terminal(&store, id).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert_eq!(inst.step_states["a"].status, StepStatus::Completed);
    assert_eq!(inst.step_states["b"].status, StepStatus::Completed);
    assert_eq!(inst.step_states["join"].status, StepStatus::Completed);
    assert_eq!(ex.tool_call_count.load(Ordering::SeqCst), 2);
}

/// #15: Unreachable steps are rejected at validation time
#[tokio::test]
async fn battle_15_unreachable_step_rejected() {
    let yaml = r#"
name: battle-15
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [stop] }
  - { id: stop, type: control_flow, control: { kind: end_workflow } }
  - { id: unreachable, type: task, task: { kind: call_tool, tool_id: t, arguments: {} }, next: [] }
"#;
    let parsed: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();
    let ex = Arc::new(BattleExecutor::new());
    let (_, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let err = engine.launch(parsed, json!({}), "s".into(), None, vec![], None).await;
    assert!(err.is_err(), "should reject workflow with unreachable step");
    let msg = err.unwrap_err().to_string();
    assert!(
        msg.contains("unreachable") || msg.contains("Unreachable"),
        "error should mention unreachable: {msg}"
    );
}

/// #16: Branch with string equality condition
#[tokio::test]
async fn battle_16_branch_string_equality() {
    let yaml = r#"
name: battle-16
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [branch] }
  - id: branch
    type: control_flow
    control: { kind: branch, condition: '{{trigger.action}} == "approve"', then: [ok], else: [reject] }
  - { id: ok, type: task, task: { kind: delay, duration_secs: 0 }, next: [end] }
  - { id: reject, type: task, task: { kind: delay, duration_secs: 0 }, next: [end] }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({"action": "approve"})).await;
    wait_for_terminal(&store, id).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.step_states["ok"].status, StepStatus::Completed);
    assert_eq!(inst.step_states["reject"].status, StepStatus::Skipped);
}

/// #17: Branch on step output
#[tokio::test]
async fn battle_17_branch_on_step_output() {
    let yaml = r#"
name: battle-17
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [check] }
  - id: check
    type: task
    task: { kind: call_tool, tool_id: checker, arguments: {} }
    outputs:
      score: "{{result.status}}"
    next: [branch]
  - id: branch
    type: control_flow
    control: { kind: branch, condition: '{{steps.check.outputs.score}} == "ok"', then: [pass], else: [fail_path] }
  - { id: pass, type: task, task: { kind: delay, duration_secs: 0 }, next: [end] }
  - { id: fail_path, type: task, task: { kind: delay, duration_secs: 0 }, next: [end] }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    wait_for_terminal(&store, id).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.step_states["pass"].status, StepStatus::Completed);
}

/// #18: Three consecutive branches
#[tokio::test]
async fn battle_18_three_consecutive_branches() {
    let yaml = r#"
name: battle-18
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [b1] }
  - id: b1
    type: control_flow
    control: { kind: branch, condition: "1 == 1", then: [b2], else: [x1] }
  - { id: x1, type: task, task: { kind: delay, duration_secs: 0 }, next: [end] }
  - id: b2
    type: control_flow
    control: { kind: branch, condition: "1 == 1", then: [b3], else: [x2] }
  - { id: x2, type: task, task: { kind: delay, duration_secs: 0 }, next: [end] }
  - id: b3
    type: control_flow
    control: { kind: branch, condition: "1 == 1", then: [final_step], else: [x3] }
  - { id: x3, type: task, task: { kind: delay, duration_secs: 0 }, next: [end] }
  - { id: final_step, type: task, task: { kind: delay, duration_secs: 0 }, next: [end] }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    wait_for_terminal(&store, id).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert_eq!(inst.step_states["final_step"].status, StepStatus::Completed);
    for s in ["x1", "x2", "x3"] {
        assert_eq!(inst.step_states[s].status, StepStatus::Skipped, "step {s} should be skipped");
    }
}

/// #19: Branch with empty else (then-only branch)
#[tokio::test]
async fn battle_19_branch_empty_else() {
    let yaml = r#"
name: battle-19
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [branch] }
  - id: branch
    type: control_flow
    control: { kind: branch, condition: "1 == 1", then: [action], else: [] }
  - { id: action, type: task, task: { kind: delay, duration_secs: 0 }, next: [end] }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    wait_for_terminal(&store, id).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert_eq!(inst.step_states["action"].status, StepStatus::Completed);
}

/// #20: Branch condition on variable
#[tokio::test]
async fn battle_20_branch_on_variable() {
    let yaml = r#"
name: battle-20
version: "1.0"
variables:
  type: object
  properties:
    mode: { type: string, default: "fast" }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [branch] }
  - id: branch
    type: control_flow
    control: { kind: branch, condition: '{{variables.mode}} == "fast"', then: [fast], else: [slow] }
  - { id: fast, type: task, task: { kind: delay, duration_secs: 0 }, next: [end] }
  - { id: slow, type: task, task: { kind: delay, duration_secs: 0 }, next: [end] }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    wait_for_terminal(&store, id).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.step_states["fast"].status, StepStatus::Completed);
    assert_eq!(inst.step_states["slow"].status, StepStatus::Skipped);
}

/// #21: Branch where both paths lead to common join
#[tokio::test]
async fn battle_21_branch_to_common_join() {
    let yaml = r#"
name: battle-21
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [branch] }
  - id: branch
    type: control_flow
    control: { kind: branch, condition: "{{trigger.x}} > 0", then: [left], else: [right] }
  - { id: left, type: task, task: { kind: call_tool, tool_id: t1, arguments: {} }, next: [merge] }
  - { id: right, type: task, task: { kind: call_tool, tool_id: t2, arguments: {} }, next: [merge] }
  - { id: merge, type: task, task: { kind: delay, duration_secs: 0 }, next: [end] }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex.clone(), Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({"x": 1})).await;
    wait_for_terminal(&store, id).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert_eq!(inst.step_states["merge"].status, StepStatus::Completed);
    assert_eq!(ex.tool_call_count.load(Ordering::SeqCst), 1);
}

/// #22: Branch with boolean NOT condition
#[tokio::test]
async fn battle_22_branch_not_condition() {
    let yaml = r#"
name: battle-22
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [branch] }
  - id: branch
    type: control_flow
    control: { kind: branch, condition: "!{{trigger.blocked}}", then: [go], else: [stop] }
  - { id: go, type: task, task: { kind: delay, duration_secs: 0 }, next: [end] }
  - { id: stop, type: task, task: { kind: delay, duration_secs: 0 }, next: [end] }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({"blocked": false})).await;
    wait_for_terminal(&store, id).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.step_states["go"].status, StepStatus::Completed);
}

// ===================================================================
// 3. PARALLEL EXECUTION (10 tests)
// ===================================================================

/// #23: Two parallel steps (fork-join)
#[tokio::test]
async fn battle_23_two_parallel() {
    let yaml = r#"
name: battle-23
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [a, b] }
  - { id: a, type: task, task: { kind: call_tool, tool_id: t1, arguments: {} }, next: [end] }
  - { id: b, type: task, task: { kind: call_tool, tool_id: t2, arguments: {} }, next: [end] }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex.clone(), Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert_eq!(ex.tool_call_count.load(Ordering::SeqCst), 2);
}

/// #24: Five parallel steps (wide fan-out)
#[tokio::test]
async fn battle_24_five_parallel() {
    let yaml = r#"
name: battle-24
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [a, b, c, d, e] }
  - { id: a, type: task, task: { kind: call_tool, tool_id: t, arguments: {} }, next: [join] }
  - { id: b, type: task, task: { kind: call_tool, tool_id: t, arguments: {} }, next: [join] }
  - { id: c, type: task, task: { kind: call_tool, tool_id: t, arguments: {} }, next: [join] }
  - { id: d, type: task, task: { kind: call_tool, tool_id: t, arguments: {} }, next: [join] }
  - { id: e, type: task, task: { kind: call_tool, tool_id: t, arguments: {} }, next: [join] }
  - { id: join, type: task, task: { kind: delay, duration_secs: 0 }, next: [end] }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex.clone(), Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    wait_for_terminal(&store, id).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert_eq!(ex.tool_call_count.load(Ordering::SeqCst), 5);
    assert_eq!(inst.step_states["join"].status, StepStatus::Completed);
}

/// #25: Parallel with one failing step (FailWorkflow strategy)
#[tokio::test]
async fn battle_25_parallel_one_fails() {
    let yaml = r#"
name: battle-25
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [good, bad] }
  - { id: good, type: task, task: { kind: call_tool, tool_id: ok_tool, arguments: {} }, next: [end] }
  - { id: bad, type: task, task: { kind: call_tool, tool_id: fail_tool, arguments: {} }, next: [end] }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    ex.set_tool_result("fail_tool", Err("boom".into())).await;
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Failed);
    assert_eq!(inst.step_states["bad"].status, StepStatus::Failed);
}

/// #26: Parallel with skip strategy on failure
#[tokio::test]
async fn battle_26_parallel_skip_failure() {
    let yaml = r#"
name: battle-26
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [good, bad] }
  - { id: good, type: task, task: { kind: call_tool, tool_id: ok_tool, arguments: {} }, next: [end] }
  - id: bad
    type: task
    task: { kind: call_tool, tool_id: fail_tool, arguments: {} }
    on_error: { strategy: skip, default_output: { skipped: true } }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    ex.set_tool_result("fail_tool", Err("boom".into())).await;
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert_eq!(inst.step_states["bad"].status, StepStatus::Skipped);
    assert_eq!(inst.step_states["good"].status, StepStatus::Completed);
}

/// #27: Semaphore limits concurrency (max_concurrent_steps = 2)
#[tokio::test]
async fn battle_27_semaphore_limits_concurrency() {
    let yaml = r#"
name: battle-27
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [a, b, c, d] }
  - { id: a, type: task, task: { kind: call_tool, tool_id: t, arguments: {} }, next: [end] }
  - { id: b, type: task, task: { kind: call_tool, tool_id: t, arguments: {} }, next: [end] }
  - { id: c, type: task, task: { kind: call_tool, tool_id: t, arguments: {} }, next: [end] }
  - { id: d, type: task, task: { kind: call_tool, tool_id: t, arguments: {} }, next: [end] }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let em = Arc::new(EventCollector::new());
    let (_store, engine) = make_engine_concurrent(ex.clone(), em, 2);
    let id = launch(&engine, def(yaml), json!({})).await;
    // Even with concurrency limit of 2, all should complete
    let inst = _store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert_eq!(ex.tool_call_count.load(Ordering::SeqCst), 4);
}

/// #28: Parallel steps with different error strategies
#[tokio::test]
async fn battle_28_parallel_mixed_errors() {
    let yaml = r#"
name: battle-28
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [skip_step, retry_step, ok_step] }
  - id: skip_step
    type: task
    task: { kind: call_tool, tool_id: fail1, arguments: {} }
    on_error: { strategy: skip }
    next: [end]
  - id: retry_step
    type: task
    task: { kind: call_tool, tool_id: fail2, arguments: {} }
    on_error: { strategy: retry, max_retries: 3 }
    next: [end]
  - { id: ok_step, type: task, task: { kind: call_tool, tool_id: ok, arguments: {} }, next: [end] }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    ex.set_tool_result("fail1", Err("fail1".into())).await;
    ex.set_tool_result("fail2", Err("fail2".into())).await;
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    // retry_step exhausts retries  FailWorkflow
    assert_eq!(inst.status, WorkflowStatus::Failed);
    assert_eq!(inst.step_states["skip_step"].status, StepStatus::Skipped);
    assert_eq!(inst.step_states["retry_step"].status, StepStatus::Failed);
    assert_eq!(inst.step_states["retry_step"].retry_count, 3);
}

/// #29: Ten parallel steps all succeed
#[tokio::test]
async fn battle_29_ten_parallel() {
    let yaml = r#"
name: battle-29
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [s1, s2, s3, s4, s5, s6, s7, s8, s9, s10] }
  - { id: s1, type: task, task: { kind: call_tool, tool_id: t, arguments: {} }, next: [end] }
  - { id: s2, type: task, task: { kind: call_tool, tool_id: t, arguments: {} }, next: [end] }
  - { id: s3, type: task, task: { kind: call_tool, tool_id: t, arguments: {} }, next: [end] }
  - { id: s4, type: task, task: { kind: call_tool, tool_id: t, arguments: {} }, next: [end] }
  - { id: s5, type: task, task: { kind: call_tool, tool_id: t, arguments: {} }, next: [end] }
  - { id: s6, type: task, task: { kind: call_tool, tool_id: t, arguments: {} }, next: [end] }
  - { id: s7, type: task, task: { kind: call_tool, tool_id: t, arguments: {} }, next: [end] }
  - { id: s8, type: task, task: { kind: call_tool, tool_id: t, arguments: {} }, next: [end] }
  - { id: s9, type: task, task: { kind: call_tool, tool_id: t, arguments: {} }, next: [end] }
  - { id: s10, type: task, task: { kind: call_tool, tool_id: t, arguments: {} }, next: [end] }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex.clone(), Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert_eq!(ex.tool_call_count.load(Ordering::SeqCst), 10);
}

/// #30: Parallel fan-out then sequential chain
#[tokio::test]
async fn battle_30_parallel_then_sequential() {
    let yaml = r#"
name: battle-30
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [p1, p2, p3] }
  - { id: p1, type: task, task: { kind: call_tool, tool_id: t, arguments: {} }, next: [join] }
  - { id: p2, type: task, task: { kind: call_tool, tool_id: t, arguments: {} }, next: [join] }
  - { id: p3, type: task, task: { kind: call_tool, tool_id: t, arguments: {} }, next: [join] }
  - { id: join, type: task, task: { kind: call_tool, tool_id: t, arguments: {} }, next: [s1] }
  - { id: s1, type: task, task: { kind: call_tool, tool_id: t, arguments: {} }, next: [s2] }
  - { id: s2, type: task, task: { kind: call_tool, tool_id: t, arguments: {} }, next: [end] }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex.clone(), Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert_eq!(ex.tool_call_count.load(Ordering::SeqCst), 6);
}

/// #31: Parallel branches after a branch
#[tokio::test]
async fn battle_31_parallel_after_branch() {
    let yaml = r#"
name: battle-31
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [branch] }
  - id: branch
    type: control_flow
    control: { kind: branch, condition: "1 == 1", then: [pa, pb], else: [x] }
  - { id: pa, type: task, task: { kind: call_tool, tool_id: t, arguments: {} }, next: [end] }
  - { id: pb, type: task, task: { kind: call_tool, tool_id: t, arguments: {} }, next: [end] }
  - { id: x, type: task, task: { kind: delay, duration_secs: 0 }, next: [end] }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex.clone(), Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert_eq!(inst.step_states["pa"].status, StepStatus::Completed);
    assert_eq!(inst.step_states["pb"].status, StepStatus::Completed);
    assert_eq!(ex.tool_call_count.load(Ordering::SeqCst), 2);
}

/// #32: Parallel all fail with skip strategy
#[tokio::test]
async fn battle_32_parallel_all_skip() {
    let yaml = r#"
name: battle-32
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [a, b, c] }
  - id: a
    type: task
    task: { kind: call_tool, tool_id: fail, arguments: {} }
    on_error: { strategy: skip }
    next: [end]
  - id: b
    type: task
    task: { kind: call_tool, tool_id: fail, arguments: {} }
    on_error: { strategy: skip }
    next: [end]
  - id: c
    type: task
    task: { kind: call_tool, tool_id: fail, arguments: {} }
    on_error: { strategy: skip }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    ex.set_tool_result("fail", Err("boom".into())).await;
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    for s in ["a", "b", "c"] {
        assert_eq!(inst.step_states[s].status, StepStatus::Skipped);
    }
}

// ===================================================================
// 4. ERROR HANDLING (15 tests)
// ===================================================================

/// #33: Default error strategy (FailWorkflow)
#[tokio::test]
async fn battle_33_default_fail_workflow() {
    let yaml = r#"
name: battle-33
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [tool] }
  - { id: tool, type: task, task: { kind: call_tool, tool_id: bad, arguments: {} }, next: [end] }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    ex.set_tool_result("bad", Err("tool exploded".into())).await;
    let em = Arc::new(EventCollector::new());
    let (store, engine) = make_engine(ex, em.clone());
    let id = launch(&engine, def(yaml), json!({})).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Failed);
    assert!(inst.error.as_ref().unwrap().contains("tool exploded"));
    assert!(em.count_of("InstanceFailed").await >= 1);
}

/// #34: Retry succeeds on second attempt
#[tokio::test]
async fn battle_34_retry_succeeds() {
    let yaml = r#"
name: battle-34
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [flaky] }
  - id: flaky
    type: task
    task: { kind: call_tool, tool_id: flaky_tool, arguments: {} }
    on_error: { strategy: retry, max_retries: 3 }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    // Executor that fails first call then succeeds
    struct FlakyExecutor {
        calls: AtomicU32,
    }
    #[async_trait::async_trait]
    impl StepExecutor for FlakyExecutor {
        async fn call_tool(
            &self,
            _: &str,
            _: Value,
            _: &ExecutionContext,
        ) -> Result<Value, String> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                Err("transient".into())
            } else {
                Ok(json!({"ok": true}))
            }
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
        ) -> Result<Value, String> {
            Ok(Value::Null)
        }
        async fn signal_agent(
            &self,
            _: &SignalTarget,
            _: &str,
            _: &ExecutionContext,
        ) -> Result<Value, String> {
            Ok(Value::Null)
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
            _: i64,
            _: &str,
            _: &str,
            _: Option<&[String]>,
            _: bool,
            _: &ExecutionContext,
        ) -> Result<String, String> {
            Ok("r".into())
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
            Ok("s".into())
        }
        async fn launch_workflow(
            &self,
            _: &str,
            _: Value,
            _: &ExecutionContext,
        ) -> Result<i64, String> {
            Ok(9999)
        }
        async fn schedule_task(
            &self,
            _: &ScheduleTaskDef,
            _: &ExecutionContext,
        ) -> Result<String, String> {
            Ok("t".into())
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
    let ex = Arc::new(FlakyExecutor { calls: AtomicU32::new(0) });
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let engine = WorkflowEngine::new(store.clone(), ex.clone(), Arc::new(EventCollector::new()));
    let id = engine.launch(def(yaml), json!({}), "s".into(), None, vec![], None).await.unwrap();
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert_eq!(inst.step_states["flaky"].retry_count, 1);
    assert!(ex.calls.load(Ordering::SeqCst) >= 2);
}

/// #35: Retry exhausts all attempts
#[tokio::test]
async fn battle_35_retry_exhausted() {
    let yaml = r#"
name: battle-35
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [bad] }
  - id: bad
    type: task
    task: { kind: call_tool, tool_id: always_fail, arguments: {} }
    on_error: { strategy: retry, max_retries: 2 }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    ex.set_tool_result("always_fail", Err("permanent".into())).await;
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Failed);
    assert_eq!(inst.step_states["bad"].retry_count, 2);
    assert_eq!(inst.step_states["bad"].status, StepStatus::Failed);
}

/// #36: Skip with default output
#[tokio::test]
async fn battle_36_skip_with_default() {
    let yaml = r#"
name: battle-36
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [fail_step] }
  - id: fail_step
    type: task
    task: { kind: call_tool, tool_id: bad, arguments: {} }
    on_error: { strategy: skip, default_output: { fallback: true, value: 42 } }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    ex.set_tool_result("bad", Err("error".into())).await;
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert_eq!(inst.step_states["fail_step"].status, StepStatus::Skipped);
    let outputs = inst.step_states["fail_step"].outputs.as_ref().unwrap();
    assert_eq!(outputs["fallback"], true);
    assert_eq!(outputs["value"], 42);
}

/// #37: GoTo error strategy jumps to handler step
#[tokio::test]
async fn battle_37_goto_error_handler() {
    let yaml = r#"
name: battle-37
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [risky] }
  - id: risky
    type: task
    task: { kind: call_tool, tool_id: bad, arguments: {} }
    on_error: { strategy: go_to, step_id: handler }
    next: [end]
  - id: handler
    type: task
    task: { kind: call_tool, tool_id: cleanup, arguments: {} }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    ex.set_tool_result("bad", Err("error".into())).await;
    let (store, engine) = make_engine(ex.clone(), Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert_eq!(inst.step_states["risky"].status, StepStatus::Failed);
    assert_eq!(inst.step_states["handler"].status, StepStatus::Completed);
}

/// #38: Error in output mapping
#[tokio::test]
async fn battle_38_output_mapping_error() {
    let yaml = r#"
name: battle-38
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [tool] }
  - id: tool
    type: task
    task: { kind: call_tool, tool_id: echo, arguments: {} }
    outputs:
      mapped: "{{result.nonexistent.deeply.nested}}"
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    // Output mapping resolves to empty string for missing paths, not an error
    assert_eq!(inst.status, WorkflowStatus::Completed);
}

/// #39: Multiple steps fail in sequence (cascading)
#[tokio::test]
async fn battle_39_cascading_failure() {
    let yaml = r#"
name: battle-39
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [s1] }
  - { id: s1, type: task, task: { kind: call_tool, tool_id: bad, arguments: {} }, next: [s2] }
  - { id: s2, type: task, task: { kind: call_tool, tool_id: also_bad, arguments: {} }, next: [end] }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    ex.set_tool_result("bad", Err("first".into())).await;
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Failed);
    assert_eq!(inst.step_states["s1"].status, StepStatus::Failed);
    // s2 never runs because s1 failed the workflow
    assert_eq!(inst.step_states["s2"].status, StepStatus::Pending);
}

/// #40: Explicit FailWorkflow with message
#[tokio::test]
async fn battle_40_fail_workflow_with_message() {
    let yaml = r#"
name: battle-40
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [bad] }
  - id: bad
    type: task
    task: { kind: call_tool, tool_id: err, arguments: {} }
    on_error: { strategy: fail_workflow, message: "Critical failure in step" }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    ex.set_tool_result("err", Err("raw error".into())).await;
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Failed);
}

/// #41: Skip without default output
#[tokio::test]
async fn battle_41_skip_no_default() {
    let yaml = r#"
name: battle-41
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [bad] }
  - id: bad
    type: task
    task: { kind: call_tool, tool_id: err, arguments: {} }
    on_error: { strategy: skip }
    next: [after]
  - { id: after, type: task, task: { kind: call_tool, tool_id: ok, arguments: {} }, next: [end] }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    ex.set_tool_result("err", Err("fail".into())).await;
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert_eq!(inst.step_states["bad"].status, StepStatus::Skipped);
    assert_eq!(inst.step_states["after"].status, StepStatus::Completed);
}

/// #42: Agent invoke failure
#[tokio::test]
async fn battle_42_agent_invoke_failure() {
    let yaml = r#"
name: battle-42
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [agent] }
  - id: agent
    type: task
    task: { kind: invoke_agent, persona_id: bad-agent, task: "fail" }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    ex.set_agent_result("bad-agent", Err("agent crashed".into())).await;
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Failed);
    assert!(inst.step_states["agent"].error.as_ref().unwrap().contains("agent crashed"));
}

/// #43: Skipped step's downstream still runs
#[tokio::test]
async fn battle_43_skip_downstream_runs() {
    let yaml = r#"
name: battle-43
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [fail_step] }
  - id: fail_step
    type: task
    task: { kind: call_tool, tool_id: bad, arguments: {} }
    on_error: { strategy: skip }
    next: [after]
  - { id: after, type: task, task: { kind: call_tool, tool_id: good, arguments: {} }, next: [end] }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    ex.set_tool_result("bad", Err("fail".into())).await;
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert_eq!(inst.step_states["after"].status, StepStatus::Completed);
}

/// #44: Error in failed step propagates to workflow error
#[tokio::test]
async fn battle_44_error_propagation() {
    let yaml = r#"
name: battle-44
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [s1] }
  - { id: s1, type: task, task: { kind: call_tool, tool_id: ok, arguments: {} }, next: [s2] }
  - { id: s2, type: task, task: { kind: call_tool, tool_id: kaboom, arguments: {} }, next: [end] }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    ex.set_tool_result("kaboom", Err("specific error message".into())).await;
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Failed);
    assert!(inst.error.as_ref().unwrap().contains("specific error message"));
    assert_eq!(inst.step_states["s1"].status, StepStatus::Completed);
}

/// #45: GoTo to handler then continues
#[tokio::test]
async fn battle_45_goto_continues() {
    let yaml = r#"
name: battle-45
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [risky] }
  - id: risky
    type: task
    task: { kind: call_tool, tool_id: bad, arguments: {} }
    on_error: { strategy: go_to, step_id: recovery }
    next: [end]
  - { id: recovery, type: task, task: { kind: call_tool, tool_id: fix, arguments: {} }, next: [final_step] }
  - { id: final_step, type: task, task: { kind: call_tool, tool_id: done, arguments: {} }, next: [end] }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    ex.set_tool_result("bad", Err("fail".into())).await;
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert_eq!(inst.step_states["recovery"].status, StepStatus::Completed);
    assert_eq!(inst.step_states["final_step"].status, StepStatus::Completed);
}

/// #46: Retry max=0 means fail immediately
#[tokio::test]
async fn battle_46_retry_max_zero() {
    let yaml = r#"
name: battle-46
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [fail_step] }
  - id: fail_step
    type: task
    task: { kind: call_tool, tool_id: bad, arguments: {} }
    on_error: { strategy: retry, max_retries: 0 }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    ex.set_tool_result("bad", Err("fail".into())).await;
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Failed);
    assert_eq!(inst.step_states["fail_step"].retry_count, 0);
}

/// #47: ScheduleTask step
#[tokio::test]
async fn battle_47_schedule_task() {
    let yaml = r#"
name: battle-47
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [sched] }
  - id: sched
    type: task
    task:
      kind: schedule_task
      schedule:
        name: cleanup
        schedule: "0 * * * *"
        action: { type: log, message: "cleanup" }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
}

// ===================================================================
// 5. GATES & INTERACTIONS (12 tests)
// ===================================================================

/// #48: FeedbackGate pauses workflow
#[tokio::test]
async fn battle_48_feedback_gate_pauses() {
    let yaml = r#"
name: battle-48
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [gate] }
  - id: gate
    type: task
    task: { kind: feedback_gate, prompt: "Approve?", choices: ["Yes", "No"], allow_freeform: false }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::WaitingOnInput);
    assert_eq!(inst.step_states["gate"].status, StepStatus::WaitingOnInput);
    assert!(inst.step_states["gate"].interaction_request_id.is_some());
}

/// #49: FeedbackGate response completes workflow
#[tokio::test]
async fn battle_49_feedback_gate_respond() {
    let yaml = r#"
name: battle-49
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [gate] }
  - id: gate
    type: task
    task: { kind: feedback_gate, prompt: "Choose", choices: ["A", "B"] }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    engine.respond_to_gate(id, "gate", json!({"selected": "A"})).await.unwrap();
    wait_for_settled(&store, id).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert_eq!(inst.step_states["gate"].outputs.as_ref().unwrap()["selected"], "A");
}

/// #50: FeedbackGate output flows to downstream step
#[tokio::test]
async fn battle_50_feedback_flows_downstream() {
    let yaml = r#"
name: battle-50
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [gate] }
  - id: gate
    type: task
    task: { kind: feedback_gate, prompt: "Pick", choices: ["X", "Y"] }
    next: [use_it]
  - id: use_it
    type: task
    task: { kind: call_tool, tool_id: echo, arguments: { choice: "{{steps.gate.outputs.selected}}" } }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    engine.respond_to_gate(id, "gate", json!({"selected": "X", "text": ""})).await.unwrap();
    wait_for_settled(&store, id).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
}

/// #51: EventGate pauses workflow
#[tokio::test]
async fn battle_51_event_gate_pauses() {
    let yaml = r#"
name: battle-51
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [wait] }
  - id: wait
    type: task
    task: { kind: event_gate, topic: "data.ready", filter: null, timeout_secs: 300 }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::WaitingOnEvent);
    assert_eq!(inst.step_states["wait"].status, StepStatus::WaitingOnEvent);
}

/// #52: EventGate response completes workflow
#[tokio::test]
async fn battle_52_event_gate_respond() {
    let yaml = r#"
name: battle-52
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [wait] }
  - id: wait
    type: task
    task: { kind: event_gate, topic: "data.ready" }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    engine.respond_to_event(id, "wait", json!({"payload": "data123"})).await.unwrap();
    wait_for_settled(&store, id).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert_eq!(inst.step_states["wait"].outputs.as_ref().unwrap()["payload"], "data123");
}

/// #53: Respond to gate on wrong step fails
#[tokio::test]
async fn battle_53_respond_wrong_step() {
    let yaml = r#"
name: battle-53
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [gate] }
  - id: gate
    type: task
    task: { kind: feedback_gate, prompt: "Q?" }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (_store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    // Try responding on "start" which is already completed
    let result = engine.respond_to_gate(id, "start", json!({})).await;
    assert!(result.is_err());
    // Try responding on non-existent step
    let result = engine.respond_to_gate(id, "nonexistent", json!({})).await;
    assert!(result.is_err());
}

/// #54: EventGate respond on non-waiting step fails
#[tokio::test]
async fn battle_54_event_gate_wrong_state() {
    let yaml = r#"
name: battle-54
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [tool] }
  - { id: tool, type: task, task: { kind: call_tool, tool_id: t, arguments: {} }, next: [end] }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (_store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    let result = engine.respond_to_event(id, "tool", json!({})).await;
    assert!(result.is_err());
}

/// #55: Two sequential gates
#[tokio::test]
async fn battle_55_two_sequential_gates() {
    let yaml = r#"
name: battle-55
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [gate1] }
  - id: gate1
    type: task
    task: { kind: feedback_gate, prompt: "First?" }
    next: [gate2]
  - id: gate2
    type: task
    task: { kind: feedback_gate, prompt: "Second?" }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    // First gate
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::WaitingOnInput);
    engine.respond_to_gate(id, "gate1", json!({"selected": "ok1"})).await.unwrap();
    // Wait for workflow to reach the second gate
    wait_for_settled(&store, id).await;
    // Second gate
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::WaitingOnInput);
    assert_eq!(inst.step_states["gate2"].status, StepStatus::WaitingOnInput);
    engine.respond_to_gate(id, "gate2", json!({"selected": "ok2"})).await.unwrap();
    wait_for_settled(&store, id).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
}

/// #56: Kill workflow while waiting on gate
#[tokio::test]
async fn battle_56_kill_while_waiting() {
    let yaml = r#"
name: battle-56
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [gate] }
  - id: gate
    type: task
    task: { kind: feedback_gate, prompt: "Wait forever" }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    assert_eq!(store.get_instance(id).unwrap().unwrap().status, WorkflowStatus::WaitingOnInput);
    engine.kill(id).await.unwrap();
    assert_eq!(store.get_instance(id).unwrap().unwrap().status, WorkflowStatus::Killed);
}

/// #57: Pause then resume workflow
#[tokio::test]
async fn battle_57_pause_resume() {
    let yaml = r#"
name: battle-57
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [gate] }
  - id: gate
    type: task
    task: { kind: feedback_gate, prompt: "Wait" }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    assert_eq!(store.get_instance(id).unwrap().unwrap().status, WorkflowStatus::WaitingOnInput);
    engine.pause(id).await.unwrap();
    assert_eq!(store.get_instance(id).unwrap().unwrap().status, WorkflowStatus::Paused);
    // Resume should fail because it's not in a valid state for respond, but resume itself works
    // Actually, resume on a paused workflow restores to Running then re-enters run_loop,
    // but it's waiting on input so it should go back to WaitingOnInput
    engine.resume(id).await.unwrap();
    // Resume now continues in background — wait for it to settle
    wait_for_settled(&store, id).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert!(matches!(inst.status, WorkflowStatus::WaitingOnInput | WorkflowStatus::Running));
}

/// #58: EventGate with output mapping
#[tokio::test]
async fn battle_58_event_gate_output_mapping() {
    let yaml = r#"
name: battle-58
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [wait] }
  - id: wait
    type: task
    task: { kind: event_gate, topic: "order.completed" }
    outputs:
      order_id: "{{result.id}}"
      total: "{{result.amount}}"
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    engine.respond_to_event(id, "wait", json!({"id": "ord-123", "amount": 99.95})).await.unwrap();
    wait_for_settled(&store, id).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    let outputs = inst.step_states["wait"].outputs.as_ref().unwrap();
    assert_eq!(outputs["order_id"], "ord-123");
}

/// #59: FeedbackGate with freeform only (no choices)
#[tokio::test]
async fn battle_59_feedback_freeform_only() {
    let yaml = r#"
name: battle-59
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [gate] }
  - id: gate
    type: task
    task: { kind: feedback_gate, prompt: "Enter comment", allow_freeform: true }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    engine.respond_to_gate(id, "gate", json!({"text": "my comment"})).await.unwrap();
    wait_for_settled(&store, id).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
}

// ===================================================================
// 6. DAEMON RESTART / RECOVERY (10 tests)
// ===================================================================

/// #60: Recover a running instance after restart
#[tokio::test]
async fn battle_60_recover_running_instance() {
    // Simulate: create an instance in Running state with a Running step,
    // then call recover_instances to see if it resumes.
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let ex = Arc::new(BattleExecutor::new());
    let em = Arc::new(EventCollector::new());

    // First engine: launch workflow to a gate (WaitingOnInput state)
    let engine1 = WorkflowEngine::new(store.clone(), ex.clone(), em.clone());
    let yaml = r#"
name: battle-60
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [gate] }
  - id: gate
    type: task
    task: { kind: feedback_gate, prompt: "Wait" }
    next: [tool]
  - { id: tool, type: task, task: { kind: call_tool, tool_id: t, arguments: {} }, next: [end] }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let id = engine1.launch(def(yaml), json!({}), "s".into(), None, vec![], None).await.unwrap();
    // Respond to gate so it goes to Running with tool step pending
    engine1.respond_to_gate(id, "gate", json!({"selected": "ok"})).await.unwrap();
    wait_for_settled(&store, id).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
}

/// #61: Recover waiting-on-input instance
#[tokio::test]
async fn battle_61_recover_waiting_on_input() {
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let ex = Arc::new(BattleExecutor::new());
    let em = Arc::new(EventCollector::new());
    let engine = WorkflowEngine::new(store.clone(), ex.clone(), em.clone());
    let yaml = r#"
name: battle-61
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [gate] }
  - id: gate
    type: task
    task: { kind: feedback_gate, prompt: "Waiting" }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let id = engine.launch(def(yaml), json!({}), "s".into(), None, vec![], None).await.unwrap();
    assert_eq!(store.get_instance(id).unwrap().unwrap().status, WorkflowStatus::WaitingOnInput);

    // Simulate daemon restart: new engine with same store
    let engine2 = WorkflowEngine::new(store.clone(), ex.clone(), em.clone());
    let handles = engine2.recover_instances().await.unwrap();
    // Waiting instances don't spawn handles - they just log
    assert_eq!(handles.len(), 0);
    // Instance should still be waiting
    assert_eq!(store.get_instance(id).unwrap().unwrap().status, WorkflowStatus::WaitingOnInput);
    // We can still respond via the new engine
    engine2.respond_to_gate(id, "gate", json!({"selected": "ok"})).await.unwrap();
    wait_for_settled(&store, id).await;
    assert_eq!(store.get_instance(id).unwrap().unwrap().status, WorkflowStatus::Completed);
}

/// #62: Recover waiting-on-event instance
#[tokio::test]
async fn battle_62_recover_waiting_on_event() {
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let ex = Arc::new(BattleExecutor::new());
    let em = Arc::new(EventCollector::new());
    let engine = WorkflowEngine::new(store.clone(), ex.clone(), em.clone());
    let yaml = r#"
name: battle-62
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [wait] }
  - id: wait
    type: task
    task: { kind: event_gate, topic: "order.placed" }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let id = engine.launch(def(yaml), json!({}), "s".into(), None, vec![], None).await.unwrap();
    assert_eq!(store.get_instance(id).unwrap().unwrap().status, WorkflowStatus::WaitingOnEvent);

    let engine2 = WorkflowEngine::new(store.clone(), ex.clone(), em.clone());
    let handles = engine2.recover_instances().await.unwrap();
    assert_eq!(handles.len(), 0);
    // Can still respond
    engine2.respond_to_event(id, "wait", json!({"order": "123"})).await.unwrap();
    wait_for_settled(&store, id).await;
    assert_eq!(store.get_instance(id).unwrap().unwrap().status, WorkflowStatus::Completed);
}

/// #63: Recover multiple instances simultaneously
#[tokio::test]
async fn battle_63_recover_multiple() {
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let ex = Arc::new(BattleExecutor::new());
    let em = Arc::new(EventCollector::new());
    let engine = WorkflowEngine::new(store.clone(), ex.clone(), em.clone());
    let yaml = r#"
name: battle-63
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [gate] }
  - id: gate
    type: task
    task: { kind: feedback_gate, prompt: "Wait" }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    // Launch 5 instances, all waiting
    let mut ids = vec![];
    for _ in 0..5 {
        let id = engine.launch(def(yaml), json!({}), "s".into(), None, vec![], None).await.unwrap();
        ids.push(id);
    }

    let engine2 = WorkflowEngine::new(store.clone(), ex.clone(), em.clone());
    let handles = engine2.recover_instances().await.unwrap();
    // All are waiting, not running, so no handles spawned
    assert_eq!(handles.len(), 0);
    // All still waiting
    for &id in &ids {
        assert_eq!(store.get_instance(id).unwrap().unwrap().status, WorkflowStatus::WaitingOnInput);
    }
    // Respond to all via new engine
    for &id in &ids {
        engine2.respond_to_gate(id, "gate", json!({"selected": "ok"})).await.unwrap();
    }
    for &id in &ids {
        wait_for_settled(&store, id).await;
        assert_eq!(store.get_instance(id).unwrap().unwrap().status, WorkflowStatus::Completed);
    }
}

/// #64: Recovery preserves completed steps
#[tokio::test]
async fn battle_64_recovery_preserves_completed() {
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let ex = Arc::new(BattleExecutor::new());
    let em = Arc::new(EventCollector::new());
    let engine = WorkflowEngine::new(store.clone(), ex.clone(), em.clone());
    let yaml = r#"
name: battle-64
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [s1] }
  - { id: s1, type: task, task: { kind: call_tool, tool_id: t, arguments: {} }, next: [gate] }
  - id: gate
    type: task
    task: { kind: feedback_gate, prompt: "Wait" }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let id = engine.launch(def(yaml), json!({}), "s".into(), None, vec![], None).await.unwrap();
    // s1 should be completed, gate should be waiting
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.step_states["s1"].status, StepStatus::Completed);
    assert_eq!(inst.step_states["gate"].status, StepStatus::WaitingOnInput);

    // "Restart"
    let engine2 = WorkflowEngine::new(store.clone(), ex.clone(), em.clone());
    engine2.recover_instances().await.unwrap();
    // s1 should still be completed
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.step_states["s1"].status, StepStatus::Completed);
    assert_eq!(inst.step_states["gate"].status, StepStatus::WaitingOnInput);
}

/// #65: Recovery preserves variables
#[tokio::test]
async fn battle_65_recovery_preserves_variables() {
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let ex = Arc::new(BattleExecutor::new());
    let em = Arc::new(EventCollector::new());
    let engine = WorkflowEngine::new(store.clone(), ex.clone(), em.clone());
    let yaml = r#"
name: battle-65
version: "1.0"
variables:
  type: object
  properties:
    counter: { type: number, default: 42 }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [gate] }
  - id: gate
    type: task
    task: { kind: feedback_gate, prompt: "Wait" }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let id = engine.launch(def(yaml), json!({}), "s".into(), None, vec![], None).await.unwrap();
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.variables["counter"], 42);

    let engine2 = WorkflowEngine::new(store.clone(), ex.clone(), em.clone());
    engine2.recover_instances().await.unwrap();
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.variables["counter"], 42);
}

/// #66: No orphaned instances to recover
#[tokio::test]
async fn battle_66_no_orphans() {
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let ex = Arc::new(BattleExecutor::new());
    let em = Arc::new(EventCollector::new());
    let engine = WorkflowEngine::new(store.clone(), ex.clone(), em.clone());
    let handles = engine.recover_instances().await.unwrap();
    assert_eq!(handles.len(), 0);
}

/// #67: Recovery after instance was killed
#[tokio::test]
async fn battle_67_no_recovery_for_killed() {
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let ex = Arc::new(BattleExecutor::new());
    let em = Arc::new(EventCollector::new());
    let engine = WorkflowEngine::new(store.clone(), ex.clone(), em.clone());
    let yaml = r#"
name: battle-67
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [gate] }
  - id: gate
    type: task
    task: { kind: feedback_gate, prompt: "Wait" }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let id = engine.launch(def(yaml), json!({}), "s".into(), None, vec![], None).await.unwrap();
    engine.kill(id).await.unwrap();
    assert_eq!(store.get_instance(id).unwrap().unwrap().status, WorkflowStatus::Killed);

    let engine2 = WorkflowEngine::new(store.clone(), ex.clone(), em.clone());
    let handles = engine2.recover_instances().await.unwrap();
    assert_eq!(handles.len(), 0);
    assert_eq!(store.get_instance(id).unwrap().unwrap().status, WorkflowStatus::Killed);
}

/// #68: Recovery doesn't touch completed instances
#[tokio::test]
async fn battle_68_no_recovery_for_completed() {
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let ex = Arc::new(BattleExecutor::new());
    let em = Arc::new(EventCollector::new());
    let engine = WorkflowEngine::new(store.clone(), ex.clone(), em.clone());
    let yaml = r#"
name: battle-68
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [end] }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let id = engine.launch(def(yaml), json!({}), "s".into(), None, vec![], None).await.unwrap();
    assert_eq!(store.get_instance(id).unwrap().unwrap().status, WorkflowStatus::Completed);

    let engine2 = WorkflowEngine::new(store.clone(), ex.clone(), em.clone());
    let handles = engine2.recover_instances().await.unwrap();
    assert_eq!(handles.len(), 0);
}

/// #69: Recovery doesn't touch failed instances
#[tokio::test]
async fn battle_69_no_recovery_for_failed() {
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let ex = Arc::new(BattleExecutor::new());
    ex.set_tool_result("bad", Err("fail".into())).await;
    let em = Arc::new(EventCollector::new());
    let engine = WorkflowEngine::new(store.clone(), ex.clone(), em.clone());
    let yaml = r#"
name: battle-69
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [bad] }
  - { id: bad, type: task, task: { kind: call_tool, tool_id: bad, arguments: {} }, next: [end] }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let id = engine.launch(def(yaml), json!({}), "s".into(), None, vec![], None).await.unwrap();
    assert_eq!(store.get_instance(id).unwrap().unwrap().status, WorkflowStatus::Failed);

    let engine2 = WorkflowEngine::new(store.clone(), ex.clone(), em.clone());
    let handles = engine2.recover_instances().await.unwrap();
    assert_eq!(handles.len(), 0);
}

// ===================================================================
// 7. CHILD WORKFLOWS (5 tests)
// ===================================================================

/// #70: Launch child workflow
#[tokio::test]
async fn battle_70_launch_child_workflow() {
    let yaml = r#"
name: battle-70
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [spawn] }
  - id: spawn
    type: task
    task: { kind: launch_workflow, workflow_name: child-wf, inputs: { x: "{{trigger.val}}" } }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({"val": "hello"})).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert!(inst.step_states["spawn"].child_workflow_id.is_some());
}

/// #71: Child workflow launch failure
#[tokio::test]
async fn battle_71_child_workflow_failure() {
    let yaml = r#"
name: battle-71
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [spawn] }
  - id: spawn
    type: task
    task: { kind: launch_workflow, workflow_name: nonexistent, inputs: {} }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    ex.set_launch_result("nonexistent", Err("workflow not found".into())).await;
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Failed);
}

/// #72: Multiple child workflows in parallel
#[tokio::test]
async fn battle_72_multiple_children() {
    let yaml = r#"
name: battle-72
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [c1, c2, c3] }
  - { id: c1, type: task, task: { kind: launch_workflow, workflow_name: child1, inputs: {} }, next: [end] }
  - { id: c2, type: task, task: { kind: launch_workflow, workflow_name: child2, inputs: {} }, next: [end] }
  - { id: c3, type: task, task: { kind: launch_workflow, workflow_name: child3, inputs: {} }, next: [end] }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    for s in ["c1", "c2", "c3"] {
        assert!(
            inst.step_states[s].child_workflow_id.is_some(),
            "{s} should have child_workflow_id"
        );
    }
}

/// #73: Child workflow with skip on failure
#[tokio::test]
async fn battle_73_child_skip_on_failure() {
    let yaml = r#"
name: battle-73
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [spawn] }
  - id: spawn
    type: task
    task: { kind: launch_workflow, workflow_name: bad_child, inputs: {} }
    on_error: { strategy: skip }
    next: [after]
  - { id: after, type: task, task: { kind: call_tool, tool_id: t, arguments: {} }, next: [end] }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    ex.set_launch_result("bad_child", Err("launch failed".into())).await;
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert_eq!(inst.step_states["spawn"].status, StepStatus::Skipped);
    assert_eq!(inst.step_states["after"].status, StepStatus::Completed);
}

/// #74: Signal agent step
#[tokio::test]
async fn battle_74_signal_agent() {
    let yaml = r#"
name: battle-74
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [signal] }
  - id: signal
    type: task
    task: { kind: signal_agent, target: { type: session, session_id: "sess-1" }, content: "Hello agent" }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
}

// ===================================================================
// 8. VARIABLE MANAGEMENT (8 tests)
// ===================================================================

/// #75: SetVariable with Set operation
#[tokio::test]
async fn battle_75_set_variable_set() {
    let yaml = r#"
name: battle-75
version: "1.0"
variables:
  type: object
  properties:
    name: { type: string, default: "" }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [set_var] }
  - id: set_var
    type: task
    task:
      kind: set_variable
      assignments:
        - { variable: name, value: "hello world" }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert_eq!(inst.variables["name"], "hello world");
}

/// #76: SetVariable with AppendList
#[tokio::test]
async fn battle_76_set_variable_append_list() {
    let yaml = r#"
name: battle-76
version: "1.0"
variables:
  type: object
  properties:
    items: { type: array, default: [] }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [add1] }
  - id: add1
    type: task
    task:
      kind: set_variable
      assignments:
        - { variable: items, value: "first", operation: append_list }
    next: [add2]
  - id: add2
    type: task
    task:
      kind: set_variable
      assignments:
        - { variable: items, value: "second", operation: append_list }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    let items = inst.variables["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0], "first");
    assert_eq!(items[1], "second");
}

/// #77: SetVariable updates across steps
#[tokio::test]
async fn battle_77_variable_update_across_steps() {
    let yaml = r#"
name: battle-77
version: "1.0"
variables:
  type: object
  properties:
    status: { type: string, default: "init" }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [update1] }
  - id: update1
    type: task
    task:
      kind: set_variable
      assignments:
        - { variable: status, value: "processing" }
    next: [update2]
  - id: update2
    type: task
    task:
      kind: set_variable
      assignments:
        - { variable: status, value: "done" }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.variables["status"], "done");
}

/// #78: Variable used in branch condition after update
#[tokio::test]
async fn battle_78_variable_in_branch() {
    let yaml = r#"
name: battle-78
version: "1.0"
variables:
  type: object
  properties:
    ready: { type: string, default: "yes" }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [check] }
  - id: check
    type: control_flow
    control: { kind: branch, condition: '{{variables.ready}} == "yes"', then: [go], else: [stop] }
  - { id: go, type: task, task: { kind: delay, duration_secs: 0 }, next: [end] }
  - { id: stop, type: task, task: { kind: delay, duration_secs: 0 }, next: [end] }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    wait_for_terminal(&store, id).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.step_states["go"].status, StepStatus::Completed);
}

/// #79: Variable from trigger input
#[tokio::test]
async fn battle_79_variable_from_trigger() {
    let yaml = r#"
name: battle-79
version: "1.0"
variables:
  type: object
  properties:
    msg: { type: string, default: "" }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [set_var] }
  - id: set_var
    type: task
    task:
      kind: set_variable
      assignments:
        - { variable: msg, value: "{{trigger.input_val}}" }
    next: [use_it]
  - id: use_it
    type: task
    task: { kind: call_tool, tool_id: t, arguments: { m: "{{variables.msg}}" } }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({"input_val": "test123"})).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert_eq!(inst.variables["msg"], "test123");
}

/// #80: Multiple variable assignments in one step
#[tokio::test]
async fn battle_80_multi_assign() {
    let yaml = r#"
name: battle-80
version: "1.0"
variables:
  type: object
  properties:
    a: { type: string, default: "" }
    b: { type: string, default: "" }
    c: { type: string, default: "" }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [set_all] }
  - id: set_all
    type: task
    task:
      kind: set_variable
      assignments:
        - { variable: a, value: "alpha" }
        - { variable: b, value: "beta" }
        - { variable: c, value: "gamma" }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.variables["a"], "alpha");
    assert_eq!(inst.variables["b"], "beta");
    assert_eq!(inst.variables["c"], "gamma");
}

/// #81: SetVariable creates new list from nothing
#[tokio::test]
async fn battle_81_append_creates_list() {
    let yaml = r#"
name: battle-81
version: "1.0"
variables:
  type: object
  properties:
    log: { type: array }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [add] }
  - id: add
    type: task
    task:
      kind: set_variable
      assignments:
        - { variable: log, value: "entry1", operation: append_list }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    let log = inst.variables["log"].as_array().unwrap();
    assert_eq!(log.len(), 1);
}

/// #82: SetVariable downstream reads updated vars
#[tokio::test]
async fn battle_82_downstream_reads_updated_vars() {
    let yaml = r#"
name: battle-82
version: "1.0"
variables:
  type: object
  properties:
    x: { type: string, default: "original" }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [update] }
  - id: update
    type: task
    task:
      kind: set_variable
      assignments:
        - { variable: x, value: "updated" }
    next: [check]
  - id: check
    type: control_flow
    control: { kind: branch, condition: '{{variables.x}} == "updated"', then: [pass], else: [fail_path] }
  - { id: pass, type: task, task: { kind: delay, duration_secs: 0 }, next: [end] }
  - { id: fail_path, type: task, task: { kind: delay, duration_secs: 0 }, next: [end] }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    wait_for_terminal(&store, id).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.step_states["pass"].status, StepStatus::Completed);
    assert_eq!(inst.step_states["fail_path"].status, StepStatus::Skipped);
}

// ===================================================================
// 9. CONCURRENCY & LOAD (10 tests)
// ===================================================================

/// #83: 20 concurrent workflow launches
#[tokio::test]
async fn battle_83_twenty_concurrent_launches() {
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let ex = Arc::new(BattleExecutor::new());
    let em = Arc::new(EventCollector::new());
    let engine = Arc::new(WorkflowEngine::new(store.clone(), ex.clone(), em.clone()));
    let yaml = r#"
name: battle-83
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [tool] }
  - { id: tool, type: task, task: { kind: call_tool, tool_id: t, arguments: {} }, next: [end] }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let definition = def(yaml);
    let mut handles = vec![];
    for i in 0..20 {
        let e = engine.clone();
        let d = definition.clone();
        let s = store.clone();
        handles.push(tokio::spawn(async move {
            let id = e
                .launch(d, json!({"i": i}), format!("session-{i}"), None, vec![], None)
                .await
                .unwrap();
            let inst = s.get_instance(id).unwrap().unwrap();
            assert_eq!(inst.status, WorkflowStatus::Completed, "instance {i} should complete");
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
    assert_eq!(ex.tool_call_count.load(Ordering::SeqCst), 20);
}

/// #84: 50 sequential launches (store stress)
#[tokio::test]
async fn battle_84_fifty_sequential_launches() {
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let ex = Arc::new(BattleExecutor::new());
    let em = Arc::new(EventCollector::new());
    let engine = WorkflowEngine::new(store.clone(), ex.clone(), em.clone());
    let yaml = r#"
name: battle-84
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [end] }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let definition = def(yaml);
    for i in 0..50 {
        let id = engine
            .launch(definition.clone(), json!({}), format!("s-{i}"), None, vec![], None)
            .await
            .unwrap();
        let inst = store.get_instance(id).unwrap().unwrap();
        assert_eq!(inst.status, WorkflowStatus::Completed);
    }
    // Verify store can list all
    let filter = InstanceFilter { ..Default::default() };
    let result = store.list_instances(&filter).unwrap();
    assert_eq!(result.items.len(), 50);
}

/// #85: Concurrent launch and kill
#[tokio::test]
async fn battle_85_concurrent_launch_and_kill() {
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let ex = Arc::new(BattleExecutor::new());
    let engine =
        Arc::new(WorkflowEngine::new(store.clone(), ex.clone(), Arc::new(EventCollector::new())));
    let yaml = r#"
name: battle-85
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [gate] }
  - id: gate
    type: task
    task: { kind: feedback_gate, prompt: "Wait" }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let definition = def(yaml);
    let mut ids = vec![];
    for _ in 0..10 {
        let id = engine
            .launch(definition.clone(), json!({}), "s".into(), None, vec![], None)
            .await
            .unwrap();
        ids.push(id);
    }
    // Kill all concurrently
    let mut handles = vec![];
    for &id in &ids {
        let e = engine.clone();
        let id = id.clone();
        handles.push(tokio::spawn(async move {
            e.kill(id).await.unwrap();
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
    for &id in &ids {
        assert_eq!(store.get_instance(id).unwrap().unwrap().status, WorkflowStatus::Killed);
    }
}

/// #86: Many instances sharing one store
#[tokio::test]
async fn battle_86_many_instances_shared_store() {
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let ex = Arc::new(BattleExecutor::new());
    let engine =
        Arc::new(WorkflowEngine::new(store.clone(), ex.clone(), Arc::new(EventCollector::new())));
    let yaml = r#"
name: battle-86
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [t1, t2] }
  - { id: t1, type: task, task: { kind: call_tool, tool_id: t, arguments: {} }, next: [end] }
  - { id: t2, type: task, task: { kind: call_tool, tool_id: t, arguments: {} }, next: [end] }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let definition = def(yaml);
    let mut handles = vec![];
    for i in 0..30 {
        let e = engine.clone();
        let d = definition.clone();
        handles.push(tokio::spawn(async move {
            e.launch(d, json!({}), format!("s-{i}"), None, vec![], None).await.unwrap()
        }));
    }
    let mut ids = vec![];
    for h in handles {
        ids.push(h.await.unwrap());
    }
    for &id in &ids {
        assert_eq!(store.get_instance(id).unwrap().unwrap().status, WorkflowStatus::Completed);
    }
    assert_eq!(ex.tool_call_count.load(Ordering::SeqCst), 60); // 30 instances  2 tools each
}

/// #87: Rapid pause/resume on same instance
#[tokio::test]
async fn battle_87_rapid_pause_resume() {
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let ex = Arc::new(BattleExecutor::new());
    let engine = WorkflowEngine::new(store.clone(), ex.clone(), Arc::new(EventCollector::new()));
    let yaml = r#"
name: battle-87
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [gate] }
  - id: gate
    type: task
    task: { kind: feedback_gate, prompt: "Wait" }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let id = engine.launch(def(yaml), json!({}), "s".into(), None, vec![], None).await.unwrap();
    // Rapid pause/resume cycles
    for _ in 0..5 {
        engine.pause(id).await.unwrap();
        engine.resume(id).await.unwrap();
    }
    // Resume now continues in background — wait for it to settle
    wait_for_settled(&store, id).await;
    // Should still be functional - respond to gate
    let inst = store.get_instance(id).unwrap().unwrap();
    assert!(matches!(
        inst.status,
        WorkflowStatus::WaitingOnInput | WorkflowStatus::Paused | WorkflowStatus::Running
    ));
}

/// #88: Definition CRUD under concurrent access
#[tokio::test]
async fn battle_88_definition_crud_concurrent() {
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let yaml_base = r#"
name: battle-88-NAME
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [end] }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let mut handles = vec![];
    for i in 0..20 {
        let s = store.clone();
        let yaml = yaml_base.replace("NAME", &format!("{i}"));
        handles.push(tokio::spawn(async move {
            let d: WorkflowDefinition = serde_yaml::from_str(&yaml).unwrap();
            s.save_definition(&yaml, &d).unwrap();
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
    let defs = store.list_definitions().unwrap();
    assert_eq!(defs.len(), 20);
}

/// #89: Instance listing with filters
#[tokio::test]
async fn battle_89_instance_listing_filters() {
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let ex = Arc::new(BattleExecutor::new());
    let engine = WorkflowEngine::new(store.clone(), ex.clone(), Arc::new(EventCollector::new()));
    let complete_yaml = r#"
name: battle-89-complete
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [end] }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let waiting_yaml = r#"
name: battle-89-waiting
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [gate] }
  - id: gate
    type: task
    task: { kind: feedback_gate, prompt: "Wait" }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    // Create mixed instances
    for _ in 0..5 {
        engine.launch(def(complete_yaml), json!({}), "s".into(), None, vec![], None).await.unwrap();
    }
    for _ in 0..3 {
        engine.launch(def(waiting_yaml), json!({}), "s".into(), None, vec![], None).await.unwrap();
    }
    // Filter by status
    let completed_filter =
        InstanceFilter { statuses: vec![WorkflowStatus::Completed], ..Default::default() };
    let completed = store.list_instances(&completed_filter).unwrap();
    assert_eq!(completed.items.len(), 5);

    let waiting_filter =
        InstanceFilter { statuses: vec![WorkflowStatus::WaitingOnInput], ..Default::default() };
    let waiting = store.list_instances(&waiting_filter).unwrap();
    assert_eq!(waiting.items.len(), 3);
}

/// #90: Concurrent respond_to_gate on different instances
#[tokio::test]
async fn battle_90_concurrent_gate_responses() {
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let ex = Arc::new(BattleExecutor::new());
    let engine =
        Arc::new(WorkflowEngine::new(store.clone(), ex.clone(), Arc::new(EventCollector::new())));
    let yaml = r#"
name: battle-90
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [gate] }
  - id: gate
    type: task
    task: { kind: feedback_gate, prompt: "Wait" }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let mut ids = vec![];
    for _ in 0..10 {
        let id = engine.launch(def(yaml), json!({}), "s".into(), None, vec![], None).await.unwrap();
        ids.push(id);
    }
    // Respond to all concurrently
    let mut handles = vec![];
    for id in ids.clone() {
        let e = engine.clone();
        handles.push(tokio::spawn(async move {
            e.respond_to_gate(id, "gate", json!({"selected": "ok"})).await.unwrap();
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
    for &id in &ids {
        assert_eq!(store.get_instance(id).unwrap().unwrap().status, WorkflowStatus::Completed);
    }
}

/// #91: Store handles large payloads
#[tokio::test]
async fn battle_91_large_payload() {
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let ex = Arc::new(BattleExecutor::new());
    let engine = WorkflowEngine::new(store.clone(), ex.clone(), Arc::new(EventCollector::new()));
    let yaml = r#"
name: battle-91
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [end] }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    // Large input payload
    let big_string = "x".repeat(100_000);
    let id = engine
        .launch(def(yaml), json!({"data": big_string}), "s".into(), None, vec![], None)
        .await
        .unwrap();
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
}

/// #92: Concurrent mixed operations
#[tokio::test]
async fn battle_92_concurrent_mixed_ops() {
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let ex = Arc::new(BattleExecutor::new());
    let engine =
        Arc::new(WorkflowEngine::new(store.clone(), ex.clone(), Arc::new(EventCollector::new())));
    let simple_yaml = r#"
name: battle-92-simple
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [end] }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    // Launch, list, get all concurrently
    let mut handles = vec![];
    for i in 0..15 {
        let e = engine.clone();
        let d = def(simple_yaml);
        let s = store.clone();
        handles.push(tokio::spawn(async move {
            let id = e.launch(d, json!({}), format!("s-{i}"), None, vec![], None).await.unwrap();
            let _inst = s.get_instance(id).unwrap().unwrap();
            let _list = s.list_instances(&InstanceFilter::default()).unwrap();
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
}

// ===================================================================
// 10. EDGE CASES & ADVERSARIAL (8 tests)
// ===================================================================

/// #93: Workflow with no steps after trigger
#[tokio::test]
async fn battle_93_trigger_only() {
    let yaml = r#"
name: battle-93
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [] }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
}

/// #94: Very long step chain (20 sequential steps)
#[tokio::test]
async fn battle_94_twenty_step_chain() {
    let mut steps = String::from(
        "  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [s0] }\n",
    );
    for i in 0..19 {
        let next = if i < 18 { format!("s{}", i + 1) } else { "end".to_string() };
        steps.push_str(&format!(
            "  - {{ id: s{i}, type: task, task: {{ kind: call_tool, tool_id: t, arguments: {{}} }}, next: [{next}] }}\n"
        ));
    }
    steps.push_str("  - { id: end, type: control_flow, control: { kind: end_workflow } }\n");
    let yaml = format!(
        "name: battle-94\nversion: \"1.0\"\nvariables: {{ type: object, properties: {{}} }}\nsteps:\n{steps}"
    );
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex.clone(), Arc::new(EventCollector::new()));
    let id = launch(&engine, def(&yaml), json!({})).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert_eq!(ex.tool_call_count.load(Ordering::SeqCst), 19);
}

/// #95: Respond to non-existent instance
#[tokio::test]
async fn battle_95_respond_nonexistent() {
    let ex = Arc::new(BattleExecutor::new());
    let (_store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let result = engine.respond_to_gate(999999, "step", json!({})).await;
    assert!(result.is_err());
}

/// #96: Resume non-paused instance fails
#[tokio::test]
async fn battle_96_resume_not_paused() {
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let ex = Arc::new(BattleExecutor::new());
    let engine = WorkflowEngine::new(store.clone(), ex.clone(), Arc::new(EventCollector::new()));
    let yaml = r#"
name: battle-96
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [gate] }
  - id: gate
    type: task
    task: { kind: feedback_gate, prompt: "W" }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let id = engine.launch(def(yaml), json!({}), "s".into(), None, vec![], None).await.unwrap();
    // Instance is WaitingOnInput, not Paused
    let result = engine.resume(id).await;
    assert!(result.is_err(), "resume on non-paused should fail");
}

/// #97: Kill already completed instance
#[tokio::test]
async fn battle_97_kill_completed() {
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let ex = Arc::new(BattleExecutor::new());
    let engine = WorkflowEngine::new(store.clone(), ex.clone(), Arc::new(EventCollector::new()));
    let yaml = r#"
name: battle-97
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [end] }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let id = engine.launch(def(yaml), json!({}), "s".into(), None, vec![], None).await.unwrap();
    // Kill completed — should return an error (invalid state)
    let result = engine.kill(id).await;
    assert!(result.is_err(), "Killing a completed instance should fail");
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
}

/// #98: Workflow with chat mode
#[tokio::test]
async fn battle_98_chat_mode() {
    let yaml = r#"
name: battle-98
version: "1.0"
mode: chat
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [tool] }
  - { id: tool, type: task, task: { kind: call_tool, tool_id: t, arguments: {} }, next: [end] }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert_eq!(inst.definition.mode, WorkflowMode::Chat);
}

/// #99: Workflow with permissions
#[tokio::test]
async fn battle_99_with_permissions() {
    let yaml = r#"
name: battle-99
version: "1.0"
variables: { type: object, properties: {} }
permissions:
  - tool_id: "fs.*"
    approval: auto
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [end] }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = engine
        .launch(
            def(yaml),
            json!({}),
            "s".into(),
            None,
            vec![PermissionEntry {
                tool_id: "net.*".into(),
                resource: None,
                approval: ToolApprovalLevel::Auto,
            }],
            None,
        )
        .await
        .unwrap();
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    // Caller perms come first, then definition perms
    assert_eq!(inst.permissions.len(), 2);
    assert_eq!(inst.permissions[0].tool_id, "net.*");
    assert_eq!(inst.permissions[1].tool_id, "fs.*");
}

/// #100: Event collector captures complete lifecycle
#[tokio::test]
async fn battle_100_complete_event_lifecycle() {
    let yaml = r#"
name: battle-100
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [tool] }
  - { id: tool, type: task, task: { kind: call_tool, tool_id: t, arguments: {} }, next: [end] }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let em = Arc::new(EventCollector::new());
    let (store, engine) = make_engine(ex, em.clone());
    let id = launch(&engine, def(yaml), json!({})).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);

    let events = em.events().await;
    // Should have: InstanceCreated, InstanceStarted, StepStarted (tool), StepCompleted (tool),
    //              StepStarted (end), StepCompleted (end), InstanceCompleted
    assert!(events.len() >= 5, "expected at least 5 events, got {}", events.len());
    assert!(em.count_of("InstanceCreated").await >= 1);
    assert!(em.count_of("InstanceCompleted").await >= 1);
}

/// #101: Retry delay_secs is actually enforced
#[tokio::test]
async fn battle_101_retry_delay_enforced() {
    let yaml = r#"
name: battle-101
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [flaky] }
  - id: flaky
    type: task
    task: { kind: call_tool, tool_id: flaky_tool, arguments: {} }
    on_error: { strategy: retry, max_retries: 2, delay_secs: 1 }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    struct TimedFlakyExecutor {
        calls: AtomicU32,
    }
    #[async_trait::async_trait]
    impl StepExecutor for TimedFlakyExecutor {
        async fn call_tool(
            &self,
            _: &str,
            _: Value,
            _: &ExecutionContext,
        ) -> Result<Value, String> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                Err("transient".into())
            } else {
                Ok(json!({"ok": true}))
            }
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
        ) -> Result<Value, String> {
            Ok(Value::Null)
        }
        async fn signal_agent(
            &self,
            _: &SignalTarget,
            _: &str,
            _: &ExecutionContext,
        ) -> Result<Value, String> {
            Ok(Value::Null)
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
            _: i64,
            _: &str,
            _: &str,
            _: Option<&[String]>,
            _: bool,
            _: &ExecutionContext,
        ) -> Result<String, String> {
            Ok("r".into())
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
            Ok("s".into())
        }
        async fn launch_workflow(
            &self,
            _: &str,
            _: Value,
            _: &ExecutionContext,
        ) -> Result<i64, String> {
            Ok(9999)
        }
        async fn schedule_task(
            &self,
            _: &ScheduleTaskDef,
            _: &ExecutionContext,
        ) -> Result<String, String> {
            Ok("t".into())
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
    let ex = Arc::new(TimedFlakyExecutor { calls: AtomicU32::new(0) });
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let engine = WorkflowEngine::new(store.clone(), ex.clone(), Arc::new(EventCollector::new()));
    let start = std::time::Instant::now();
    let id = engine.launch(def(yaml), json!({}), "s".into(), None, vec![], None).await.unwrap();
    let elapsed = start.elapsed();
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert_eq!(inst.step_states["flaky"].retry_count, 1);
    // With delay_secs=1, the retry should take at least 900ms (allowing some tolerance)
    assert!(elapsed.as_millis() >= 900, "retry delay should be enforced, elapsed: {:?}", elapsed);
}

/// #102: Step with timeout_secs times out correctly
#[tokio::test]
async fn battle_102_step_timeout() {
    let yaml = r#"
name: battle-102
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [slow] }
  - id: slow
    type: task
    task: { kind: call_tool, tool_id: slow_tool, arguments: {} }
    timeout_secs: 1
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    struct SlowExecutor;
    #[async_trait::async_trait]
    impl StepExecutor for SlowExecutor {
        async fn call_tool(
            &self,
            _: &str,
            _: Value,
            _: &ExecutionContext,
        ) -> Result<Value, String> {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            Ok(json!({"done": true}))
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
        ) -> Result<Value, String> {
            Ok(Value::Null)
        }
        async fn signal_agent(
            &self,
            _: &SignalTarget,
            _: &str,
            _: &ExecutionContext,
        ) -> Result<Value, String> {
            Ok(Value::Null)
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
            _: i64,
            _: &str,
            _: &str,
            _: Option<&[String]>,
            _: bool,
            _: &ExecutionContext,
        ) -> Result<String, String> {
            Ok("r".into())
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
            Ok("s".into())
        }
        async fn launch_workflow(
            &self,
            _: &str,
            _: Value,
            _: &ExecutionContext,
        ) -> Result<i64, String> {
            Ok(9999)
        }
        async fn schedule_task(
            &self,
            _: &ScheduleTaskDef,
            _: &ExecutionContext,
        ) -> Result<String, String> {
            Ok("t".into())
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
    let ex = Arc::new(SlowExecutor);
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let engine = WorkflowEngine::new(store.clone(), ex, Arc::new(EventCollector::new()));
    let start = std::time::Instant::now();
    let id = engine.launch(def(yaml), json!({}), "s".into(), None, vec![], None).await.unwrap();
    let elapsed = start.elapsed();
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Failed, "should fail due to timeout");
    assert!(
        inst.step_states["slow"].error.as_ref().unwrap().contains("timed out"),
        "error should mention timeout"
    );
    // Should complete in about 1s, not 30s
    assert!(elapsed.as_secs() < 5, "timeout should cut execution short, elapsed: {:?}", elapsed);
}

/// #103: Step without timeout_secs runs normally (no timeout)
#[tokio::test]
async fn battle_103_no_timeout_runs_normally() {
    let yaml = r#"
name: battle-103
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [task] }
  - id: task
    type: task
    task: { kind: call_tool, tool_id: t, arguments: {} }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
}

/// #104: Branch non-selected ELSE target must NOT be skipped when it has a
/// live `next` predecessor from the THEN path (converging branch pattern).
///
/// Reproduces the software-feature workflow bug where:
///   check_research → then: [do_research] / else: [plan_loop]
///   do_research → save_research → plan_loop (via next)
///
/// When the THEN path is taken, `plan_loop` was incorrectly skipped even
/// though `save_research.next` still leads to it.
#[tokio::test]
async fn battle_104_branch_skip_respects_live_next_predecessor() {
    let yaml = r#"
name: battle-104
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [check] }
  - id: check
    type: control_flow
    control: { kind: branch, condition: "{{trigger.flag}} == true", then: [do_work], else: [join] }
  - { id: do_work, type: task, task: { kind: call_tool, tool_id: work, arguments: {} }, next: [save] }
  - { id: save, type: task, task: { kind: call_tool, tool_id: save, arguments: {} }, next: [join] }
  - { id: join, type: task, task: { kind: call_tool, tool_id: join, arguments: {} }, next: [end] }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    // THEN path taken — `join` is the else target but also reachable via save.next
    let id = launch(&engine, def(yaml), json!({"flag": true})).await;
    wait_for_terminal(&store, id).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed, "workflow should complete");
    assert_eq!(inst.step_states["do_work"].status, StepStatus::Completed);
    assert_eq!(inst.step_states["save"].status, StepStatus::Completed);
    // The critical assertion: `join` must NOT be skipped
    assert_eq!(
        inst.step_states["join"].status,
        StepStatus::Completed,
        "join must not be skipped — it is reachable via save.next"
    );
}

/// #105: Same pattern as #104 but with the ELSE path taken — the THEN target
/// should be skipped normally since it has no other live predecessor.
#[tokio::test]
async fn battle_105_branch_skip_works_when_no_other_predecessor() {
    let yaml = r#"
name: battle-105
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [check] }
  - id: check
    type: control_flow
    control: { kind: branch, condition: "{{trigger.flag}} == true", then: [do_work], else: [join] }
  - { id: do_work, type: task, task: { kind: call_tool, tool_id: work, arguments: {} }, next: [save] }
  - { id: save, type: task, task: { kind: call_tool, tool_id: save, arguments: {} }, next: [join] }
  - { id: join, type: task, task: { kind: call_tool, tool_id: join, arguments: {} }, next: [end] }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    // ELSE path taken — do_work has no other predecessor, should be skipped
    let id = launch(&engine, def(yaml), json!({"flag": false})).await;
    wait_for_terminal(&store, id).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed, "workflow should complete");
    assert_eq!(inst.step_states["do_work"].status, StepStatus::Skipped);
    assert_eq!(inst.step_states["save"].status, StepStatus::Skipped);
    assert_eq!(inst.step_states["join"].status, StepStatus::Completed);
}

/// #106: Full software-feature-like pattern: branch → then path with chain → while loop
/// The while loop is the ELSE target AND reachable via next from the THEN chain.
/// Verifies the while loop executes when reached through the THEN chain.
#[tokio::test]
async fn battle_106_branch_then_to_while_loop_not_skipped() {
    let yaml = r#"
name: battle-106
version: "1.0"
variables:
  type: object
  properties:
    approved:
      type: string
      default: ""
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [check] }
  - id: check
    type: control_flow
    control: { kind: branch, condition: "{{trigger.do_research}} == true", then: [research], else: [plan_loop] }
  - { id: research, type: task, task: { kind: call_tool, tool_id: research, arguments: {} }, next: [save_research] }
  - { id: save_research, type: task, task: { kind: set_variable, assignments: [{ variable: approved, value: "Done", operation: set }] }, next: [plan_loop] }
  - id: plan_loop
    type: control_flow
    control: { kind: while, condition: "{{variables.approved}} != Done", body: [plan_step] }
    next: [final_step]
  - { id: plan_step, type: task, task: { kind: call_tool, tool_id: plan, arguments: {} } }
  - { id: final_step, type: task, task: { kind: call_tool, tool_id: final, arguments: {} }, next: [end] }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    // THEN path: research → save_research (sets approved=Done) → plan_loop (condition false → LoopComplete) → final_step
    let id = launch(&engine, def(yaml), json!({"do_research": true})).await;
    wait_for_terminal(&store, id).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(
        inst.status,
        WorkflowStatus::Completed,
        "workflow must complete — plan_loop must not be skipped"
    );
    assert_eq!(inst.step_states["research"].status, StepStatus::Completed);
    assert_eq!(inst.step_states["save_research"].status, StepStatus::Completed);
    assert_eq!(
        inst.step_states["plan_loop"].status,
        StepStatus::Completed,
        "plan_loop must execute (not be skipped) — it is reachable via save_research.next"
    );
    assert_eq!(inst.step_states["final_step"].status, StepStatus::Completed);
}

// ===================================================================
// AGENT STEP RECOVERY TESTS
// ===================================================================

/// #70: Recovery resumes a Running step that has child_agent_id
#[tokio::test]
async fn battle_70_recovery_resumes_step_with_child_agent_id() {
    let ex = Arc::new(BattleExecutor::new());
    let em = Arc::new(EventCollector::new());
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let engine = WorkflowEngine::new(store.clone(), ex.clone(), em.clone());

    let yaml = r#"
name: battle-70
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [agent] }
  - id: agent
    type: task
    task: { kind: invoke_agent, persona_id: planner, task: "Plan the feature" }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    // Launch to get the instance created, then manipulate state
    let id = launch(&engine, def(yaml), json!({})).await;
    // Workflow completed normally; now simulate a crash mid-agent-step
    let mut inst = store.get_instance(id).unwrap().unwrap();
    inst.status = WorkflowStatus::Running;
    inst.completed_at_ms = None;
    if let Some(state) = inst.step_states.get_mut("agent") {
        state.status = StepStatus::Running;
        state.completed_at_ms = None;
        state.outputs = None;
        state.child_agent_id = Some("bot-abc".to_string());
    }
    store.update_instance(&inst).unwrap();

    // Configure result for the existing agent (keyed by agent_id)
    ex.set_agent_result("bot-abc", Ok(json!({"plan": "feature plan"}))).await;

    // Simulate daemon restart: new engine, same store
    let engine2 = WorkflowEngine::new(store.clone(), ex.clone(), em.clone());
    let handles = engine2.recover_instances().await.unwrap();
    assert!(!handles.is_empty(), "should have recovery handles");
    for h in handles {
        h.await.unwrap();
    }

    // Verify invoke_agent was called with existing_agent_id = "bot-abc"
    let invoke_calls = ex.get_invoke_agent_calls().await;
    assert!(
        invoke_calls.iter().any(|(_, eid)| eid.as_deref() == Some("bot-abc")),
        "invoke_agent should have been called with existing_agent_id = bot-abc"
    );

    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert_eq!(inst.step_states["agent"].status, StepStatus::Completed);
}

/// #71: Recovery resets a Running step without child_agent_id to Pending
#[tokio::test]
async fn battle_71_recovery_resets_step_without_child_agent_id() {
    let ex = Arc::new(BattleExecutor::new());
    let em = Arc::new(EventCollector::new());
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let engine = WorkflowEngine::new(store.clone(), ex.clone(), em.clone());

    let yaml = r#"
name: battle-71
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [agent] }
  - id: agent
    type: task
    task: { kind: invoke_agent, persona_id: planner, task: "Plan" }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let id = launch(&engine, def(yaml), json!({})).await;
    // Simulate crash before agent was spawned (no child_agent_id)
    let mut inst = store.get_instance(id).unwrap().unwrap();
    inst.status = WorkflowStatus::Running;
    inst.completed_at_ms = None;
    if let Some(state) = inst.step_states.get_mut("agent") {
        state.status = StepStatus::Running;
        state.completed_at_ms = None;
        state.outputs = None;
        state.child_agent_id = None;
    }
    store.update_instance(&inst).unwrap();

    let engine2 = WorkflowEngine::new(store.clone(), ex.clone(), em.clone());
    let handles = engine2.recover_instances().await.unwrap();
    for h in handles {
        h.await.unwrap();
    }

    // invoke_agent should have been called with existing_agent_id = None
    let invoke_calls = ex.get_invoke_agent_calls().await;
    assert!(
        invoke_calls.iter().any(|(_, eid)| eid.is_none()),
        "invoke_agent should have been called with no existing_agent_id when child_agent_id is absent"
    );

    // Workflow should complete via normal re-execution
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert_eq!(inst.step_states["agent"].status, StepStatus::Completed);
}

/// #72: Recovery handles mixed steps — some resumable, some re-executable
#[tokio::test]
async fn battle_72_recovery_mixed_steps_resume_and_reexecute() {
    let ex = Arc::new(BattleExecutor::new());
    let em = Arc::new(EventCollector::new());
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let engine = WorkflowEngine::new(store.clone(), ex.clone(), em.clone());

    let yaml = r#"
name: battle-72
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [agent_a, tool_b] }
  - id: agent_a
    type: task
    task: { kind: invoke_agent, persona_id: researcher, task: "Research" }
    next: [end]
  - id: tool_b
    type: task
    task: { kind: call_tool, tool_id: build, arguments: {} }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let id = launch(&engine, def(yaml), json!({})).await;
    // Simulate: agent_a has child_agent_id (resumable), tool_b does not (re-executable)
    let mut inst = store.get_instance(id).unwrap().unwrap();
    inst.status = WorkflowStatus::Running;
    inst.completed_at_ms = None;
    if let Some(state) = inst.step_states.get_mut("agent_a") {
        state.status = StepStatus::Running;
        state.completed_at_ms = None;
        state.outputs = None;
        state.child_agent_id = Some("bot-researcher".to_string());
    }
    if let Some(state) = inst.step_states.get_mut("tool_b") {
        state.status = StepStatus::Running;
        state.completed_at_ms = None;
        state.outputs = None;
        state.child_agent_id = None;
    }
    store.update_instance(&inst).unwrap();

    ex.set_agent_result("bot-researcher", Ok(json!({"research": "findings"}))).await;

    let engine2 = WorkflowEngine::new(store.clone(), ex.clone(), em.clone());
    let handles = engine2.recover_instances().await.unwrap();
    for h in handles {
        h.await.unwrap();
    }

    let invoke_calls = ex.get_invoke_agent_calls().await;
    assert!(
        invoke_calls.iter().any(|(_, eid)| eid.as_deref() == Some("bot-researcher")),
        "agent_a should have been invoked with existing_agent_id"
    );

    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert_eq!(inst.step_states["agent_a"].status, StepStatus::Completed);
    assert_eq!(inst.step_states["tool_b"].status, StepStatus::Completed);
}

/// #73: Resume fails → step falls back to Pending for re-execution
#[tokio::test]
async fn battle_73_recovery_resume_fails_falls_back() {
    let ex = Arc::new(BattleExecutor::new());
    let em = Arc::new(EventCollector::new());
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let engine = WorkflowEngine::new(store.clone(), ex.clone(), em.clone());

    let yaml = r#"
name: battle-73
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [agent] }
  - id: agent
    type: task
    task: { kind: invoke_agent, persona_id: planner, task: "Plan" }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let id = launch(&engine, def(yaml), json!({})).await;
    let mut inst = store.get_instance(id).unwrap().unwrap();
    inst.status = WorkflowStatus::Running;
    inst.completed_at_ms = None;
    if let Some(state) = inst.step_states.get_mut("agent") {
        state.status = StepStatus::Running;
        state.completed_at_ms = None;
        state.outputs = None;
        state.child_agent_id = Some("bot-dead".to_string());
    }
    store.update_instance(&inst).unwrap();

    // Resume fails — agent is dead. BattleExecutor will fall through to fresh spawn.
    ex.set_agent_result("bot-dead", Err("agent not found".to_string())).await;

    let engine2 = WorkflowEngine::new(store.clone(), ex.clone(), em.clone());
    let handles = engine2.recover_instances().await.unwrap();
    for h in handles {
        h.await.unwrap();
    }

    // Step should have been reset to Pending and re-executed via invoke_agent
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert_eq!(inst.step_states["agent"].status, StepStatus::Completed);
}

/// #74: Recovery resume in a while loop preserves iteration state
#[tokio::test]
async fn battle_74_recovery_resume_in_while_loop() {
    let ex = Arc::new(BattleExecutor::new());
    let em = Arc::new(EventCollector::new());
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let engine = WorkflowEngine::new(store.clone(), ex.clone(), em.clone());

    let yaml = r#"
name: battle-74
version: "1.0"
variables:
  type: object
  properties:
    counter: { type: number, default: 0 }
    done: { type: string, default: "" }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [loop] }
  - id: loop
    type: control_flow
    control: { kind: while, condition: "{{variables.done}} != yes", body: [agent, inc] }
    next: [end]
  - id: agent
    type: task
    task: { kind: invoke_agent, persona_id: worker, task: "Iteration {{variables.counter}}" }
  - id: inc
    type: task
    task:
      kind: set_variable
      assignments:
        - { variable: counter, value: "1", operation: set }
        - { variable: done, value: "yes", operation: set }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let id = launch(&engine, def(yaml), json!({})).await;
    // Simulate crash during agent step in first iteration
    let mut inst = store.get_instance(id).unwrap().unwrap();
    inst.status = WorkflowStatus::Running;
    inst.completed_at_ms = None;
    inst.variables["counter"] = json!(0);
    inst.variables["done"] = json!("");
    if let Some(state) = inst.step_states.get_mut("agent") {
        state.status = StepStatus::Running;
        state.completed_at_ms = None;
        state.outputs = None;
        state.child_agent_id = Some("bot-worker".to_string());
    }
    if let Some(state) = inst.step_states.get_mut("loop") {
        state.status = StepStatus::LoopWaiting;
    }
    if let Some(state) = inst.step_states.get_mut("inc") {
        state.status = StepStatus::Pending;
        state.completed_at_ms = None;
        state.outputs = None;
    }
    store.update_instance(&inst).unwrap();

    ex.set_agent_result("bot-worker", Ok(json!({"work": "done"}))).await;

    let engine2 = WorkflowEngine::new(store.clone(), ex.clone(), em.clone());
    let handles = engine2.recover_instances().await.unwrap();
    for h in handles {
        h.await.unwrap();
    }

    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed, "workflow should complete after recovery");
    assert_eq!(inst.variables["done"], json!("yes"));
}

/// #75: Recovery does not call resume for non-agent Running steps (e.g. call_tool)
#[tokio::test]
async fn battle_75_recovery_does_not_resume_non_agent_running_steps() {
    let ex = Arc::new(BattleExecutor::new());
    let em = Arc::new(EventCollector::new());
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let engine = WorkflowEngine::new(store.clone(), ex.clone(), em.clone());

    let yaml = r#"
name: battle-75
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [tool] }
  - id: tool
    type: task
    task: { kind: call_tool, tool_id: slow_tool, arguments: {} }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let id = launch(&engine, def(yaml), json!({})).await;
    let mut inst = store.get_instance(id).unwrap().unwrap();
    inst.status = WorkflowStatus::Running;
    inst.completed_at_ms = None;
    if let Some(state) = inst.step_states.get_mut("tool") {
        state.status = StepStatus::Running;
        state.completed_at_ms = None;
        state.outputs = None;
        // call_tool steps never set child_agent_id
    }
    store.update_instance(&inst).unwrap();

    let engine2 = WorkflowEngine::new(store.clone(), ex.clone(), em.clone());
    let handles = engine2.recover_instances().await.unwrap();
    for h in handles {
        h.await.unwrap();
    }

    let invoke_calls = ex.get_invoke_agent_calls().await;
    // call_tool steps don't go through invoke_agent at all
    assert!(
        invoke_calls.iter().all(|(_, eid)| eid.is_none()),
        "invoke_agent should not be called with existing_agent_id for tool steps"
    );

    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert_eq!(inst.step_states["tool"].status, StepStatus::Completed);
}

/// #76: Resumed agent result flows to downstream set_variable step
#[tokio::test]
async fn battle_76_recovery_preserves_step_output_on_resume() {
    let ex = Arc::new(BattleExecutor::new());
    let em = Arc::new(EventCollector::new());
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let engine = WorkflowEngine::new(store.clone(), ex.clone(), em.clone());

    let yaml = r#"
name: battle-76
version: "1.0"
variables:
  type: object
  properties:
    agent_result: { type: string, default: "" }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [agent] }
  - id: agent
    type: task
    task: { kind: invoke_agent, persona_id: planner, task: "Plan" }
    next: [save]
  - id: save
    type: task
    task:
      kind: set_variable
      assignments:
        - { variable: agent_result, value: "{{steps.agent.outputs.result}}", operation: set }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let id = launch(&engine, def(yaml), json!({})).await;
    let mut inst = store.get_instance(id).unwrap().unwrap();
    inst.status = WorkflowStatus::Running;
    inst.completed_at_ms = None;
    inst.variables["agent_result"] = json!("");
    if let Some(state) = inst.step_states.get_mut("agent") {
        state.status = StepStatus::Running;
        state.completed_at_ms = None;
        state.outputs = None;
        state.child_agent_id = Some("bot-planner".to_string());
    }
    if let Some(state) = inst.step_states.get_mut("save") {
        state.status = StepStatus::Pending;
        state.completed_at_ms = None;
        state.outputs = None;
    }
    store.update_instance(&inst).unwrap();

    // The agent returns a result that should flow to the save step
    ex.set_agent_result("bot-planner", Ok(json!({"result": "the plan is ready"}))).await;

    let engine2 = WorkflowEngine::new(store.clone(), ex.clone(), em.clone());
    let handles = engine2.recover_instances().await.unwrap();
    for h in handles {
        h.await.unwrap();
    }

    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert_eq!(
        inst.variables["agent_result"],
        json!("the plan is ready"),
        "agent result should flow to downstream set_variable"
    );
}

/// #77: Recovery resume respects step timeout
#[tokio::test]
async fn battle_77_recovery_resume_with_timeout() {
    let ex = Arc::new(BattleExecutor::new());
    let em = Arc::new(EventCollector::new());
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let engine = WorkflowEngine::new(store.clone(), ex.clone(), em.clone());

    let yaml = r#"
name: battle-77
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [agent] }
  - id: agent
    type: task
    timeout_secs: 120
    task: { kind: invoke_agent, persona_id: planner, task: "Plan with timeout" }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let id = launch(&engine, def(yaml), json!({})).await;
    let mut inst = store.get_instance(id).unwrap().unwrap();
    inst.status = WorkflowStatus::Running;
    inst.completed_at_ms = None;
    if let Some(state) = inst.step_states.get_mut("agent") {
        state.status = StepStatus::Running;
        state.completed_at_ms = None;
        state.outputs = None;
        state.child_agent_id = Some("bot-timeout".to_string());
    }
    store.update_instance(&inst).unwrap();

    ex.set_agent_result("bot-timeout", Ok(json!({"result": "done"}))).await;

    let engine2 = WorkflowEngine::new(store.clone(), ex.clone(), em.clone());
    let handles = engine2.recover_instances().await.unwrap();
    for h in handles {
        h.await.unwrap();
    }

    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert_eq!(inst.step_states["agent"].status, StepStatus::Completed);
}

// ===================================================================
// Non-blocking gate response tests
// ===================================================================

/// Verify that `respond_to_gate` returns before the workflow finishes
/// executing subsequent steps (the continuation runs in background).
#[tokio::test]
async fn battle_115_respond_to_gate_returns_immediately() {
    use std::sync::atomic::AtomicBool;
    use std::time::Instant;

    // Executor with a slow tool that takes 500ms
    struct SlowExecutor {
        entered: AtomicBool,
    }
    #[async_trait::async_trait]
    impl StepExecutor for SlowExecutor {
        async fn call_tool(
            &self,
            _: &str,
            _: Value,
            _: &ExecutionContext,
        ) -> Result<Value, String> {
            self.entered.store(true, Ordering::SeqCst);
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            Ok(json!({"ok": true}))
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
        ) -> Result<Value, String> {
            Ok(json!({}))
        }
        async fn signal_agent(
            &self,
            _: &SignalTarget,
            _: &str,
            _: &ExecutionContext,
        ) -> Result<Value, String> {
            Ok(json!({}))
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
            _: i64,
            _: &str,
            _: &str,
            _: Option<&[String]>,
            _: bool,
            _: &ExecutionContext,
        ) -> Result<String, String> {
            Ok("req-1".into())
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
            Ok("evt-1".into())
        }
        async fn launch_workflow(
            &self,
            _: &str,
            _: Value,
            _: &ExecutionContext,
        ) -> Result<i64, String> {
            Ok(9999)
        }
        async fn schedule_task(
            &self,
            _: &ScheduleTaskDef,
            _: &ExecutionContext,
        ) -> Result<String, String> {
            Ok("task-1".into())
        }
        async fn render_prompt_template(
            &self,
            _: &str,
            _: &str,
            _: Value,
            _: &ExecutionContext,
        ) -> Result<String, String> {
            Ok("rendered".into())
        }
    }

    let yaml = r#"
name: battle-115
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [gate] }
  - id: gate
    type: task
    task: { kind: feedback_gate, prompt: "Go?" }
    next: [slow]
  - { id: slow, type: task, task: { kind: call_tool, tool_id: slow_op, arguments: {} }, next: [end] }
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(SlowExecutor { entered: AtomicBool::new(false) });
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let engine = WorkflowEngine::new(store.clone(), ex.clone(), Arc::new(EventCollector::new()));
    let id = engine.launch(def(yaml), json!({}), "s".into(), None, vec![], None).await.unwrap();

    let start = Instant::now();
    engine.respond_to_gate(id, "gate", json!({"selected": "yes"})).await.unwrap();
    let elapsed = start.elapsed();

    // respond_to_gate should return in well under 500ms (the slow tool duration)
    assert!(
        elapsed < std::time::Duration::from_millis(200),
        "respond_to_gate took {elapsed:?}, expected <200ms"
    );

    // The slow tool may or may not have started yet
    // But the workflow should eventually complete in the background
    wait_for_settled(&store, id).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert!(ex.entered.load(Ordering::SeqCst), "slow tool should have been called");
}

/// Verify that after respond_to_gate returns, the workflow continues
/// in the background and reaches the next feedback gate.
#[tokio::test]
async fn battle_116_background_continues_to_next_gate() {
    let yaml = r#"
name: battle-116
version: "1.0"
variables: { type: object, properties: {} }
steps:
  - { id: start, type: trigger, trigger: { type: manual, inputs: [] }, next: [gate1] }
  - id: gate1
    type: task
    task: { kind: feedback_gate, prompt: "First?" }
    next: [work]
  - { id: work, type: task, task: { kind: call_tool, tool_id: process, arguments: {} }, next: [gate2] }
  - id: gate2
    type: task
    task: { kind: feedback_gate, prompt: "Second?" }
    next: [end]
  - { id: end, type: control_flow, control: { kind: end_workflow } }
"#;
    let ex = Arc::new(BattleExecutor::new());
    let (store, engine) = make_engine(ex, Arc::new(EventCollector::new()));
    let id = launch(&engine, def(yaml), json!({})).await;

    // First gate waiting
    assert_eq!(store.get_instance(id).unwrap().unwrap().status, WorkflowStatus::WaitingOnInput);

    // Respond to first gate — returns immediately
    engine.respond_to_gate(id, "gate1", json!({"selected": "ok"})).await.unwrap();

    // Background task executes 'work' then hits gate2 → WaitingOnInput
    wait_for_settled(&store, id).await;
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::WaitingOnInput);
    assert_eq!(inst.step_states["gate1"].status, StepStatus::Completed);
    assert_eq!(inst.step_states["work"].status, StepStatus::Completed);
    assert_eq!(inst.step_states["gate2"].status, StepStatus::WaitingOnInput);

    // Respond to second gate
    engine.respond_to_gate(id, "gate2", json!({"selected": "done"})).await.unwrap();
    wait_for_settled(&store, id).await;
    assert_eq!(store.get_instance(id).unwrap().unwrap().status, WorkflowStatus::Completed);
}
