//! E2E: Workflow-spawned agent `ask_user` appears via BOTH SSE paths
//!
//! Regression tests for the bug where a chat session launches a workflow,
//! the workflow invokes an agent, the agent calls `ask_user`, but the
//! question never appears in the chat thread.
//!
//! The chat UI receives questions through two push-based channels:
//!   1. **Agent stage SSE** (`/api/v1/chat/sessions/{id}/agents/stream`)
//!      — `SupervisorEvent::AgentOutput { event: QuestionAsked }` events
//!   2. **Interactions SSE** (`/api/v1/interactions/stream`)
//!      — full snapshot of all pending interactions rebuilt on every change
//!
//! Existing coverage:
//!   - `e2e_scenario1_agent_question`: validates REST pending-questions endpoint
//!   - `e2e_question_sse_race`: validates interactions SSE (without agent stage)
//!
//! This file adds:
//!   - Agent stage SSE delivers QuestionAsked for workflow-spawned agents
//!   - Both SSE streams deliver the question simultaneously
//!   - Interaction counts propagate for workflow agents (REST path)

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

fn build_questioner_provider() -> ScriptedProvider {
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
                        "choices": ["Red", "Blue", "Green"],
                        "allow_freeform": false
                    }),
                ),
                ScriptedProvider::text_response("mock", "test-model", "The user chose Blue."),
            ],
        )
        .default_responses(vec![ScriptedProvider::text_response(
            "mock",
            "test-model",
            "session agent default",
        )])
}

fn workflow_yaml() -> &'static str {
    include_str!("../../../tests/fixtures/workflows/scenario1-agent-question.yaml")
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Connect to an SSE endpoint and spawn a background task that collects
/// parsed JSON data events into an mpsc channel.
/// Returns both the receiver and the task handle (caller should abort on cleanup).
async fn subscribe_sse(
    client: &reqwest::Client,
    url: &str,
) -> (tokio::sync::mpsc::Receiver<Value>, tokio::task::JoinHandle<()>) {
    let response = client.get(url).send().await.expect("SSE connect");
    assert!(response.status().is_success(), "SSE connect to {url}: {}", response.status());

    let (tx, rx) = tokio::sync::mpsc::channel::<Value>(256);
    let handle = tokio::spawn(async move {
        use tokio_stream::StreamExt;
        let mut stream = response.bytes_stream();
        let mut buf = String::new();

        while let Some(chunk) = stream.next().await {
            let Ok(bytes) = chunk else { continue };
            buf.push_str(&String::from_utf8_lossy(&bytes));

            while let Some(pos) = buf.find("\n\n") {
                let block = buf[..pos].to_string();
                buf = buf[pos + 2..].to_string();

                for line in block.lines() {
                    if let Some(data) = line.strip_prefix("data: ") {
                        if let Ok(parsed) = serde_json::from_str::<Value>(data) {
                            let _ = tx.send(parsed).await;
                        }
                    }
                }
            }
        }
    });
    (rx, handle)
}

/// Wait for an SSE event matching a predicate, returning the first match.
async fn wait_for_sse_event<F>(
    rx: &mut tokio::sync::mpsc::Receiver<Value>,
    timeout: Duration,
    predicate: F,
) -> Option<Value>
where
    F: Fn(&Value) -> bool,
{
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
            Ok(Some(event)) => {
                if predicate(&event) {
                    return Some(event);
                }
            }
            Ok(None) => return None, // channel closed
            Err(_) => continue,      // timeout, try again
        }
    }
    None
}

/// Common setup: save workflow def + create session.
async fn setup_workflow_session(client: &reqwest::Client, base: &str) -> String {
    let resp = client
        .post(format!("{base}/api/v1/workflows/definitions"))
        .json(&json!({ "yaml": workflow_yaml() }))
        .send()
        .await
        .expect("save definition");
    assert!(resp.status().is_success(), "save def: {}", resp.status());

    let resp =
        client.post(format!("{base}/api/v1/chat/sessions")).send().await.expect("create session");
    assert!(resp.status().is_success());
    let session: Value = resp.json().await.expect("session json");
    session["id"].as_str().expect("session id").to_string()
}

/// Launch the scenario1-agent-question workflow on the given session.
async fn launch_workflow(client: &reqwest::Client, base: &str, session_id: &str) {
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
    assert!(resp.status().is_success(), "launch: {}", resp.status());
}

/// Answer a pending question and wait for the workflow to finish.
async fn answer_and_wait(
    client: &reqwest::Client,
    base: &str,
    session_id: &str,
    agent_id: &str,
    request_id: &str,
) {
    let _ = client
        .post(format!("{base}/api/v1/chat/sessions/{session_id}/agents/{agent_id}/interaction"))
        .json(&json!({
            "request_id": request_id,
            "payload": { "type": "answer", "selected_choice": 1, "text": "Blue" }
        }))
        .send()
        .await;

    wait_for(TIMEOUT, DEFAULT_POLL_INTERVAL, || {
        let client = client.clone();
        let url = format!("{base}/api/v1/workflows/instances?session_id={session_id}");
        async move {
            let resp = client.get(&url).send().await.ok()?;
            let body: Value = resp.json().await.ok()?;
            body["items"]
                .as_array()?
                .iter()
                .find(|i| matches!(i["status"].as_str(), Some("completed") | Some("failed")))
                .cloned()
        }
    })
    .await
    .expect("workflow should complete");
}

fn is_question_asked(ev: &Value) -> bool {
    ev.get("type").and_then(|t| t.as_str()) == Some("agent_output")
        && ev.get("event").and_then(|inner| inner.get("type")).and_then(|t| t.as_str())
            == Some("question_asked")
}

// ── Test 1: Agent stage SSE delivers workflow agent QuestionAsked ────────────

/// Validates that the per-session agent stage SSE stream delivers
/// `SupervisorEvent::AgentOutput { event: QuestionAsked }` when a
/// workflow-spawned agent calls `ask_user`.
///
/// This is the primary push channel the frontend uses to show questions
/// in the chat thread.
#[tokio::test]
async fn workflow_agent_question_appears_in_agent_stage_sse() {
    let provider = build_questioner_provider();
    let mut router = ModelRouter::new();
    router.register_provider(provider);
    let router = Arc::new(router);

    let daemon =
        TestDaemon::builder().with_model_router(router).spawn().await.expect("test daemon");
    let client = build_client();
    let base = &daemon.base_url;

    let session_id = setup_workflow_session(&client, base).await;

    // Subscribe to agent stage SSE BEFORE launching the workflow —
    // mirrors the frontend's `agent_stage_subscribe` behaviour.
    let agent_stage_url = format!("{base}/api/v1/chat/sessions/{session_id}/agents/stream");
    let (mut rx, sse_handle) = subscribe_sse(&client, &agent_stage_url).await;

    // Drain the initial snapshot.
    let _ = wait_for_sse_event(&mut rx, Duration::from_secs(10), |ev| {
        ev.get("type").and_then(|t| t.as_str()) == Some("snapshot")
    })
    .await;

    launch_workflow(&client, base, &session_id).await;

    // Wait for QuestionAsked on the agent stage SSE.
    let question_event = wait_for_sse_event(&mut rx, TIMEOUT, is_question_asked).await.expect(
        "Agent stage SSE should deliver QuestionAsked for workflow-spawned agents. \
             If this fails, the chat UI will never see the question.",
    );

    // Verify the event content matches what the frontend expects.
    let inner = &question_event["event"];
    assert_eq!(inner["text"].as_str().unwrap(), "What is your favorite color?");
    assert_eq!(inner["choices"].as_array().expect("choices").len(), 3);
    assert!(inner["request_id"].as_str().is_some(), "must have request_id");
    assert!(question_event["agent_id"].as_str().is_some(), "must have agent_id");

    // Clean up: answer and wait for completion.
    let agent_id = question_event["agent_id"].as_str().unwrap();
    let request_id = inner["request_id"].as_str().unwrap();
    answer_and_wait(&client, base, &session_id, agent_id, request_id).await;

    sse_handle.abort();
    daemon.stop().await.expect("stop daemon");
}

/// Validates that BOTH the agent stage SSE and interactions SSE deliver the
/// question from a workflow-spawned agent, with the correct session_id.
///
/// The frontend uses both paths redundantly for reliability.
#[tokio::test]
async fn workflow_agent_question_appears_in_both_sse_streams() {
    let provider = build_questioner_provider();
    let mut router = ModelRouter::new();
    router.register_provider(provider);
    let router = Arc::new(router);

    let daemon =
        TestDaemon::builder().with_model_router(router).spawn().await.expect("test daemon");
    let client = build_client();
    let base = &daemon.base_url;

    let session_id = setup_workflow_session(&client, base).await;

    // Subscribe to BOTH SSE streams before launching.
    let (mut agent_rx, agent_sse_handle) =
        subscribe_sse(&client, &format!("{base}/api/v1/chat/sessions/{session_id}/agents/stream"))
            .await;
    let (mut interactions_rx, interactions_sse_handle) =
        subscribe_sse(&client, &format!("{base}/api/v1/interactions/stream")).await;

    // Drain initial events.
    let _ = wait_for_sse_event(&mut agent_rx, Duration::from_secs(5), |ev| {
        ev.get("type").and_then(|t| t.as_str()) == Some("snapshot")
    })
    .await;
    let _ = wait_for_sse_event(&mut interactions_rx, Duration::from_secs(5), |ev| {
        ev.get("interactions").is_some()
    })
    .await;

    launch_workflow(&client, base, &session_id).await;

    // Wait for QuestionAsked on the agent stage SSE.
    let agent_q = wait_for_sse_event(&mut agent_rx, TIMEOUT, is_question_asked)
        .await
        .expect("agent stage SSE must deliver QuestionAsked for workflow agent");

    // Wait for the question in the interactions SSE snapshot.
    let interactions_snap = wait_for_sse_event(&mut interactions_rx, TIMEOUT, |ev| {
        ev.get("interactions").and_then(|items| items.as_array()).map_or(false, |items| {
            items.iter().any(|item| {
                item["type"].as_str() == Some("question")
                    && item["text"].as_str().map_or(false, |t| t.contains("favorite color"))
            })
        })
    })
    .await
    .expect("interactions SSE must deliver snapshot with workflow agent's question");

    // Verify agent stage SSE content.
    let inner = &agent_q["event"];
    assert_eq!(inner["text"].as_str().unwrap(), "What is your favorite color?");
    assert!(inner["request_id"].as_str().is_some());
    let agent_id = agent_q["agent_id"].as_str().expect("agent_id");
    assert!(!agent_id.is_empty());

    // Verify interactions SSE has correct session_id and routing.
    let q_item = interactions_snap["interactions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| {
            item["type"].as_str() == Some("question")
                && item["text"].as_str().map_or(false, |t| t.contains("favorite color"))
        })
        .expect("question in snapshot");

    assert_eq!(
        q_item["session_id"].as_str(),
        Some(session_id.as_str()),
        "interactions SSE question must carry the parent session_id"
    );
    assert_eq!(
        q_item["routing"].as_str(),
        Some("session"),
        "routing should be 'session' for workflow agents on a chat session"
    );

    // Clean up.
    let request_id = inner["request_id"].as_str().unwrap();
    answer_and_wait(&client, base, &session_id, agent_id, request_id).await;

    agent_sse_handle.abort();
    interactions_sse_handle.abort();
    daemon.stop().await.expect("stop daemon");
}

/// Validates that interaction badge counts propagate correctly for workflow
/// agents. Uses REST polling only (no SSE subscription) to isolate count
/// propagation from SSE delivery.
#[tokio::test]
async fn workflow_agent_question_propagates_interaction_counts() {
    let provider = build_questioner_provider();
    let mut router = ModelRouter::new();
    router.register_provider(provider);
    let router = Arc::new(router);

    let daemon =
        TestDaemon::builder().with_model_router(router).spawn().await.expect("test daemon");
    let client = build_client();
    let base = &daemon.base_url;

    let session_id = setup_workflow_session(&client, base).await;
    launch_workflow(&client, base, &session_id).await;

    // Wait for question to appear via REST pending-questions.
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
    .expect("question should appear");

    // Verify interaction counts include the question.
    let resp = client
        .get(format!("{base}/api/v1/pending-interaction-counts"))
        .send()
        .await
        .expect("counts");
    assert!(resp.status().is_success());
    let counts: Value = resp.json().await.expect("counts json");
    let agent_id = question["agent_id"].as_str().expect("agent_id");
    let agent_ref = format!("agent/{agent_id}");
    assert_eq!(
        counts[&agent_ref]["questions"].as_u64(),
        Some(1),
        "agent entity should have 1 question, got: {counts}"
    );

    // Clean up.
    let request_id = question["request_id"].as_str().expect("request_id");
    answer_and_wait(&client, base, &session_id, agent_id, request_id).await;

    daemon.stop().await.expect("stop daemon");
}
