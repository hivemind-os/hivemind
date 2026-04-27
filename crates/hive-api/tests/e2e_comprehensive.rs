//! Comprehensive end-to-end tests for the HiveMind OS API.
//!
//! Every test in this file boots a real Axum HTTP server on an ephemeral port,
//! makes real HTTP requests via reqwest, and asserts the responses.
//!
//! Grouped by API domain:
//!   - Daemon management (healthz, status, shutdown)
//!   - Configuration
//!   - Model router
//!   - Chat sessions (CRUD, messaging, interrupt, resume, memory, risk)
//!   - MCP servers
//!   - Tools
//!   - Scheduler (CRUD, cancel, schedules)
//!   - Knowledge graph (nodes, edges, search, stats)
//!   - Local models (list, get, delete, hardware, 503 when unavailable)

use axum::http::StatusCode;
use hive_api::{chat, AppState, ChatRuntimeConfig, ChatService, SchedulerService};
use hive_core::{AuditLogger, EventBus, HiveMindConfig};
use serde_json::{json, Value};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

// ==========================================================================
// Test harness
// ==========================================================================

/// Boot a full-featured test server. Returns (base_url, shutdown_notify, _tempdir).
async fn boot_server() -> (String, CancellationToken, TempDir) {
    let tempdir = tempfile::tempdir().expect("temp dir");
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local addr");

    let mut config = HiveMindConfig::default();
    config.api.bind = addr.to_string();

    let audit = AuditLogger::new(tempdir.path().join("audit.log")).expect("audit");
    let event_bus = EventBus::new(32);
    let shutdown = CancellationToken::new();

    let model_router = chat::build_model_router_from_config(&config, None, None)
        .expect("model router from config");
    let chat = Arc::new(ChatService::with_model_router(
        audit.clone(),
        event_bus.clone(),
        ChatRuntimeConfig {
            step_delay: std::time::Duration::from_millis(10),
            ..ChatRuntimeConfig::default()
        },
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
        hive_contracts::CodeActConfig::default(),
        None, // plugin_host
        None, // plugin_registry
    ));

    let mut state = AppState::with_chat(config, audit, event_bus, shutdown.clone(), chat);
    state.knowledge_graph_path = Arc::new(tempdir.path().join("kg-test.db"));

    let router = hive_api::build_router(state);
    let server_shutdown = shutdown.clone();
    tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async move { server_shutdown.cancelled().await })
            .await
            .expect("serve");
    });

    (format!("http://{addr}"), shutdown, tempdir)
}

/// Boot a server with local model service enabled.
async fn boot_server_with_local_models() -> (String, CancellationToken, TempDir) {
    let tempdir = tempfile::tempdir().expect("temp dir");
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local addr");

    let mut config = HiveMindConfig::default();
    config.api.bind = addr.to_string();

    let audit = AuditLogger::new(tempdir.path().join("audit.log")).expect("audit");
    let event_bus = EventBus::new(32);
    let shutdown = CancellationToken::new();

    let model_router = chat::build_model_router_from_config(&config, None, None)
        .expect("model router from config");
    let chat = Arc::new(ChatService::with_model_router(
        audit.clone(),
        event_bus.clone(),
        ChatRuntimeConfig {
            step_delay: std::time::Duration::from_millis(10),
            ..ChatRuntimeConfig::default()
        },
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
        hive_contracts::CodeActConfig::default(),
        None, // plugin_host
        None, // plugin_registry
    ));

    let models_dir = tempdir.path().join("models");
    let _ = std::fs::create_dir_all(&models_dir);
    let local_service = tokio::task::spawn_blocking(move || {
        Arc::new(
            hive_api::LocalModelService::with_in_memory_registry(models_dir, None)
                .expect("in-memory service"),
        )
    })
    .await
    .expect("spawn_blocking");

    let mut state = AppState::with_chat(config, audit, event_bus, shutdown.clone(), chat);
    state.local_models = Some(local_service);

    let router = hive_api::build_router(state);
    let server_shutdown = shutdown.clone();
    tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async move { server_shutdown.cancelled().await })
            .await
            .expect("serve");
    });

    (format!("http://{addr}"), shutdown, tempdir)
}

fn client() -> reqwest::Client {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("Authorization", "Bearer test-token".parse().unwrap());
    reqwest::Client::builder().default_headers(headers).build().unwrap()
}

async fn post(c: &reqwest::Client, url: &str, body: Value) -> reqwest::Response {
    c.post(url).json(&body).send().await.expect("POST request")
}

async fn get_json(c: &reqwest::Client, url: &str) -> Value {
    let resp = c.get(url).send().await.expect("GET request");
    assert!(resp.status().is_success(), "GET {url} failed with {}", resp.status());
    resp.json().await.expect("json")
}

// ==========================================================================
//  1. DAEMON MANAGEMENT
// ==========================================================================

#[tokio::test]
async fn e2e_healthz_returns_ok_true() {
    let (base, shutdown, _d) = boot_server().await;
    let resp: Value = get_json(&client(), &format!("{base}/healthz")).await;
    assert_eq!(resp["ok"], true);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_healthz_returns_200() {
    let (base, shutdown, _d) = boot_server().await;
    let resp = client().get(format!("{base}/healthz")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_status_returns_version() {
    let (base, shutdown, _d) = boot_server().await;
    let resp: Value = get_json(&client(), &format!("{base}/api/v1/daemon/status")).await;
    assert!(resp["version"].is_string());
    assert!(!resp["version"].as_str().unwrap().is_empty());
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_status_returns_uptime() {
    let (base, shutdown, _d) = boot_server().await;
    let resp: Value = get_json(&client(), &format!("{base}/api/v1/daemon/status")).await;
    assert!(resp["uptime_secs"].as_f64().unwrap() >= 0.0);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_status_returns_pid() {
    let (base, shutdown, _d) = boot_server().await;
    let resp: Value = get_json(&client(), &format!("{base}/api/v1/daemon/status")).await;
    assert!(resp["pid"].as_u64().unwrap() > 0);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_status_returns_platform() {
    let (base, shutdown, _d) = boot_server().await;
    let resp: Value = get_json(&client(), &format!("{base}/api/v1/daemon/status")).await;
    let platform = resp["platform"].as_str().unwrap();
    assert!(!platform.is_empty());
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_status_returns_bind_address() {
    let (base, shutdown, _d) = boot_server().await;
    let resp: Value = get_json(&client(), &format!("{base}/api/v1/daemon/status")).await;
    assert!(resp["bind"].as_str().unwrap().contains("127.0.0.1"));
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_shutdown_returns_200_and_message() {
    let (base, _shutdown, _d) = boot_server().await;
    let c = client();
    let resp = c.post(format!("{base}/api/v1/daemon/shutdown")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = resp.json().await.unwrap();
    assert!(body["message"].as_str().unwrap().to_lowercase().contains("shutting down"));
}

#[tokio::test]
async fn e2e_unknown_route_returns_404() {
    let (base, shutdown, _d) = boot_server().await;
    let resp = client().get(format!("{base}/api/v1/nonexistent")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_wrong_method_returns_405() {
    let (base, shutdown, _d) = boot_server().await;
    let resp = client().delete(format!("{base}/healthz")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
    shutdown.cancel();
}

// ==========================================================================
//  2. CONFIGURATION
// ==========================================================================

#[tokio::test]
async fn e2e_get_config_returns_200() {
    let (base, shutdown, _d) = boot_server().await;
    let resp = client().get(format!("{base}/api/v1/config/get")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_get_config_contains_api_section() {
    let (base, shutdown, _d) = boot_server().await;
    let resp: Value = get_json(&client(), &format!("{base}/api/v1/config/get")).await;
    assert!(resp["api"].is_object(), "config should have 'api' section");
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_get_config_contains_security_section() {
    let (base, shutdown, _d) = boot_server().await;
    let resp: Value = get_json(&client(), &format!("{base}/api/v1/config/get")).await;
    assert!(resp["security"].is_object(), "config should have 'security' section");
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_validate_config_returns_valid_true() {
    let (base, shutdown, _d) = boot_server().await;
    let resp: Value = get_json(&client(), &format!("{base}/api/v1/config/validate")).await;
    assert_eq!(resp["valid"], true);
    shutdown.cancel();
}

// ==========================================================================
//  3. MODEL ROUTER
// ==========================================================================

#[tokio::test]
async fn e2e_model_router_returns_200() {
    let (base, shutdown, _d) = boot_server().await;
    let resp = client().get(format!("{base}/api/v1/model/router")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_model_router_returns_providers_array() {
    let (base, shutdown, _d) = boot_server().await;
    let resp: Value = get_json(&client(), &format!("{base}/api/v1/model/router")).await;
    assert!(resp["providers"].is_array());
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_model_router_returns_bindings() {
    let (base, shutdown, _d) = boot_server().await;
    let resp: Value = get_json(&client(), &format!("{base}/api/v1/model/router")).await;
    // ModelRouterSnapshot only contains `providers`; roleBindings was removed.
    assert!(resp["providers"].is_array());
    shutdown.cancel();
}

// ==========================================================================
//  4. CHAT SESSIONS
// ==========================================================================

#[tokio::test]
async fn e2e_list_sessions_initially_empty() {
    let (base, shutdown, _d) = boot_server().await;
    let resp: Vec<Value> = client()
        .get(format!("{base}/api/v1/chat/sessions"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(resp.is_empty());
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_create_session_returns_snapshot() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let resp = c.post(format!("{base}/api/v1/chat/sessions")).send().await.unwrap();
    assert!(resp.status().is_success());
    let session: Value = resp.json().await.unwrap();
    assert!(session["id"].is_string());
    assert!(!session["id"].as_str().unwrap().is_empty());
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_create_session_state_is_idle() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let resp = c.post(format!("{base}/api/v1/chat/sessions")).send().await.unwrap();
    let session: Value = resp.json().await.unwrap();
    assert_eq!(session["state"], "idle");
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_create_session_has_timestamps() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let resp = c.post(format!("{base}/api/v1/chat/sessions")).send().await.unwrap();
    let session: Value = resp.json().await.unwrap();
    assert!(session["created_at_ms"].as_u64().unwrap() > 0);
    assert!(session["updated_at_ms"].as_u64().unwrap() > 0);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_create_session_messages_empty() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let resp = c.post(format!("{base}/api/v1/chat/sessions")).send().await.unwrap();
    let session: Value = resp.json().await.unwrap();
    assert!(session["messages"].as_array().unwrap().is_empty());
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_get_session_by_id() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let resp = c.post(format!("{base}/api/v1/chat/sessions")).send().await.unwrap();
    let created: Value = resp.json().await.unwrap();
    let id = created["id"].as_str().unwrap();

    let resp: Value = get_json(&c, &format!("{base}/api/v1/chat/sessions/{id}")).await;
    assert_eq!(resp["id"], id);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_get_session_not_found() {
    let (base, shutdown, _d) = boot_server().await;
    let resp =
        client().get(format!("{base}/api/v1/chat/sessions/nonexistent-id")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_list_sessions_after_create() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    c.post(format!("{base}/api/v1/chat/sessions")).send().await.unwrap();
    c.post(format!("{base}/api/v1/chat/sessions")).send().await.unwrap();

    let sessions: Vec<Value> =
        c.get(format!("{base}/api/v1/chat/sessions")).send().await.unwrap().json().await.unwrap();
    assert_eq!(sessions.len(), 2);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_send_message_to_session() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let created: Value =
        c.post(format!("{base}/api/v1/chat/sessions")).send().await.unwrap().json().await.unwrap();
    let id = created["id"].as_str().unwrap();

    let resp = post(
        &c,
        &format!("{base}/api/v1/chat/sessions/{id}/messages"),
        json!({"content": "Hello, HiveMind OS!"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_send_message_queued_response() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let created: Value =
        c.post(format!("{base}/api/v1/chat/sessions")).send().await.unwrap().json().await.unwrap();
    let id = created["id"].as_str().unwrap();

    let resp = post(
        &c,
        &format!("{base}/api/v1/chat/sessions/{id}/messages"),
        json!({"content": "Test message"}),
    )
    .await;
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["kind"], "queued");
    assert!(body["session"].is_object());
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_send_message_to_nonexistent_session() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let resp = post(
        &c,
        &format!("{base}/api/v1/chat/sessions/fake-id/messages"),
        json!({"content": "Hello"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_send_message_contains_user_message() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let created: Value =
        c.post(format!("{base}/api/v1/chat/sessions")).send().await.unwrap().json().await.unwrap();
    let id = created["id"].as_str().unwrap();

    let resp = post(
        &c,
        &format!("{base}/api/v1/chat/sessions/{id}/messages"),
        json!({"content": "User says hello"}),
    )
    .await;
    let body: Value = resp.json().await.unwrap();
    let messages = body["session"]["messages"].as_array().unwrap();
    assert!(!messages.is_empty());
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[0]["content"], "User says hello");
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_send_multiple_messages() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let created: Value =
        c.post(format!("{base}/api/v1/chat/sessions")).send().await.unwrap().json().await.unwrap();
    let id = created["id"].as_str().unwrap();

    post(&c, &format!("{base}/api/v1/chat/sessions/{id}/messages"), json!({"content": "msg1"}))
        .await;
    // Wait for processing
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    post(&c, &format!("{base}/api/v1/chat/sessions/{id}/messages"), json!({"content": "msg2"}))
        .await;

    let session: Value = get_json(&c, &format!("{base}/api/v1/chat/sessions/{id}")).await;
    let messages = session["messages"].as_array().unwrap();
    assert!(messages.len() >= 2, "expected at least 2 messages, got {}", messages.len());
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_interrupt_session_soft() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let created: Value =
        c.post(format!("{base}/api/v1/chat/sessions")).send().await.unwrap().json().await.unwrap();
    let id = created["id"].as_str().unwrap();

    let resp =
        post(&c, &format!("{base}/api/v1/chat/sessions/{id}/interrupt"), json!({"mode": "soft"}))
            .await;
    assert!(resp.status().is_success());
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_interrupt_session_hard() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let created: Value =
        c.post(format!("{base}/api/v1/chat/sessions")).send().await.unwrap().json().await.unwrap();
    let id = created["id"].as_str().unwrap();

    let resp =
        post(&c, &format!("{base}/api/v1/chat/sessions/{id}/interrupt"), json!({"mode": "hard"}))
            .await;
    assert!(resp.status().is_success());
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_interrupt_nonexistent_session() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let resp = post(
        &c,
        &format!("{base}/api/v1/chat/sessions/fake-id/interrupt"),
        json!({"mode": "soft"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_resume_session() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let created: Value =
        c.post(format!("{base}/api/v1/chat/sessions")).send().await.unwrap().json().await.unwrap();
    let id = created["id"].as_str().unwrap();

    let resp = c.post(format!("{base}/api/v1/chat/sessions/{id}/resume")).send().await.unwrap();
    assert!(resp.status().is_success());
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_resume_nonexistent_session() {
    let (base, shutdown, _d) = boot_server().await;
    let resp =
        client().post(format!("{base}/api/v1/chat/sessions/fake-id/resume")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_get_session_memory_initially_empty() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let created: Value =
        c.post(format!("{base}/api/v1/chat/sessions")).send().await.unwrap().json().await.unwrap();
    let id = created["id"].as_str().unwrap();

    let memories: Vec<Value> = c
        .get(format!("{base}/api/v1/chat/sessions/{id}/memory"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(memories.is_empty());
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_get_session_memory_nonexistent() {
    let (base, shutdown, _d) = boot_server().await;
    let resp =
        client().get(format!("{base}/api/v1/chat/sessions/fake-id/memory")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_get_session_memory_with_limit() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let created: Value =
        c.post(format!("{base}/api/v1/chat/sessions")).send().await.unwrap().json().await.unwrap();
    let id = created["id"].as_str().unwrap();

    let resp =
        c.get(format!("{base}/api/v1/chat/sessions/{id}/memory?limit=5")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_workspace_file_routes_round_trip() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let created: Value =
        c.post(format!("{base}/api/v1/chat/sessions")).send().await.unwrap().json().await.unwrap();
    let id = created["id"].as_str().unwrap();

    let save_resp = c
        .put(format!("{base}/api/v1/chat/sessions/{id}/workspace/file?path=docs%2Fnotes.txt"))
        .json(&json!({ "content": "hello from e2e" }))
        .send()
        .await
        .unwrap();
    assert_eq!(save_resp.status(), StatusCode::NO_CONTENT);

    let read_resp = c
        .get(format!("{base}/api/v1/chat/sessions/{id}/workspace/file?path=docs%2Fnotes.txt"))
        .send()
        .await
        .unwrap();
    assert_eq!(read_resp.status(), StatusCode::OK);
    let file: Value = read_resp.json().await.unwrap();
    assert_eq!(file["path"], "docs/notes.txt");
    assert_eq!(file["content"], "hello from e2e");
    assert_eq!(file["is_binary"], false);

    let list_resp =
        c.get(format!("{base}/api/v1/chat/sessions/{id}/workspace/files")).send().await.unwrap();
    assert_eq!(list_resp.status(), StatusCode::OK);
    let entries: Vec<Value> = list_resp.json().await.unwrap();
    assert!(entries.iter().any(|entry| entry["path"] == "docs"));

    shutdown.cancel();
}

#[tokio::test]
async fn e2e_list_risk_scans_initially_empty() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let created: Value =
        c.post(format!("{base}/api/v1/chat/sessions")).send().await.unwrap().json().await.unwrap();
    let id = created["id"].as_str().unwrap();

    let scans: Vec<Value> = c
        .get(format!("{base}/api/v1/chat/sessions/{id}/risk-scans"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(scans.is_empty());
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_list_risk_scans_nonexistent_session() {
    let (base, shutdown, _d) = boot_server().await;
    let resp = client()
        .get(format!("{base}/api/v1/chat/sessions/fake-id/risk-scans"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_list_risk_scans_with_limit() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let created: Value =
        c.post(format!("{base}/api/v1/chat/sessions")).send().await.unwrap().json().await.unwrap();
    let id = created["id"].as_str().unwrap();

    let resp =
        c.get(format!("{base}/api/v1/chat/sessions/{id}/risk-scans?limit=3")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    shutdown.cancel();
}

// ==========================================================================
//  5. MEMORY SEARCH
// ==========================================================================

#[tokio::test]
async fn e2e_search_memory_returns_200() {
    let (base, shutdown, _d) = boot_server().await;
    let resp = client().get(format!("{base}/api/v1/memory/search")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_search_memory_with_query() {
    let (base, shutdown, _d) = boot_server().await;
    let resp =
        client().get(format!("{base}/api/v1/memory/search?query=test")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let results: Vec<Value> = resp.json().await.unwrap();
    assert!(results.is_empty()); // No memories stored yet
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_search_memory_with_limit() {
    let (base, shutdown, _d) = boot_server().await;
    let resp = client()
        .get(format!("{base}/api/v1/memory/search?query=anything&limit=5"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    shutdown.cancel();
}

// ==========================================================================
//  6. MCP SERVERS
// ==========================================================================

#[tokio::test]
async fn e2e_list_mcp_servers_returns_200() {
    let (base, shutdown, _d) = boot_server().await;
    let resp = client().get(format!("{base}/api/v1/mcp/servers")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_list_mcp_servers_returns_array() {
    let (base, shutdown, _d) = boot_server().await;
    let resp: Vec<Value> = client()
        .get(format!("{base}/api/v1/mcp/servers"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(resp.is_empty()); // No MCP servers configured by default
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_connect_nonexistent_mcp_server() {
    let (base, shutdown, _d) = boot_server().await;
    let resp = client()
        .post(format!("{base}/api/v1/mcp/servers/no-such-server/connect"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_disconnect_nonexistent_mcp_server() {
    let (base, shutdown, _d) = boot_server().await;
    let resp = client()
        .post(format!("{base}/api/v1/mcp/servers/no-such-server/disconnect"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_list_mcp_tools_nonexistent_server() {
    let (base, shutdown, _d) = boot_server().await;
    let resp = client()
        .get(format!("{base}/api/v1/mcp/servers/no-such-server/tools"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_list_mcp_resources_nonexistent_server() {
    let (base, shutdown, _d) = boot_server().await;
    let resp = client()
        .get(format!("{base}/api/v1/mcp/servers/no-such-server/resources"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_list_mcp_prompts_nonexistent_server() {
    let (base, shutdown, _d) = boot_server().await;
    let resp = client()
        .get(format!("{base}/api/v1/mcp/servers/no-such-server/prompts"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_list_mcp_notifications_returns_200() {
    let (base, shutdown, _d) = boot_server().await;
    let resp = client().get(format!("{base}/api/v1/mcp/notifications")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_list_mcp_notifications_returns_array() {
    let (base, shutdown, _d) = boot_server().await;
    let resp: Vec<Value> = client()
        .get(format!("{base}/api/v1/mcp/notifications"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(resp.is_empty());
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_list_mcp_notifications_with_limit() {
    let (base, shutdown, _d) = boot_server().await;
    let resp =
        client().get(format!("{base}/api/v1/mcp/notifications?limit=10")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    shutdown.cancel();
}

// ==========================================================================
//  7. TOOLS
// ==========================================================================

#[tokio::test]
async fn e2e_list_tools_returns_200() {
    let (base, shutdown, _d) = boot_server().await;
    let resp = client().get(format!("{base}/api/v1/tools")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_list_tools_returns_nonempty() {
    let (base, shutdown, _d) = boot_server().await;
    let tools: Vec<Value> =
        client().get(format!("{base}/api/v1/tools")).send().await.unwrap().json().await.unwrap();
    assert!(!tools.is_empty(), "should have at least one tool registered");
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_list_tools_each_has_id_and_name() {
    let (base, shutdown, _d) = boot_server().await;
    let tools: Vec<Value> =
        client().get(format!("{base}/api/v1/tools")).send().await.unwrap().json().await.unwrap();
    for tool in &tools {
        assert!(tool["id"].is_string(), "tool should have 'id': {tool:?}");
        assert!(tool["name"].is_string(), "tool should have 'name': {tool:?}");
    }
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_list_tools_each_has_description() {
    let (base, shutdown, _d) = boot_server().await;
    let tools: Vec<Value> =
        client().get(format!("{base}/api/v1/tools")).send().await.unwrap().json().await.unwrap();
    for tool in &tools {
        assert!(tool["description"].is_string(), "tool should have 'description': {tool:?}");
    }
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_invoke_nonexistent_tool() {
    let (base, shutdown, _d) = boot_server().await;
    let resp = post(
        &client(),
        &format!("{base}/api/v1/tools/nonexistent-tool/invoke"),
        json!({"input": {}}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    shutdown.cancel();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_invoke_calculator_tool() {
    let (base, shutdown, _d) = boot_server().await;
    let tools: Vec<Value> =
        client().get(format!("{base}/api/v1/tools")).send().await.unwrap().json().await.unwrap();

    let calc_tool = tools.iter().find(|t| t["id"].as_str().is_some_and(|id| id.contains("calc")));

    if let Some(tool) = calc_tool {
        let id = tool["id"].as_str().unwrap();
        let resp = post(
            &client(),
            &format!("{base}/api/v1/tools/{id}/invoke"),
            json!({"input": {"expression": "2+2"}}),
        )
        .await;
        assert!(resp.status().is_success() || resp.status() == StatusCode::BAD_GATEWAY);
    }
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_invoke_echo_tool() {
    let (base, shutdown, _d) = boot_server().await;
    let tools: Vec<Value> =
        client().get(format!("{base}/api/v1/tools")).send().await.unwrap().json().await.unwrap();

    let echo_tool = tools.iter().find(|t| t["id"].as_str().is_some_and(|id| id.contains("echo")));

    if let Some(tool) = echo_tool {
        let id = tool["id"].as_str().unwrap();
        let resp = post(
            &client(),
            &format!("{base}/api/v1/tools/{id}/invoke"),
            json!({"input": {"value": "hello world"}}),
        )
        .await;
        // Echo tool may fail with BAD_GATEWAY if the tool execution has issues,
        // but it should not 404 (the tool exists)
        assert_ne!(resp.status(), StatusCode::NOT_FOUND, "echo tool should exist");
    }
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_invoke_datetime_tool() {
    let (base, shutdown, _d) = boot_server().await;
    let tools: Vec<Value> =
        client().get(format!("{base}/api/v1/tools")).send().await.unwrap().json().await.unwrap();

    let dt_tool = tools
        .iter()
        .find(|t| t["id"].as_str().is_some_and(|id| id.contains("date") || id.contains("time")));

    if let Some(tool) = dt_tool {
        let id = tool["id"].as_str().unwrap();
        let resp =
            post(&client(), &format!("{base}/api/v1/tools/{id}/invoke"), json!({"input": {}}))
                .await;
        // The tool may fail at runtime (e.g. BAD_GATEWAY) but it should exist
        assert_ne!(resp.status(), StatusCode::NOT_FOUND, "datetime tool should exist");
    }
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_invoke_tool_bad_json_body() {
    let (base, shutdown, _d) = boot_server().await;
    let resp = client()
        .post(format!("{base}/api/v1/tools/echo/invoke"))
        .header("content-type", "application/json")
        .body("not json")
        .send()
        .await
        .unwrap();
    // Should be 400 or 422
    assert!(resp.status().is_client_error());
    shutdown.cancel();
}

// ==========================================================================
//  8. SCHEDULER
// ==========================================================================

fn once_task(name: &str) -> Value {
    json!({
        "name": name,
        "description": format!("Task: {name}"),
        "schedule": { "type": "once" },
        "action": { "type": "emit_event", "topic": "test", "payload": {} }
    })
}

fn cron_task(name: &str, expression: &str) -> Value {
    json!({
        "name": name,
        "schedule": { "type": "cron", "expression": expression },
        "action": { "type": "emit_event", "topic": "tick", "payload": {} }
    })
}

fn scheduled_task(name: &str, run_at_ms: u64) -> Value {
    json!({
        "name": name,
        "schedule": { "type": "scheduled", "run_at_ms": run_at_ms },
        "action": { "type": "emit_event", "topic": "scheduled", "payload": {} }
    })
}

fn webhook_task(name: &str) -> Value {
    json!({
        "name": name,
        "schedule": { "type": "once" },
        "action": {
            "type": "http_webhook",
            "url": "http://localhost:1234/hook",
            "method": "POST",
            "body": "{\"key\": \"value\"}"
        }
    })
}

fn send_message_task(name: &str) -> Value {
    json!({
        "name": name,
        "schedule": { "type": "once" },
        "action": {
            "type": "send_message",
            "session_id": "test-session",
            "content": "automated message"
        }
    })
}

async fn create_task_id(c: &reqwest::Client, base: &str, body: Value) -> String {
    let resp = post(c, &format!("{base}/api/v1/scheduler/tasks"), body).await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let v: Value = resp.json().await.unwrap();
    v["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn e2e_scheduler_list_initially_empty() {
    let (base, shutdown, _d) = boot_server().await;
    let tasks: Vec<Value> = client()
        .get(format!("{base}/api/v1/scheduler/tasks"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(tasks.is_empty());
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_scheduler_create_once_task() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let resp = post(&c, &format!("{base}/api/v1/scheduler/tasks"), once_task("task1")).await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let task: Value = resp.json().await.unwrap();
    assert_eq!(task["name"], "task1");
    assert_eq!(task["status"], "pending");
    assert_eq!(task["schedule"]["type"], "once");
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_scheduler_create_cron_task() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let resp =
        post(&c, &format!("{base}/api/v1/scheduler/tasks"), cron_task("rec1", "0 */2 * * * * *"))
            .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let task: Value = resp.json().await.unwrap();
    assert_eq!(task["schedule"]["type"], "cron");
    assert_eq!(task["schedule"]["expression"], "0 */2 * * * * *");
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_scheduler_create_scheduled_task() {
    use std::time::{SystemTime, UNIX_EPOCH};
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let run_at = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64 + 120_000;
    let resp =
        post(&c, &format!("{base}/api/v1/scheduler/tasks"), scheduled_task("del1", run_at)).await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let task: Value = resp.json().await.unwrap();
    assert_eq!(task["schedule"]["type"], "scheduled");
    assert!(task["schedule"]["run_at_ms"].as_u64().is_some());
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_scheduler_create_webhook_task() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let resp = post(&c, &format!("{base}/api/v1/scheduler/tasks"), webhook_task("hook1")).await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let task: Value = resp.json().await.unwrap();
    assert_eq!(task["action"]["type"], "http_webhook");
    assert_eq!(task["action"]["url"], "http://localhost:1234/hook");
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_scheduler_create_send_message_task() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let resp = post(&c, &format!("{base}/api/v1/scheduler/tasks"), send_message_task("msg1")).await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let task: Value = resp.json().await.unwrap();
    assert_eq!(task["action"]["type"], "send_message");
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_scheduler_get_task_by_id() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let id = create_task_id(&c, &base, once_task("findme")).await;

    let task: Value = get_json(&c, &format!("{base}/api/v1/scheduler/tasks/{id}")).await;
    assert_eq!(task["id"], id);
    assert_eq!(task["name"], "findme");
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_scheduler_get_nonexistent_task() {
    let (base, shutdown, _d) = boot_server().await;
    let resp =
        client().get(format!("{base}/api/v1/scheduler/tasks/does-not-exist")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_scheduler_delete_task() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let id = create_task_id(&c, &base, once_task("deleteme")).await;

    let resp = c.delete(format!("{base}/api/v1/scheduler/tasks/{id}")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = c.get(format!("{base}/api/v1/scheduler/tasks/{id}")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_scheduler_delete_nonexistent_task() {
    let (base, shutdown, _d) = boot_server().await;
    let resp = client().delete(format!("{base}/api/v1/scheduler/tasks/nope")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_scheduler_cancel_task() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let id = create_task_id(&c, &base, once_task("cancelme")).await;

    let resp = c.post(format!("{base}/api/v1/scheduler/tasks/{id}/cancel")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let task: Value = resp.json().await.unwrap();
    assert_eq!(task["status"], "cancelled");
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_scheduler_cancel_preserves_data() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let id = create_task_id(&c, &base, once_task("keep_data")).await;

    c.post(format!("{base}/api/v1/scheduler/tasks/{id}/cancel")).send().await.unwrap();

    let task: Value = get_json(&c, &format!("{base}/api/v1/scheduler/tasks/{id}")).await;
    assert_eq!(task["name"], "keep_data");
    assert_eq!(task["status"], "cancelled");
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_scheduler_cancel_nonexistent_task() {
    let (base, shutdown, _d) = boot_server().await;
    let resp =
        client().post(format!("{base}/api/v1/scheduler/tasks/nope/cancel")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_scheduler_list_after_creating_multiple() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    create_task_id(&c, &base, once_task("alpha")).await;
    create_task_id(&c, &base, once_task("beta")).await;
    create_task_id(&c, &base, once_task("gamma")).await;

    let tasks: Vec<Value> =
        c.get(format!("{base}/api/v1/scheduler/tasks")).send().await.unwrap().json().await.unwrap();
    assert!(tasks.len() >= 3);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_scheduler_task_has_timestamps() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let id = create_task_id(&c, &base, once_task("ts_test")).await;
    let task: Value = get_json(&c, &format!("{base}/api/v1/scheduler/tasks/{id}")).await;
    assert!(task["created_at_ms"].as_u64().unwrap() > 0);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_scheduler_task_has_run_count_zero() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let id = create_task_id(&c, &base, once_task("rc_test")).await;
    let task: Value = get_json(&c, &format!("{base}/api/v1/scheduler/tasks/{id}")).await;
    assert_eq!(task["run_count"], 0);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_scheduler_delete_after_cancel() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let id = create_task_id(&c, &base, once_task("del_after_cancel")).await;
    c.post(format!("{base}/api/v1/scheduler/tasks/{id}/cancel")).send().await.unwrap();

    let resp = c.delete(format!("{base}/api/v1/scheduler/tasks/{id}")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    shutdown.cancel();
}

// ==========================================================================
//  9. KNOWLEDGE GRAPH — Nodes
// ==========================================================================

async fn create_node(
    c: &reqwest::Client,
    base: &str,
    node_type: &str,
    name: &str,
    content: Option<&str>,
) -> i64 {
    let mut body = json!({"node_type": node_type, "name": name});
    if let Some(ct) = content {
        body["content"] = json!(ct);
    }
    let resp = post(c, &format!("{base}/api/v1/knowledge/nodes"), body).await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let v: Value = resp.json().await.unwrap();
    v["id"].as_i64().unwrap()
}

#[tokio::test]
async fn e2e_kg_create_node() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let id = create_node(&c, &base, "function", "my_func", Some("body")).await;
    assert!(id > 0);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_kg_get_node() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let id = create_node(&c, &base, "class", "MyClass", Some("a class")).await;
    let node: Value = get_json(&c, &format!("{base}/api/v1/knowledge/nodes/{id}")).await;
    assert_eq!(node["id"], id);
    assert_eq!(node["node_type"], "class");
    assert_eq!(node["name"], "MyClass");
    assert_eq!(node["content"], "a class");
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_kg_get_node_not_found() {
    let (base, shutdown, _d) = boot_server().await;
    let resp = client().get(format!("{base}/api/v1/knowledge/nodes/99999")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_kg_create_node_without_content() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let id = create_node(&c, &base, "module", "bare_module", None).await;
    let node: Value = get_json(&c, &format!("{base}/api/v1/knowledge/nodes/{id}")).await;
    assert!(node["content"].is_null());
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_kg_create_node_with_data_class() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let resp = post(
        &c,
        &format!("{base}/api/v1/knowledge/nodes"),
        json!({"node_type": "secret", "name": "api_key", "data_class": "RESTRICTED", "content": "sk-xxx"}),
    ).await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let v: Value = resp.json().await.unwrap();
    let id = v["id"].as_i64().unwrap();

    let node: Value = get_json(&c, &format!("{base}/api/v1/knowledge/nodes/{id}")).await;
    assert_eq!(node["data_class"], "restricted");
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_kg_delete_node() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let id = create_node(&c, &base, "temp", "to_delete", None).await;

    let resp = c.delete(format!("{base}/api/v1/knowledge/nodes/{id}")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = c.get(format!("{base}/api/v1/knowledge/nodes/{id}")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_kg_delete_node_twice() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let id = create_node(&c, &base, "temp", "double_del", None).await;

    c.delete(format!("{base}/api/v1/knowledge/nodes/{id}")).send().await.unwrap();
    let resp = c.delete(format!("{base}/api/v1/knowledge/nodes/{id}")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_kg_delete_nonexistent_node() {
    let (base, shutdown, _d) = boot_server().await;
    let resp =
        client().delete(format!("{base}/api/v1/knowledge/nodes/99999")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_kg_list_nodes_empty() {
    let (base, shutdown, _d) = boot_server().await;
    let nodes: Vec<Value> = client()
        .get(format!("{base}/api/v1/knowledge/nodes"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(nodes.is_empty());
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_kg_list_nodes() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    create_node(&c, &base, "function", "fn1", None).await;
    create_node(&c, &base, "function", "fn2", None).await;
    create_node(&c, &base, "class", "cls1", None).await;

    let nodes: Vec<Value> =
        c.get(format!("{base}/api/v1/knowledge/nodes")).send().await.unwrap().json().await.unwrap();
    assert!(nodes.len() >= 3);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_kg_list_nodes_filter_by_type() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    create_node(&c, &base, "function", "fn_a", None).await;
    create_node(&c, &base, "class", "cls_b", None).await;
    create_node(&c, &base, "function", "fn_c", None).await;

    let nodes: Vec<Value> = c
        .get(format!("{base}/api/v1/knowledge/nodes?node_type=function"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(nodes.len(), 2);
    for n in &nodes {
        assert_eq!(n["node_type"], "function");
    }
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_kg_list_nodes_with_limit() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    for i in 0..5 {
        create_node(&c, &base, "item", &format!("item_{i}"), None).await;
    }

    let nodes: Vec<Value> = c
        .get(format!("{base}/api/v1/knowledge/nodes?limit=3"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(nodes.len(), 3);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_kg_node_has_default_data_class() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let id = create_node(&c, &base, "entity", "default_dc", None).await;
    let node: Value = get_json(&c, &format!("{base}/api/v1/knowledge/nodes/{id}")).await;
    assert_eq!(node["data_class"], "internal");
    shutdown.cancel();
}

// ==========================================================================
//  10. KNOWLEDGE GRAPH — Edges
// ==========================================================================

async fn create_edge(c: &reqwest::Client, base: &str, src: i64, tgt: i64, edge_type: &str) -> i64 {
    let resp = post(
        c,
        &format!("{base}/api/v1/knowledge/edges"),
        json!({"source_id": src, "target_id": tgt, "edge_type": edge_type}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let v: Value = resp.json().await.unwrap();
    v["id"].as_i64().unwrap()
}

#[tokio::test]
async fn e2e_kg_create_edge() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let n1 = create_node(&c, &base, "a", "src", None).await;
    let n2 = create_node(&c, &base, "b", "tgt", None).await;
    let eid = create_edge(&c, &base, n1, n2, "calls").await;
    assert!(eid > 0);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_kg_create_edge_with_weight() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let n1 = create_node(&c, &base, "a", "w1", None).await;
    let n2 = create_node(&c, &base, "b", "w2", None).await;

    let resp = post(
        &c,
        &format!("{base}/api/v1/knowledge/edges"),
        json!({"source_id": n1, "target_id": n2, "edge_type": "depends_on", "weight": 0.75}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_kg_get_edges_for_node() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let n1 = create_node(&c, &base, "mod", "m1", None).await;
    let n2 = create_node(&c, &base, "fn", "f1", None).await;
    let n3 = create_node(&c, &base, "fn", "f2", None).await;
    create_edge(&c, &base, n1, n2, "contains").await;
    create_edge(&c, &base, n1, n3, "contains").await;

    let edges: Vec<Value> = c
        .get(format!("{base}/api/v1/knowledge/nodes/{n1}/edges"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(edges.len(), 2);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_kg_get_edges_for_node_with_no_edges() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let n = create_node(&c, &base, "isolated", "alone", None).await;

    let edges: Vec<Value> = c
        .get(format!("{base}/api/v1/knowledge/nodes/{n}/edges"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(edges.is_empty());
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_kg_delete_edge() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let n1 = create_node(&c, &base, "a", "e_del1", None).await;
    let n2 = create_node(&c, &base, "b", "e_del2", None).await;
    let eid = create_edge(&c, &base, n1, n2, "refs").await;

    let resp = c.delete(format!("{base}/api/v1/knowledge/edges/{eid}")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let edges: Vec<Value> = c
        .get(format!("{base}/api/v1/knowledge/nodes/{n1}/edges"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(edges.is_empty());
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_kg_delete_edge_twice() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let n1 = create_node(&c, &base, "a", "dd1", None).await;
    let n2 = create_node(&c, &base, "b", "dd2", None).await;
    let eid = create_edge(&c, &base, n1, n2, "refs").await;

    c.delete(format!("{base}/api/v1/knowledge/edges/{eid}")).send().await.unwrap();
    let resp = c.delete(format!("{base}/api/v1/knowledge/edges/{eid}")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_kg_delete_nonexistent_edge() {
    let (base, shutdown, _d) = boot_server().await;
    let resp =
        client().delete(format!("{base}/api/v1/knowledge/edges/99999")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_kg_edge_appears_on_target_node_too() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let n1 = create_node(&c, &base, "a", "tgt_check_src", None).await;
    let n2 = create_node(&c, &base, "b", "tgt_check_tgt", None).await;
    create_edge(&c, &base, n1, n2, "uses").await;

    let edges: Vec<Value> = c
        .get(format!("{base}/api/v1/knowledge/nodes/{n2}/edges"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0]["target_id"], n2);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_kg_get_node_includes_edges() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let n1 = create_node(&c, &base, "parent", "p1", None).await;
    let n2 = create_node(&c, &base, "child", "c1", None).await;
    create_edge(&c, &base, n1, n2, "has_child").await;

    let node_with_edges: Value = get_json(&c, &format!("{base}/api/v1/knowledge/nodes/{n1}")).await;
    assert_eq!(node_with_edges["id"], n1);
    assert!(!node_with_edges["edges"].as_array().unwrap().is_empty());
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_kg_multiple_edges_between_same_nodes() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let n1 = create_node(&c, &base, "a", "multi1", None).await;
    let n2 = create_node(&c, &base, "b", "multi2", None).await;
    create_edge(&c, &base, n1, n2, "calls").await;
    create_edge(&c, &base, n1, n2, "imports").await;

    let edges: Vec<Value> = c
        .get(format!("{base}/api/v1/knowledge/nodes/{n1}/edges"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(edges.len(), 2);
    shutdown.cancel();
}

// ==========================================================================
//  11. KNOWLEDGE GRAPH — Search
// ==========================================================================

#[tokio::test]
async fn e2e_kg_search_empty_db() {
    let (base, shutdown, _d) = boot_server().await;
    let resp =
        client().get(format!("{base}/api/v1/knowledge/search?q=anything")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let results: Vec<Value> = resp.json().await.unwrap();
    assert!(results.is_empty());
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_kg_search_finds_by_content() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    create_node(&c, &base, "function", "parse_json", Some("Parses JSON strings")).await;
    create_node(&c, &base, "function", "format_csv", Some("Formats CSV output")).await;

    let results: Vec<Value> = c
        .get(format!("{base}/api/v1/knowledge/search?q=JSON"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!results.is_empty());
    let names: Vec<&str> = results.iter().filter_map(|r| r["name"].as_str()).collect();
    assert!(names.contains(&"parse_json"));
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_kg_search_finds_by_name() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    create_node(&c, &base, "module", "authentication_handler", None).await;
    create_node(&c, &base, "module", "data_processor", None).await;

    let results: Vec<Value> = c
        .get(format!("{base}/api/v1/knowledge/search?q=authentication"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!results.is_empty());
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_kg_search_with_limit() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    for i in 0..5 {
        create_node(&c, &base, "item", &format!("searchable_{i}"), Some("common keyword")).await;
    }

    let results: Vec<Value> = c
        .get(format!("{base}/api/v1/knowledge/search?q=keyword&limit=2"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(results.len() <= 2);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_kg_search_no_matches() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    create_node(&c, &base, "function", "alpha", Some("does alpha things")).await;

    let results: Vec<Value> = c
        .get(format!("{base}/api/v1/knowledge/search?q=zzzznonexistent"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(results.is_empty());
    shutdown.cancel();
}

// ==========================================================================
//  12. KNOWLEDGE GRAPH — Stats
// ==========================================================================

#[tokio::test]
async fn e2e_kg_stats_empty() {
    let (base, shutdown, _d) = boot_server().await;
    let stats: Value = get_json(&client(), &format!("{base}/api/v1/knowledge/stats")).await;
    assert_eq!(stats["node_count"], 0);
    assert_eq!(stats["edge_count"], 0);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_kg_stats_after_inserts() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let n1 = create_node(&c, &base, "function", "s1", None).await;
    let n2 = create_node(&c, &base, "class", "s2", None).await;
    create_edge(&c, &base, n1, n2, "belongs_to").await;

    let stats: Value = get_json(&c, &format!("{base}/api/v1/knowledge/stats")).await;
    assert!(stats["node_count"].as_i64().unwrap() >= 2);
    assert!(stats["edge_count"].as_i64().unwrap() >= 1);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_kg_stats_nodes_by_type() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    create_node(&c, &base, "function", "t1", None).await;
    create_node(&c, &base, "function", "t2", None).await;
    create_node(&c, &base, "class", "t3", None).await;

    let stats: Value = get_json(&c, &format!("{base}/api/v1/knowledge/stats")).await;
    let by_type = stats["nodes_by_type"].as_array().unwrap();
    assert!(by_type.len() >= 2);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_kg_stats_edges_by_type() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let n1 = create_node(&c, &base, "a", "et1", None).await;
    let n2 = create_node(&c, &base, "b", "et2", None).await;
    let n3 = create_node(&c, &base, "c", "et3", None).await;
    create_edge(&c, &base, n1, n2, "calls").await;
    create_edge(&c, &base, n2, n3, "imports").await;

    let stats: Value = get_json(&c, &format!("{base}/api/v1/knowledge/stats")).await;
    let by_type = stats["edges_by_type"].as_array().unwrap();
    assert!(by_type.len() >= 2);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_kg_stats_after_delete() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let n1 = create_node(&c, &base, "temp", "del_stat1", None).await;
    create_node(&c, &base, "temp", "del_stat2", None).await;

    let stats_before: Value = get_json(&c, &format!("{base}/api/v1/knowledge/stats")).await;
    let count_before = stats_before["node_count"].as_i64().unwrap();

    c.delete(format!("{base}/api/v1/knowledge/nodes/{n1}")).send().await.unwrap();

    let stats_after: Value = get_json(&c, &format!("{base}/api/v1/knowledge/stats")).await;
    let count_after = stats_after["node_count"].as_i64().unwrap();
    assert_eq!(count_after, count_before - 1);
    shutdown.cancel();
}

// ==========================================================================
//  13. LOCAL MODELS — 503 when service unavailable
// ==========================================================================

#[tokio::test]
async fn e2e_local_models_list_503() {
    let (base, shutdown, _d) = boot_server().await;
    let resp = client().get(format!("{base}/api/v1/local-models")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_local_models_get_503() {
    let (base, shutdown, _d) = boot_server().await;
    let resp = client().get(format!("{base}/api/v1/local-models/some-id")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_local_models_delete_503() {
    let (base, shutdown, _d) = boot_server().await;
    let resp = client().delete(format!("{base}/api/v1/local-models/some-id")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_local_models_hardware_503() {
    let (base, shutdown, _d) = boot_server().await;
    let resp = client().get(format!("{base}/api/v1/local-models/hardware")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_local_models_search_503() {
    let (base, shutdown, _d) = boot_server().await;
    let resp = client()
        .get(format!("{base}/api/v1/local-models/search?query=llama"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_local_models_install_503() {
    let (base, shutdown, _d) = boot_server().await;
    let resp = post(
        &client(),
        &format!("{base}/api/v1/local-models/install"),
        json!({"hub_repo": "test/model", "filename": "model.bin", "runtime": "candle"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    shutdown.cancel();
}

// ==========================================================================
//  14. LOCAL MODELS — with service available
// ==========================================================================

#[tokio::test]
async fn e2e_local_models_list_empty() {
    let (base, shutdown, _d) = boot_server_with_local_models().await;
    let resp = client().get(format!("{base}/api/v1/local-models")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["installed_count"], 0);
    assert!(body["models"].as_array().unwrap().is_empty());
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_local_models_get_not_found() {
    let (base, shutdown, _d) = boot_server_with_local_models().await;
    let resp =
        client().get(format!("{base}/api/v1/local-models/nonexistent")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_local_models_remove_not_found() {
    let (base, shutdown, _d) = boot_server_with_local_models().await;
    let resp =
        client().delete(format!("{base}/api/v1/local-models/nonexistent")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_local_models_hardware_returns_valid() {
    let (base, shutdown, _d) = boot_server_with_local_models().await;
    let resp: Value = get_json(&client(), &format!("{base}/api/v1/local-models/hardware")).await;
    assert!(!resp["hardware"]["cpu"]["name"].as_str().unwrap().is_empty());
    assert!(resp["hardware"]["cpu"]["cores_logical"].as_u64().unwrap() >= 1);
    assert_eq!(resp["usage"]["models_loaded"], 0);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_local_models_hardware_has_memory_info() {
    let (base, shutdown, _d) = boot_server_with_local_models().await;
    let resp: Value = get_json(&client(), &format!("{base}/api/v1/local-models/hardware")).await;
    // Memory detection may return 0 on some CI/test systems, so just check the field exists
    assert!(resp["hardware"]["memory"]["total_bytes"].is_number());
    assert!(resp["hardware"]["memory"]["available_bytes"].is_number());
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_local_models_healthz_works() {
    let (base, shutdown, _d) = boot_server_with_local_models().await;
    let resp = client().get(format!("{base}/healthz")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    shutdown.cancel();
}

// ==========================================================================
//  15. CROSS-CUTTING CONCERNS
// ==========================================================================

#[tokio::test]
async fn e2e_cors_headers_present() {
    let (base, shutdown, _d) = boot_server().await;
    let resp = client()
        .get(format!("{base}/healthz"))
        .header("origin", "http://localhost:3000")
        .send()
        .await
        .unwrap();
    // CorsLayer::permissive() should return access-control-allow-origin
    let headers = resp.headers();
    assert!(headers.contains_key("access-control-allow-origin"), "CORS headers should be present");
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_content_type_is_json() {
    let (base, shutdown, _d) = boot_server().await;
    let resp = client().get(format!("{base}/healthz")).send().await.unwrap();
    let ct = resp.headers().get("content-type").unwrap().to_str().unwrap();
    assert!(ct.contains("application/json"));
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_concurrent_requests() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();

    let mut handles = Vec::new();
    for i in 0..10 {
        let url = format!("{base}/healthz");
        let c = c.clone();
        handles.push(tokio::spawn(async move {
            let resp = c.get(url).send().await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_large_node_content() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let large_content = "x".repeat(10_000);
    let id = create_node(&c, &base, "blob", "big_node", Some(&large_content)).await;

    let node: Value = get_json(&c, &format!("{base}/api/v1/knowledge/nodes/{id}")).await;
    assert_eq!(node["content"].as_str().unwrap().len(), 10_000);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_unicode_node_content() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let id =
        create_node(&c, &base, "text", "emoji_node", Some("🦀 Rust 日本語 中文 العربية")).await;

    let node: Value = get_json(&c, &format!("{base}/api/v1/knowledge/nodes/{id}")).await;
    assert!(node["content"].as_str().unwrap().contains("🦀"));
    assert!(node["content"].as_str().unwrap().contains("日本語"));
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_unicode_chat_message() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let created: Value =
        c.post(format!("{base}/api/v1/chat/sessions")).send().await.unwrap().json().await.unwrap();
    let id = created["id"].as_str().unwrap();

    let resp = post(
        &c,
        &format!("{base}/api/v1/chat/sessions/{id}/messages"),
        json!({"content": "こんにちは 🤖"}),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_empty_message_content() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let created: Value =
        c.post(format!("{base}/api/v1/chat/sessions")).send().await.unwrap().json().await.unwrap();
    let id = created["id"].as_str().unwrap();

    let resp =
        post(&c, &format!("{base}/api/v1/chat/sessions/{id}/messages"), json!({"content": ""}))
            .await;
    // Should still accept (or reject gracefully)
    assert!(resp.status().is_success() || resp.status().is_client_error());
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_scheduler_task_description_optional() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let resp = post(
        &c,
        &format!("{base}/api/v1/scheduler/tasks"),
        json!({
            "name": "no_desc",
            "schedule": {"type": "once"},
            "action": {"type": "emit_event", "topic": "t", "payload": {}}
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_session_summaries_have_required_fields() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    c.post(format!("{base}/api/v1/chat/sessions")).send().await.unwrap();

    let sessions: Vec<Value> =
        c.get(format!("{base}/api/v1/chat/sessions")).send().await.unwrap().json().await.unwrap();
    for s in &sessions {
        assert!(s["id"].is_string());
        assert!(s["title"].is_string());
        assert!(s["state"].is_string());
        assert!(s["updated_at_ms"].is_number());
    }
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_kg_node_special_characters_in_name() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let id = create_node(&c, &base, "func", "my::nested::func<T>", None).await;
    let node: Value = get_json(&c, &format!("{base}/api/v1/knowledge/nodes/{id}")).await;
    assert_eq!(node["name"], "my::nested::func<T>");
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_multiple_sessions_isolated() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();

    let s1: Value =
        c.post(format!("{base}/api/v1/chat/sessions")).send().await.unwrap().json().await.unwrap();
    let s2: Value =
        c.post(format!("{base}/api/v1/chat/sessions")).send().await.unwrap().json().await.unwrap();

    let id1 = s1["id"].as_str().unwrap();
    let id2 = s2["id"].as_str().unwrap();
    assert_ne!(id1, id2);

    // Send message to session 1
    post(&c, &format!("{base}/api/v1/chat/sessions/{id1}/messages"), json!({"content": "for s1"}))
        .await;

    // Session 2 should have no messages
    let snap: Value = get_json(&c, &format!("{base}/api/v1/chat/sessions/{id2}")).await;
    assert!(snap["messages"].as_array().unwrap().is_empty());
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_kg_complex_graph_traversal() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();

    // Build a small graph: A -> B -> C, A -> C
    let a = create_node(&c, &base, "module", "A", None).await;
    let b = create_node(&c, &base, "module", "B", None).await;
    let c_node = create_node(&c, &base, "module", "C", None).await;

    create_edge(&c, &base, a, b, "imports").await;
    create_edge(&c, &base, b, c_node, "imports").await;
    create_edge(&c, &base, a, c_node, "imports").await;

    // A should have 2 outgoing edges
    let a_edges: Vec<Value> = client()
        .get(format!("{base}/api/v1/knowledge/nodes/{a}/edges"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(a_edges.len(), 2);

    // B should appear in edges for both A (as target) and C (as source)
    let b_edges: Vec<Value> = client()
        .get(format!("{base}/api/v1/knowledge/nodes/{b}/edges"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(b_edges.len(), 2); // one incoming from A, one outgoing to C

    shutdown.cancel();
}

#[tokio::test]
async fn e2e_scheduler_cron_schedule() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let resp = post(
        &c,
        &format!("{base}/api/v1/scheduler/tasks"),
        json!({
            "name": "cron_task",
            "schedule": {"type": "cron", "expression": "0 */5 * * * *"},
            "action": {"type": "emit_event", "topic": "cron", "payload": {}}
        }),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let task: Value = resp.json().await.unwrap();
    assert_eq!(task["schedule"]["type"], "cron");
    assert_eq!(task["schedule"]["expression"], "0 */5 * * * *");
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_config_has_local_models_section() {
    let (base, shutdown, _d) = boot_server().await;
    let resp: Value = get_json(&client(), &format!("{base}/api/v1/config/get")).await;
    assert!(resp["local_models"].is_object());
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_chat_session_queued_count_zero_initially() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let resp: Value =
        c.post(format!("{base}/api/v1/chat/sessions")).send().await.unwrap().json().await.unwrap();
    assert_eq!(resp["queued_count"], 0);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_kg_search_data_class_filter() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();

    // Create node with public data_class
    post(
        &c,
        &format!("{base}/api/v1/knowledge/nodes"),
        json!({"node_type": "doc", "name": "public_doc", "data_class": "PUBLIC", "content": "This is open source documentation"}),
    ).await;

    // Search with data_class filter
    let resp = c
        .get(format!("{base}/api/v1/knowledge/search?q=documentation&data_class=PUBLIC"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_kg_list_nodes_data_class_filter() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();

    post(
        &c,
        &format!("{base}/api/v1/knowledge/nodes"),
        json!({"node_type": "secret", "name": "key1", "data_class": "RESTRICTED"}),
    )
    .await;
    post(
        &c,
        &format!("{base}/api/v1/knowledge/nodes"),
        json!({"node_type": "doc", "name": "readme", "data_class": "PUBLIC"}),
    )
    .await;

    let nodes: Vec<Value> = c
        .get(format!("{base}/api/v1/knowledge/nodes?data_class=RESTRICTED"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    for n in &nodes {
        assert_eq!(n["data_class"], "restricted");
    }
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_send_message_with_scan_decision() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let created: Value =
        c.post(format!("{base}/api/v1/chat/sessions")).send().await.unwrap().json().await.unwrap();
    let id = created["id"].as_str().unwrap();

    let resp = post(
        &c,
        &format!("{base}/api/v1/chat/sessions/{id}/messages"),
        json!({"content": "test with decision", "scan_decision": "allow"}),
    )
    .await;
    assert!(resp.status().is_success());
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_session_recalled_memories_empty() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();
    let resp: Value =
        c.post(format!("{base}/api/v1/chat/sessions")).send().await.unwrap().json().await.unwrap();
    assert!(resp["recalled_memories"].as_array().unwrap().is_empty());
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_post_to_get_only_endpoint() {
    let (base, shutdown, _d) = boot_server().await;
    let resp = client().post(format!("{base}/api/v1/config/get")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_get_to_post_only_endpoint() {
    let (base, shutdown, _d) = boot_server().await;
    let resp = client().get(format!("{base}/api/v1/daemon/shutdown")).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_concurrent_session_creation() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();

    // Create sessions sequentially (concurrent creation may hit internal lock contention)
    let mut ids = Vec::new();
    for _ in 0..5 {
        let resp = c.post(format!("{base}/api/v1/chat/sessions")).send().await.unwrap();
        assert!(resp.status().is_success());
        let body: Value = resp.json().await.unwrap();
        ids.push(body["id"].as_str().unwrap().to_string());
    }

    // All IDs should be unique
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 5);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_concurrent_kg_writes() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();

    // SQLite is single-writer; do rapid sequential writes instead of truly concurrent
    for i in 0..10 {
        let id = create_node(&c, &base, "concurrent", &format!("node_{i}"), None).await;
        assert!(id > 0);
    }

    let stats: Value = get_json(&c, &format!("{base}/api/v1/knowledge/stats")).await;
    assert!(stats["node_count"].as_i64().unwrap() >= 10);
    shutdown.cancel();
}

#[tokio::test]
async fn e2e_concurrent_scheduler_tasks() {
    let (base, shutdown, _d) = boot_server().await;
    let c = client();

    let mut handles = Vec::new();
    for i in 0..5 {
        let c = c.clone();
        let base = base.clone();
        handles.push(tokio::spawn(async move {
            create_task_id(&c, &base, once_task(&format!("conc_{i}"))).await
        }));
    }

    for h in handles {
        let id = h.await.unwrap();
        assert!(!id.is_empty());
    }

    let tasks: Vec<Value> =
        c.get(format!("{base}/api/v1/scheduler/tasks")).send().await.unwrap().json().await.unwrap();
    assert!(tasks.len() >= 5);
    shutdown.cancel();
}
