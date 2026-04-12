use axum::http::StatusCode;
use hive_api::{
    build_router, chat, AppState, ChatRuntimeConfig, ChatService, HardwareSummary,
    LocalModelSummary, SchedulerService,
};
use hive_core::{AuditLogger, EventBus, HiveMindConfig};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::Notify;

/// Helper that boots a test server and returns (base_url, shutdown_notify, tempdir).
async fn boot_test_server() -> (String, Arc<Notify>, TempDir) {
    let tempdir = tempfile::tempdir().expect("temp dir");
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind ephemeral port");
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
    // Use with_chat so local_models is None (avoids the rusqlite-in-async-context panic).
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

/// Helper that boots a test server *with* local model service initialised.
async fn boot_test_server_with_local_models() -> (String, Arc<Notify>, TempDir) {
    let tempdir = tempfile::tempdir().expect("temp dir");
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind ephemeral port");
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

    // Build the local model serviceusing an in-memory registry.
    // We have to do this outside of the async runtime to avoid the rusqlite drop issue,
    // so we spawn_blocking it.
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

fn authed_client() -> reqwest::Client {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("Authorization", "Bearer test-token".parse().unwrap());
    reqwest::Client::builder().default_headers(headers).build().unwrap()
}

// ---------------------------------------------------------------------------
// Tests: endpoints return correct responses when local_models is None
// ---------------------------------------------------------------------------

#[tokio::test]
async fn local_models_list_returns_503_when_service_not_initialised() {
    let (base_url, shutdown, _dir) = boot_test_server().await;
    let resp = authed_client()
        .get(format!("{base_url}/api/v1/local-models"))
        .send()
        .await
        .expect("request");
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    shutdown.notify_waiters();
}

#[tokio::test]
async fn local_models_hardware_returns_503_when_service_not_initialised() {
    let (base_url, shutdown, _dir) = boot_test_server().await;
    let resp = authed_client()
        .get(format!("{base_url}/api/v1/local-models/hardware"))
        .send()
        .await
        .expect("request");
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    shutdown.notify_waiters();
}

#[tokio::test]
async fn local_models_search_returns_503_when_service_not_initialised() {
    let (base_url, shutdown, _dir) = boot_test_server().await;
    let resp = authed_client()
        .get(format!("{base_url}/api/v1/local-models/search?query=llama"))
        .send()
        .await
        .expect("request");
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    shutdown.notify_waiters();
}

// ---------------------------------------------------------------------------
// Tests: endpoints return correct responses when local_models is available
// ---------------------------------------------------------------------------

#[tokio::test]
async fn local_models_list_returns_empty_when_no_models_installed() {
    let (base_url, shutdown, _dir) = boot_test_server_with_local_models().await;
    let resp = authed_client()
        .get(format!("{base_url}/api/v1/local-models"))
        .send()
        .await
        .expect("request");
    assert_eq!(resp.status(), StatusCode::OK);
    let body: LocalModelSummary = resp.json().await.expect("json");
    assert_eq!(body.installed_count, 0);
    assert!(body.models.is_empty());
    shutdown.notify_waiters();
}

#[tokio::test]
async fn local_models_get_returns_404_for_unknown_model() {
    let (base_url, shutdown, _dir) = boot_test_server_with_local_models().await;
    let resp = authed_client()
        .get(format!("{base_url}/api/v1/local-models/nonexistent"))
        .send()
        .await
        .expect("request");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    shutdown.notify_waiters();
}

#[tokio::test]
async fn local_models_remove_returns_404_for_unknown_model() {
    let (base_url, shutdown, _dir) = boot_test_server_with_local_models().await;
    let client = authed_client();
    let resp = client
        .delete(format!("{base_url}/api/v1/local-models/nonexistent"))
        .send()
        .await
        .expect("request");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    shutdown.notify_waiters();
}

#[tokio::test]
async fn local_models_hardware_returns_valid_info() {
    let (base_url, shutdown, _dir) = boot_test_server_with_local_models().await;
    let resp = authed_client()
        .get(format!("{base_url}/api/v1/local-models/hardware"))
        .send()
        .await
        .expect("request");
    assert_eq!(resp.status(), StatusCode::OK);
    let body: HardwareSummary = resp.json().await.expect("json");
    assert!(!body.hardware.cpu.name.is_empty());
    assert!(body.hardware.cpu.cores_logical >= 1);
    assert_eq!(body.usage.models_loaded, 0);
    shutdown.notify_waiters();
}

#[tokio::test]
async fn healthz_still_works_with_local_models_enabled() {
    let (base_url, shutdown, _dir) = boot_test_server_with_local_models().await;
    let resp = authed_client().get(format!("{base_url}/healthz")).send().await.expect("request");
    assert!(resp.status().is_success());
    shutdown.notify_waiters();
}
