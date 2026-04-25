//! Integration tests for ForEach preview_count (progressive batch / canary).
//!
//! These verify that ForEach loops with preview_count pause after N items,
//! allow user review, and correctly resume or abort.

use async_trait::async_trait;
use hive_workflow::*;
use serde_json::{json, Value};
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
    panic!("instance {} did not reach terminal state", instance_id);
}

async fn wait_for_waiting(store: &WorkflowStore, instance_id: i64) {
    for _ in 0..200 {
        let inst = store.get_instance(instance_id).unwrap().unwrap();
        if matches!(
            inst.status,
            WorkflowStatus::WaitingOnInput
                | WorkflowStatus::Completed
                | WorkflowStatus::Failed
                | WorkflowStatus::Killed
        ) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("instance {} did not reach waiting/terminal state", instance_id);
}

// ---------------------------------------------------------------------------
// Recording executor — counts tool calls
// ---------------------------------------------------------------------------

struct CountingExecutor {
    tool_calls: Mutex<Vec<(String, Value)>>,
    feedback_requests: Mutex<Vec<(i64, String, String)>>, // (instance_id, step_id, prompt)
}

impl CountingExecutor {
    fn new() -> Self {
        Self {
            tool_calls: Mutex::new(Vec::new()),
            feedback_requests: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl StepExecutor for CountingExecutor {
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
        // Return the item being processed for verification
        let item = arguments.get("item").cloned().unwrap_or(Value::Null);
        Ok(json!({"processed": true, "item": item}))
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
        Ok(json!({"result": "done"}))
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
        instance_id: i64,
        step_id: &str,
        prompt: &str,
        _choices: Option<&[String]>,
        _allow_freeform: bool,
        _ctx: &ExecutionContext,
    ) -> Result<String, String> {
        self.feedback_requests
            .lock()
            .await
            .push((instance_id, step_id.to_string(), prompt.to_string()));
        Ok(format!("preview-req-{}", step_id))
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
        Ok("rendered".to_string())
    }
}

// ---------------------------------------------------------------------------
// Build engine helper
// ---------------------------------------------------------------------------

fn build_engine(
    store: Arc<WorkflowStore>,
    executor: Arc<dyn StepExecutor>,
) -> WorkflowEngine {
    let emitter = Arc::new(NullEventEmitter);
    WorkflowEngine::new(store, executor, emitter)
}

async fn launch(
    engine: &WorkflowEngine,
    yaml: &str,
    inputs: Value,
) -> i64 {
    let definition: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();
    engine
        .launch_with_id(
            definition,
            inputs,
            "test-session".into(),
            None,
            vec![],
            None,
            None,
            ExecutionMode::Normal,
        )
        .await
        .unwrap()
}

// ===================================================================
// T3.1: ForEach with preview_count pauses after N items
// ===================================================================

#[tokio::test]
async fn test_foreach_preview_pauses_after_n_items() {
    let yaml = r#"
name: preview-test
version: "1.0"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs:
        - name: items
          type: array
    next: [loop]
  - id: loop
    type: control_flow
    control:
      kind: for_each
      collection: "{{trigger.items}}"
      item_var: item
      body: [process]
      preview_count: 2
    next: [done]
  - id: process
    type: task
    task:
      kind: call_tool
      tool_id: my.tool
      arguments:
        item: "{{variables.item}}"
  - id: done
    type: control_flow
    control:
      kind: end_workflow
"#;

    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let executor = Arc::new(CountingExecutor::new());
    let engine = build_engine(store.clone(), executor.clone());

    let id = launch(&engine, yaml, json!({"items": [1, 2, 3, 4, 5]})).await;
    wait_for_waiting(&store, id).await;

    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::WaitingOnInput);

    // Verify exactly 2 tool calls happened (preview batch)
    assert_eq!(executor.tool_calls.lock().await.len(), 2);

    // Verify feedback request was created for the loop step
    let reqs = executor.feedback_requests.lock().await;
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].1, "loop"); // step_id
    assert!(reqs[0].2.contains("2/5")); // prompt mentions progress

    // Verify loop state has preview_paused = true
    let loop_state = inst.active_loops.get("loop").unwrap();
    assert!(loop_state.preview_paused);

    // Verify preview_results accumulated
    let results = loop_state.preview_results.as_ref().unwrap();
    assert!(!results.is_empty());
}

// ===================================================================
// T3.2: ForEach preview — "Continue All" resumes remaining items
// ===================================================================

#[tokio::test]
async fn test_foreach_preview_continue_all_resumes() {
    let yaml = r#"
name: preview-continue
version: "1.0"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs:
        - name: items
          type: array
    next: [loop]
  - id: loop
    type: control_flow
    control:
      kind: for_each
      collection: "{{trigger.items}}"
      item_var: item
      body: [process]
      preview_count: 2
    next: [done]
  - id: process
    type: task
    task:
      kind: call_tool
      tool_id: my.tool
      arguments:
        item: "{{variables.item}}"
  - id: done
    type: control_flow
    control:
      kind: end_workflow
"#;

    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let executor = Arc::new(CountingExecutor::new());
    let engine = build_engine(store.clone(), executor.clone());

    let id = launch(&engine, yaml, json!({"items": [1, 2, 3, 4, 5]})).await;
    wait_for_waiting(&store, id).await;

    // 2 calls so far
    assert_eq!(executor.tool_calls.lock().await.len(), 2);

    // Respond: Continue All
    engine
        .respond_to_gate(id, "loop", json!({"selected": "Continue All", "text": ""}))
        .await
        .unwrap();

    wait_for_terminal(&store, id).await;

    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);

    // All 5 items should have been processed
    assert_eq!(executor.tool_calls.lock().await.len(), 5);

    // Loop output should show completed
    let loop_outputs = inst.step_states["loop"].outputs.as_ref().unwrap();
    assert_eq!(loop_outputs["completed"], true);
    // iteration_count is the 0-based index of the last completed iteration
    assert_eq!(loop_outputs["iteration_count"], 4);
}

// ===================================================================
// T3.3: ForEach preview — "Abort" stops with partial results
// ===================================================================

#[tokio::test]
async fn test_foreach_preview_abort_stops() {
    let yaml = r#"
name: preview-abort
version: "1.0"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs:
        - name: items
          type: array
    next: [loop]
  - id: loop
    type: control_flow
    control:
      kind: for_each
      collection: "{{trigger.items}}"
      item_var: item
      body: [process]
      preview_count: 2
    next: [done]
  - id: process
    type: task
    task:
      kind: call_tool
      tool_id: my.tool
      arguments:
        item: "{{variables.item}}"
  - id: done
    type: control_flow
    control:
      kind: end_workflow
"#;

    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let executor = Arc::new(CountingExecutor::new());
    let engine = build_engine(store.clone(), executor.clone());

    let id = launch(&engine, yaml, json!({"items": [1, 2, 3, 4, 5]})).await;
    wait_for_waiting(&store, id).await;

    // 2 calls so far
    assert_eq!(executor.tool_calls.lock().await.len(), 2);

    // Respond: Abort
    engine
        .respond_to_gate(id, "loop", json!({"selected": "Abort", "text": ""}))
        .await
        .unwrap();

    wait_for_terminal(&store, id).await;

    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);

    // Only 2 items processed (preview batch only)
    assert_eq!(executor.tool_calls.lock().await.len(), 2);

    // Loop output should show aborted with partial info
    let loop_outputs = inst.step_states["loop"].outputs.as_ref().unwrap();
    assert_eq!(loop_outputs["aborted"], true);
    assert_eq!(loop_outputs["completed"], false);
    assert_eq!(loop_outputs["processed_count"], 2);
    assert_eq!(loop_outputs["total_count"], 5);
}

// ===================================================================
// T3.4: ForEach preview_count=0 means disabled (no pause)
// ===================================================================

#[tokio::test]
async fn test_foreach_preview_count_zero_disabled() {
    let yaml = r#"
name: preview-disabled
version: "1.0"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs:
        - name: items
          type: array
    next: [loop]
  - id: loop
    type: control_flow
    control:
      kind: for_each
      collection: "{{trigger.items}}"
      item_var: item
      body: [process]
      preview_count: 0
    next: [done]
  - id: process
    type: task
    task:
      kind: call_tool
      tool_id: my.tool
      arguments:
        item: "{{variables.item}}"
  - id: done
    type: control_flow
    control:
      kind: end_workflow
"#;

    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let executor = Arc::new(CountingExecutor::new());
    let engine = build_engine(store.clone(), executor.clone());

    let id = launch(&engine, yaml, json!({"items": [1, 2, 3, 4, 5, 6, 7, 8, 9, 10]})).await;
    wait_for_terminal(&store, id).await;

    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);

    // All 10 items processed without pause
    assert_eq!(executor.tool_calls.lock().await.len(), 10);
}

// ===================================================================
// T3.5: ForEach preview_count larger than collection skips pause
// ===================================================================

#[tokio::test]
async fn test_foreach_preview_count_exceeds_collection_no_pause() {
    let yaml = r#"
name: preview-exceeds
version: "1.0"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs:
        - name: items
          type: array
    next: [loop]
  - id: loop
    type: control_flow
    control:
      kind: for_each
      collection: "{{trigger.items}}"
      item_var: item
      body: [process]
      preview_count: 5
    next: [done]
  - id: process
    type: task
    task:
      kind: call_tool
      tool_id: my.tool
      arguments:
        item: "{{variables.item}}"
  - id: done
    type: control_flow
    control:
      kind: end_workflow
"#;

    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let executor = Arc::new(CountingExecutor::new());
    let engine = build_engine(store.clone(), executor.clone());

    let id = launch(&engine, yaml, json!({"items": [1, 2]})).await;
    wait_for_terminal(&store, id).await;

    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);

    // All 2 items processed, no pause
    assert_eq!(executor.tool_calls.lock().await.len(), 2);
}

// ===================================================================
// T3.6: ForEach without preview_count works normally (no regression)
// ===================================================================

#[tokio::test]
async fn test_foreach_no_preview_count_normal() {
    let yaml = r#"
name: no-preview
version: "1.0"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs:
        - name: items
          type: array
    next: [loop]
  - id: loop
    type: control_flow
    control:
      kind: for_each
      collection: "{{trigger.items}}"
      item_var: item
      body: [process]
    next: [done]
  - id: process
    type: task
    task:
      kind: call_tool
      tool_id: my.tool
      arguments:
        item: "{{variables.item}}"
  - id: done
    type: control_flow
    control:
      kind: end_workflow
"#;

    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let executor = Arc::new(CountingExecutor::new());
    let engine = build_engine(store.clone(), executor.clone());

    let id = launch(&engine, yaml, json!({"items": ["a", "b", "c"]})).await;
    wait_for_terminal(&store, id).await;

    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert_eq!(executor.tool_calls.lock().await.len(), 3);
}

// ===================================================================
// T3.7: Preview count 1 — pauses after first item, then continues
// ===================================================================

#[tokio::test]
async fn test_foreach_preview_count_one() {
    let yaml = r#"
name: preview-one
version: "1.0"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs:
        - name: items
          type: array
    next: [loop]
  - id: loop
    type: control_flow
    control:
      kind: for_each
      collection: "{{trigger.items}}"
      item_var: item
      body: [process]
      preview_count: 1
    next: [done]
  - id: process
    type: task
    task:
      kind: call_tool
      tool_id: my.tool
      arguments:
        item: "{{variables.item}}"
  - id: done
    type: control_flow
    control:
      kind: end_workflow
"#;

    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let executor = Arc::new(CountingExecutor::new());
    let engine = build_engine(store.clone(), executor.clone());

    let id = launch(&engine, yaml, json!({"items": [10, 20, 30]})).await;
    wait_for_waiting(&store, id).await;

    // Only 1 call after preview
    assert_eq!(executor.tool_calls.lock().await.len(), 1);

    // Continue
    engine
        .respond_to_gate(id, "loop", json!({"selected": "Continue All", "text": ""}))
        .await
        .unwrap();

    wait_for_terminal(&store, id).await;

    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert_eq!(executor.tool_calls.lock().await.len(), 3);
}

// ===================================================================
// T3.8: Preview state survives persistence round-trip
// ===================================================================

#[tokio::test]
async fn test_foreach_preview_state_persists() {
    let yaml = r#"
name: preview-persist
version: "1.0"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs:
        - name: items
          type: array
    next: [loop]
  - id: loop
    type: control_flow
    control:
      kind: for_each
      collection: "{{trigger.items}}"
      item_var: item
      body: [process]
      preview_count: 2
    next: [done]
  - id: process
    type: task
    task:
      kind: call_tool
      tool_id: my.tool
      arguments:
        item: "{{variables.item}}"
  - id: done
    type: control_flow
    control:
      kind: end_workflow
"#;

    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let executor = Arc::new(CountingExecutor::new());
    let engine = build_engine(store.clone(), executor.clone());

    let id = launch(&engine, yaml, json!({"items": [1, 2, 3, 4]})).await;
    wait_for_waiting(&store, id).await;

    // Verify state survives store round-trip
    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::WaitingOnInput);

    let loop_state = inst.active_loops.get("loop").unwrap();
    assert!(loop_state.preview_paused);
    assert!(loop_state.preview_results.is_some());
    assert_eq!(loop_state.collection.as_ref().unwrap().len(), 4);

    // Serialize and deserialize the loop state
    let json = serde_json::to_string(loop_state).unwrap();
    let restored: LoopState = serde_json::from_str(&json).unwrap();
    assert!(restored.preview_paused);
    assert!(restored.preview_results.is_some());

    // Resuming should still work after persistence round-trip
    engine
        .respond_to_gate(id, "loop", json!({"selected": "Continue All", "text": ""}))
        .await
        .unwrap();

    wait_for_terminal(&store, id).await;

    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert_eq!(executor.tool_calls.lock().await.len(), 4);
}

// ===================================================================
// T3.9: Preview_count with empty collection — no pause, no error
// ===================================================================

#[tokio::test]
async fn test_foreach_preview_empty_collection() {
    let yaml = r#"
name: preview-empty
version: "1.0"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs:
        - name: items
          type: array
    next: [loop]
  - id: loop
    type: control_flow
    control:
      kind: for_each
      collection: "{{trigger.items}}"
      item_var: item
      body: [process]
      preview_count: 3
    next: [done]
  - id: process
    type: task
    task:
      kind: call_tool
      tool_id: my.tool
      arguments:
        item: "{{variables.item}}"
  - id: done
    type: control_flow
    control:
      kind: end_workflow
"#;

    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let executor = Arc::new(CountingExecutor::new());
    let engine = build_engine(store.clone(), executor.clone());

    let id = launch(&engine, yaml, json!({"items": []})).await;
    wait_for_terminal(&store, id).await;

    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert_eq!(executor.tool_calls.lock().await.len(), 0);
}

// ===================================================================
// T3.10: preview_count equal to collection length — no pause
// ===================================================================

#[tokio::test]
async fn test_foreach_preview_count_equals_collection() {
    let yaml = r#"
name: preview-equals
version: "1.0"
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs:
        - name: items
          type: array
    next: [loop]
  - id: loop
    type: control_flow
    control:
      kind: for_each
      collection: "{{trigger.items}}"
      item_var: item
      body: [process]
      preview_count: 3
    next: [done]
  - id: process
    type: task
    task:
      kind: call_tool
      tool_id: my.tool
      arguments:
        item: "{{variables.item}}"
  - id: done
    type: control_flow
    control:
      kind: end_workflow
"#;

    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let executor = Arc::new(CountingExecutor::new());
    let engine = build_engine(store.clone(), executor.clone());

    // Collection has exactly 3 items, preview_count is 3
    // The pause fires when next_iteration == 3, but collection.len() == 3
    // so the next check is `3 < 3` which is false → LoopComplete
    // Pause condition also requires `next_iteration == preview_count && next_iteration < total`
    // So it should not pause when preview_count == collection length
    let id = launch(&engine, yaml, json!({"items": ["a", "b", "c"]})).await;
    wait_for_terminal(&store, id).await;

    let inst = store.get_instance(id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert_eq!(executor.tool_calls.lock().await.len(), 3);
}
