/// Integration tests for the Agent Kit export/import API endpoints.
///
/// These tests validate the full round-trip: create personas + workflows,
/// export to `.agentkit`, preview import, and apply import with namespace
/// remapping and cross-reference rewriting.
use axum::http::StatusCode;
use hive_api::{build_router, chat, AppState, ChatRuntimeConfig, ChatService, SchedulerService};
use hive_core::{AuditLogger, EventBus, HiveMindConfig};
use serde_json::{json, Value};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::Notify;

// ── Test server bootstrap ───────────────────────────────────────────────

async fn boot_server() -> (String, Arc<Notify>, TempDir) {
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
        None,
        None,
        None,
        None,
        None,
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

    let mut state = AppState::with_chat(config, audit, event_bus, shutdown.clone(), chat);
    state.personas_dir = tempdir.path().join("personas");
    std::fs::create_dir_all(&state.personas_dir).expect("create personas dir");

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

// ── Helpers ─────────────────────────────────────────────────────────────

async fn save_persona(client: &reqwest::Client, base: &str, id: &str, name: &str) {
    let persona = json!([{
        "id": id,
        "name": name,
        "description": format!("Test persona {name}"),
        "system_prompt": "You are a helpful agent.",
        "loop_strategy": "react",
        "allowed_tools": ["*"],
    }]);
    let resp = client
        .put(format!("{base}/api/v1/config/personas"))
        .json(&persona)
        .send()
        .await
        .expect("save persona");
    assert!(resp.status().is_success(), "save persona failed: {}", resp.status());
}

async fn save_workflow(client: &reqwest::Client, base: &str, yaml: &str) {
    let resp = client
        .post(format!("{base}/api/v1/workflows/definitions"))
        .json(&json!({ "yaml": yaml }))
        .send()
        .await
        .expect("save workflow");
    assert!(
        resp.status().is_success() || resp.status() == StatusCode::CREATED,
        "save workflow failed: {} - {}",
        resp.status(),
        resp.text().await.unwrap_or_default()
    );
}

fn test_workflow_yaml(name: &str, persona_id: &str) -> String {
    format!(
        r#"name: {name}
description: Test workflow for agent kit
mode: chat
steps:
  - id: trigger
    type: trigger
    trigger:
      type: manual
    next:
      - agent_step
  - id: agent_step
    type: task
    task:
      kind: invoke_agent
      persona_id: {persona_id}
      task: "do something"
"#
    )
}

async fn export_kit(
    client: &reqwest::Client,
    base: &str,
    kit_name: &str,
    persona_ids: &[&str],
    workflow_names: &[&str],
) -> Value {
    let resp = client
        .post(format!("{base}/api/v1/agent-kits/export"))
        .json(&json!({
            "kit_name": kit_name,
            "persona_ids": persona_ids,
            "workflow_names": workflow_names,
        }))
        .send()
        .await
        .expect("export");
    assert!(resp.status().is_success(), "export failed: {}", resp.status());
    resp.json().await.expect("export json")
}

async fn preview_kit(
    client: &reqwest::Client,
    base: &str,
    content: &str,
    namespace: &str,
) -> Value {
    let resp = client
        .post(format!("{base}/api/v1/agent-kits/preview"))
        .json(&json!({
            "content": content,
            "target_namespace": namespace,
        }))
        .send()
        .await
        .expect("preview");
    assert!(resp.status().is_success(), "preview failed: {}", resp.status());
    resp.json().await.expect("preview json")
}

async fn import_kit(
    client: &reqwest::Client,
    base: &str,
    content: &str,
    namespace: &str,
    selected: &[&str],
) -> Value {
    let resp = client
        .post(format!("{base}/api/v1/agent-kits/import"))
        .json(&json!({
            "content": content,
            "target_namespace": namespace,
            "selected_items": selected,
        }))
        .send()
        .await
        .expect("import");
    assert!(resp.status().is_success(), "import failed: {} - {}", resp.status(), "check body");
    resp.json().await.expect("import json")
}

// ── Tests ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_full_round_trip() {
    let (base, shutdown, _dir) = boot_server().await;
    let client = authed_client();

    // 1. Create test data
    save_persona(&client, &base, "test/bot", "Test Bot").await;
    save_workflow(&client, &base, &test_workflow_yaml("test/flow", "test/bot")).await;

    // 2. Export
    let export_resp = export_kit(&client, &base, "Test Kit", &["test/bot"], &["test/flow"]).await;
    let content = export_resp["content"].as_str().expect("content field");
    assert!(!content.is_empty());

    // 3. Preview under new namespace
    let preview = preview_kit(&client, &base, content, "imported").await;
    assert!(preview["errors"].as_array().unwrap().is_empty());
    let items = preview["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);

    let persona_item = items.iter().find(|i| i["kind"] == "persona").unwrap();
    assert_eq!(persona_item["new_id"], "imported/bot");
    assert!(!persona_item["overwrites_existing"].as_bool().unwrap());

    let wf_item = items.iter().find(|i| i["kind"] == "workflow").unwrap();
    assert_eq!(wf_item["new_id"], "imported/flow");

    // 4. Import
    let result =
        import_kit(&client, &base, content, "imported", &["imported/bot", "imported/flow"]).await;
    assert_eq!(result["imported_personas"].as_array().unwrap().len(), 1);
    assert_eq!(result["imported_workflows"].as_array().unwrap().len(), 1);
    assert!(result["errors"].as_array().unwrap().is_empty());

    // 5. Verify imported persona exists
    let resp = client.get(format!("{base}/api/v1/config/personas")).send().await.unwrap();
    let personas: Vec<Value> = resp.json().await.unwrap();
    assert!(personas.iter().any(|p| p["id"] == "imported/bot"));

    shutdown.notify_waiters();
}

#[tokio::test]
async fn test_renamespacing_cross_refs() {
    let (base, shutdown, _dir) = boot_server().await;
    let client = authed_client();

    save_persona(&client, &base, "ns/agent", "Agent").await;
    save_workflow(&client, &base, &test_workflow_yaml("ns/process", "ns/agent")).await;

    // Export
    let export_resp = export_kit(&client, &base, "Ref Kit", &["ns/agent"], &["ns/process"]).await;
    let content = export_resp["content"].as_str().unwrap();

    // Import under new namespace
    let result =
        import_kit(&client, &base, content, "newns", &["newns/agent", "newns/process"]).await;
    assert!(result["errors"].as_array().unwrap().is_empty());

    // Fetch imported workflow and verify persona_id was rewritten
    let resp = client
        .get(format!(
            "{base}/api/v1/workflows/definitions/{}",
            urlencoding::encode("newns/process")
        ))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let wf: Value = resp.json().await.unwrap();
    let yaml = wf["yaml"].as_str().unwrap();
    assert!(
        yaml.contains("persona_id: newns/agent"),
        "Expected rewritten persona_id in workflow YAML, got: {}",
        yaml
    );

    shutdown.notify_waiters();
}

#[tokio::test]
async fn test_overwrite_scenario() {
    let (base, shutdown, _dir) = boot_server().await;
    let client = authed_client();

    save_persona(&client, &base, "ow/bot", "Original Bot").await;
    save_workflow(&client, &base, &test_workflow_yaml("ow/flow", "ow/bot")).await;

    // Export
    let export_resp = export_kit(&client, &base, "OW Kit", &["ow/bot"], &["ow/flow"]).await;
    let content = export_resp["content"].as_str().unwrap();

    // Preview under SAME namespace → should detect overwrites
    let preview = preview_kit(&client, &base, content, "ow").await;
    let items = preview["items"].as_array().unwrap();
    for item in items {
        assert!(
            item["overwrites_existing"].as_bool().unwrap(),
            "Expected overwrite for {}",
            item["new_id"]
        );
    }

    // Apply anyway
    let result = import_kit(&client, &base, content, "ow", &["ow/bot", "ow/flow"]).await;
    assert!(result["errors"].as_array().unwrap().is_empty());
    for p in result["imported_personas"].as_array().unwrap() {
        assert!(p["overwritten"].as_bool().unwrap());
    }

    shutdown.notify_waiters();
}

#[tokio::test]
async fn test_system_namespace_rejected() {
    let (base, shutdown, _dir) = boot_server().await;
    let client = authed_client();

    save_persona(&client, &base, "x/bot", "Bot").await;
    let export_resp = export_kit(&client, &base, "Kit", &["x/bot"], &[]).await;
    let content = export_resp["content"].as_str().unwrap();

    // Preview with system namespace → errors
    let preview = preview_kit(&client, &base, content, "system").await;
    let errors = preview["errors"].as_array().unwrap();
    assert!(!errors.is_empty());
    assert!(errors[0].as_str().unwrap().contains("system"));

    // Import with system namespace → should fail
    let resp = client
        .post(format!("{base}/api/v1/agent-kits/import"))
        .json(&json!({
            "content": content,
            "target_namespace": "system",
            "selected_items": ["system/bot"],
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    shutdown.notify_waiters();
}

#[tokio::test]
async fn test_selective_import() {
    let (base, shutdown, _dir) = boot_server().await;
    let client = authed_client();

    save_persona(&client, &base, "sel/p1", "P1").await;
    save_persona(&client, &base, "sel/p2", "P2").await;
    save_workflow(&client, &base, &test_workflow_yaml("sel/wf", "sel/p1")).await;

    let export_resp =
        export_kit(&client, &base, "Sel Kit", &["sel/p1", "sel/p2"], &["sel/wf"]).await;
    let content = export_resp["content"].as_str().unwrap();

    // Import only p1, skip p2 and wf
    let result = import_kit(&client, &base, content, "pick", &["pick/p1"]).await;
    assert_eq!(result["imported_personas"].as_array().unwrap().len(), 1);
    assert!(result["imported_workflows"].as_array().unwrap().is_empty());
    assert_eq!(result["skipped"].as_array().unwrap().len(), 2);

    shutdown.notify_waiters();
}

#[tokio::test]
async fn test_external_ref_warnings() {
    let (base, shutdown, _dir) = boot_server().await;
    let client = authed_client();

    // Workflow references a persona NOT included in the export
    save_workflow(&client, &base, &test_workflow_yaml("ext/wf", "external/missing-agent")).await;

    let export_resp = export_kit(&client, &base, "Ext Kit", &[], &["ext/wf"]).await;
    let content = export_resp["content"].as_str().unwrap();

    let preview = preview_kit(&client, &base, content, "tgt").await;
    let warnings = preview["warnings"].as_array().unwrap();
    assert!(!warnings.is_empty(), "Expected external ref warning, got none");
    assert!(warnings[0].as_str().unwrap().contains("external/missing-agent"));

    shutdown.notify_waiters();
}

#[tokio::test]
async fn test_invalid_archive_not_zip() {
    let (base, shutdown, _dir) = boot_server().await;
    let client = authed_client();

    use base64::Engine;
    let bad_content = base64::engine::general_purpose::STANDARD.encode(b"not a zip file");

    let resp = client
        .post(format!("{base}/api/v1/agent-kits/preview"))
        .json(&json!({
            "content": bad_content,
            "target_namespace": "test",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    shutdown.notify_waiters();
}

#[tokio::test]
async fn test_invalid_archive_no_manifest() {
    let (base, shutdown, _dir) = boot_server().await;
    let client = authed_client();

    // Create a valid ZIP without manifest.json
    use base64::Engine;
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        use std::io::Write;
        let mut zip = zip::ZipWriter::new(&mut buf);
        zip.start_file("random.txt", zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(b"hello").unwrap();
        zip.finish().unwrap();
    }
    let content = base64::engine::general_purpose::STANDARD.encode(buf.into_inner());

    let resp = client
        .post(format!("{base}/api/v1/agent-kits/preview"))
        .json(&json!({
            "content": content,
            "target_namespace": "test",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    shutdown.notify_waiters();
}
