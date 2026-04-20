//! Integration tests proving plugin events can trigger workflows via TriggerManager.
//!
//! These tests simulate the full flow: a plugin emits an event → EventBus publishes
//! with topic `plugin.event.<plugin>.<event>` → TriggerManager matches EventPattern
//! triggers → launches workflow.

use hive_core::EventBus;
use hive_workflow::store::{WorkflowPersistence, WorkflowStore};
use hive_workflow::types::*;
use hive_workflow_service::{TriggerManager, WorkflowService};
use serde_json::json;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn plugin_event_trigger_yaml(name: &str, version: &str, topic: &str) -> String {
    format!(
        r#"
name: {name}
version: "{version}"
description: "Plugin event-triggered workflow"
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

// ===========================================================================
// 1. Plugin event pattern trigger fires (exact topic match)
// ===========================================================================

/// Publish `plugin.event.test.hello` on the EventBus, verify a workflow with
/// an exact EventPattern trigger on that topic gets launched.
#[tokio::test]
async fn test_plugin_event_pattern_trigger_fires() {
    let bus = EventBus::new(128);
    let store: Arc<dyn WorkflowPersistence> = Arc::new(WorkflowStore::in_memory().unwrap());
    let tm = Arc::new(TriggerManager::new(bus.clone(), Arc::clone(&store)));
    let svc = Arc::new(WorkflowService::in_memory().unwrap());
    tm.set_workflow_service(Arc::clone(&svc)).await;

    // Save and register a workflow triggered by plugin.event.test.hello
    let yaml = plugin_event_trigger_yaml(
        "user/plugin-hello-wf",
        "1.0",
        "plugin.event.test.hello",
    );
    svc.save_definition(&yaml).await.unwrap();
    let (def, _) = svc.get_definition("user/plugin-hello-wf", "1.0").await.unwrap();
    tm.register_definition(&def).await;

    // Start TriggerManager background loop
    tm.start().await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Simulate plugin emitting event (PluginHost broadcasts → EventBus)
    bus.publish(
        "plugin.event.test.hello",
        "plugin:test-plugin",
        json!({"key": "value", "from_plugin": true}),
    )
    .unwrap();

    // Wait for trigger evaluation and workflow launch
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let result = svc.list_instances(&InstanceFilter::default()).await.unwrap();
    assert_eq!(
        result.total, 1,
        "expected exactly one workflow instance launched by plugin event trigger"
    );
    assert_eq!(result.items[0].definition_name, "user/plugin-hello-wf");

    tm.stop().await;
}

// ===========================================================================
// 2. Plugin event wildcard trigger matches
// ===========================================================================

/// Register a wildcard trigger `plugin.event.test.*` and publish
/// `plugin.event.test.hello` — verify the wildcard matches and fires.
#[tokio::test]
async fn test_plugin_event_wildcard_trigger() {
    let bus = EventBus::new(128);
    let store: Arc<dyn WorkflowPersistence> = Arc::new(WorkflowStore::in_memory().unwrap());
    let tm = Arc::new(TriggerManager::new(bus.clone(), Arc::clone(&store)));
    let svc = Arc::new(WorkflowService::in_memory().unwrap());
    tm.set_workflow_service(Arc::clone(&svc)).await;

    // Wildcard trigger: matches any event under plugin.event.test.*
    let yaml = plugin_event_trigger_yaml(
        "user/plugin-wildcard-wf",
        "1.0",
        "plugin.event.test.*",
    );
    svc.save_definition(&yaml).await.unwrap();
    let (def, _) = svc.get_definition("user/plugin-wildcard-wf", "1.0").await.unwrap();
    tm.register_definition(&def).await;

    // Start TriggerManager
    tm.start().await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Publish event that should match the wildcard
    bus.publish(
        "plugin.event.test.hello",
        "plugin:test-plugin",
        json!({"action": "greet"}),
    )
    .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let result = svc.list_instances(&InstanceFilter::default()).await.unwrap();
    assert_eq!(
        result.total, 1,
        "wildcard trigger plugin.event.test.* should match plugin.event.test.hello"
    );
    assert_eq!(result.items[0].definition_name, "user/plugin-wildcard-wf");

    tm.stop().await;
}

// ===========================================================================
// 3. Plugin event no match → no trigger
// ===========================================================================

/// Register a trigger for `plugin.event.orders.created`, publish
/// `plugin.event.test.hello` — verify NO workflow is launched.
#[tokio::test]
async fn test_plugin_event_no_match_no_trigger() {
    let bus = EventBus::new(128);
    let store: Arc<dyn WorkflowPersistence> = Arc::new(WorkflowStore::in_memory().unwrap());
    let tm = Arc::new(TriggerManager::new(bus.clone(), Arc::clone(&store)));
    let svc = Arc::new(WorkflowService::in_memory().unwrap());
    tm.set_workflow_service(Arc::clone(&svc)).await;

    // Trigger listens for orders.created — NOT test.hello
    let yaml = plugin_event_trigger_yaml(
        "user/plugin-orders-wf",
        "1.0",
        "plugin.event.orders.created",
    );
    svc.save_definition(&yaml).await.unwrap();
    let (def, _) = svc.get_definition("user/plugin-orders-wf", "1.0").await.unwrap();
    tm.register_definition(&def).await;

    // Start TriggerManager
    tm.start().await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Publish a non-matching event
    bus.publish(
        "plugin.event.test.hello",
        "plugin:test-plugin",
        json!({"key": "value"}),
    )
    .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let result = svc.list_instances(&InstanceFilter::default()).await.unwrap();
    assert_eq!(
        result.total, 0,
        "non-matching plugin event should NOT trigger any workflow"
    );

    tm.stop().await;
}

// ===========================================================================
// 4. Broad wildcard `plugin.event.*` matches deeper topics
// ===========================================================================

/// Register `plugin.event.*` and verify it matches `plugin.event.test.hello`
/// (wildcard spans across dot-separated segments).
#[tokio::test]
async fn test_plugin_event_broad_wildcard_trigger() {
    let bus = EventBus::new(128);
    let store: Arc<dyn WorkflowPersistence> = Arc::new(WorkflowStore::in_memory().unwrap());
    let tm = Arc::new(TriggerManager::new(bus.clone(), Arc::clone(&store)));
    let svc = Arc::new(WorkflowService::in_memory().unwrap());
    tm.set_workflow_service(Arc::clone(&svc)).await;

    let yaml = plugin_event_trigger_yaml(
        "user/plugin-broad-wf",
        "1.0",
        "plugin.event.*",
    );
    svc.save_definition(&yaml).await.unwrap();
    let (def, _) = svc.get_definition("user/plugin-broad-wf", "1.0").await.unwrap();
    tm.register_definition(&def).await;

    tm.start().await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    bus.publish(
        "plugin.event.test.hello",
        "plugin:test-plugin",
        json!({"broad": true}),
    )
    .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let result = svc.list_instances(&InstanceFilter::default()).await.unwrap();
    assert_eq!(
        result.total, 1,
        "broad wildcard plugin.event.* should match plugin.event.test.hello"
    );

    tm.stop().await;
}

// ===========================================================================
// 5. Multiple plugin events only fire matching triggers
// ===========================================================================

/// Register two workflows with different plugin event topics, publish events
/// for both, and verify each only triggers its own workflow.
#[tokio::test]
async fn test_plugin_event_multiple_triggers_selective() {
    let bus = EventBus::new(128);
    let store: Arc<dyn WorkflowPersistence> = Arc::new(WorkflowStore::in_memory().unwrap());
    let tm = Arc::new(TriggerManager::new(bus.clone(), Arc::clone(&store)));
    let svc = Arc::new(WorkflowService::in_memory().unwrap());
    tm.set_workflow_service(Arc::clone(&svc)).await;

    // Register two different plugin-triggered workflows
    let yaml_a = plugin_event_trigger_yaml(
        "user/plugin-alpha-wf",
        "1.0",
        "plugin.event.alpha.action",
    );
    svc.save_definition(&yaml_a).await.unwrap();
    let (def_a, _) = svc.get_definition("user/plugin-alpha-wf", "1.0").await.unwrap();
    tm.register_definition(&def_a).await;

    let yaml_b = plugin_event_trigger_yaml(
        "user/plugin-beta-wf",
        "1.0",
        "plugin.event.beta.action",
    );
    svc.save_definition(&yaml_b).await.unwrap();
    let (def_b, _) = svc.get_definition("user/plugin-beta-wf", "1.0").await.unwrap();
    tm.register_definition(&def_b).await;

    tm.start().await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Only publish alpha event
    bus.publish(
        "plugin.event.alpha.action",
        "plugin:alpha-plugin",
        json!({"source": "alpha"}),
    )
    .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let result = svc.list_instances(&InstanceFilter::default()).await.unwrap();
    assert_eq!(result.total, 1, "only the alpha workflow should have been launched");
    assert_eq!(result.items[0].definition_name, "user/plugin-alpha-wf");

    tm.stop().await;
}
