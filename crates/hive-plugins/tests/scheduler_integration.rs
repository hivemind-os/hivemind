//! Integration tests for plugin scheduler support (host/schedule and host/unschedule).
//!
//! Verifies that plugins can schedule and unschedule tasks via the host API,
//! and that the host handler receives correct parameters.

mod helpers;

use serde_json::json;

use helpers::PluginTestEnv;

// ── Schedule ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_plugin_can_schedule_task() {
    let env = PluginTestEnv::new().await.expect("setup");

    // Call the test_scheduler tool with action=schedule
    let result = env
        .call_tool(
            "test_scheduler",
            json!({
                "action": "schedule",
                "id": "my-poll-task",
                "intervalSeconds": 120
            }),
        )
        .await
        .expect("schedule tool should succeed");

    // The tool should return success content
    let content = result["content"].as_str().unwrap_or("");
    assert!(
        content.contains("Scheduled task"),
        "expected success message, got: {:?}",
        result
    );

    // Verify the host handler captured the schedule request
    {
        let schedules = env.schedules.lock();
        assert_eq!(schedules.len(), 1, "should have one schedule request");
        assert_eq!(schedules[0]["action"], "schedule");
        assert_eq!(schedules[0]["id"], "my-poll-task");
        assert_eq!(schedules[0]["intervalSeconds"], 120);
    }

    env.shutdown().await.unwrap();
}

// ── Unschedule ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_plugin_can_unschedule_task() {
    let env = PluginTestEnv::new().await.expect("setup");

    // First schedule a task
    env.call_tool(
        "test_scheduler",
        json!({
            "action": "schedule",
            "id": "temp-task",
            "intervalSeconds": 60
        }),
    )
    .await
    .expect("schedule should succeed");

    // Then unschedule it
    let result = env
        .call_tool(
            "test_scheduler",
            json!({
                "action": "unschedule",
                "id": "temp-task"
            }),
        )
        .await
        .expect("unschedule tool should succeed");

    let content = result["content"].as_str().unwrap_or("");
    assert!(
        content.contains("Unscheduled task"),
        "expected unschedule message, got: {:?}",
        result
    );

    // Verify the host handler captured both requests
    {
        let schedules = env.schedules.lock();
        assert_eq!(schedules.len(), 2, "should have two schedule requests");
        assert_eq!(schedules[0]["action"], "schedule");
        assert_eq!(schedules[0]["id"], "temp-task");
        assert_eq!(schedules[1]["action"], "unschedule");
        assert_eq!(schedules[1]["id"], "temp-task");
    }

    env.shutdown().await.unwrap();
}

// ── Parameter Verification ──────────────────────────────────────────────────

#[tokio::test]
async fn test_plugin_schedule_params() {
    let env = PluginTestEnv::new().await.expect("setup");

    // Schedule with a specific interval
    env.call_tool(
        "test_scheduler",
        json!({
            "action": "schedule",
            "id": "daily-sync",
            "intervalSeconds": 3600
        }),
    )
    .await
    .expect("schedule should succeed");

    // Verify correct params were received by the host handler
    {
        let schedules = env.schedules.lock();
        assert_eq!(schedules.len(), 1);

        let req = &schedules[0];
        assert_eq!(req["id"], "daily-sync");
        assert_eq!(req["intervalSeconds"], 3600);
        assert_eq!(req["action"], "schedule");
    }

    env.shutdown().await.unwrap();
}
