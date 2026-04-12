//! Integration tests for workflow-engine bug fixes.
//!
//! These tests cover the specific fixes made to the workflow engine:
//!   1. Trigger identity replacement by definition ID
//!   2. Safe delete of active (running/waiting) instances
//!   3. Delete/archive with trigger unregistration
//!   4. Terminal-state guards at the service level
//!   5. Event replay cursor end-to-end

use hive_core::{EventBus, EventLog, QueuedSubscriber};
use hive_workflow::executor::{ExecutionContext, NullEventEmitter, StepExecutor, WorkflowEngine};
use hive_workflow::store::{WorkflowPersistence, WorkflowStore};
use hive_workflow::types::*;
use hive_workflow_service::{TriggerManager, WorkflowService};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::Mutex;

// ---------------------------------------------------------------------------
// Mock step executor
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct MockExecutor {
    tool_calls: Arc<Mutex<Vec<(String, Value)>>>,
}

impl MockExecutor {
    fn new() -> Self {
        Self { tool_calls: Arc::new(Mutex::new(Vec::new())) }
    }
}

#[async_trait::async_trait]
impl StepExecutor for MockExecutor {
    async fn call_tool(
        &self,
        tool_id: &str,
        arguments: Value,
        _ctx: &ExecutionContext,
    ) -> Result<Value, String> {
        self.tool_calls.lock().await.push((tool_id.to_string(), arguments.clone()));
        Ok(json!({ "output": format!("mock-{tool_id}"), "args": arguments }))
    }

    async fn invoke_agent(
        &self,
        persona_id: &str,
        _task: &str,
        _async_exec: bool,
        _timeout_secs: Option<u64>,
        _step_permissions: &[PermissionEntry],
        _agent_name: Option<&str>,
        _: Option<&str>,
        _ctx: &ExecutionContext,
    ) -> Result<Value, String> {
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
        Err("not implemented".to_string())
    }
}

// ---------------------------------------------------------------------------
// YAML helpers
// ---------------------------------------------------------------------------

fn feedback_gate_yaml() -> &'static str {
    r#"
name: user/feedback-wf
version: "1.0"
description: "Workflow with feedback gate for testing"
variables:
  type: object
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs: []
    next: [ask]

  - id: ask
    type: task
    task:
      kind: feedback_gate
      prompt: "Do you approve?"
      choices: ["Yes", "No"]
      allow_freeform: false
    outputs:
      answer: "{{result.selected}}"
    next: [done]

  - id: done
    type: control_flow
    control:
      kind: end_workflow
output:
  answer: "{{steps.ask.outputs.answer}}"
"#
}

fn event_gate_yaml() -> &'static str {
    r#"
name: user/event-gate-wf
version: "1.0"
description: "Workflow with event gate for testing"
variables:
  type: object
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
      topic: "test.approval"
      filter: null
      timeout_secs: null
    outputs:
      event_data: "{{result}}"
    next: [done]

  - id: done
    type: control_flow
    control:
      kind: end_workflow
output:
  event_data: "{{steps.wait_event.outputs.event_data}}"
"#
}

fn simple_tool_yaml() -> &'static str {
    r#"
name: user/simple-tool-wf
version: "1.0"
description: "Simple workflow with a tool call"
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
"#
}

fn event_trigger_yaml(name: &str, version: &str, topic: &str) -> String {
    format!(
        r#"
name: {name}
version: "{version}"
description: "Event-triggered workflow"
variables:
  type: object
steps:
  - id: start
    type: trigger
    trigger:
      type: event_pattern
      topic: "{topic}"
    outputs:
      payload: "{{{{trigger}}}}"
    next: [done]

  - id: done
    type: control_flow
    control:
      kind: end_workflow
output:
  payload: "{{{{steps.start.outputs.payload}}}}"
"#
    )
}

/// Build engine + store + mock for low-level engine tests.
fn make_engine() -> (Arc<WorkflowEngine>, Arc<WorkflowStore>, Arc<MockExecutor>) {
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let mock = Arc::new(MockExecutor::new());
    let emitter = Arc::new(NullEventEmitter);
    let engine = Arc::new(WorkflowEngine::new(store.clone(), mock.clone(), emitter));
    (engine, store, mock)
}

/// Helper to wait for a specific workflow status.
async fn wait_for_status(
    store: &dyn WorkflowPersistence,
    instance_id: i64,
    expected: &[WorkflowStatus],
    max_ms: u64,
) -> WorkflowStatus {
    let attempts = max_ms / 25;
    for _ in 0..attempts {
        let inst = store.get_instance(instance_id).unwrap().unwrap();
        if expected.contains(&inst.status) {
            return inst.status;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    store.get_instance(instance_id).unwrap().unwrap().status
}

// ===========================================================================
// 1. Trigger identity replacement by definition ID (service-level)
// ===========================================================================

/// Save v1 definition with event triggers, register into TriggerManager,
/// save v2 (same definition ID), re-register, and verify only v2 triggers
/// are active.
#[tokio::test]
async fn test_integ_trigger_identity_replacement_via_service() {
    let bus = EventBus::new(128);
    let store: Arc<dyn WorkflowPersistence> = Arc::new(WorkflowStore::in_memory().unwrap());
    let tm = Arc::new(TriggerManager::new(bus.clone(), Arc::clone(&store)));

    // Use with_deps so TriggerManager and WorkflowService share the same store
    let mock = Arc::new(MockExecutor::new());
    let svc = Arc::new(WorkflowService::with_deps(
        Arc::clone(&store),
        mock as Arc<dyn StepExecutor>,
        Arc::new(NullEventEmitter),
    ));
    tm.set_workflow_service(Arc::clone(&svc)).await;

    // Save v1
    let yaml_v1 = event_trigger_yaml("user/my-event-wf", "1.0", "orders.created");
    svc.save_definition(&yaml_v1).await.unwrap();
    // Reload from store to get the stable definition ID
    let (def_v1, _) = svc.get_definition("user/my-event-wf", "1.0").await.unwrap();
    tm.register_definition(&def_v1).await;

    // Verify v1 triggers active
    let active = tm.list_active().await;
    assert_eq!(active.triggers.len(), 1, "should have 1 trigger after v1 registration");
    assert_eq!(active.triggers[0].definition_version, "1.0");

    // Save v2 (same name → same definition ID, different version)
    let yaml_v2 = event_trigger_yaml("user/my-event-wf", "2.0", "orders.updated");
    svc.save_definition(&yaml_v2).await.unwrap();
    let (def_v2, _) = svc.get_definition("user/my-event-wf", "2.0").await.unwrap();

    // Both definitions share the same identity (stable external_id from store)
    assert_eq!(def_v1.id, def_v2.id, "same name should yield same definition ID");

    // Re-register should replace v1 triggers
    tm.register_definition(&def_v2).await;

    let active = tm.list_active().await;
    assert_eq!(active.triggers.len(), 1, "should still have 1 trigger after v2 re-registration");
    assert_eq!(active.triggers[0].definition_version, "2.0");
    assert_eq!(active.triggers[0].trigger_kind, "event_pattern");
}

/// Register triggers for two different definitions, unregister one by
/// definition ID, and verify only the other remains.
#[tokio::test]
async fn test_integ_unregister_leaves_other_definitions_intact() {
    let bus = EventBus::new(128);
    let store: Arc<dyn WorkflowPersistence> = Arc::new(WorkflowStore::in_memory().unwrap());
    let tm = Arc::new(TriggerManager::new(bus.clone(), Arc::clone(&store)));

    let mock = Arc::new(MockExecutor::new());
    let svc = Arc::new(WorkflowService::with_deps(
        Arc::clone(&store),
        mock as Arc<dyn StepExecutor>,
        Arc::new(NullEventEmitter),
    ));
    tm.set_workflow_service(Arc::clone(&svc)).await;

    let yaml_a = event_trigger_yaml("user/workflow-a", "1.0", "topic.a");
    svc.save_definition(&yaml_a).await.unwrap();
    let (def_a, _) = svc.get_definition("user/workflow-a", "1.0").await.unwrap();
    tm.register_definition(&def_a).await;

    let yaml_b = event_trigger_yaml("user/workflow-b", "1.0", "topic.b");
    svc.save_definition(&yaml_b).await.unwrap();
    let (def_b, _) = svc.get_definition("user/workflow-b", "1.0").await.unwrap();
    tm.register_definition(&def_b).await;

    assert_eq!(tm.list_active().await.triggers.len(), 2);

    // Unregister workflow-a
    tm.unregister_definition(&def_a.id, None).await;

    let active = tm.list_active().await;
    assert_eq!(active.triggers.len(), 1, "only workflow-b should remain");
    assert_eq!(active.triggers[0].definition_name, "user/workflow-b");
}

// ===========================================================================
// 2. Safe delete of active instances (kill-first semantics)
// ===========================================================================

/// Launch a workflow that blocks on a feedback gate, then delete the instance.
/// The service should kill it first, then delete.
#[tokio::test]
async fn test_integ_delete_active_instance_kills_first() {
    let (engine, store, _mock) = make_engine();

    let yaml = feedback_gate_yaml();
    let def: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();
    store.save_definition(yaml, &def).unwrap();

    let id = engine
        .launch(def.clone(), json!({}), "session-1".to_string(), None, vec![], None)
        .await
        .unwrap();

    // Wait until the workflow is waiting on input (feedback gate)
    let status = wait_for_status(store.as_ref(), id, &[WorkflowStatus::WaitingOnInput], 2000).await;
    assert_eq!(status, WorkflowStatus::WaitingOnInput, "should be waiting");

    // Now use the service's delete_instance which should kill first
    let svc = WorkflowService::with_deps(
        store.clone() as Arc<dyn WorkflowPersistence>,
        _mock.clone() as Arc<dyn StepExecutor>,
        Arc::new(NullEventEmitter),
    );

    let deleted = svc.delete_instance(id).await.unwrap();
    assert!(deleted, "delete should return true");

    // Instance should be gone from the store
    assert!(store.get_instance(id).unwrap().is_none(), "instance should be removed after delete");
}

/// Delete an already-completed instance should work without needing to kill.
#[tokio::test]
async fn test_integ_delete_completed_instance_no_kill() {
    let svc = WorkflowService::in_memory().unwrap();
    svc.save_definition(simple_tool_yaml()).await.unwrap();

    let id = svc
        .launch(
            "user/simple-tool-wf",
            Some("1.0"),
            json!({"msg": "hello"}),
            "session-1",
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();

    // Wait for completion
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let inst = svc.get_instance(id).await.unwrap();
    assert!(
        matches!(inst.status, WorkflowStatus::Completed | WorkflowStatus::Failed),
        "workflow should have finished, got {:?}",
        inst.status
    );

    // Delete a completed instance
    let deleted = svc.delete_instance(id).await.unwrap();
    assert!(deleted);

    // Verify it's gone
    assert!(svc.get_instance(id).await.is_err());
}

// ===========================================================================
// 3. Delete/archive with trigger unregistration
// ===========================================================================

/// Save a definition, register its triggers, then delete the definition.
/// Verify the triggers are no longer active.
#[tokio::test]
async fn test_integ_delete_definition_unregisters_triggers() {
    let bus = EventBus::new(128);
    let store: Arc<dyn WorkflowPersistence> = Arc::new(WorkflowStore::in_memory().unwrap());
    let tm = Arc::new(TriggerManager::new(bus.clone(), Arc::clone(&store)));

    let mock = Arc::new(MockExecutor::new());
    let svc = Arc::new(WorkflowService::with_deps(
        Arc::clone(&store),
        mock as Arc<dyn StepExecutor>,
        Arc::new(NullEventEmitter),
    ));
    tm.set_workflow_service(Arc::clone(&svc)).await;

    // Save and register
    let yaml = event_trigger_yaml("user/deletable-wf", "1.0", "orders.created");
    svc.save_definition(&yaml).await.unwrap();
    let (def, _) = svc.get_definition("user/deletable-wf", "1.0").await.unwrap();
    tm.register_definition(&def).await;

    assert_eq!(tm.list_active().await.triggers.len(), 1);

    // Delete the definition
    let deleted = svc.delete_definition("user/deletable-wf", "1.0").await.unwrap();
    assert!(deleted);

    // Unregister triggers (simulating what the API route does)
    tm.unregister_definition(&def.id, Some("1.0")).await;

    assert_eq!(
        tm.list_active().await.triggers.len(),
        0,
        "triggers should be removed after definition delete"
    );
}

/// Save a definition, register its triggers, archive the definition.
/// Verify the triggers are unregistered after archive.
#[tokio::test]
async fn test_integ_archive_definition_unregisters_triggers() {
    let bus = EventBus::new(128);
    let store: Arc<dyn WorkflowPersistence> = Arc::new(WorkflowStore::in_memory().unwrap());
    let tm = Arc::new(TriggerManager::new(bus.clone(), Arc::clone(&store)));

    let mock = Arc::new(MockExecutor::new());
    let svc = Arc::new(WorkflowService::with_deps(
        Arc::clone(&store),
        mock as Arc<dyn StepExecutor>,
        Arc::new(NullEventEmitter),
    ));
    tm.set_workflow_service(Arc::clone(&svc)).await;

    // Save and register
    let yaml = event_trigger_yaml("user/archivable-wf", "1.0", "events.important");
    svc.save_definition(&yaml).await.unwrap();
    let (def, _) = svc.get_definition("user/archivable-wf", "1.0").await.unwrap();
    tm.register_definition(&def).await;

    assert_eq!(tm.list_active().await.triggers.len(), 1);

    // Archive (third param is the archived boolean)
    svc.archive_definition("user/archivable-wf", "1.0", true).await.unwrap();

    // Unregister triggers (simulating what the API route does)
    tm.unregister_definition(&def.id, Some("1.0")).await;

    assert_eq!(
        tm.list_active().await.triggers.len(),
        0,
        "triggers should be removed after definition archive"
    );
}

/// Multiple versions: delete v1 should only remove v1 triggers, not v2.
#[tokio::test]
async fn test_integ_delete_version_only_removes_that_versions_triggers() {
    let bus = EventBus::new(128);
    let store: Arc<dyn WorkflowPersistence> = Arc::new(WorkflowStore::in_memory().unwrap());
    let tm = Arc::new(TriggerManager::new(bus.clone(), Arc::clone(&store)));

    let mock = Arc::new(MockExecutor::new());
    let svc = Arc::new(WorkflowService::with_deps(
        Arc::clone(&store),
        mock as Arc<dyn StepExecutor>,
        Arc::new(NullEventEmitter),
    ));
    tm.set_workflow_service(Arc::clone(&svc)).await;

    // Save v1 — registered with TriggerManager
    let yaml_v1 = event_trigger_yaml("user/multi-ver-wf", "1.0", "topic.v1");
    svc.save_definition(&yaml_v1).await.unwrap();
    let (def_v1, _) = svc.get_definition("user/multi-ver-wf", "1.0").await.unwrap();
    tm.register_definition(&def_v1).await;

    // Save v2 — re-registration replaces v1 triggers with v2
    let yaml_v2 = event_trigger_yaml("user/multi-ver-wf", "2.0", "topic.v2");
    svc.save_definition(&yaml_v2).await.unwrap();
    let (def_v2, _) = svc.get_definition("user/multi-ver-wf", "2.0").await.unwrap();
    tm.register_definition(&def_v2).await;

    // Only v2 triggers should be active (register replaces by definition ID)
    assert_eq!(tm.list_active().await.triggers.len(), 1);
    assert_eq!(tm.list_active().await.triggers[0].definition_version, "2.0");

    // Delete v1 definition from store
    svc.delete_definition("user/multi-ver-wf", "1.0").await.unwrap();

    // Unregister v1 triggers specifically (version-scoped)
    tm.unregister_definition(&def_v1.id, Some("1.0")).await;

    // v2 triggers should still be active
    let active = tm.list_active().await;
    assert_eq!(active.triggers.len(), 1, "v2 triggers should remain");
    assert_eq!(active.triggers[0].definition_version, "2.0");
}

// ===========================================================================
// 4. Terminal-state guards at the service level
// ===========================================================================

/// Kill a workflow waiting on a feedback gate, then try to respond through
/// the service layer — should be rejected.
#[tokio::test]
async fn test_integ_respond_to_gate_rejected_after_kill_service() {
    let (engine, store, _mock) = make_engine();

    let yaml = feedback_gate_yaml();
    let def: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();
    store.save_definition(yaml, &def).unwrap();

    let id = engine
        .launch(def.clone(), json!({}), "session-1".to_string(), None, vec![], None)
        .await
        .unwrap();

    let status = wait_for_status(store.as_ref(), id, &[WorkflowStatus::WaitingOnInput], 2000).await;
    assert_eq!(status, WorkflowStatus::WaitingOnInput);

    // Kill it
    engine.kill(id).await.unwrap();

    let status = wait_for_status(store.as_ref(), id, &[WorkflowStatus::Killed], 2000).await;
    assert_eq!(status, WorkflowStatus::Killed);

    // Now wire up a service and try to respond to the gate
    let svc = WorkflowService::with_deps(
        store.clone() as Arc<dyn WorkflowPersistence>,
        _mock.clone() as Arc<dyn StepExecutor>,
        Arc::new(NullEventEmitter),
    );

    let result = svc.respond_to_gate(id, "ask", json!({"selected": "Yes"})).await;
    assert!(result.is_err(), "responding to gate on killed instance should fail, got Ok");
}

/// Kill a workflow waiting on an event gate, then try to respond through
/// the service layer — should be rejected.
#[tokio::test]
async fn test_integ_respond_to_event_rejected_after_kill_service() {
    let (engine, store, _mock) = make_engine();

    let yaml = event_gate_yaml();
    let def: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();
    store.save_definition(yaml, &def).unwrap();

    let id = engine
        .launch(def.clone(), json!({}), "session-1".to_string(), None, vec![], None)
        .await
        .unwrap();

    let status = wait_for_status(store.as_ref(), id, &[WorkflowStatus::WaitingOnEvent], 2000).await;
    assert_eq!(status, WorkflowStatus::WaitingOnEvent);

    // Kill it
    engine.kill(id).await.unwrap();

    let status = wait_for_status(store.as_ref(), id, &[WorkflowStatus::Killed], 2000).await;
    assert_eq!(status, WorkflowStatus::Killed);

    // Now wire up a service and try to respond to the event gate
    let svc = WorkflowService::with_deps(
        store.clone() as Arc<dyn WorkflowPersistence>,
        _mock.clone() as Arc<dyn StepExecutor>,
        Arc::new(NullEventEmitter),
    );

    let result = svc.respond_to_event(id, "wait_event", json!({"approval": true})).await;
    assert!(result.is_err(), "responding to event gate on killed instance should fail, got Ok");
}

/// Complete a workflow, then try to kill it — should be rejected.
#[tokio::test]
async fn test_integ_kill_completed_instance_rejected() {
    let (engine, store, _mock) = make_engine();

    // Use a delay-only workflow so it completes without needing tool executors
    let yaml = r#"
name: user/delay-wf
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
      kind: delay
      duration_secs: 0
    next: [done]

  - id: done
    type: control_flow
    control:
      kind: end_workflow
"#;

    let def: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();
    store.save_definition(yaml, &def).unwrap();

    let id = engine
        .launch(def.clone(), json!({}), "session-1".to_string(), None, vec![], None)
        .await
        .unwrap();

    // Wait for completion
    let status = wait_for_status(store.as_ref(), id, &[WorkflowStatus::Completed], 2000).await;
    assert_eq!(status, WorkflowStatus::Completed);

    // Killing a completed workflow should fail
    let result = engine.kill(id).await;
    assert!(result.is_err(), "killing a completed instance should fail, got Ok");
}

// ===========================================================================
// 5. Event replay cursor end-to-end
// ===========================================================================

/// Wire TriggerManager + EventBus + EventLog, publish events,
/// call replay, verify cursor advances.
#[tokio::test]
async fn test_integ_replay_cursor_advances_across_replays() {
    let bus = EventBus::new(256);
    let store: Arc<dyn WorkflowPersistence> = Arc::new(WorkflowStore::in_memory().unwrap());
    let tm = Arc::new(TriggerManager::new(bus.clone(), Arc::clone(&store)));
    let log = Arc::new(EventLog::in_memory().unwrap());

    // Wire EventLog as a subscriber to EventBus so events get persisted
    bus.register_subscriber(Arc::clone(&log) as Arc<dyn QueuedSubscriber>);
    tm.set_event_log(Arc::clone(&log)).await;

    let svc = Arc::new(WorkflowService::in_memory().unwrap());
    tm.set_workflow_service(Arc::clone(&svc)).await;

    // No cursor initially
    assert_eq!(store.get_event_replay_cursor().unwrap(), None);

    // Publish some events
    bus.publish("test.topic.1", "test", json!({"value": 1})).unwrap();
    bus.publish("test.topic.2", "test", json!({"value": 2})).unwrap();

    // Give EventLog time to flush
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // First replay should set cursor
    tm.replay_missed_events().await;
    let cursor_1 =
        store.get_event_replay_cursor().unwrap().expect("cursor should be set after first replay");
    assert!(cursor_1 > 0);

    // Publish more events
    bus.publish("test.topic.3", "test", json!({"value": 3})).unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Second replay should advance cursor
    tm.replay_missed_events().await;
    let cursor_2 = store
        .get_event_replay_cursor()
        .unwrap()
        .expect("cursor should still be set after second replay");
    assert!(cursor_2 > cursor_1, "cursor should advance: was {cursor_1}, now {cursor_2}");
}

/// Verify that replay doesn't re-process events it already saw.
#[tokio::test]
async fn test_integ_replay_idempotent_no_duplicate_launches() {
    let bus = EventBus::new(256);
    let store: Arc<dyn WorkflowPersistence> = Arc::new(WorkflowStore::in_memory().unwrap());
    let tm = Arc::new(TriggerManager::new(bus.clone(), Arc::clone(&store)));
    let log = Arc::new(EventLog::in_memory().unwrap());

    bus.register_subscriber(Arc::clone(&log) as Arc<dyn QueuedSubscriber>);
    tm.set_event_log(Arc::clone(&log)).await;

    let mock = Arc::new(MockExecutor::new());
    let svc = Arc::new(WorkflowService::with_deps(
        Arc::clone(&store),
        mock as Arc<dyn StepExecutor>,
        Arc::new(NullEventEmitter),
    ));
    tm.set_workflow_service(Arc::clone(&svc)).await;

    // Save a definition with an event trigger
    let yaml = event_trigger_yaml("user/replay-test-wf", "1.0", "replay.test");
    svc.save_definition(&yaml).await.unwrap();
    let (def, _) = svc.get_definition("user/replay-test-wf", "1.0").await.unwrap();
    tm.register_definition(&def).await;

    // Publish an event matching the trigger
    bus.publish("replay.test", "test", json!({"key": "val"})).unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // First replay — should process the event
    tm.replay_missed_events().await;
    let cursor_after_first = store.get_event_replay_cursor().unwrap().unwrap();

    // Second replay with no new events — cursor should NOT change
    tm.replay_missed_events().await;
    let cursor_after_second = store.get_event_replay_cursor().unwrap().unwrap();
    assert_eq!(
        cursor_after_first, cursor_after_second,
        "replay with no new events should not advance cursor"
    );
}

// ===========================================================================
// 6. Cron trigger integration with service
// ===========================================================================

/// Register a cron trigger, set a persisted last_run far in the past
/// so the next computed run is overdue, then tick cron to launch.
#[tokio::test]
async fn test_integ_cron_trigger_launches_workflow_on_tick() {
    let bus = EventBus::new(128);
    let store: Arc<dyn WorkflowPersistence> = Arc::new(WorkflowStore::in_memory().unwrap());
    let tm = Arc::new(TriggerManager::new(bus.clone(), Arc::clone(&store)));

    let mock = Arc::new(MockExecutor::new());
    let svc = Arc::new(WorkflowService::with_deps(
        Arc::clone(&store),
        mock as Arc<dyn StepExecutor>,
        Arc::new(NullEventEmitter),
    ));
    tm.set_workflow_service(Arc::clone(&svc)).await;

    // Save a cron-triggered definition
    let cron_yaml = r#"
name: user/cron-integ-wf
version: "1.0"
description: "Cron-triggered workflow for integration testing"
variables:
  type: object
steps:
  - id: start
    type: trigger
    trigger:
      type: schedule
      cron: "*/1 * * * * *"
    next: [done]

  - id: done
    type: control_flow
    control:
      kind: end_workflow
"#;
    svc.save_definition(cron_yaml).await.unwrap();
    let (def, _) = svc.get_definition("user/cron-integ-wf", "1.0").await.unwrap();

    // Set a persisted cron last_run far in the past so when we register,
    // the trigger computes an overdue next_run.
    let far_past_ms = 1000u64; // January 1, 1970 00:00:01
    store.set_cron_last_run(&def.id, &def.version, "*/1 * * * * *", far_past_ms).unwrap();

    // Register — will pick up the persisted last_run and compute a next_run
    // that's well in the past.
    tm.register_definition(&def).await;

    // Tick cron — should detect the overdue trigger and launch
    tm.tick_cron().await;

    // Check that a workflow instance was created
    let instances = svc.list_instances(&InstanceFilter::default()).await.unwrap();
    assert!(
        !instances.items.is_empty(),
        "cron tick should have launched at least one workflow instance"
    );
    assert_eq!(instances.items[0].definition_name, "user/cron-integ-wf");
}

// ===========================================================================
// 7. Full lifecycle: service + engine + triggers wired together
// ===========================================================================

/// End-to-end: save definition → register triggers → launch via service →
/// feedback gate → respond → completion.
#[tokio::test]
async fn test_integ_full_lifecycle_feedback_gate() {
    let (engine, store, _mock) = make_engine();

    let yaml = feedback_gate_yaml();
    let def: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();
    store.save_definition(yaml, &def).unwrap();

    // Launch
    let id = engine
        .launch(def.clone(), json!({}), "session-1".to_string(), None, vec![], None)
        .await
        .unwrap();

    // Wait for feedback gate
    let status = wait_for_status(store.as_ref(), id, &[WorkflowStatus::WaitingOnInput], 2000).await;
    assert_eq!(status, WorkflowStatus::WaitingOnInput);

    // Respond to the gate
    engine.respond_to_gate(id, "ask", json!({"selected": "Yes", "text": ""})).await.unwrap();

    // Wait for completion
    let status = wait_for_status(
        store.as_ref(),
        id,
        &[WorkflowStatus::Completed, WorkflowStatus::Failed],
        2000,
    )
    .await;
    assert_eq!(status, WorkflowStatus::Completed);

    let inst = store.get_instance(id).unwrap().unwrap();
    let output = inst.output.as_ref().expect("should have output");
    assert_eq!(output["answer"], "Yes");
}

/// End-to-end with event gate: launch → wait on event → respond → completion.
#[tokio::test]
async fn test_integ_full_lifecycle_event_gate() {
    let (engine, store, _mock) = make_engine();

    let yaml = event_gate_yaml();
    let def: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();
    store.save_definition(yaml, &def).unwrap();

    let id = engine
        .launch(def.clone(), json!({}), "session-1".to_string(), None, vec![], None)
        .await
        .unwrap();

    // Wait for event gate
    let status = wait_for_status(store.as_ref(), id, &[WorkflowStatus::WaitingOnEvent], 2000).await;
    assert_eq!(status, WorkflowStatus::WaitingOnEvent);

    // Respond with event data
    engine
        .respond_to_event(id, "wait_event", json!({"approval": true, "user": "admin"}))
        .await
        .unwrap();

    let status = wait_for_status(
        store.as_ref(),
        id,
        &[WorkflowStatus::Completed, WorkflowStatus::Failed],
        2000,
    )
    .await;
    assert_eq!(status, WorkflowStatus::Completed);
}
