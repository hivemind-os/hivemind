//! E2E Scenario 1: Chat → Workflow → Agent Question → Answer → Results
//!
//! Validates the full flow:
//! 1. Save a workflow definition with an `invoke_agent` step
//! 2. Create a chat session
//! 3. Launch the workflow with the session as parent
//! 4. The agent uses `ask_user` → a pending question appears
//! 5. Respond to the question
//! 6. The agent completes, workflow completes
//! 7. Verify chat history contains the expected messages

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

/// Build a ScriptedProvider whose questioner agent calls ask_user then returns text.
fn build_scenario1_provider() -> ScriptedProvider {
    ScriptedProvider::new("mock", "test-model")
        // Questioner agent: match on the task prompt (system prompt won't contain persona name)
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
                        "choices": ["Red", "Blue", "Green"],
                        "allow_freeform": false
                    }),
                ),
                ScriptedProvider::text_response(
                    "mock",
                    "test-model",
                    "The user's favorite color is Blue.",
                ),
            ],
        )
        // Default (session agent): plain text (should not be needed for workflow-only test)
        .default_responses(vec![ScriptedProvider::text_response(
            "mock",
            "test-model",
            "I am the session agent.",
        )])
}

fn build_model_router(provider: ScriptedProvider) -> Arc<ModelRouter> {
    let mut router = ModelRouter::new();
    router.register_provider(provider);
    Arc::new(router)
}

fn workflow_yaml() -> &'static str {
    include_str!("../../../tests/fixtures/workflows/scenario1-agent-question.yaml")
}

#[tokio::test]
async fn workflow_agent_question_routes_through_session() {
    // ── Setup ──────────────────────────────────────────────────────────
    let provider = build_scenario1_provider();
    let router = build_model_router(provider);

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
    assert!(
        resp.status().is_success(),
        "save definition failed: {} — {}",
        resp.status(),
        resp.text().await.unwrap_or_default()
    );

    // ── 2. Create a chat session ───────────────────────────────────────
    let resp =
        client.post(format!("{base}/api/v1/chat/sessions")).send().await.expect("create session");
    assert!(resp.status().is_success(), "create session failed: {}", resp.status());
    let session: Value = resp.json().await.expect("session json");
    let session_id = session["id"].as_str().expect("session id");

    // ── 3. Launch the workflow with this session as parent ──────────────
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
    assert!(
        resp.status().is_success(),
        "launch workflow failed: {} — {}",
        resp.status(),
        resp.text().await.unwrap_or_default()
    );
    let launch: Value = resp.json().await.expect("launch json");
    let instance_id = launch["instance_id"].as_i64().expect("instance_id");

    // ── 4. Wait for pending question from the questioner agent ─────────
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

    let request_id = question["request_id"].as_str().expect("request_id");
    let agent_id = question["agent_id"].as_str().expect("agentId");

    // Verify question content
    assert_eq!(question["text"].as_str().unwrap(), "What is your favorite color?");
    let choices = question["choices"].as_array().expect("choices array");
    assert_eq!(choices.len(), 3);

    // ── 5. Respond to the question ─────────────────────────────────────
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
    assert!(
        resp.status().is_success(),
        "respond failed: {} — {}",
        resp.status(),
        resp.text().await.unwrap_or_default()
    );

    // ── 6. Wait for workflow to complete ───────────────────────────────
    wait_for(TIMEOUT, DEFAULT_POLL_INTERVAL, || {
        let client = client.clone();
        let url = format!("{base}/api/v1/workflows/instances?session_id={session_id}");
        async move {
            let resp = client.get(&url).send().await.ok()?;
            let body: Value = resp.json().await.ok()?;
            let items = body["items"].as_array()?;
            items
                .iter()
                .find(|inst| {
                    inst["status"].as_str() == Some("completed")
                        && inst["id"].as_i64() == Some(instance_id)
                })
                .cloned()
        }
    })
    .await
    .expect("timed out waiting for workflow completion");

    // ── 7. Verify no pending questions remain ──────────────────────────
    let resp = client
        .get(format!("{base}/api/v1/chat/sessions/{session_id}/pending-questions"))
        .send()
        .await
        .expect("list pending questions");
    let questions: Vec<Value> = resp.json().await.expect("questions json");
    assert!(questions.is_empty(), "expected no pending questions, got {}", questions.len());

    // ── Teardown ───────────────────────────────────────────────────────
    daemon.stop().await.expect("stop daemon");
}
