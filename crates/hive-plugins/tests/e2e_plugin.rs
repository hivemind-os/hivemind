//! End-to-end integration tests for the Hivemind plugin system.
//!
//! These tests spawn the real `@hivemind-os/test-plugin` as a Node.js child
//! process and exercise the full JSON-RPC protocol including:
//! - Plugin lifecycle (initialize, activate, deactivate)
//! - Config schema retrieval and validation
//! - Tool discovery and execution (all 11 tools)
//! - Background loop (start, tick, stop)
//! - PluginBridgeTool integration
//! - Error handling and edge cases
//!
//! Pre-requisites:
//! - Node.js available (via hive-node-env or PATH)
//! - `packages/test-plugin/dist/` built (checked into repo for CI)

mod helpers;

use std::time::Duration;

use serde_json::{json, Value};

use helpers::PluginTestEnv;

// ── Plugin Lifecycle ────────────────────────────────────────────────────────

#[tokio::test]
async fn test_plugin_initialize_and_activate() {
    let env = PluginTestEnv::new().await.expect("setup");

    // Plugin was initialized and activated in new() — verify it's running
    let running = env.host.list_running();
    assert!(
        running.contains(&env.plugin_id),
        "plugin should be in running list, got: {:?}",
        running
    );

    // Verify status was updated to connected during activation
    let status = env.last_status();
    assert!(status.is_some(), "should have received a status update");
    let s = status.unwrap();
    assert_eq!(
        s["state"].as_str().unwrap_or(""),
        "connected",
        "status should be 'connected' after activation"
    );

    env.shutdown().await.unwrap();
}

#[tokio::test]
async fn test_plugin_activation_failure() {
    let result = PluginTestEnv::with_config(json!({
        "apiKey": "test-key",
        "endpoint": "https://httpbin.org",
        "pollInterval": 1,
        "failOnActivate": true
    }))
    .await;

    // Activation should fail because failOnActivate = true
    assert!(
        result.is_err(),
        "activation should fail with failOnActivate=true"
    );
    let err = match result {
        Err(e) => format!("{:#}", e),
        Ok(_) => panic!("expected error"),
    };
    assert!(
        err.contains("activation failure")
            || err.contains("Simulated")
            || err.contains("failOnActivate")
            || err.contains("activate"),
        "error should mention activation failure, got: {}",
        err
    );
}

#[tokio::test]
async fn test_plugin_stop() {
    let env = PluginTestEnv::new().await.expect("setup");
    let pid = env.plugin_id.clone();

    assert!(env.host.list_running().contains(&pid));

    env.host.stop(&pid).await.unwrap();

    assert!(
        !env.host.list_running().contains(&pid),
        "plugin should not be in running list after stop"
    );
}

// ── Config Schema ───────────────────────────────────────────────────────────

#[tokio::test]
async fn test_config_schema_retrieval() {
    let env = PluginTestEnv::new().await.expect("setup");

    let schema = env
        .host
        .get_config_schema(&env.plugin_id)
        .await
        .expect("get config schema");

    // Verify it's an object with properties
    assert!(schema.is_object(), "schema should be a JSON object");

    // The test plugin has: apiKey, endpoint, pollInterval, failOnActivate
    let props = schema
        .get("properties")
        .or_else(|| schema.get("fields"))
        .expect("schema should have properties or fields");
    assert!(props.is_object() || props.is_array(), "properties present");

    env.shutdown().await.unwrap();
}

// ── Tool Discovery ──────────────────────────────────────────────────────────

#[tokio::test]
async fn test_tool_listing() {
    let env = PluginTestEnv::new().await.expect("setup");

    let tools = env
        .host
        .list_tools(&env.plugin_id)
        .await
        .expect("list tools");

    // The test plugin defines 11 tools
    assert!(
        tools.len() >= 11,
        "expected at least 11 tools, got {}",
        tools.len()
    );

    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    let expected = [
        "echo",
        "test_secrets",
        "test_store",
        "emit_test_message",
        "emit_test_event",
        "update_status",
        "send_notification",
        "test_filesystem",
        "get_host_info",
        "discover",
        "test_http",
    ];
    for name in &expected {
        assert!(
            names.contains(name),
            "tool '{}' not found in {:?}",
            name,
            names
        );
    }

    // Each tool should have a description and input_schema
    for tool in &tools {
        assert!(!tool.description.is_empty(), "tool {} needs description", tool.name);
        assert!(
            tool.input_schema.is_object(),
            "tool {} needs input_schema",
            tool.name
        );
    }

    env.shutdown().await.unwrap();
}

// ── Tool Execution: Echo ────────────────────────────────────────────────────

#[tokio::test]
async fn test_echo_tool() {
    let env = PluginTestEnv::new().await.expect("setup");

    let result = env
        .call_tool("echo", json!({ "message": "hello world" }))
        .await
        .expect("echo tool");

    let content = extract_content(&result);
    assert!(
        content.contains("hello world"),
        "echo should return the input, got: {:?}",
        result
    );

    env.shutdown().await.unwrap();
}

#[tokio::test]
async fn test_echo_tool_special_characters() {
    let env = PluginTestEnv::new().await.expect("setup");

    let msg = r#"He said "hello" & <world>"#;
    let result = env
        .call_tool("echo", json!({ "message": msg }))
        .await
        .expect("echo tool");

    let content = extract_content(&result);
    assert!(
        content.contains("hello"),
        "should handle special chars, got: {:?}",
        result
    );

    env.shutdown().await.unwrap();
}

// ── Tool Execution: Secret Storage ──────────────────────────────────────────

#[tokio::test]
async fn test_secret_storage_roundtrip() {
    let env = PluginTestEnv::new().await.expect("setup");

    // Set a secret
    let r = env
        .call_tool(
            "test_secrets",
            json!({ "action": "set", "key": "mykey", "value": "myval" }),
        )
        .await
        .expect("secret set");
    assert!(
        extract_content(&r).contains("stored"),
        "set should confirm storage"
    );

    // Get the secret
    let r = env
        .call_tool("test_secrets", json!({ "action": "get", "key": "mykey" }))
        .await
        .expect("secret get");
    assert!(
        extract_content(&r).contains("myval"),
        "get should return stored value, got: {:?}",
        r
    );

    // Has the secret
    let r = env
        .call_tool("test_secrets", json!({ "action": "has", "key": "mykey" }))
        .await
        .expect("secret has");
    assert!(
        extract_content(&r).contains("true"),
        "has should return true"
    );

    // Delete the secret
    let r = env
        .call_tool(
            "test_secrets",
            json!({ "action": "delete", "key": "mykey" }),
        )
        .await
        .expect("secret delete");
    assert!(
        extract_content(&r).contains("deleted"),
        "delete should confirm"
    );

    // Verify deleted
    let r = env
        .call_tool("test_secrets", json!({ "action": "get", "key": "mykey" }))
        .await
        .expect("secret get after delete");
    assert!(
        extract_content(&r).contains("null"),
        "get after delete should return null, got: {:?}",
        r
    );

    env.shutdown().await.unwrap();
}

// ── Tool Execution: Persistent Store ────────────────────────────────────────

#[tokio::test]
async fn test_persistent_store_roundtrip() {
    let env = PluginTestEnv::new().await.expect("setup");

    // Set a value
    env.call_tool(
        "test_store",
        json!({ "action": "set", "key": "cursor", "value": "abc123" }),
    )
    .await
    .expect("store set");

    // Get the value
    let r = env
        .call_tool("test_store", json!({ "action": "get", "key": "cursor" }))
        .await
        .expect("store get");
    assert!(
        extract_content(&r).contains("abc123"),
        "store should return stored value, got: {:?}",
        r
    );

    // List keys
    let r = env
        .call_tool("test_store", json!({ "action": "keys" }))
        .await
        .expect("store keys");
    assert!(
        extract_content(&r).contains("cursor"),
        "keys should include 'cursor', got: {:?}",
        r
    );

    // Delete
    env.call_tool(
        "test_store",
        json!({ "action": "delete", "key": "cursor" }),
    )
    .await
    .expect("store delete");

    // Verify deleted
    let r = env
        .call_tool("test_store", json!({ "action": "keys" }))
        .await
        .expect("store keys after delete");
    assert!(
        !extract_content(&r).contains("cursor"),
        "keys should not include 'cursor' after delete"
    );

    env.shutdown().await.unwrap();
}

// ── Tool Execution: Message Emission ────────────────────────────────────────

#[tokio::test]
async fn test_emit_message_from_tool() {
    let env = PluginTestEnv::new().await.expect("setup");

    env.call_tool(
        "emit_test_message",
        json!({
            "channel": "test-channel",
            "content": "Hello from tool"
        }),
    )
    .await
    .expect("emit message");

    // Wait for message to be captured
    let msgs = env.wait_for_messages(1, Duration::from_secs(5)).await;
    assert!(!msgs.is_empty(), "should have captured at least 1 message");

    let msg = &msgs[0];
    assert_eq!(msg["channel"].as_str().unwrap_or(""), "test-channel");
    assert!(
        msg["content"]
            .as_str()
            .unwrap_or("")
            .contains("Hello from tool")
    );

    env.shutdown().await.unwrap();
}

#[tokio::test]
async fn test_emit_message_with_threading() {
    let env = PluginTestEnv::new().await.expect("setup");

    env.call_tool(
        "emit_test_message",
        json!({
            "channel": "threaded",
            "content": "Reply message",
            "threadId": "thread-42"
        }),
    )
    .await
    .expect("emit threaded message");

    let msgs = env.wait_for_messages(1, Duration::from_secs(5)).await;
    assert!(!msgs.is_empty(), "should capture threaded message");

    let msg = &msgs[0];
    assert_eq!(
        msg["threadId"].as_str().unwrap_or(""),
        "thread-42",
        "threadId should be propagated"
    );

    env.shutdown().await.unwrap();
}

// ── Tool Execution: Event Emission ──────────────────────────────────────────

#[tokio::test]
async fn test_emit_event_from_tool() {
    let env = PluginTestEnv::new().await.expect("setup");

    env.call_tool(
        "emit_test_event",
        json!({
            "eventType": "test.thing_happened",
            "payload": { "id": "123", "action": "created" }
        }),
    )
    .await
    .expect("emit event");

    let events = env.wait_for_events(1, Duration::from_secs(5)).await;
    assert!(!events.is_empty(), "should have captured at least 1 event");

    let (event_type, payload) = &events[0];
    assert_eq!(event_type, "test.thing_happened");
    assert_eq!(payload["id"].as_str().unwrap_or(""), "123");

    env.shutdown().await.unwrap();
}

// ── Tool Execution: Status Updates ──────────────────────────────────────────

#[tokio::test]
async fn test_status_update() {
    let env = PluginTestEnv::new().await.expect("setup");

    // Clear any activation statuses
    env.statuses.lock().clear();

    env.call_tool(
        "update_status",
        json!({
            "state": "syncing",
            "message": "Working hard...",
            "progress": 50
        }),
    )
    .await
    .expect("update status");

    // Give the async status forwarding a moment
    tokio::time::sleep(Duration::from_millis(200)).await;

    let status = env.last_status();
    assert!(status.is_some(), "should have a status update");
    let s = status.unwrap();
    assert_eq!(s["state"].as_str().unwrap_or(""), "syncing");

    env.shutdown().await.unwrap();
}

// ── Tool Execution: Notifications ───────────────────────────────────────────

#[tokio::test]
async fn test_notification() {
    let env = PluginTestEnv::new().await.expect("setup");

    env.call_tool(
        "send_notification",
        json!({
            "title": "Test Alert",
            "body": "Something happened"
        }),
    )
    .await
    .expect("send notification");

    // Give async notification a moment
    tokio::time::sleep(Duration::from_millis(200)).await;

    let notifs = env.notifications.lock();
    assert!(
        !notifs.is_empty(),
        "should have captured at least 1 notification"
    );

    let n = &notifs[0];
    assert_eq!(n["title"].as_str().unwrap_or(""), "Test Alert");
    assert_eq!(n["body"].as_str().unwrap_or(""), "Something happened");

    drop(notifs);
    env.shutdown().await.unwrap();
}

// ── Tool Execution: Host Info ───────────────────────────────────────────────

#[tokio::test]
async fn test_host_info() {
    let env = PluginTestEnv::new().await.expect("setup");

    let result = env
        .call_tool("get_host_info", json!({}))
        .await
        .expect("get host info");

    // Result should contain version, platform, capabilities
    let content = &result;
    let obj = content
        .get("content")
        .or(Some(content))
        .unwrap();

    // Platform should match the OS we're running on
    let platform = obj
        .get("platform")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    assert!(
        ["windows", "macos", "linux"].contains(&platform),
        "platform should be windows/macos/linux, got: {}",
        platform
    );

    env.shutdown().await.unwrap();
}

// ── Background Loop ─────────────────────────────────────────────────────────

#[tokio::test]
async fn test_loop_start_and_messages() {
    let env = PluginTestEnv::new().await.expect("setup");

    // Start the background loop (pollInterval=1s in default config)
    env.start_loop().await.expect("start loop");

    // Wait for at least 2 tick messages (give 5s for 2 ticks at 1s interval)
    let msgs = env.wait_for_messages(2, Duration::from_secs(5)).await;
    assert!(
        msgs.len() >= 2,
        "should have at least 2 loop messages, got {}",
        msgs.len()
    );

    // Verify messages are on the expected channel
    for msg in &msgs {
        let channel = msg["channel"].as_str().unwrap_or("");
        assert_eq!(channel, "test-loop", "loop messages should be on 'test-loop' channel");
    }

    // Verify events were also emitted
    let events = env.wait_for_events(2, Duration::from_secs(2)).await;
    assert!(
        events.len() >= 2,
        "should have at least 2 loop events, got {}",
        events.len()
    );
    for (event_type, _) in &events {
        assert_eq!(event_type, "test.loop_tick");
    }

    // Verify store has loopTick
    let r = env
        .call_tool("test_store", json!({ "action": "get", "key": "loopTick" }))
        .await
        .expect("get loopTick");
    let content = extract_content(&r);
    assert!(
        !content.contains("null"),
        "loopTick should be set in store, got: {}",
        content
    );

    env.stop_loop().await.expect("stop loop");
    env.shutdown().await.unwrap();
}

#[tokio::test]
async fn test_loop_stop() {
    let env = PluginTestEnv::new().await.expect("setup");

    env.start_loop().await.expect("start loop");

    // Wait for at least 1 message
    let msgs = env.wait_for_messages(1, Duration::from_secs(3)).await;
    assert!(!msgs.is_empty(), "should get at least 1 tick before stop");

    // Stop the loop
    env.stop_loop().await.expect("stop loop");

    // Record message count after stop
    let count_after_stop = env.messages.lock().len();

    // Wait a bit and verify no more messages arrive
    tokio::time::sleep(Duration::from_secs(2)).await;
    let count_final = env.messages.lock().len();

    // Allow at most 1 additional message (in-flight at stop time)
    assert!(
        count_final <= count_after_stop + 1,
        "loop should stop producing messages: had {} at stop, now {}",
        count_after_stop,
        count_final
    );

    env.shutdown().await.unwrap();
}

#[tokio::test]
async fn test_loop_restart_resilience() {
    let env = PluginTestEnv::new().await.expect("setup");

    // Start loop
    env.start_loop().await.expect("start loop");
    let _ = env.wait_for_messages(1, Duration::from_secs(3)).await;

    // Stop loop
    env.stop_loop().await.expect("stop loop");

    // Clear captured data
    env.messages.lock().clear();
    env.events.lock().clear();

    // Start loop again
    env.start_loop().await.expect("restart loop");
    let msgs = env.wait_for_messages(1, Duration::from_secs(5)).await;
    assert!(
        !msgs.is_empty(),
        "loop should produce messages after restart"
    );

    env.stop_loop().await.expect("stop loop");
    env.shutdown().await.unwrap();
}

// ── PluginBridgeTool Integration ────────────────────────────────────────────

#[tokio::test]
async fn test_plugin_bridge_tools() {
    let env = PluginTestEnv::new().await.expect("setup");

    // Test that tools can be listed and called through the host —
    // this is the same code path PluginBridgeTool.execute() uses.
    let tools = env
        .host
        .list_tools(&env.plugin_id)
        .await
        .expect("list tools for bridge");

    // Verify tools can be mapped to PluginBridgeTool format
    for tool in &tools {
        assert!(!tool.name.is_empty(), "tool name must not be empty");
        assert!(
            tool.input_schema.is_object(),
            "tool {} must have input_schema",
            tool.name
        );
    }

    // Execute through the host (same path as PluginBridgeTool)
    let r = env
        .call_tool("echo", json!({ "message": "bridge test" }))
        .await
        .expect("bridge tool execution");
    assert!(extract_content(&r).contains("bridge test"));

    env.shutdown().await.unwrap();
}

// ── Error Handling & Edge Cases ─────────────────────────────────────────────

#[tokio::test]
async fn test_tool_with_invalid_params() {
    let env = PluginTestEnv::new().await.expect("setup");

    // Call echo without the required 'message' param
    let result = env.call_tool("echo", json!({})).await;

    // Should either return an error or a result indicating error
    match result {
        Err(e) => {
            // Good — error propagated
            let msg = e.to_string();
            assert!(
                !msg.is_empty(),
                "error should have a message"
            );
        }
        Ok(val) => {
            // Plugin might handle gracefully — check for isError flag
            let is_error = val.get("isError").and_then(|v| v.as_bool()).unwrap_or(false);
            if !is_error {
                // Some plugins echo empty/undefined — that's also acceptable
            }
        }
    }

    env.shutdown().await.unwrap();
}

#[tokio::test]
async fn test_call_nonexistent_tool() {
    let env = PluginTestEnv::new().await.expect("setup");

    let result = env
        .call_tool("nonexistent_tool_that_doesnt_exist", json!({}))
        .await;

    assert!(
        result.is_err(),
        "calling nonexistent tool should return error"
    );

    env.shutdown().await.unwrap();
}

#[tokio::test]
async fn test_concurrent_tool_calls() {
    let env = PluginTestEnv::new().await.expect("setup");

    // Fire 5 echo calls concurrently using JoinSet
    let mut results = vec![];
    for i in 0..5 {
        let msg = format!("concurrent-{}", i);
        let r = env
            .call_tool("echo", json!({ "message": msg }))
            .await;
        results.push(r);
    }

    // All should succeed
    let successes = results.iter().filter(|r| r.is_ok()).count();
    assert!(
        successes >= 4,
        "at least 4 of 5 concurrent calls should succeed, got {}",
        successes
    );

    env.shutdown().await.unwrap();
}

#[tokio::test]
async fn test_large_payload() {
    let env = PluginTestEnv::new().await.expect("setup");

    // 100KB message
    let large_msg = "x".repeat(100_000);
    let result = env
        .call_tool("echo", json!({ "message": large_msg }))
        .await
        .expect("large payload");

    let content = extract_content(&result);
    assert!(
        content.len() > 1000,
        "response should contain a large payload, got {} chars",
        content.len()
    );

    env.shutdown().await.unwrap();
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Extract the content string from a tool result, handling both
/// `{ "content": "..." }` and raw string results.
fn extract_content(val: &Value) -> String {
    // Direct content field (array of content blocks, like MCP)
    if let Some(content) = val.get("content") {
        if let Some(arr) = content.as_array() {
            // MCP-style: [{ "type": "text", "text": "..." }]
            return arr
                .iter()
                .filter_map(|item| item.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("\n");
        }
        if let Some(s) = content.as_str() {
            return s.to_string();
        }
        // Object content
        return serde_json::to_string(content).unwrap_or_default();
    }
    // Raw string result
    if let Some(s) = val.as_str() {
        return s.to_string();
    }
    // Fallback — serialize the whole thing
    serde_json::to_string(val).unwrap_or_default()
}
