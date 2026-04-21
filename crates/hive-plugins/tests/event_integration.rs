//! Integration tests proving plugin events flow through EventBus and EventLog.
//!
//! Pipeline under test:
//!   Plugin → `host/emitEvent` → PluginHost::subscribe_events() → EventBus → EventLog

mod helpers;

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tokio::time::timeout;

use helpers::PluginTestEnv;
use hive_core::{EventBus, EventLog, QueuedSubscriber};

/// Spawn the forwarding task that bridges PluginHost events into the EventBus.
/// Replicates the pattern from `hive-api/src/lib.rs`.
fn spawn_event_forwarder(env: &PluginTestEnv, bus: &EventBus) {
    let mut plugin_rx = env.host.subscribe_events();
    let bus = bus.clone();
    tokio::spawn(async move {
        loop {
            match plugin_rx.recv().await {
                Ok(evt) => {
                    let topic = format!("plugin.event.{}", evt.event_type);
                    let source = format!("plugin:{}", evt.plugin_id);
                    let _ = bus.publish(&topic, &source, evt.payload);
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    });
}

#[tokio::test]
async fn test_plugin_event_reaches_eventbus() {
    let env = PluginTestEnv::new().await.expect("setup test env");
    let bus = EventBus::new(1024);

    // Subscribe before emitting so we don't miss the event.
    let mut rx = bus.subscribe_queued("plugin.event");

    // Wire forwarding from PluginHost → EventBus.
    spawn_event_forwarder(&env, &bus);

    // Emit an event via the test plugin tool.
    env.call_tool(
        "emit_test_event",
        json!({
            "eventType": "test.hello",
            "payload": { "key": "value" }
        }),
    )
    .await
    .expect("emit_test_event tool call");

    // Wait for the event to arrive on the bus.
    let envelope = timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("timeout waiting for event")
        .expect("channel closed");

    assert_eq!(envelope.topic, "plugin.event.test.hello");
    assert!(
        envelope.source.starts_with("plugin:"),
        "source should start with 'plugin:' but was: {}",
        envelope.source
    );
    assert_eq!(envelope.payload["key"], "value");

    env.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn test_plugin_event_stored_in_eventlog() {
    let env = PluginTestEnv::new().await.expect("setup test env");
    let bus = EventBus::new(1024);

    // Create an in-memory EventLog and register it as a queued subscriber.
    let event_log = EventLog::in_memory().expect("create in-memory event log");
    let event_log = Arc::new(event_log);
    bus.register_subscriber(event_log.clone() as Arc<dyn QueuedSubscriber>);

    // Wire forwarding.
    spawn_event_forwarder(&env, &bus);

    // Emit event via plugin.
    env.call_tool(
        "emit_test_event",
        json!({
            "eventType": "test.stored",
            "payload": { "stored": true }
        }),
    )
    .await
    .expect("emit_test_event tool call");

    // Allow async processing to complete.
    event_log.flush().await;

    // Query the EventLog.
    let results = event_log.query_events(Some("plugin.event"), None, None, None, Some(100));

    assert!(!results.is_empty(), "EventLog should contain at least one plugin event");

    let stored = results
        .iter()
        .find(|e| e.topic == "plugin.event.test.stored")
        .expect("should find event with topic plugin.event.test.stored");

    assert!(stored.source.starts_with("plugin:"), "source should start with 'plugin:'",);
    assert_eq!(stored.payload["stored"], true);

    env.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn test_plugin_loop_events_reach_eventbus() {
    let env = PluginTestEnv::new().await.expect("setup test env");
    let bus = EventBus::new(1024);

    // Subscribe for loop tick events.
    let mut rx = bus.subscribe_queued("plugin.event.test.loop_tick");

    // Wire forwarding.
    spawn_event_forwarder(&env, &bus);

    // Start the plugin loop (emits test.loop_tick events every pollInterval seconds).
    env.start_loop().await.expect("start loop");

    // Wait for at least one loop tick event.
    let envelope = timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("timeout waiting for loop_tick event")
        .expect("channel closed");

    assert_eq!(envelope.topic, "plugin.event.test.loop_tick");
    assert!(envelope.source.starts_with("plugin:"), "source should start with 'plugin:'");
    // The loop tick payload should have a tick count.
    assert!(
        envelope.payload.get("tick").is_some(),
        "loop_tick event should have a 'tick' field in payload, got: {}",
        envelope.payload
    );

    env.stop_loop().await.expect("stop loop");
    env.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn test_multiple_plugin_events_ordered() {
    let env = PluginTestEnv::new().await.expect("setup test env");
    let bus = EventBus::new(1024);

    // Subscribe before emitting.
    let mut rx = bus.subscribe_queued("plugin.event.test.seq");

    // Wire forwarding.
    spawn_event_forwarder(&env, &bus);

    // Emit 5 events rapidly.
    for i in 0..5 {
        env.call_tool(
            "emit_test_event",
            json!({
                "eventType": "test.seq",
                "payload": { "index": i }
            }),
        )
        .await
        .expect("emit_test_event tool call");
    }

    // Collect all 5 events.
    let mut received = Vec::new();
    let deadline = Duration::from_secs(10);
    let start = tokio::time::Instant::now();

    while received.len() < 5 {
        let remaining = deadline.saturating_sub(start.elapsed());
        if remaining.is_zero() {
            panic!("timeout: only received {}/5 events", received.len());
        }
        match timeout(remaining, rx.recv()).await {
            Ok(Some(env)) => received.push(env),
            Ok(None) => panic!("channel closed after {} events", received.len()),
            Err(_) => panic!("timeout: only received {}/5 events", received.len()),
        }
    }

    assert_eq!(received.len(), 5);

    // Verify order.
    for (i, envelope) in received.iter().enumerate() {
        assert_eq!(envelope.topic, "plugin.event.test.seq");
        assert_eq!(
            envelope.payload["index"], i,
            "event {} has wrong index: expected {}, got {}",
            i, i, envelope.payload["index"]
        );
    }

    env.shutdown().await.expect("shutdown");
}
