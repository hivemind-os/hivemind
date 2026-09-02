//! E2E: persona `allowed_tools` is applied on the live chat path.
//!
//! Restricted personas must receive always-on tools (`core.ask_user`,
//! `core.activate_skill`) plus glob matches, and must not receive privileged
//! `core.*` tools such as `core.spawn_agent`.

use hive_contracts::Persona;
use hive_model::ModelRouter;
use hive_test_utils::{
    wait_for, RecordedCall, ScriptedProvider, TestDaemon, DEFAULT_POLL_INTERVAL,
};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(30);

fn auth_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer test-token"));
    headers
}

fn cad_persona() -> Persona {
    let mut persona = Persona::default_persona();
    persona.id = "system/3d-print/cad-designer".to_string();
    persona.name = "CAD Designer".to_string();
    persona.allowed_tools = vec![
        "shell.*".to_string(),
        "core.ask_user".to_string(),
        "filesystem.read".to_string(),
        "filesystem.write".to_string(),
        "filesystem.list".to_string(),
        "filesystem.glob".to_string(),
        "filesystem.exists".to_string(),
        "core.activate_skill".to_string(),
    ];
    persona
}

#[tokio::test]
async fn cad_persona_session_does_not_expose_privileged_core_tools() {
    let recorded: Arc<Mutex<Vec<RecordedCall>>> = Arc::new(Mutex::new(Vec::new()));
    let provider = ScriptedProvider::new("mock", "test-model")
        .with_shared_calls(Arc::clone(&recorded))
        .default_responses(vec![ScriptedProvider::text_response("mock", "test-model", "ready")]);
    let mut router = ModelRouter::new();
    router.register_provider(provider);

    let daemon = TestDaemon::builder()
        .with_model_router(Arc::new(router))
        .with_personas(vec![cad_persona()])
        .spawn()
        .await
        .expect("test daemon");

    let client = reqwest::Client::builder().default_headers(auth_headers()).build().unwrap();
    let base = &daemon.base_url;

    let resp = client
        .post(format!("{base}/api/v1/chat/sessions"))
        .json(&json!({
            "title": "cad-allowlist",
            "persona_id": "system/3d-print/cad-designer"
        }))
        .send()
        .await
        .expect("create session");
    assert!(resp.status().is_success(), "create session: {}", resp.status());
    let session: Value = resp.json().await.expect("session json");
    let session_id = session["id"].as_str().expect("session id");

    let resp = client
        .post(format!("{base}/api/v1/chat/sessions/{session_id}/messages"))
        .json(&json!({ "content": "hello", "attachments": [] }))
        .send()
        .await
        .expect("send message");
    assert!(resp.status().is_success(), "send message: {}", resp.status());

    let tools = wait_for(TIMEOUT, DEFAULT_POLL_INTERVAL, || {
        let recorded = Arc::clone(&recorded);
        async move {
            let calls = recorded.lock().unwrap();
            calls.first().map(|c| c.tool_ids.clone())
        }
    })
    .await
    .expect("timed out waiting for model call");

    assert!(
        tools.iter().any(|t| t == "core.ask_user"),
        "always-on ask_user missing from {tools:?}"
    );
    assert!(
        tools.iter().any(|t| t == "core.activate_skill"),
        "always-on activate_skill missing from {tools:?}"
    );
    assert!(
        tools.iter().any(|t| t == "shell.execute" || t.starts_with("shell.")),
        "shell.* glob should attach shell.execute, got {tools:?}"
    );
    assert!(
        tools.iter().any(|t| t == "filesystem.read"),
        "filesystem.read should be allowed, got {tools:?}"
    );
    assert!(
        !tools.iter().any(|t| t == "core.spawn_agent"),
        "spawn_agent must not leak onto a CAD allowlist: {tools:?}"
    );
    assert!(
        !tools.iter().any(|t| t == "core.data_store"),
        "data_store must not leak onto a CAD allowlist: {tools:?}"
    );
    assert!(
        !tools.iter().any(|t| t == "core.schedule_task"),
        "schedule_task must not leak onto a CAD allowlist: {tools:?}"
    );

    daemon.stop().await.expect("stop daemon");
}
