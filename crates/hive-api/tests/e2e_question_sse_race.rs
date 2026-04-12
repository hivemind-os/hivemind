//! E2E test: Interactions SSE delivers the question atomically
//!
//! Validates that when an agent calls `core.ask_user`, the interactions SSE
//! push-path delivers a snapshot containing the question — proving the gate
//! request is created before the event is emitted (no race condition).

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

/// Build a ScriptedProvider whose agent calls ask_user.
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
                        "choices": ["Red", "Blue", "Green"],
                        "allow_freeform": false
                    }),
                ),
                ScriptedProvider::text_response("mock", "test-model", "Done."),
            ],
        )
        .default_responses(vec![ScriptedProvider::text_response(
            "mock",
            "test-model",
            "session agent",
        )])
}

fn workflow_yaml() -> &'static str {
    include_str!("../../../tests/fixtures/workflows/scenario1-agent-question.yaml")
}

/// Verify the question appears via the interactions SSE push path, not polling.
#[tokio::test]
async fn question_appears_in_interactions_sse_snapshot() {
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
    assert!(resp.status().is_success(), "save definition: {}", resp.status());

    // ── 2. Create a chat session ───────────────────────────────────────
    let resp =
        client.post(format!("{base}/api/v1/chat/sessions")).send().await.expect("create session");
    assert!(resp.status().is_success());
    let session: Value = resp.json().await.expect("session json");
    let session_id = session["id"].as_str().expect("session id").to_string();

    // ── 3. Subscribe to the interactions SSE BEFORE launching workflow ─
    // This ensures we don't miss any snapshots.
    let sse_url = format!("{base}/api/v1/interactions/stream");
    let sse_response = client.get(&sse_url).send().await.expect("connect interactions SSE");
    assert!(sse_response.status().is_success(), "SSE connect: {}", sse_response.status());

    // Spawn a background task that collects SSE snapshot events.
    let (snapshot_tx, mut snapshot_rx) = tokio::sync::mpsc::channel::<Vec<Value>>(64);
    let sse_task = tokio::spawn(async move {
        use tokio_stream::StreamExt;
        let mut stream = sse_response.bytes_stream();
        let mut buf = String::new();

        while let Some(chunk) = stream.next().await {
            let Ok(bytes) = chunk else { continue };
            let chunk_str = String::from_utf8_lossy(&bytes);
            buf.push_str(&chunk_str);

            // Parse SSE events from the buffer.
            while let Some(pos) = buf.find("\n\n") {
                let block = buf[..pos].to_string();
                buf = buf[pos + 2..].to_string();

                // Extract data lines from the event block.
                for line in block.lines() {
                    if let Some(data) = line.strip_prefix("data: ") {
                        if let Ok(parsed) = serde_json::from_str::<Value>(data) {
                            if let Some(items) = parsed["interactions"].as_array() {
                                let _ = snapshot_tx.send(items.clone()).await;
                            }
                        }
                    }
                }
            }
        }
        eprintln!("[SSE] stream ended");
    });

    // ── 4. Launch the workflow ──────────────────────────────────────────
    let resp = client
        .post(format!("{base}/api/v1/workflows/instances"))
        .json(&json!({
            "definition": "test/scenario1-agent-question",
            "parent_session_id": &session_id,
            "inputs": {}
        }))
        .send()
        .await
        .expect("launch workflow");
    assert!(resp.status().is_success(), "launch: {}", resp.status());

    // ── 5. Read SSE snapshots until one contains the question ──────────
    // With the race condition fixed, the snapshot triggered by the
    // QuestionAdded event WILL contain the question because
    // gate.create_request() runs before event_tx.send().
    let deadline = tokio::time::Instant::now() + TIMEOUT;
    let mut found_question: Option<Value> = None;
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_secs(5), snapshot_rx.recv()).await {
            Ok(Some(interactions)) => {
                for item in &interactions {
                    if item["type"].as_str() == Some("question")
                        && item["text"].as_str().map_or(false, |t| t.contains("favorite color"))
                        && item["session_id"].as_str() == Some(session_id.as_str())
                    {
                        found_question = Some(item.clone());
                    }
                }
                if found_question.is_some() {
                    break;
                }
            }
            Ok(None) => break,  // channel closed
            Err(_) => continue, // timeout, try again
        }
    }
    sse_task.abort();

    let question = found_question.expect(
        "The interactions SSE should have pushed a snapshot containing the question. \
         If this fails, the gate.create_request() / event_tx.send() ordering may be wrong.",
    );

    // Verify the question content is correct.
    assert_eq!(question["text"].as_str().unwrap(), "What is your favorite color?");
    let choices = question["choices"].as_array().expect("choices");
    assert_eq!(choices.len(), 3);
    assert_eq!(question["session_id"].as_str().unwrap(), session_id);

    // ── 6. Respond so the workflow can complete ────────────────────────
    let agent_id = question["agent_id"].as_str().expect("agentId");
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
        .expect("respond");
    assert!(resp.status().is_success(), "respond: {}", resp.status());

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
    .expect("workflow should complete");

    daemon.stop().await.expect("stop daemon");
}
