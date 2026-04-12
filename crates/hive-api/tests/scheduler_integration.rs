use axum::http::StatusCode;
use hive_api::{build_router, chat, AppState, ChatRuntimeConfig, ChatService, SchedulerService};
use hive_core::{AuditLogger, EventBus, HiveMindConfig};
use serde_json::{json, Value};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::Notify;

/// Boot a test server with an in-memory scheduler.
async fn boot_scheduler_server() -> (String, Arc<Notify>, TempDir) {
    let tempdir = tempfile::tempdir().expect("temp dir");
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local addr");

    let mut config = HiveMindConfig::default();
    config.api.bind = addr.to_string();

    let audit = AuditLogger::new(tempdir.path().join("audit.log")).expect("audit");
    let event_bus = EventBus::new(32);
    let shutdown = Arc::new(Notify::new());

    let model_router = chat::build_model_router_from_config(&config, None, None)
        .expect("model router from config");
    let chat = Arc::new(ChatService::with_model_router(
        audit.clone(),
        event_bus.clone(),
        ChatRuntimeConfig::default(),
        tempdir.path().to_path_buf(),
        tempdir.path().join("knowledge.db"),
        config.security.prompt_injection.clone(),
        hive_contracts::config::CommandPolicyConfig::default(),
        tempdir.path().join("risk-ledger.db"),
        model_router,
        hive_api::canvas_ws::CanvasSessionRegistry::new(),
        hive_contracts::ContextCompactionConfig::default(),
        "127.0.0.1:0".to_string(),
        Default::default(),
        None, // mcp
        None, // mcp_catalog
        None, // connector_registry
        None, // connector_audit_log
        None, // connector_service
        Arc::new(
            SchedulerService::in_memory(
                event_bus.clone(),
                hive_scheduler::SchedulerConfig::default(),
            )
            .expect("test scheduler"),
        ),
        Arc::new(parking_lot::RwLock::new(std::collections::HashMap::new())),
        Arc::new(parking_lot::RwLock::new(hive_contracts::SandboxConfig::default())),
        Arc::new(hive_contracts::DetectedShells::default()),
        hive_contracts::ToolLimitsConfig::default(),
    ));

    let state = AppState::with_chat(config, audit, event_bus, shutdown.clone(), chat);

    let router = build_router(state);
    let server_shutdown = shutdown.clone();

    tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async move { server_shutdown.notified().await })
            .await
            .expect("serve");
    });

    (format!("http://{addr}"), shutdown, tempdir)
}

fn sample_task(name: &str) -> Value {
    json!({
        "name": name,
        "description": format!("Test task: {name}"),
        "schedule": { "type": "once" },
        "action": {
            "type": "emit_event",
            "topic": "test.topic",
            "payload": { "key": "value" }
        }
    })
}

/// POST helper returning response.
async fn post_json(client: &reqwest::Client, url: &str, body: Value) -> reqwest::Response {
    client.post(url).json(&body).send().await.expect("POST request")
}

/// Create a task and return its id.
async fn create_task(client: &reqwest::Client, base: &str, name: &str) -> String {
    let resp =
        post_json(client, &format!("{base}/api/v1/scheduler/tasks"), sample_task(name)).await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let v: Value = resp.json().await.expect("json");
    v["id"].as_str().expect("id field").to_string()
}

fn authed_client() -> reqwest::Client {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("Authorization", "Bearer test-token".parse().unwrap());
    reqwest::Client::builder().default_headers(headers).build().unwrap()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_create_and_get_task() {
    let (base, shutdown, _dir) = boot_scheduler_server().await;
    let client = authed_client();

    let id = create_task(&client, &base, "build_project").await;

    let resp =
        client.get(format!("{base}/api/v1/scheduler/tasks/{id}")).send().await.expect("GET task");
    assert_eq!(resp.status(), StatusCode::OK);

    let task: Value = resp.json().await.expect("json");
    assert_eq!(task["id"], id);
    assert_eq!(task["name"], "build_project");
    assert_eq!(task["status"], "pending");
    assert_eq!(task["schedule"]["type"], "once");

    shutdown.notify_waiters();
}

#[tokio::test]
async fn test_list_tasks() {
    let (base, shutdown, _dir) = boot_scheduler_server().await;
    let client = authed_client();

    create_task(&client, &base, "task_alpha").await;
    create_task(&client, &base, "task_beta").await;

    let resp = client.get(format!("{base}/api/v1/scheduler/tasks")).send().await.expect("list");
    assert_eq!(resp.status(), StatusCode::OK);

    let tasks: Vec<Value> = resp.json().await.expect("json");
    assert!(tasks.len() >= 2, "expected at least 2 tasks, got {}", tasks.len());

    let names: Vec<&str> = tasks.iter().filter_map(|t| t["name"].as_str()).collect();
    assert!(names.contains(&"task_alpha"));
    assert!(names.contains(&"task_beta"));

    shutdown.notify_waiters();
}

#[tokio::test]
async fn test_delete_task() {
    let (base, shutdown, _dir) = boot_scheduler_server().await;
    let client = authed_client();

    let id = create_task(&client, &base, "ephemeral_task").await;

    // Delete
    let resp =
        client.delete(format!("{base}/api/v1/scheduler/tasks/{id}")).send().await.expect("DELETE");
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // GET should 404
    let resp = client
        .get(format!("{base}/api/v1/scheduler/tasks/{id}"))
        .send()
        .await
        .expect("GET after delete");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    shutdown.notify_waiters();
}

#[tokio::test]
async fn test_cancel_task() {
    let (base, shutdown, _dir) = boot_scheduler_server().await;
    let client = authed_client();

    let id = create_task(&client, &base, "cancellable_task").await;

    // Cancel
    let resp = client
        .post(format!("{base}/api/v1/scheduler/tasks/{id}/cancel"))
        .send()
        .await
        .expect("cancel");
    assert_eq!(resp.status(), StatusCode::OK);

    let task: Value = resp.json().await.expect("json");
    assert_eq!(task["status"], "cancelled");

    // Verify via GET
    let resp = client
        .get(format!("{base}/api/v1/scheduler/tasks/{id}"))
        .send()
        .await
        .expect("GET after cancel");
    assert_eq!(resp.status(), StatusCode::OK);
    let task: Value = resp.json().await.expect("json");
    assert_eq!(task["status"], "cancelled");

    shutdown.notify_waiters();
}

#[tokio::test]
async fn test_get_nonexistent_task_returns_404() {
    let (base, shutdown, _dir) = boot_scheduler_server().await;
    let client = authed_client();

    let resp = client
        .get(format!("{base}/api/v1/scheduler/tasks/does-not-exist"))
        .send()
        .await
        .expect("GET nonexistent");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    shutdown.notify_waiters();
}

#[tokio::test]
async fn test_create_task_with_cron_schedule() {
    let (base, shutdown, _dir) = boot_scheduler_server().await;
    let client = authed_client();

    let body = json!({
        "name": "cron_check",
        "schedule": { "type": "cron", "expression": "0 */5 * * * * *" },
        "action": {
            "type": "http_webhook",
            "url": "http://localhost:9999/hook",
            "method": "POST",
            "body": "{\"ping\": true}"
        }
    });

    let resp = post_json(&client, &format!("{base}/api/v1/scheduler/tasks"), body).await;
    assert_eq!(resp.status(), StatusCode::CREATED);

    let task: Value = resp.json().await.expect("json");
    assert_eq!(task["name"], "cron_check");
    assert_eq!(task["schedule"]["type"], "cron");
    assert_eq!(task["schedule"]["expression"], "0 */5 * * * * *");
    assert_eq!(task["action"]["type"], "http_webhook");

    shutdown.notify_waiters();
}
