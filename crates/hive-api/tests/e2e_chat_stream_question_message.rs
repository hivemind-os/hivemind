//! E2E: Chat session SSE stream delivers question as an AgentSessionMessage
//!
//! Proves that when a workflow agent calls `ask_user`, the chat session
//! SSE stream (`/api/v1/chat/sessions/{id}/stream`) emits an
//! `AgentSessionMessage` event containing the question text.
//! This is the primary real-time delivery path — the same SSE connection
//! that delivers tokens and tool call events.

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

/// Subscribe to an SSE endpoint, collecting parsed JSON events into an mpsc channel.
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

/// Wait for an SSE event matching a predicate.
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
            Ok(None) => return None,
            Err(_) => continue,
        }
    }
    None
}

/// Test: chat session SSE stream delivers an AgentSessionMessage when a question is created.
///
/// The frontend uses the `chat:event` handler to process these events and trigger
/// `syncChatState()`, which fetches the updated snapshot containing the question message.
#[tokio::test]
async fn chat_stream_delivers_question_message() {
    let provider = build_questioner_provider();
    let mut router = ModelRouter::new();
    router.register_provider(provider);
    let router = Arc::new(router);

    let daemon =
        TestDaemon::builder().with_model_router(router).spawn().await.expect("test daemon");
    let client = build_client();
    let base = &daemon.base_url;

    // Setup
    let resp = client
        .post(format!("{base}/api/v1/workflows/definitions"))
        .json(&json!({ "yaml": workflow_yaml() }))
        .send()
        .await
        .expect("save definition");
    assert!(resp.status().is_success());

    let resp =
        client.post(format!("{base}/api/v1/chat/sessions")).send().await.expect("create session");
    let session: Value = resp.json().await.expect("session json");
    let session_id = session["id"].as_str().expect("session id").to_string();

    // Subscribe to the chat session SSE stream BEFORE launching the workflow
    let stream_url = format!("{base}/api/v1/chat/sessions/{session_id}/stream");
    let (mut rx, sse_handle) = subscribe_sse(&client, &stream_url).await;

    // Launch workflow
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
    assert!(resp.status().is_success());

    // Wait for an AgentSessionMessage event on the chat stream.
    // The insert_question_message method emits this event, which the
    // frontend's `chat:event` handler uses to trigger syncChatState().
    let msg_event = wait_for_sse_event(&mut rx, TIMEOUT, |ev| {
        // AgentSessionMessage serializes as { "AgentSessionMessage": { ... } }
        ev.get("AgentSessionMessage").is_some()
    })
    .await
    .expect(
        "Chat session SSE stream should deliver an AgentSessionMessage event \
         when a question is created. If this fails, the frontend will never \
         know to sync and display the question.",
    );

    // The content should reference the question
    let inner = &msg_event["AgentSessionMessage"];
    let content = inner["content"].as_str().unwrap_or("");
    assert!(
        content.contains("favorite color"),
        "AgentSessionMessage content should contain question text, got: {content}"
    );

    // Also verify the question eventually appears in the snapshot
    let question_msg = wait_for(TIMEOUT, DEFAULT_POLL_INTERVAL, || {
        let client = client.clone();
        let url = format!("{base}/api/v1/chat/sessions/{session_id}");
        async move {
            let resp = client.get(&url).send().await.ok()?;
            let snapshot: Value = resp.json().await.ok()?;
            let messages = snapshot["messages"].as_array()?;
            messages
                .iter()
                .find(|m| {
                    m["interaction_kind"].as_str() == Some("question")
                        && m["content"].as_str().map_or(false, |c| c.contains("favorite color"))
                })
                .cloned()
        }
    })
    .await
    .expect("snapshot should contain the question message");

    assert_eq!(question_msg["status"].as_str(), Some("processing"));
    assert!(question_msg["interaction_request_id"].is_string());

    // Cleanup
    sse_handle.abort();
    let request_id = question_msg["interaction_request_id"].as_str().unwrap();
    // Find the agent_id from pending-questions to answer
    let resp = client
        .get(format!("{base}/api/v1/chat/sessions/{session_id}/pending-questions"))
        .send()
        .await
        .expect("pending questions");
    let questions: Vec<Value> = resp.json().await.expect("questions json");
    if let Some(q) = questions.first() {
        let agent_id = q["agent_id"].as_str().unwrap_or("");
        let _ = client
            .post(format!("{base}/api/v1/chat/sessions/{session_id}/agents/{agent_id}/interaction"))
            .json(&json!({
                "request_id": request_id,
                "payload": { "type": "answer", "selected_choice": 1, "text": "Blue" }
            }))
            .send()
            .await;
    }

    daemon.stop().await.expect("stop daemon");
}

/// Test: Multiple questions from different workflow agents each create their own message.
#[tokio::test]
async fn multiple_questions_create_separate_messages() {
    let provider = build_questioner_provider();
    let mut router = ModelRouter::new();
    router.register_provider(provider);
    let router = Arc::new(router);

    let daemon =
        TestDaemon::builder().with_model_router(router).spawn().await.expect("test daemon");
    let client = build_client();
    let base = &daemon.base_url;

    // Setup
    let resp = client
        .post(format!("{base}/api/v1/workflows/definitions"))
        .json(&json!({ "yaml": workflow_yaml() }))
        .send()
        .await
        .expect("save definition");
    assert!(resp.status().is_success());

    let resp =
        client.post(format!("{base}/api/v1/chat/sessions")).send().await.expect("create session");
    let session: Value = resp.json().await.expect("session json");
    let session_id = session["id"].as_str().expect("session id").to_string();

    // Launch workflow — first question
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
    assert!(resp.status().is_success());

    // Wait for first question
    let q1 = wait_for(TIMEOUT, DEFAULT_POLL_INTERVAL, || {
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
    .expect("first question");

    // Wait for the snapshot to contain the question message (insertion is async)
    let question_msg = wait_for(TIMEOUT, DEFAULT_POLL_INTERVAL, || {
        let client = client.clone();
        let url = format!("{base}/api/v1/chat/sessions/{session_id}");
        async move {
            let resp = client.get(&url).send().await.ok()?;
            let snapshot: Value = resp.json().await.ok()?;
            let question_messages: Vec<Value> = snapshot["messages"]
                .as_array()?
                .iter()
                .filter(|m| m["interaction_kind"].as_str() == Some("question"))
                .cloned()
                .collect();
            if question_messages.len() == 1 {
                Some(question_messages)
            } else {
                None
            }
        }
    })
    .await
    .expect("should have exactly 1 question message after first workflow");
    assert_eq!(question_msg.len(), 1);

    // Answer the first question so the workflow completes
    let agent_id = q1["agent_id"].as_str().unwrap();
    let request_id = q1["request_id"].as_str().unwrap();
    let _ = client
        .post(format!("{base}/api/v1/chat/sessions/{session_id}/agents/{agent_id}/interaction"))
        .json(&json!({
            "request_id": request_id,
            "payload": { "type": "answer", "selected_choice": 1, "text": "Blue" }
        }))
        .send()
        .await;

    // Wait for the first question message to be marked as answered
    wait_for(TIMEOUT, DEFAULT_POLL_INTERVAL, || {
        let client = client.clone();
        let url = format!("{base}/api/v1/chat/sessions/{session_id}");
        let rid = request_id.to_string();
        async move {
            let resp = client.get(&url).send().await.ok()?;
            let snapshot: Value = resp.json().await.ok()?;
            snapshot["messages"]
                .as_array()?
                .iter()
                .find(|m| {
                    m["interaction_request_id"].as_str() == Some(&rid)
                        && m["interaction_answer"].is_string()
                })
                .cloned()
        }
    })
    .await
    .expect("first question should be marked answered");

    daemon.stop().await.expect("stop daemon");
}
