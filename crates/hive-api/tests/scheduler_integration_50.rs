//! 50 integration tests for the task scheduler, covering CRUD, owner scoping,
//! execution history, tick-loop execution with mock HTTP servers, update
//! behaviour, filtering, agent tool usage, and agent invocation scenarios.

use axum::http::StatusCode;
use hive_api::{build_router, chat, AppState, ChatRuntimeConfig, ChatService, SchedulerService};
use hive_core::{AuditLogger, EventBus, HiveMindConfig};
use hive_tools::ScheduleTaskTool;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::Notify;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Boot a test server backed by an in-memory scheduler. Returns the base URL,
/// a shutdown notifier, the TempDir (must stay alive), and the SchedulerService.
async fn boot() -> (String, Arc<Notify>, TempDir, Arc<SchedulerService>) {
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
        addr.to_string(),
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
        None, // plugin_host
        None, // plugin_registry
    ));

    let state = AppState::with_chat(config, audit, event_bus, shutdown.clone(), chat);
    let scheduler = Arc::clone(&state.scheduler);

    let router = build_router(state);
    let server_shutdown = shutdown.clone();

    tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async move { server_shutdown.notified().await })
            .await
            .expect("serve");
    });

    (format!("http://{addr}"), shutdown, tempdir, scheduler)
}

fn emit_task(name: &str) -> Value {
    json!({
        "name": name,
        "description": format!("Test: {name}"),
        "schedule": { "type": "once" },
        "action": { "type": "emit_event", "topic": "test.topic", "payload": {} }
    })
}

fn scheduled_task(name: &str, run_at_ms: u64) -> Value {
    json!({
        "name": name,
        "schedule": { "type": "scheduled", "run_at_ms": run_at_ms },
        "action": { "type": "emit_event", "topic": "scheduled", "payload": {} }
    })
}

fn webhook_task(name: &str, url: &str, method: &str) -> Value {
    json!({
        "name": name,
        "schedule": { "type": "once" },
        "action": { "type": "http_webhook", "url": url, "method": method }
    })
}

fn msg_task(name: &str, session: &str, content: &str) -> Value {
    json!({
        "name": name,
        "schedule": { "type": "once" },
        "action": { "type": "send_message", "session_id": session, "content": content }
    })
}

fn cron_task(name: &str, expr: &str) -> Value {
    json!({
        "name": name,
        "schedule": { "type": "cron", "expression": expr },
        "action": { "type": "emit_event", "topic": "cron.fire", "payload": {} }
    })
}

fn owned_task(name: &str, session: &str, agent: Option<&str>) -> Value {
    let mut v = emit_task(name);
    v["owner_session_id"] = json!(session);
    if let Some(a) = agent {
        v["owner_agent_id"] = json!(a);
    }
    v
}

async fn create(client: &reqwest::Client, base: &str, body: Value) -> Value {
    let r = client
        .post(format!("{base}/api/v1/scheduler/tasks"))
        .json(&body)
        .send()
        .await
        .expect("POST");
    assert_eq!(r.status(), StatusCode::CREATED, "create failed: {}", r.status());
    r.json().await.expect("json")
}

async fn get_task(client: &reqwest::Client, base: &str, id: &str) -> (StatusCode, Value) {
    let r = client.get(format!("{base}/api/v1/scheduler/tasks/{id}")).send().await.expect("GET");
    let status = r.status();
    let body: Value = r.json().await.unwrap_or(json!(null));
    (status, body)
}

async fn list(client: &reqwest::Client, base: &str) -> Vec<Value> {
    let r = client.get(format!("{base}/api/v1/scheduler/tasks")).send().await.expect("LIST");
    r.json().await.expect("json")
}

async fn list_filtered(client: &reqwest::Client, base: &str, query: &str) -> Vec<Value> {
    let r = client
        .get(format!("{base}/api/v1/scheduler/tasks?{query}"))
        .send()
        .await
        .expect("LIST filtered");
    r.json().await.expect("json")
}

async fn get_runs(client: &reqwest::Client, base: &str, task_id: &str) -> (StatusCode, Value) {
    let r = client
        .get(format!("{base}/api/v1/scheduler/tasks/{task_id}/runs"))
        .send()
        .await
        .expect("GET runs");
    let status = r.status();
    let body: Value = r.json().await.unwrap_or(json!(null));
    (status, body)
}

fn authed_client() -> reqwest::Client {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("Authorization", "Bearer test-token".parse().unwrap());
    reqwest::Client::builder().default_headers(headers).build().unwrap()
}

// ============================================================================
// 1-10: Basic CRUD
// ============================================================================

#[tokio::test]
async fn t01_create_returns_201_with_id() {
    let (base, shutdown, _d, _) = boot().await;
    let c = authed_client();
    let task = create(&c, &base, emit_task("t01")).await;
    assert!(task["id"].as_str().unwrap().starts_with("task-"));
    assert_eq!(task["status"], "pending");
    shutdown.notify_waiters();
}

#[tokio::test]
async fn t02_get_returns_created_task() {
    let (base, shutdown, _d, _) = boot().await;
    let c = authed_client();
    let task = create(&c, &base, emit_task("t02")).await;
    let id = task["id"].as_str().unwrap();
    let (status, fetched) = get_task(&c, &base, id).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fetched["name"], "t02");
    shutdown.notify_waiters();
}

#[tokio::test]
async fn t03_get_nonexistent_returns_404() {
    let (base, shutdown, _d, _) = boot().await;
    let c = authed_client();
    let (status, _) = get_task(&c, &base, "no-such-task").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    shutdown.notify_waiters();
}

#[tokio::test]
async fn t04_list_empty_returns_empty_array() {
    let (base, shutdown, _d, _) = boot().await;
    let c = authed_client();
    let tasks = list(&c, &base).await;
    assert!(tasks.is_empty());
    shutdown.notify_waiters();
}

#[tokio::test]
async fn t05_list_returns_all_created() {
    let (base, shutdown, _d, _) = boot().await;
    let c = authed_client();
    for i in 0..5 {
        create(&c, &base, emit_task(&format!("t05_{i}"))).await;
    }
    let tasks = list(&c, &base).await;
    assert_eq!(tasks.len(), 5);
    shutdown.notify_waiters();
}

#[tokio::test]
async fn t06_delete_removes_task() {
    let (base, shutdown, _d, _) = boot().await;
    let c = authed_client();
    let task = create(&c, &base, emit_task("t06")).await;
    let id = task["id"].as_str().unwrap();
    let r = c.delete(format!("{base}/api/v1/scheduler/tasks/{id}")).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::NO_CONTENT);
    let (status, _) = get_task(&c, &base, id).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    shutdown.notify_waiters();
}

#[tokio::test]
async fn t07_delete_nonexistent_returns_404() {
    let (base, shutdown, _d, _) = boot().await;
    let c = authed_client();
    let r = c.delete(format!("{base}/api/v1/scheduler/tasks/nope")).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
    shutdown.notify_waiters();
}

#[tokio::test]
async fn t08_cancel_sets_cancelled() {
    let (base, shutdown, _d, _) = boot().await;
    let c = authed_client();
    let task = create(&c, &base, emit_task("t08")).await;
    let id = task["id"].as_str().unwrap();
    let r = c.post(format!("{base}/api/v1/scheduler/tasks/{id}/cancel")).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let body: Value = r.json().await.unwrap();
    assert_eq!(body["status"], "cancelled");
    shutdown.notify_waiters();
}

#[tokio::test]
async fn t09_cancel_nonexistent_returns_404() {
    let (base, shutdown, _d, _) = boot().await;
    let c = authed_client();
    let r = c.post(format!("{base}/api/v1/scheduler/tasks/nope/cancel")).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
    shutdown.notify_waiters();
}

#[tokio::test]
async fn t10_cancel_already_cancelled_is_idempotent() {
    let (base, shutdown, _d, _) = boot().await;
    let c = authed_client();
    let task = create(&c, &base, emit_task("t10")).await;
    let id = task["id"].as_str().unwrap();
    c.post(format!("{base}/api/v1/scheduler/tasks/{id}/cancel")).send().await.unwrap();
    let r = c.post(format!("{base}/api/v1/scheduler/tasks/{id}/cancel")).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let body: Value = r.json().await.unwrap();
    assert_eq!(body["status"], "cancelled");
    shutdown.notify_waiters();
}

// ============================================================================
// 11-15: Schedule types
// ============================================================================

#[tokio::test]
async fn t11_once_schedule_has_immediate_next_run() {
    let (base, shutdown, _d, _) = boot().await;
    let c = authed_client();
    let task = create(&c, &base, emit_task("t11")).await;
    assert!(task["next_run_ms"].as_u64().is_some());
    shutdown.notify_waiters();
}

#[tokio::test]
async fn t12_scheduled_has_future_next_run() {
    let (base, shutdown, _d, _) = boot().await;
    let c = authed_client();
    let run_at =
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64 + 3_600_000;
    let task = create(&c, &base, scheduled_task("t12", run_at)).await;
    let next = task["next_run_ms"].as_u64().unwrap();
    let created = task["created_at_ms"].as_u64().unwrap();
    assert!(next >= created + 3_500_000, "next_run_ms should be in the future");
    shutdown.notify_waiters();
}

#[tokio::test]
async fn t13_cron_schedule_stores_expression() {
    let (base, shutdown, _d, _) = boot().await;
    let c = authed_client();
    let task = create(&c, &base, cron_task("t13", "0 */2 * * * * *")).await;
    assert_eq!(task["schedule"]["type"], "cron");
    assert_eq!(task["schedule"]["expression"], "0 */2 * * * * *");
    shutdown.notify_waiters();
}

#[tokio::test]
async fn t14_cron_schedule_stores_expression() {
    let (base, shutdown, _d, _) = boot().await;
    let c = authed_client();
    let task = create(&c, &base, cron_task("t14", "0 0 * * * *")).await;
    assert_eq!(task["schedule"]["type"], "cron");
    assert_eq!(task["schedule"]["expression"], "0 0 * * * *");
    assert!(task["next_run_ms"].as_u64().is_some());
    shutdown.notify_waiters();
}

#[tokio::test]
async fn t15_action_types_round_trip() {
    let (base, shutdown, _d, _) = boot().await;
    let c = authed_client();

    let wh = create(&c, &base, webhook_task("t15_wh", "http://example.com/hook", "PUT")).await;
    assert_eq!(wh["action"]["type"], "http_webhook");
    assert_eq!(wh["action"]["method"], "PUT");

    let sm = create(&c, &base, msg_task("t15_sm", "sess-1", "hello")).await;
    assert_eq!(sm["action"]["type"], "send_message");
    assert_eq!(sm["action"]["session_id"], "sess-1");

    let ev = create(&c, &base, emit_task("t15_ev")).await;
    assert_eq!(ev["action"]["type"], "emit_event");

    shutdown.notify_waiters();
}

// ============================================================================
// 16-20: Update (PUT) endpoint
// ============================================================================

#[tokio::test]
async fn t16_update_name() {
    let (base, shutdown, _d, _) = boot().await;
    let c = authed_client();
    let task = create(&c, &base, emit_task("t16_orig")).await;
    let id = task["id"].as_str().unwrap();
    let r = c
        .put(format!("{base}/api/v1/scheduler/tasks/{id}"))
        .json(&json!({"name": "t16_updated"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let body: Value = r.json().await.unwrap();
    assert_eq!(body["name"], "t16_updated");
    shutdown.notify_waiters();
}

#[tokio::test]
async fn t17_update_description() {
    let (base, shutdown, _d, _) = boot().await;
    let c = authed_client();
    let task = create(&c, &base, emit_task("t17")).await;
    let id = task["id"].as_str().unwrap();
    let r = c
        .put(format!("{base}/api/v1/scheduler/tasks/{id}"))
        .json(&json!({"description": "new desc"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let body: Value = r.json().await.unwrap();
    assert_eq!(body["description"], "new desc");
    shutdown.notify_waiters();
}

#[tokio::test]
async fn t18_update_schedule() {
    let (base, shutdown, _d, _) = boot().await;
    let c = authed_client();
    let task = create(&c, &base, emit_task("t18")).await;
    let id = task["id"].as_str().unwrap();
    let r = c
        .put(format!("{base}/api/v1/scheduler/tasks/{id}"))
        .json(&json!({"schedule": {"type": "cron", "expression": "0 * * * * * *"}}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let body: Value = r.json().await.unwrap();
    assert_eq!(body["schedule"]["type"], "cron");
    assert_eq!(body["schedule"]["expression"], "0 * * * * * *");
    shutdown.notify_waiters();
}

#[tokio::test]
async fn t19_update_action() {
    let (base, shutdown, _d, _) = boot().await;
    let c = authed_client();
    let task = create(&c, &base, emit_task("t19")).await;
    let id = task["id"].as_str().unwrap();
    let r = c
        .put(format!("{base}/api/v1/scheduler/tasks/{id}"))
        .json(
            &json!({"action": {"type": "http_webhook", "url": "http://x.com/h", "method": "GET"}}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let body: Value = r.json().await.unwrap();
    assert_eq!(body["action"]["type"], "http_webhook");
    shutdown.notify_waiters();
}

#[tokio::test]
async fn t20_update_nonexistent_returns_404() {
    let (base, shutdown, _d, _) = boot().await;
    let c = authed_client();
    let r = c
        .put(format!("{base}/api/v1/scheduler/tasks/nope"))
        .json(&json!({"name": "whatever"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
    shutdown.notify_waiters();
}

// ============================================================================
// 21-25: Owner scoping & filtering
// ============================================================================

#[tokio::test]
async fn t21_owner_session_id_persisted() {
    let (base, shutdown, _d, _) = boot().await;
    let c = authed_client();
    let task = create(&c, &base, owned_task("t21", "session-abc", None)).await;
    assert_eq!(task["owner_session_id"], "session-abc");
    assert!(task["owner_agent_id"].is_null());
    shutdown.notify_waiters();
}

#[tokio::test]
async fn t22_owner_agent_id_persisted() {
    let (base, shutdown, _d, _) = boot().await;
    let c = authed_client();
    let task = create(&c, &base, owned_task("t22", "s1", Some("agent-x"))).await;
    assert_eq!(task["owner_session_id"], "s1");
    assert_eq!(task["owner_agent_id"], "agent-x");
    shutdown.notify_waiters();
}

#[tokio::test]
async fn t23_filter_by_session_id() {
    let (base, shutdown, _d, _) = boot().await;
    let c = authed_client();
    create(&c, &base, owned_task("t23_a", "session-1", None)).await;
    create(&c, &base, owned_task("t23_b", "session-2", None)).await;
    create(&c, &base, owned_task("t23_c", "session-1", None)).await;

    let filtered = list_filtered(&c, &base, "session_id=session-1").await;
    assert_eq!(filtered.len(), 2);
    for t in &filtered {
        assert_eq!(t["owner_session_id"], "session-1");
    }
    shutdown.notify_waiters();
}

#[tokio::test]
async fn t24_filter_by_agent_id() {
    let (base, shutdown, _d, _) = boot().await;
    let c = authed_client();
    create(&c, &base, owned_task("t24_a", "s1", Some("agent-a"))).await;
    create(&c, &base, owned_task("t24_b", "s1", Some("agent-b"))).await;
    create(&c, &base, owned_task("t24_c", "s2", Some("agent-a"))).await;

    let filtered = list_filtered(&c, &base, "agent_id=agent-a").await;
    assert_eq!(filtered.len(), 2);
    for t in &filtered {
        assert_eq!(t["owner_agent_id"], "agent-a");
    }
    shutdown.notify_waiters();
}

#[tokio::test]
async fn t25_filter_by_session_and_agent() {
    let (base, shutdown, _d, _) = boot().await;
    let c = authed_client();
    create(&c, &base, owned_task("t25_a", "s1", Some("agent-x"))).await;
    create(&c, &base, owned_task("t25_b", "s1", Some("agent-y"))).await;
    create(&c, &base, owned_task("t25_c", "s2", Some("agent-x"))).await;

    let filtered = list_filtered(&c, &base, "session_id=s1&agent_id=agent-x").await;
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0]["name"], "t25_a");
    shutdown.notify_waiters();
}

// ============================================================================
// 26-30: Execution history (task_runs)
// ============================================================================

#[tokio::test]
async fn t26_runs_empty_for_new_task() {
    let (base, shutdown, _d, _) = boot().await;
    let c = authed_client();
    let task = create(&c, &base, emit_task("t26")).await;
    let id = task["id"].as_str().unwrap();
    let (status, runs) = get_runs(&c, &base, id).await;
    assert_eq!(status, StatusCode::OK);
    assert!(runs.as_array().unwrap().is_empty());
    shutdown.notify_waiters();
}

#[tokio::test]
async fn t27_runs_endpoint_returns_404_for_missing_task() {
    let (base, shutdown, _d, _) = boot().await;
    let c = authed_client();
    let (status, _) = get_runs(&c, &base, "missing-id").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    shutdown.notify_waiters();
}

#[tokio::test]
async fn t28_successful_execution_records_run() {
    let bus = EventBus::new(32);
    let svc = Arc::new(
        SchedulerService::in_memory(bus, hive_scheduler::SchedulerConfig::default()).expect("svc"),
    );
    let task = svc
        .create_task(hive_contracts::CreateTaskRequest {
            name: "t28".into(),
            description: None,
            schedule: hive_contracts::TaskSchedule::Once,
            action: hive_contracts::TaskAction::EmitEvent {
                topic: "test".into(),
                payload: json!({}),
            },
            owner_session_id: None,
            owner_agent_id: None,
            max_retries: None,
            retry_delay_ms: None,
        })
        .unwrap();

    svc.force_all_due();
    svc.tick().await;

    let runs = svc.list_task_runs(&task.id).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, hive_api::TaskRunStatus::Success);
    assert!(runs[0].error.is_none());
    assert!(runs[0].completed_at_ms.is_some());
}

#[tokio::test]
async fn t29_failed_execution_records_run_with_error() {
    use axum::{routing::post, Router};

    let app = Router::new().route(
        "/api/v1/chat/sessions/{sid}/messages",
        post(|| async { (StatusCode::INTERNAL_SERVER_ERROR, "boom") }),
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let bus = EventBus::new(32);
    let svc = Arc::new(
        SchedulerService::in_memory_with_addr(
            bus,
            addr.to_string(),
            hive_scheduler::SchedulerConfig::default(),
        )
        .expect("svc"),
    );
    let task = svc
        .create_task(hive_contracts::CreateTaskRequest {
            name: "t29".into(),
            description: None,
            schedule: hive_contracts::TaskSchedule::Once,
            action: hive_contracts::TaskAction::SendMessage {
                session_id: "sess-1".into(),
                content: "will fail".into(),
            },
            owner_session_id: None,
            owner_agent_id: None,
            max_retries: None,
            retry_delay_ms: None,
        })
        .unwrap();

    svc.tick().await;

    let runs = svc.list_task_runs(&task.id).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, hive_api::TaskRunStatus::Failure);
    assert!(runs[0].error.is_some());
}

#[tokio::test]
async fn t30_multiple_cron_runs_accumulate() {
    let bus = EventBus::new(32);
    let svc = Arc::new(
        SchedulerService::in_memory(bus, hive_scheduler::SchedulerConfig::default()).expect("svc"),
    );
    svc.create_task(hive_contracts::CreateTaskRequest {
        name: "t30".into(),
        description: None,
        schedule: hive_contracts::TaskSchedule::Cron { expression: "0 */5 * * * * *".to_string() },
        action: hive_contracts::TaskAction::EmitEvent { topic: "hb".into(), payload: json!({}) },
        owner_session_id: None,
        owner_agent_id: None,
        max_retries: None,
        retry_delay_ms: None,
    })
    .unwrap();

    for _ in 0..3 {
        svc.force_all_due();
        svc.tick().await;
    }

    let tasks = svc.list_tasks();
    assert_eq!(tasks[0].run_count, 3);
    let runs = svc.list_task_runs(&tasks[0].id).unwrap();
    assert_eq!(runs.len(), 3);
}

// ============================================================================
// 31-35: Tick execution behaviour
// ============================================================================

#[tokio::test]
async fn t31_tick_executes_due_once_task() {
    let bus = EventBus::new(32);
    let svc = Arc::new(
        SchedulerService::in_memory(bus.clone(), hive_scheduler::SchedulerConfig::default())
            .expect("svc"),
    );
    svc.create_task(hive_contracts::CreateTaskRequest {
        name: "t31".into(),
        description: None,
        schedule: hive_contracts::TaskSchedule::Once,
        action: hive_contracts::TaskAction::EmitEvent {
            topic: "t31.fire".into(),
            payload: json!({"x": 1}),
        },
        owner_session_id: None,
        owner_agent_id: None,
        max_retries: None,
        retry_delay_ms: None,
    })
    .unwrap();

    // Subscribe after task creation to skip the "scheduler.task.created" event.
    let mut rx = bus.subscribe();
    svc.tick().await;

    let env = rx.try_recv().expect("event");
    assert_eq!(env.topic, "t31.fire");
    let tasks = svc.list_tasks();
    assert_eq!(tasks[0].status, hive_api::TaskStatus::Completed);
}

#[tokio::test]
async fn t32_tick_skips_cancelled_tasks() {
    let bus = EventBus::new(32);
    let svc = Arc::new(
        SchedulerService::in_memory(bus, hive_scheduler::SchedulerConfig::default()).expect("svc"),
    );
    let task = svc
        .create_task(hive_contracts::CreateTaskRequest {
            name: "t32".into(),
            description: None,
            schedule: hive_contracts::TaskSchedule::Once,
            action: hive_contracts::TaskAction::EmitEvent {
                topic: "skip".into(),
                payload: json!({}),
            },
            owner_session_id: None,
            owner_agent_id: None,
            max_retries: None,
            retry_delay_ms: None,
        })
        .unwrap();
    svc.cancel_task(&task.id).unwrap();
    svc.tick().await;
    let fetched = svc.get_task(&task.id).unwrap();
    assert_eq!(fetched.status, hive_api::TaskStatus::Cancelled);
    assert_eq!(fetched.run_count, 0);
}

#[tokio::test]
async fn t33_tick_skips_future_scheduled_task() {
    let bus = EventBus::new(32);
    let svc = Arc::new(
        SchedulerService::in_memory(bus, hive_scheduler::SchedulerConfig::default()).expect("svc"),
    );
    svc.create_task(hive_contracts::CreateTaskRequest {
        name: "t33".into(),
        description: None,
        schedule: hive_contracts::TaskSchedule::Scheduled {
            run_at_ms: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64
                + 99_999_000,
        },
        action: hive_contracts::TaskAction::EmitEvent { topic: "nope".into(), payload: json!({}) },
        owner_session_id: None,
        owner_agent_id: None,
        max_retries: None,
        retry_delay_ms: None,
    })
    .unwrap();

    svc.tick().await;
    let tasks = svc.list_tasks();
    assert_eq!(tasks[0].status, hive_api::TaskStatus::Pending);
    assert_eq!(tasks[0].run_count, 0);
}

#[tokio::test]
async fn t34_cron_resets_to_pending_after_execution() {
    let bus = EventBus::new(32);
    let svc = Arc::new(
        SchedulerService::in_memory(bus, hive_scheduler::SchedulerConfig::default()).expect("svc"),
    );
    svc.create_task(hive_contracts::CreateTaskRequest {
        name: "t34".into(),
        description: None,
        schedule: hive_contracts::TaskSchedule::Cron { expression: "0 */10 * * * * *".to_string() },
        action: hive_contracts::TaskAction::EmitEvent { topic: "hb".into(), payload: json!({}) },
        owner_session_id: None,
        owner_agent_id: None,
        max_retries: None,
        retry_delay_ms: None,
    })
    .unwrap();
    svc.force_all_due();
    svc.tick().await;
    let tasks = svc.list_tasks();
    assert_eq!(tasks[0].status, hive_api::TaskStatus::Pending);
    assert_eq!(tasks[0].run_count, 1);
    assert!(tasks[0].next_run_ms.unwrap() > tasks[0].last_run_ms.unwrap());
}

#[tokio::test]
async fn t35_webhook_success_completes_task() {
    use axum::{routing::get, Router};

    let app = Router::new().route("/ok", get(|| async { "ok" }));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let bus = EventBus::new(32);
    let svc = Arc::new(
        SchedulerService::in_memory(bus, hive_scheduler::SchedulerConfig::default()).expect("svc"),
    );
    svc.create_task(hive_contracts::CreateTaskRequest {
        name: "t35".into(),
        description: None,
        schedule: hive_contracts::TaskSchedule::Once,
        action: hive_contracts::TaskAction::HttpWebhook {
            url: format!("http://{addr}/ok"),
            method: "GET".into(),
            body: None,
            headers: None,
        },
        owner_session_id: None,
        owner_agent_id: None,
        max_retries: None,
        retry_delay_ms: None,
    })
    .unwrap();
    svc.tick().await;
    let tasks = svc.list_tasks();
    assert_eq!(tasks[0].status, hive_api::TaskStatus::Completed);
    assert_eq!(tasks[0].run_count, 1);
}

// ============================================================================
// 36-40: Agent creates tasks via ScheduleTaskTool
// ============================================================================

#[tokio::test]
async fn t36_agent_tool_creates_once_task() {
    let (_base, shutdown, _d, svc) = boot().await;
    let tool = ScheduleTaskTool::new(svc, Some("session-A".into()), vec!["*".to_string()], None);

    let result = hive_tools::Tool::execute(
        &tool,
        json!({
            "operation": "create",
            "name": "agent-task-1",
            "schedule": {"type": "once"},
            "action": {"type": "emit_event", "topic": "agent.fire", "payload": {}}
        }),
    )
    .await
    .expect("tool execute");

    let output = &result.output;
    assert!(output["id"].as_str().unwrap().starts_with("task-"));
    assert_eq!(output["owner_session_id"], "session-A");
    assert_eq!(output["status"], "pending");
    shutdown.notify_waiters();
}

#[tokio::test]
async fn t37_agent_tool_lists_only_own_session_tasks() {
    let (base, shutdown, _d, svc) = boot().await;
    let c = authed_client();

    // Create tasks for two sessions via API
    create(&c, &base, owned_task("other_task", "session-B", None)).await;

    // Agent in session-A creates a task
    let tool_a = ScheduleTaskTool::new(
        Arc::clone(&svc),
        Some("session-A".into()),
        vec!["*".to_string()],
        None,
    );
    hive_tools::Tool::execute(
        &tool_a,
        json!({
            "operation": "create",
            "name": "my-task",
            "schedule": {"type": "once"},
            "action": {"type": "emit_event", "topic": "x", "payload": {}}
        }),
    )
    .await
    .unwrap();

    // List via tool — should only see session-A's task
    let result = hive_tools::Tool::execute(&tool_a, json!({"operation": "list"})).await.unwrap();
    let tasks = result.output.as_array().unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0]["name"], "my-task");

    shutdown.notify_waiters();
}

#[tokio::test]
async fn t38_agent_tool_cancels_task() {
    let (_base, shutdown, _d, svc) = boot().await;
    let tool = ScheduleTaskTool::new(svc, Some("sess-C".into()), vec!["*".to_string()], None);

    let created = hive_tools::Tool::execute(
        &tool,
        json!({
            "operation": "create",
            "name": "cancel-me",
            "schedule": {"type": "once"},
            "action": {"type": "emit_event", "topic": "x", "payload": {}}
        }),
    )
    .await
    .unwrap();
    let task_id = created.output["id"].as_str().unwrap();

    let cancelled =
        hive_tools::Tool::execute(&tool, json!({"operation": "cancel", "task_id": task_id}))
            .await
            .unwrap();
    assert_eq!(cancelled.output["status"], "cancelled");

    shutdown.notify_waiters();
}

#[tokio::test]
async fn t39_agent_tool_updates_task() {
    let (_base, shutdown, _d, svc) = boot().await;
    let tool = ScheduleTaskTool::new(svc, None, vec!["*".to_string()], None);

    let created = hive_tools::Tool::execute(
        &tool,
        json!({
            "operation": "create",
            "name": "update-me",
            "schedule": {"type": "once"},
            "action": {"type": "emit_event", "topic": "x", "payload": {}}
        }),
    )
    .await
    .unwrap();
    let task_id = created.output["id"].as_str().unwrap();

    let updated = hive_tools::Tool::execute(
        &tool,
        json!({
            "operation": "update",
            "task_id": task_id,
            "name": "updated-name",
            "description": "new description"
        }),
    )
    .await
    .unwrap();
    assert_eq!(updated.output["name"], "updated-name");
    assert_eq!(updated.output["description"], "new description");

    shutdown.notify_waiters();
}

#[tokio::test]
async fn t40_agent_tool_deletes_and_gets_runs() {
    let (_base, shutdown, _d, svc) = boot().await;
    let tool = ScheduleTaskTool::new(Arc::clone(&svc), None, vec!["*".to_string()], None);

    let created = hive_tools::Tool::execute(
        &tool,
        json!({
            "operation": "create",
            "name": "run-me",
            "schedule": {"type": "once"},
            "action": {"type": "emit_event", "topic": "x", "payload": {}}
        }),
    )
    .await
    .unwrap();
    let task_id = created.output["id"].as_str().unwrap();

    // Execute via tick
    svc.force_all_due();
    svc.tick().await;

    // Get runs via tool
    let runs_result =
        hive_tools::Tool::execute(&tool, json!({"operation": "get_runs", "task_id": task_id}))
            .await
            .unwrap();
    let runs = runs_result.output.as_array().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["status"], "success");

    // Delete via tool
    let del_result =
        hive_tools::Tool::execute(&tool, json!({"operation": "delete", "task_id": task_id}))
            .await
            .unwrap();
    assert_eq!(del_result.output["deleted"], task_id);

    shutdown.notify_waiters();
}

// ============================================================================
// 41-45: Agent invoked by tasks (SendMessage action)
// ============================================================================

#[tokio::test]
async fn t41_send_message_invokes_session_endpoint() {
    use axum::{routing::post, Router};
    use std::sync::atomic::{AtomicUsize, Ordering};

    let call_count = Arc::new(AtomicUsize::new(0));
    let counter = call_count.clone();
    let received_body = Arc::new(parking_lot::Mutex::new(String::new()));
    let body_capture = received_body.clone();

    let app = Router::new().route(
        "/api/v1/chat/sessions/{session_id}/messages",
        post(move |body: String| {
            let counter = counter.clone();
            let body_capture = body_capture.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                *body_capture.lock() = body;
                axum::Json(json!({"kind": "queued", "session": {}}))
            }
        }),
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let bus = EventBus::new(32);
    let svc = Arc::new(
        SchedulerService::in_memory_with_addr(
            bus,
            addr.to_string(),
            hive_scheduler::SchedulerConfig::default(),
        )
        .expect("svc"),
    );
    svc.create_task(hive_contracts::CreateTaskRequest {
        name: "invoke-agent".into(),
        description: Some("Task that invokes an agent session".into()),
        schedule: hive_contracts::TaskSchedule::Once,
        action: hive_contracts::TaskAction::SendMessage {
            session_id: "agent-session-1".into(),
            content: "Please run the daily report".into(),
        },
        owner_session_id: Some("parent-session".into()),
        owner_agent_id: None,
        max_retries: None,
        retry_delay_ms: None,
    })
    .unwrap();

    svc.tick().await;

    assert_eq!(call_count.load(Ordering::SeqCst), 1);
    let body: Value = serde_json::from_str(&received_body.lock()).unwrap();
    assert_eq!(body["content"], "Please run the daily report");
    let tasks = svc.list_tasks();
    assert_eq!(tasks[0].status, hive_api::TaskStatus::Completed);
}

#[tokio::test]
async fn t42_cron_task_invokes_agent_multiple_times() {
    use axum::{routing::post, Router};
    use std::sync::atomic::{AtomicUsize, Ordering};

    let call_count = Arc::new(AtomicUsize::new(0));
    let counter = call_count.clone();

    let app = Router::new().route(
        "/api/v1/chat/sessions/{session_id}/messages",
        post(move || {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                axum::Json(json!({"kind": "queued", "session": {}}))
            }
        }),
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let bus = EventBus::new(32);
    let svc = Arc::new(
        SchedulerService::in_memory_with_addr(
            bus,
            addr.to_string(),
            hive_scheduler::SchedulerConfig::default(),
        )
        .expect("svc"),
    );
    svc.create_task(hive_contracts::CreateTaskRequest {
        name: "recurring-invoke".into(),
        description: None,
        schedule: hive_contracts::TaskSchedule::Cron { expression: "0 0 * * * * *".to_string() },
        action: hive_contracts::TaskAction::SendMessage {
            session_id: "agent-session-recurring".into(),
            content: "heartbeat check".into(),
        },
        owner_session_id: None,
        owner_agent_id: Some("monitor-agent".into()),
        max_retries: None,
        retry_delay_ms: None,
    })
    .unwrap();

    // Run 3 ticks
    for _ in 0..3 {
        svc.force_all_due();
        svc.tick().await;
    }

    assert_eq!(call_count.load(Ordering::SeqCst), 3);
    let tasks = svc.list_tasks();
    assert_eq!(tasks[0].run_count, 3);
    assert_eq!(tasks[0].status, hive_api::TaskStatus::Pending); // cron stays pending
}

#[tokio::test]
async fn t43_agent_creates_task_that_invokes_another_session() {
    // The tool now calls the scheduler directly (no HTTP round-trip),
    // so we just need an in-memory scheduler.
    let bus = EventBus::new(32);
    let svc = Arc::new(
        SchedulerService::in_memory(bus, hive_scheduler::SchedulerConfig::default()).expect("svc"),
    );

    // Agent in session-X creates a task to invoke session-Y
    let tool = ScheduleTaskTool::new(svc, Some("session-X".into()), vec!["*".to_string()], None);
    let result = hive_tools::Tool::execute(
        &tool,
        json!({
            "operation": "create",
            "name": "delegate-to-Y",
            "schedule": {"type": "once"},
            "action": {
                "type": "send_message",
                "session_id": "session-Y",
                "content": "Process the batch job"
            }
        }),
    )
    .await
    .unwrap();

    assert!(result.output["id"].as_str().unwrap().starts_with("task-"));
    assert_eq!(result.output["owner_session_id"], "session-X");
    assert_eq!(result.output["action"]["session_id"], "session-Y");
}

#[tokio::test]
async fn t44_failed_send_message_records_error_in_runs() {
    use axum::{routing::post, Router};

    // Server that rejects the message
    let app = Router::new().route(
        "/api/v1/chat/sessions/{session_id}/messages",
        post(|| async { (StatusCode::BAD_REQUEST, "session not found") }),
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let bus = EventBus::new(32);
    let svc = Arc::new(
        SchedulerService::in_memory_with_addr(
            bus,
            addr.to_string(),
            hive_scheduler::SchedulerConfig::default(),
        )
        .expect("svc"),
    );
    let task = svc
        .create_task(hive_contracts::CreateTaskRequest {
            name: "fail-invoke".into(),
            description: None,
            schedule: hive_contracts::TaskSchedule::Once,
            action: hive_contracts::TaskAction::SendMessage {
                session_id: "nonexistent-session".into(),
                content: "hello?".into(),
            },
            owner_session_id: Some("agent-sess".into()),
            owner_agent_id: Some("agent-1".into()),
            max_retries: None,
            retry_delay_ms: None,
        })
        .unwrap();

    svc.tick().await;

    let t = svc.get_task(&task.id).unwrap();
    assert_eq!(t.status, hive_api::TaskStatus::Failed);
    assert!(t.last_error.as_ref().unwrap().contains("400"));

    let runs = svc.list_task_runs(&task.id).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, hive_api::TaskRunStatus::Failure);
    assert!(runs[0].error.as_ref().unwrap().contains("400"));
}

#[tokio::test]
async fn t45_agent_creates_cron_webhook_task() {
    let (_base, shutdown, _d, svc) = boot().await;
    let tool = ScheduleTaskTool::new(
        Arc::clone(&svc),
        Some("session-webhook".into()),
        vec!["*".to_string()],
        None,
    );

    // Agent creates a cron webhook task
    let result = hive_tools::Tool::execute(
        &tool,
        json!({
            "operation": "create",
            "name": "healthcheck",
            "description": "Periodic health check",
            "schedule": {"type": "cron", "expression": "0 */5 * * * * *"},
            "action": {
                "type": "http_webhook",
                "url": "http://example.com/health",
                "method": "GET"
            }
        }),
    )
    .await
    .unwrap();

    let task_id = result.output["id"].as_str().unwrap();
    assert_eq!(result.output["schedule"]["type"], "cron");
    assert_eq!(result.output["owner_session_id"], "session-webhook");

    // Verify via direct service call
    let task = svc.get_task(task_id).unwrap();
    assert_eq!(task.name, "healthcheck");
    assert_eq!(task.description, "Periodic health check");

    shutdown.notify_waiters();
}

// ============================================================================
// 46-50: Complex multi-task & mixed scenarios
// ============================================================================

#[tokio::test]
async fn t46_mixed_schedule_types_coexist() {
    let (base, shutdown, _d, _) = boot().await;
    let c = authed_client();
    create(&c, &base, emit_task("once")).await;
    create(
        &c,
        &base,
        scheduled_task(
            "scheduled",
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64 + 300_000,
        ),
    )
    .await;
    create(&c, &base, cron_task("cron", "0 * * * * * *")).await;

    let tasks = list(&c, &base).await;
    assert_eq!(tasks.len(), 3);
    let types: Vec<&str> = tasks.iter().map(|t| t["schedule"]["type"].as_str().unwrap()).collect();
    assert!(types.contains(&"once"));
    assert!(types.contains(&"scheduled"));
    assert!(types.contains(&"cron"));
    shutdown.notify_waiters();
}

#[tokio::test]
async fn t47_concurrent_create_and_cancel() {
    let (base, shutdown, _d, _) = boot().await;
    let c = authed_client();

    let mut ids = Vec::new();
    for i in 0..10 {
        let task = create(&c, &base, emit_task(&format!("t47_{i}"))).await;
        ids.push(task["id"].as_str().unwrap().to_string());
    }
    for (i, id) in ids.iter().enumerate() {
        if i % 2 == 0 {
            c.post(format!("{base}/api/v1/scheduler/tasks/{id}/cancel")).send().await.unwrap();
        }
    }

    let tasks = list(&c, &base).await;
    assert_eq!(tasks.len(), 10);
    let cancelled: Vec<_> = tasks.iter().filter(|t| t["status"] == "cancelled").collect();
    let pending: Vec<_> = tasks.iter().filter(|t| t["status"] == "pending").collect();
    assert_eq!(cancelled.len(), 5);
    assert_eq!(pending.len(), 5);
    shutdown.notify_waiters();
}

#[tokio::test]
async fn t48_multi_owner_isolation() {
    let (base, shutdown, _d, _) = boot().await;
    let c = authed_client();

    for i in 0..3 {
        create(&c, &base, owned_task(&format!("s1_{i}"), "session-1", Some("agent-1"))).await;
        create(&c, &base, owned_task(&format!("s2_{i}"), "session-2", Some("agent-2"))).await;
    }

    let all = list(&c, &base).await;
    assert_eq!(all.len(), 6);

    let s1 = list_filtered(&c, &base, "session_id=session-1").await;
    assert_eq!(s1.len(), 3);
    for t in &s1 {
        assert!(t["name"].as_str().unwrap().starts_with("s1_"));
    }

    let s2 = list_filtered(&c, &base, "session_id=session-2").await;
    assert_eq!(s2.len(), 3);

    let a1 = list_filtered(&c, &base, "agent_id=agent-1").await;
    assert_eq!(a1.len(), 3);

    shutdown.notify_waiters();
}

#[tokio::test]
async fn t49_execution_then_update_reschedule() {
    let bus = EventBus::new(32);
    let svc = Arc::new(
        SchedulerService::in_memory(bus, hive_scheduler::SchedulerConfig::default()).expect("svc"),
    );

    let task = svc
        .create_task(hive_contracts::CreateTaskRequest {
            name: "t49".into(),
            description: None,
            schedule: hive_contracts::TaskSchedule::Once,
            action: hive_contracts::TaskAction::EmitEvent {
                topic: "t49".into(),
                payload: json!({}),
            },
            owner_session_id: None,
            owner_agent_id: None,
            max_retries: None,
            retry_delay_ms: None,
        })
        .unwrap();
    svc.tick().await;
    let completed = svc.get_task(&task.id).unwrap();
    assert_eq!(completed.status, hive_api::TaskStatus::Completed);

    // Update schedule — resets to pending
    let updated = svc
        .update_task(
            &task.id,
            hive_contracts::UpdateTaskRequest {
                name: None,
                description: None,
                schedule: Some(hive_contracts::TaskSchedule::Once),
                action: None,
                max_retries: None,
                retry_delay_ms: None,
            },
        )
        .unwrap();
    assert_eq!(updated.status, hive_api::TaskStatus::Pending);

    svc.force_all_due();
    svc.tick().await;
    let re_completed = svc.get_task(&task.id).unwrap();
    assert_eq!(re_completed.status, hive_api::TaskStatus::Completed);
    assert_eq!(re_completed.run_count, 2);

    let runs = svc.list_task_runs(&task.id).unwrap();
    assert_eq!(runs.len(), 2);
}

#[tokio::test]
async fn t50_failed_task_update_reschedule_retry() {
    use axum::{routing::post, Router};
    use std::sync::atomic::{AtomicUsize, Ordering};

    let call_count = Arc::new(AtomicUsize::new(0));
    let counter = call_count.clone();

    // First call fails, second succeeds
    let app = Router::new().route(
        "/hook",
        post(move || {
            let counter = counter.clone();
            async move {
                let n = counter.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    (StatusCode::INTERNAL_SERVER_ERROR, "fail first time")
                } else {
                    (StatusCode::OK, "ok")
                }
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let bus = EventBus::new(32);
    let svc = Arc::new(
        SchedulerService::in_memory(bus, hive_scheduler::SchedulerConfig::default()).expect("svc"),
    );
    let task = svc
        .create_task(hive_contracts::CreateTaskRequest {
            name: "t50".into(),
            description: None,
            schedule: hive_contracts::TaskSchedule::Once,
            action: hive_contracts::TaskAction::HttpWebhook {
                url: format!("http://{addr}/hook"),
                method: "POST".into(),
                body: None,
                headers: None,
            },
            owner_session_id: None,
            owner_agent_id: None,
            max_retries: None,
            retry_delay_ms: None,
        })
        .unwrap();

    // First execution — should fail
    svc.tick().await;
    let failed = svc.get_task(&task.id).unwrap();
    assert_eq!(failed.status, hive_api::TaskStatus::Failed);
    assert!(failed.last_error.is_some());

    // Update schedule — resets failed → pending
    let updated = svc
        .update_task(
            &task.id,
            hive_contracts::UpdateTaskRequest {
                name: None,
                description: None,
                schedule: Some(hive_contracts::TaskSchedule::Once),
                action: None,
                max_retries: None,
                retry_delay_ms: None,
            },
        )
        .unwrap();
    assert_eq!(updated.status, hive_api::TaskStatus::Pending);

    // Second execution — should succeed
    svc.force_all_due();
    svc.tick().await;
    let succeeded = svc.get_task(&task.id).unwrap();
    assert_eq!(succeeded.status, hive_api::TaskStatus::Completed);
    assert!(succeeded.last_error.is_none());
    assert_eq!(succeeded.run_count, 2);

    let runs = svc.list_task_runs(&task.id).unwrap();
    assert_eq!(runs.len(), 2);
    let statuses: Vec<_> = runs.iter().map(|r| r.status.clone()).collect();
    assert!(statuses.contains(&hive_api::TaskRunStatus::Success));
    assert!(statuses.contains(&hive_api::TaskRunStatus::Failure));
}

// ============================================================================
// 51-62: New action types (InvokeAgent, CallTool, CompositeAction)
// ============================================================================

// -- CRUD for new action types via API --

#[tokio::test]
async fn t51_create_invoke_agent_task_via_api() {
    let (base, shutdown, _d, _) = boot().await;
    let c = authed_client();
    let task = create(
        &c,
        &base,
        json!({
            "name": "t51-invoke",
            "schedule": {"type": "once"},
            "action": {
                "type": "invoke_agent",
                "persona_id": "analyst",
                "task": "Summarize yesterday's activity",
                "friendly_name": "daily-summary",
                "timeout_secs": 120
            }
        }),
    )
    .await;
    assert!(task["id"].as_str().unwrap().starts_with("task-"));
    assert_eq!(task["action"]["type"], "invoke_agent");
    assert_eq!(task["action"]["persona_id"], "analyst");
    assert_eq!(task["action"]["task"], "Summarize yesterday's activity");
    assert_eq!(task["action"]["friendly_name"], "daily-summary");
    assert_eq!(task["action"]["timeout_secs"], 120);
    shutdown.notify_waiters();
}

#[tokio::test]
async fn t52_create_call_tool_task_via_api() {
    let (base, shutdown, _d, _) = boot().await;
    let c = authed_client();
    let task = create(
        &c,
        &base,
        json!({
            "name": "t52-call-tool",
            "schedule": {"type": "cron", "expression": "0 0 * * * * *"},
            "action": {
                "type": "call_tool",
                "tool_id": "comm.send_external_message",
                "arguments": {
                    "connector_id": "slack-work",
                    "to": "#team-channel",
                    "body": "Daily standup reminder!"
                }
            }
        }),
    )
    .await;
    assert_eq!(task["action"]["type"], "call_tool");
    assert_eq!(task["action"]["tool_id"], "comm.send_external_message");
    assert_eq!(task["action"]["arguments"]["to"], "#team-channel");
    shutdown.notify_waiters();
}

#[tokio::test]
async fn t53_create_composite_action_task_via_api() {
    let (base, shutdown, _d, _) = boot().await;
    let c = authed_client();
    let task = create(
        &c,
        &base,
        json!({
            "name": "t53-composite",
            "schedule": {"type": "once"},
            "action": {
                "type": "composite_action",
                "actions": [
                    {"type": "emit_event", "topic": "step1", "payload": {"data": 1}},
                    {"type": "emit_event", "topic": "step2", "payload": {"data": 2}}
                ],
                "stop_on_failure": true
            }
        }),
    )
    .await;
    assert_eq!(task["action"]["type"], "composite_action");
    assert_eq!(task["action"]["actions"].as_array().unwrap().len(), 2);
    assert_eq!(task["action"]["stop_on_failure"], true);
    shutdown.notify_waiters();
}

// -- Round-trip via get --

#[tokio::test]
async fn t54_invoke_agent_round_trips_through_get() {
    let (base, shutdown, _d, _) = boot().await;
    let c = authed_client();
    let created = create(
        &c,
        &base,
        json!({
            "name": "t54",
            "schedule": {"type": "once"},
            "action": {
                "type": "invoke_agent",
                "persona_id": "reviewer",
                "task": "Review PRs"
            }
        }),
    )
    .await;
    let id = created["id"].as_str().unwrap();
    let (status, fetched) = get_task(&c, &base, id).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fetched["action"]["persona_id"], "reviewer");
    assert_eq!(fetched["action"]["task"], "Review PRs");
    // Optional fields should be null/absent
    assert!(fetched["action"]["friendly_name"].is_null());
    shutdown.notify_waiters();
}

// -- Execution: InvokeAgent without runner → graceful failure --

#[tokio::test]
async fn t55_invoke_agent_fails_gracefully_without_runner() {
    let bus = EventBus::new(32);
    let svc = Arc::new(
        SchedulerService::in_memory(bus, hive_scheduler::SchedulerConfig::default()).expect("svc"),
    );
    svc.create_task(hive_contracts::CreateTaskRequest {
        name: "t55".into(),
        description: None,
        schedule: hive_contracts::TaskSchedule::Once,
        action: hive_contracts::TaskAction::InvokeAgent {
            persona_id: "analyst".into(),
            task: "Do something".into(),
            friendly_name: None,
            timeout_secs: None,
            permissions: None,
        },
        owner_session_id: None,
        owner_agent_id: None,
        max_retries: None,
        retry_delay_ms: None,
    })
    .unwrap();

    svc.tick().await;

    let tasks = svc.list_tasks();
    assert_eq!(tasks[0].status, hive_api::TaskStatus::Failed);
    assert!(tasks[0].last_error.as_ref().unwrap().contains("no agent runner configured"));
}

// -- Execution: CallTool without executor → graceful failure --

#[tokio::test]
async fn t56_call_tool_fails_gracefully_without_executor() {
    let bus = EventBus::new(32);
    let svc = Arc::new(
        SchedulerService::in_memory(bus, hive_scheduler::SchedulerConfig::default()).expect("svc"),
    );
    svc.create_task(hive_contracts::CreateTaskRequest {
        name: "t56".into(),
        description: None,
        schedule: hive_contracts::TaskSchedule::Once,
        action: hive_contracts::TaskAction::CallTool {
            tool_id: "comm.send_external_message".into(),
            arguments: json!({"to": "#test", "body": "hi"}),
        },
        owner_session_id: None,
        owner_agent_id: None,
        max_retries: None,
        retry_delay_ms: None,
    })
    .unwrap();

    svc.tick().await;

    let tasks = svc.list_tasks();
    assert_eq!(tasks[0].status, hive_api::TaskStatus::Failed);
    assert!(tasks[0].last_error.as_ref().unwrap().contains("no tool executor configured"));
}

// -- Execution: CompositeAction with emit_events succeeds --

#[tokio::test]
async fn t57_composite_action_executes_all_emit_events() {
    let bus = EventBus::new(32);
    let svc = Arc::new(
        SchedulerService::in_memory(bus, hive_scheduler::SchedulerConfig::default()).expect("svc"),
    );
    svc.create_task(hive_contracts::CreateTaskRequest {
        name: "t57".into(),
        description: None,
        schedule: hive_contracts::TaskSchedule::Once,
        action: hive_contracts::TaskAction::CompositeAction {
            actions: vec![
                hive_contracts::TaskAction::EmitEvent {
                    topic: "step.1".into(),
                    payload: json!({"n": 1}),
                },
                hive_contracts::TaskAction::EmitEvent {
                    topic: "step.2".into(),
                    payload: json!({"n": 2}),
                },
            ],
            stop_on_failure: false,
        },
        owner_session_id: None,
        owner_agent_id: None,
        max_retries: None,
        retry_delay_ms: None,
    })
    .unwrap();

    svc.tick().await;

    let tasks = svc.list_tasks();
    assert_eq!(tasks[0].status, hive_api::TaskStatus::Completed);
    assert_eq!(tasks[0].run_count, 1);

    // Check run has a result
    let runs = svc.list_task_runs(&tasks[0].id).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, hive_api::TaskRunStatus::Success);
    assert!(runs[0].result.is_some());
}

// -- Execution: CompositeAction with stop_on_failure halts at first error --

#[tokio::test]
async fn t58_composite_stop_on_failure_halts_at_first_error() {
    let bus = EventBus::new(32);
    let svc = Arc::new(
        SchedulerService::in_memory(bus, hive_scheduler::SchedulerConfig::default()).expect("svc"),
    );
    svc.create_task(hive_contracts::CreateTaskRequest {
        name: "t58".into(),
        description: None,
        schedule: hive_contracts::TaskSchedule::Once,
        action: hive_contracts::TaskAction::CompositeAction {
            actions: vec![
                hive_contracts::TaskAction::EmitEvent { topic: "ok".into(), payload: json!({}) },
                // CallTool without executor will fail
                hive_contracts::TaskAction::CallTool {
                    tool_id: "nonexistent.tool".into(),
                    arguments: json!({}),
                },
                // This should NOT execute because stop_on_failure = true
                hive_contracts::TaskAction::EmitEvent {
                    topic: "should.not.run".into(),
                    payload: json!({}),
                },
            ],
            stop_on_failure: true,
        },
        owner_session_id: None,
        owner_agent_id: None,
        max_retries: None,
        retry_delay_ms: None,
    })
    .unwrap();

    svc.tick().await;

    let tasks = svc.list_tasks();
    assert_eq!(tasks[0].status, hive_api::TaskStatus::Failed);
    assert!(tasks[0].last_error.as_ref().unwrap().contains("CompositeAction stopped at action 1"));
}

// -- Notification: EventBus receives completion event --

#[tokio::test]
async fn t59_eventbus_receives_task_completion_notification() {
    let bus = EventBus::new(32);
    let mut rx = bus.subscribe();
    let svc = Arc::new(
        SchedulerService::in_memory(bus, hive_scheduler::SchedulerConfig::default()).expect("svc"),
    );

    svc.create_task(hive_contracts::CreateTaskRequest {
        name: "t59".into(),
        description: None,
        schedule: hive_contracts::TaskSchedule::Once,
        action: hive_contracts::TaskAction::EmitEvent {
            topic: "test.notify".into(),
            payload: json!({}),
        },
        owner_session_id: Some("sess-A".into()),
        owner_agent_id: None,
        max_retries: None,
        retry_delay_ms: None,
    })
    .unwrap();

    svc.tick().await;

    // Drain events and find our notification
    let mut found_completion = false;
    while let Ok(envelope) = rx.try_recv() {
        if envelope.topic == "scheduler.task.completed" {
            found_completion = true;
            assert_eq!(envelope.payload["task_name"], "t59");
            assert_eq!(envelope.payload["status"], "success");
        }
    }
    assert!(found_completion, "Expected scheduler.task.completed event on EventBus");
}

// -- Agent tool: create invoke_agent task --

#[tokio::test]
async fn t60_agent_tool_creates_invoke_agent_task() {
    let bus = EventBus::new(32);
    let svc = Arc::new(
        SchedulerService::in_memory(bus, hive_scheduler::SchedulerConfig::default()).expect("svc"),
    );
    let tool = ScheduleTaskTool::new(svc, Some("session-B".into()), vec!["*".to_string()], None);

    let result = hive_tools::Tool::execute(
        &tool,
        json!({
            "operation": "create",
            "name": "agent-invoke-task",
            "schedule": {"type": "cron", "expression": "0 0 * * * * *"},
            "action": {
                "type": "invoke_agent",
                "persona_id": "monitor",
                "task": "Check system health"
            }
        }),
    )
    .await
    .expect("tool execute");

    assert_eq!(result.output["action"]["type"], "invoke_agent");
    assert_eq!(result.output["action"]["persona_id"], "monitor");
    assert_eq!(result.output["owner_session_id"], "session-B");
}

// -- Agent tool: create call_tool task --

#[tokio::test]
async fn t61_agent_tool_creates_call_tool_task() {
    let bus = EventBus::new(32);
    let svc = Arc::new(
        SchedulerService::in_memory(bus, hive_scheduler::SchedulerConfig::default()).expect("svc"),
    );
    let tool = ScheduleTaskTool::new(svc, Some("session-C".into()), vec!["*".to_string()], None);

    let result = hive_tools::Tool::execute(
        &tool,
        json!({
            "operation": "create",
            "name": "scheduled-tool-call",
            "schedule": {"type": "cron", "expression": "0 0 9 * * MON-FRI *"},
            "action": {
                "type": "call_tool",
                "tool_id": "mcp.github.create_issue",
                "arguments": {"title": "Daily triage", "body": "Triage items"}
            }
        }),
    )
    .await
    .expect("tool execute");

    assert_eq!(result.output["action"]["type"], "call_tool");
    assert_eq!(result.output["action"]["tool_id"], "mcp.github.create_issue");
}

// -- TaskRun.result populated on webhook success --

#[tokio::test]
async fn t62_webhook_success_populates_task_run_result() {
    use axum::{routing::get, Router};

    let app = Router::new().route("/data", get(|| async { axum::Json(json!({"answer": 42})) }));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let bus = EventBus::new(32);
    let svc = Arc::new(
        SchedulerService::in_memory(bus, hive_scheduler::SchedulerConfig::default()).expect("svc"),
    );
    svc.create_task(hive_contracts::CreateTaskRequest {
        name: "t62".into(),
        description: None,
        schedule: hive_contracts::TaskSchedule::Once,
        action: hive_contracts::TaskAction::HttpWebhook {
            url: format!("http://{addr}/data"),
            method: "GET".into(),
            body: None,
            headers: None,
        },
        owner_session_id: None,
        owner_agent_id: None,
        max_retries: None,
        retry_delay_ms: None,
    })
    .unwrap();

    svc.tick().await;

    let tasks = svc.list_tasks();
    assert_eq!(tasks[0].status, hive_api::TaskStatus::Completed);

    let runs = svc.list_task_runs(&tasks[0].id).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, hive_api::TaskRunStatus::Success);
    // Result should contain the JSON response
    let result = runs[0].result.as_ref().expect("result should be populated");
    assert_eq!(result["answer"], 42);
}

// ============================================================================
// Approval flow + privilege scoping tests (t63-t70)
// ============================================================================

/// Tool ID resolution: sanitized ID (comm_send_message) should be resolved
/// when creating a CallTool task via the scheduler's tool_executor.
#[tokio::test]
async fn t63_call_tool_resolves_sanitized_tool_id() {
    // Create a scheduler with a tool executor that has tools
    let (_base, shutdown, _d, svc) = boot().await;

    // The scheduler from boot() has a tool executor. Check resolve_tool_id.
    if let Some(executor) = svc.tool_executor() {
        let ids = executor.list_tool_ids();
        // Find a tool with dots in its ID
        if let Some(dotted) = ids.iter().find(|id| id.contains('.')) {
            let sanitized = dotted.replace('.', "_");
            let resolved = executor.resolve_tool_id(&sanitized);
            assert_eq!(resolved, Some(dotted.clone()), "sanitized ID should resolve to canonical");
        }
    }

    let _ = shutdown;
}

/// Tool ID resolution: "functions." prefix should be stripped.
#[tokio::test]
async fn t64_call_tool_strips_functions_prefix() {
    let (_base, shutdown, _d, svc) = boot().await;

    if let Some(executor) = svc.tool_executor() {
        let ids = executor.list_tool_ids();
        if let Some(id) = ids.first() {
            let prefixed = format!("functions.{id}");
            let resolved = executor.resolve_tool_id(&prefixed);
            assert_eq!(resolved, Some(id.clone()), "functions. prefix should be stripped");
        }
    }

    let _ = shutdown;
}

/// Privilege check: creating a CallTool for a tool not in allowed_tools should fail.
#[tokio::test]
async fn t65_privilege_check_rejects_disallowed_tool() {
    let bus = EventBus::new(32);
    let svc = Arc::new(
        SchedulerService::in_memory(bus, hive_scheduler::SchedulerConfig::default()).expect("svc"),
    );

    // Only allow "comm.send_external_message" — scheduling "shell.execute" should fail
    let tool = ScheduleTaskTool::new(
        svc,
        Some("session-priv".into()),
        vec!["comm.send_external_message".to_string()],
        None,
    );

    let result = hive_tools::Tool::execute(
        &tool,
        json!({
            "operation": "create",
            "name": "restricted-task",
            "schedule": {"type": "once"},
            "action": {
                "type": "call_tool",
                "tool_id": "shell.execute",
                "arguments": {"command": "ls"}
            }
        }),
    )
    .await;

    // Should fail with privilege error
    assert!(result.is_err(), "should reject tool not in allowed_tools");
    let err = result.unwrap_err().to_string();
    assert!(err.contains("not available"), "error should mention tool not available: {err}");
}

/// Privilege check: core.* tools are always allowed regardless of allowed_tools.
#[tokio::test]
async fn t66_privilege_check_allows_core_tools() {
    let bus = EventBus::new(32);
    let svc = Arc::new(
        SchedulerService::in_memory(bus, hive_scheduler::SchedulerConfig::default()).expect("svc"),
    );

    // Empty allowed list (but core.* should still be allowed)
    let tool = ScheduleTaskTool::new(svc, Some("session-core".into()), vec![], None);

    // core.ask_user should be allowed (and since no executor → Auto approval)
    let result = hive_tools::Tool::execute(
        &tool,
        json!({
            "operation": "create",
            "name": "core-tool-task",
            "schedule": {"type": "once"},
            "action": {
                "type": "call_tool",
                "tool_id": "core.ask_user",
                "arguments": {"question": "What time is it?"}
            }
        }),
    )
    .await
    .expect("core.* should always be allowed");

    assert_eq!(result.output["action"]["type"], "call_tool");
}

/// Privilege check: mcp.* tools are always allowed regardless of allowed_tools.
#[tokio::test]
async fn t67_privilege_check_allows_mcp_tools() {
    let bus = EventBus::new(32);
    let svc = Arc::new(
        SchedulerService::in_memory(bus, hive_scheduler::SchedulerConfig::default()).expect("svc"),
    );

    let tool = ScheduleTaskTool::new(
        svc,
        Some("session-mcp".into()),
        vec![], // empty, but mcp.* should still pass
        None,
    );

    let result = hive_tools::Tool::execute(
        &tool,
        json!({
            "operation": "create",
            "name": "mcp-tool-task",
            "schedule": {"type": "once"},
            "action": {
                "type": "call_tool",
                "tool_id": "mcp.github.create_issue",
                "arguments": {"title": "Test"}
            }
        }),
    )
    .await
    .expect("mcp.* should always be allowed");

    assert_eq!(result.output["action"]["type"], "call_tool");
}

/// Privilege check: wildcard allowed_tools lets everything through.
#[tokio::test]
async fn t68_privilege_check_wildcard_allows_all() {
    let bus = EventBus::new(32);
    let svc = Arc::new(
        SchedulerService::in_memory(bus, hive_scheduler::SchedulerConfig::default()).expect("svc"),
    );

    let tool = ScheduleTaskTool::new(svc, Some("session-wild".into()), vec!["*".to_string()], None);

    let result = hive_tools::Tool::execute(
        &tool,
        json!({
            "operation": "create",
            "name": "wild-task",
            "schedule": {"type": "once"},
            "action": {
                "type": "call_tool",
                "tool_id": "shell.execute",
                "arguments": {"command": "echo hi"}
            }
        }),
    )
    .await
    .expect("wildcard should allow all tools");

    assert_eq!(result.output["action"]["type"], "call_tool");
}

/// Approval flow: tool with Ask approval returns approval_required.
#[tokio::test]
async fn t69_approval_flow_returns_approval_required() {
    use hive_contracts::permissions::{PermissionRule, SessionPermissions};

    let bus = EventBus::new(32);
    let svc = Arc::new(
        SchedulerService::in_memory(bus, hive_scheduler::SchedulerConfig::default()).expect("svc"),
    );

    // Create permissions that mark shell.execute as Ask
    let perms = SessionPermissions::with_rules(vec![PermissionRule {
        tool_pattern: "shell.execute".to_string(),
        scope: "*".to_string(),
        decision: hive_contracts::ToolApproval::Ask,
    }]);

    let tool = ScheduleTaskTool::new(
        svc,
        Some("session-ask".into()),
        vec!["*".to_string()],
        Some(Arc::new(parking_lot::Mutex::new(perms))),
    );

    let result = hive_tools::Tool::execute(
        &tool,
        json!({
            "operation": "create",
            "name": "ask-task",
            "schedule": {"type": "once"},
            "action": {
                "type": "call_tool",
                "tool_id": "shell.execute",
                "arguments": {"command": "ls"}
            }
        }),
    )
    .await
    .expect("should return approval_required, not error");

    assert_eq!(result.output["status"], "approval_required");
    assert_eq!(result.output["tool_id"], "shell.execute");
}

/// Approval flow: with user_approved=true, task is created despite Ask permission.
#[tokio::test]
async fn t70_approval_flow_user_approved_creates_task() {
    use hive_contracts::permissions::{PermissionRule, SessionPermissions};

    let bus = EventBus::new(32);
    let svc = Arc::new(
        SchedulerService::in_memory(bus, hive_scheduler::SchedulerConfig::default()).expect("svc"),
    );

    let perms = SessionPermissions::with_rules(vec![PermissionRule {
        tool_pattern: "shell.execute".to_string(),
        scope: "*".to_string(),
        decision: hive_contracts::ToolApproval::Ask,
    }]);

    let tool = ScheduleTaskTool::new(
        svc,
        Some("session-approved".into()),
        vec!["*".to_string()],
        Some(Arc::new(parking_lot::Mutex::new(perms))),
    );

    let result = hive_tools::Tool::execute(
        &tool,
        json!({
            "operation": "create",
            "name": "approved-task",
            "schedule": {"type": "once"},
            "action": {
                "type": "call_tool",
                "tool_id": "shell.execute",
                "arguments": {"command": "ls"}
            },
            "user_approved": true
        }),
    )
    .await
    .expect("user_approved should create the task");

    // Task should be created (has an id and the action)
    assert!(result.output["id"].is_string());
    assert_eq!(result.output["action"]["type"], "call_tool");
}

/// Approval flow: Deny permission blocks task creation entirely.
#[tokio::test]
async fn t71_deny_permission_blocks_creation() {
    use hive_contracts::permissions::{PermissionRule, SessionPermissions};

    let bus = EventBus::new(32);
    let svc = Arc::new(
        SchedulerService::in_memory(bus, hive_scheduler::SchedulerConfig::default()).expect("svc"),
    );

    let perms = SessionPermissions::with_rules(vec![PermissionRule {
        tool_pattern: "shell.execute".to_string(),
        scope: "*".to_string(),
        decision: hive_contracts::ToolApproval::Deny,
    }]);

    let tool = ScheduleTaskTool::new(
        svc,
        Some("session-deny".into()),
        vec!["*".to_string()],
        Some(Arc::new(parking_lot::Mutex::new(perms))),
    );

    let result = hive_tools::Tool::execute(
        &tool,
        json!({
            "operation": "create",
            "name": "denied-task",
            "schedule": {"type": "once"},
            "action": {
                "type": "call_tool",
                "tool_id": "shell.execute",
                "arguments": {"command": "rm -rf /"}
            }
        }),
    )
    .await;

    assert!(result.is_err(), "denied tool should fail");
    let err = result.unwrap_err().to_string();
    assert!(err.contains("denied"), "error should mention denied: {err}");
}

/// Composite action: privilege check validates each sub-action.
#[tokio::test]
async fn t72_composite_privilege_check() {
    let bus = EventBus::new(32);
    let svc = Arc::new(
        SchedulerService::in_memory(bus, hive_scheduler::SchedulerConfig::default()).expect("svc"),
    );

    // Only allow comm.* tools
    let tool = ScheduleTaskTool::new(
        svc,
        Some("session-comp-priv".into()),
        vec!["comm.send_external_message".to_string()],
        None,
    );

    let result = hive_tools::Tool::execute(
        &tool,
        json!({
            "operation": "create",
            "name": "composite-mixed",
            "schedule": {"type": "once"},
            "action": {
                "type": "composite_action",
                "actions": [
                    {"type": "call_tool", "tool_id": "comm.send_external_message", "arguments": {"to": "x"}},
                    {"type": "call_tool", "tool_id": "shell.execute", "arguments": {"command": "ls"}}
                ],
                "stop_on_failure": true
            }
        }),
    )
    .await;

    // shell.execute is not in allowed_tools → should fail
    assert!(result.is_err(), "composite with disallowed tool should fail");
}

/// Composite action: approval_required aggregates all tools needing approval.
#[tokio::test]
async fn t73_composite_approval_aggregation() {
    use hive_contracts::permissions::{PermissionRule, SessionPermissions};

    let bus = EventBus::new(32);
    let svc = Arc::new(
        SchedulerService::in_memory(bus, hive_scheduler::SchedulerConfig::default()).expect("svc"),
    );

    let perms = SessionPermissions::with_rules(vec![PermissionRule {
        tool_pattern: "comm.*".to_string(),
        scope: "*".to_string(),
        decision: hive_contracts::ToolApproval::Ask,
    }]);

    let tool = ScheduleTaskTool::new(
        svc,
        Some("session-comp-ask".into()),
        vec!["*".to_string()],
        Some(Arc::new(parking_lot::Mutex::new(perms))),
    );

    let result = hive_tools::Tool::execute(
        &tool,
        json!({
            "operation": "create",
            "name": "composite-ask",
            "schedule": {"type": "once"},
            "action": {
                "type": "composite_action",
                "actions": [
                    {"type": "call_tool", "tool_id": "comm.send_external_message", "arguments": {"to": "x"}},
                    {"type": "call_tool", "tool_id": "comm.read_messages", "arguments": {}},
                    {"type": "send_message", "session_id": "s1", "content": "hi"}
                ],
                "stop_on_failure": false
            }
        }),
    )
    .await
    .expect("should return approval_required, not error");

    assert_eq!(result.output["status"], "approval_required");
    let tools = result.output["tools"].as_array().expect("tools array");
    assert!(!tools.is_empty(), "should list tools needing approval");
}
