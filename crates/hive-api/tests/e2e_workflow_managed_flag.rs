//! E2E test: Verify that agents spawned by workflows have `workflow_managed: true`
//! in their AgentSpec, while agents spawned directly by chat have `workflow_managed: false`.
//!
//! This ensures the daemon recovery path correctly distinguishes between
//! workflow-managed and user-managed agents, preventing duplicate agent
//! spawning on daemon restart.

use hive_model::ModelRouter;
use hive_test_utils::{wait_for, ScriptedProvider, TestDaemon, DEFAULT_POLL_INTERVAL};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(60);

fn auth_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer test-token"));
    headers
}

fn build_client() -> reqwest::Client {
    reqwest::Client::builder().default_headers(auth_headers()).build().expect("http client")
}

/// Build a provider that handles the workflow agent (ask_user then answer).
fn build_provider() -> ScriptedProvider {
    ScriptedProvider::new("mock", "test-model")
        .on_system_contains(
            "Ask the user what color they prefer",
            vec![
                ScriptedProvider::tool_call_response(
                    "mock",
                    "test-model",
                    "tc-ask",
                    "core.ask_user",
                    json!({
                        "question": "What is your favorite color?",
                        "choices": ["Red", "Blue"],
                        "allow_freeform": false
                    }),
                ),
                ScriptedProvider::text_response("mock", "test-model", "The user chose Blue."),
            ],
        )
        .default_responses(vec![ScriptedProvider::text_response(
            "mock",
            "test-model",
            "Default response.",
        )])
}

fn workflow_yaml() -> &'static str {
    include_str!("../../../tests/fixtures/workflows/scenario1-agent-question.yaml")
}

#[tokio::test]
async fn workflow_agent_has_workflow_managed_flag() {
    // ── Setup ──────────────────────────────────────────────────────────
    let provider = build_provider();
    let mut router = ModelRouter::new();
    router.register_provider(provider);
    let router = Arc::new(router);

    let daemon =
        TestDaemon::builder().with_model_router(router).spawn().await.expect("test daemon");

    let client = build_client();
    let base = &daemon.base_url;

    // ── 1. Save workflow definition ────────────────────────────────────
    let resp = client
        .post(format!("{base}/api/v1/workflows/definitions"))
        .json(&json!({ "yaml": workflow_yaml() }))
        .send()
        .await
        .expect("save definition");
    assert!(resp.status().is_success(), "save definition failed: {}", resp.status());

    // ── 2. Create a chat session ───────────────────────────────────────
    let resp =
        client.post(format!("{base}/api/v1/chat/sessions")).send().await.expect("create session");
    assert!(resp.status().is_success());
    let session: Value = resp.json().await.expect("session json");
    let session_id = session["id"].as_str().expect("session id");

    // ── 3. Launch the workflow ─────────────────────────────────────────
    let resp = client
        .post(format!("{base}/api/v1/workflows/instances"))
        .json(&json!({
            "definition": "test/scenario1-agent-question",
            "parent_session_id": session_id,
            "inputs": {}
        }))
        .send()
        .await
        .expect("launch workflow");
    assert!(resp.status().is_success());

    // ── 4. Wait for agent to spawn (via pending question) ──────────────
    let question = wait_for(TIMEOUT, DEFAULT_POLL_INTERVAL, || {
        let client = client.clone();
        let url = format!("{base}/api/v1/chat/sessions/{session_id}/pending-questions");
        async move {
            let resp = client.get(&url).send().await.ok()?;
            let questions: Vec<Value> = resp.json().await.ok()?;
            questions
                .into_iter()
                .find(|q| q["text"].as_str().map_or(false, |t| t.contains("favorite color")))
        }
    })
    .await
    .expect("timed out waiting for pending question");

    let agent_id = question["agent_id"].as_str().expect("agentId");

    // ── 5. List session agents and check workflow_managed flag ──────────
    let resp = client
        .get(format!("{base}/api/v1/chat/sessions/{session_id}/agents"))
        .send()
        .await
        .expect("list agents");
    assert!(resp.status().is_success());
    let agents: Vec<Value> = resp.json().await.expect("agents json");

    let workflow_agent = agents
        .iter()
        .find(|a| a["agent_id"].as_str() == Some(agent_id))
        .expect("workflow agent should be in the list");

    assert_eq!(
        workflow_agent["spec"]["workflow_managed"].as_bool(),
        Some(true),
        "Agent spawned by workflow should have workflow_managed=true. Agent: {}",
        serde_json::to_string_pretty(workflow_agent).unwrap_or_default()
    );

    // ── 6. Answer the question to let workflow complete ─────────────────
    let request_id = question["request_id"].as_str().expect("request_id");
    let resp = client
        .post(format!("{base}/api/v1/chat/sessions/{session_id}/agents/{agent_id}/interaction"))
        .json(&json!({
            "request_id": request_id,
            "payload": {
                "type": "answer",
                "selected_choice": 1,
                "text": "Blue"
            }
        }))
        .send()
        .await
        .expect("respond to question");
    assert!(resp.status().is_success());

    // ── 7. Wait for workflow completion ─────────────────────────────────
    wait_for(TIMEOUT, DEFAULT_POLL_INTERVAL, || {
        let client = client.clone();
        let url = format!("{base}/api/v1/workflows/instances?session_id={session_id}");
        async move {
            let resp = client.get(&url).send().await.ok()?;
            let body: Value = resp.json().await.ok()?;
            body["items"]
                .as_array()?
                .iter()
                .find(|i| i["status"].as_str() == Some("completed"))
                .cloned()
        }
    })
    .await
    .expect("timed out waiting for workflow completion");

    // ── Teardown ───────────────────────────────────────────────────────
    daemon.stop().await.expect("stop daemon");
}
