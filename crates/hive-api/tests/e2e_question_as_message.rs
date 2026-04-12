//! E2E: Question from workflow-spawned agent appears as a ChatMessage
//!
//! Validates the core of the "questions as messages" architecture:
//! when a workflow agent calls `ask_user`, the session snapshot contains
//! a ChatMessage with `interaction_request_id`, `interaction_kind: "question"`,
//! and `status: "processing"`.

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

/// Test 1: A question from a workflow agent creates a ChatMessage in the session snapshot.
#[tokio::test]
async fn question_creates_message_in_snapshot() {
    let provider = build_questioner_provider();
    let mut router = ModelRouter::new();
    router.register_provider(provider);
    let router = Arc::new(router);

    let daemon =
        TestDaemon::builder().with_model_router(router).spawn().await.expect("test daemon");
    let client = build_client();
    let base = &daemon.base_url;

    // 1. Save workflow definition
    let resp = client
        .post(format!("{base}/api/v1/workflows/definitions"))
        .json(&json!({ "yaml": workflow_yaml() }))
        .send()
        .await
        .expect("save definition");
    assert!(resp.status().is_success(), "save def: {}", resp.status());

    // 2. Create chat session
    let resp =
        client.post(format!("{base}/api/v1/chat/sessions")).send().await.expect("create session");
    assert!(resp.status().is_success());
    let session: Value = resp.json().await.expect("session json");
    let session_id = session["id"].as_str().expect("session id").to_string();

    // 3. Launch workflow
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

    // 4. Wait for the question to appear via REST pending-questions
    //    (proves the gate was created)
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
    .expect("question should appear in pending-questions");

    let request_id = question["request_id"].as_str().expect("request_id");

    // 5. Wait for the question message to appear in the session snapshot.
    //    The message insertion is async (supervisor bridge), so we poll.
    let question_msg = wait_for(TIMEOUT, DEFAULT_POLL_INTERVAL, || {
        let client = client.clone();
        let url = format!("{base}/api/v1/chat/sessions/{session_id}");
        let rid = request_id.to_string();
        async move {
            let resp = client.get(&url).send().await.ok()?;
            let snapshot: Value = resp.json().await.ok()?;
            let messages = snapshot["messages"].as_array()?;
            messages.iter().find(|m| m["interaction_request_id"].as_str() == Some(&rid)).cloned()
        }
    })
    .await
    .expect(
        "Session snapshot should contain a ChatMessage with the question's interaction_request_id",
    );

    assert_eq!(question_msg["role"].as_str(), Some("notification"), "question message role");
    assert_eq!(
        question_msg["status"].as_str(),
        Some("processing"),
        "question message status (unanswered)"
    );
    assert_eq!(question_msg["interaction_kind"].as_str(), Some("question"), "interaction_kind");
    assert!(
        question_msg["content"].as_str().unwrap().contains("favorite color"),
        "content should contain question text"
    );

    // Verify interaction_meta has the expected fields
    let meta = &question_msg["interaction_meta"];
    assert!(meta.is_object(), "interaction_meta should be an object");
    let choices = meta["choices"].as_array().expect("choices in meta");
    assert_eq!(choices.len(), 3, "should have 3 choices");
    assert_eq!(meta["allow_freeform"].as_bool(), Some(false));

    // interaction_answer should be absent (unanswered)
    assert!(
        question_msg["interaction_answer"].is_null(),
        "interaction_answer should be null for unanswered question"
    );

    // Cleanup: answer the question and stop
    let agent_id = question["agent_id"].as_str().unwrap();
    let _ = client
        .post(format!("{base}/api/v1/chat/sessions/{session_id}/agents/{agent_id}/interaction"))
        .json(&json!({
            "request_id": request_id,
            "payload": { "type": "answer", "selected_choice": 1, "text": "Blue" }
        }))
        .send()
        .await;

    daemon.stop().await.expect("stop daemon");
}

/// Test 2: After answering, the question message has interaction_answer set and status is complete.
#[tokio::test]
async fn answered_question_updates_message() {
    let provider = build_questioner_provider();
    let mut router = ModelRouter::new();
    router.register_provider(provider);
    let router = Arc::new(router);

    let daemon =
        TestDaemon::builder().with_model_router(router).spawn().await.expect("test daemon");
    let client = build_client();
    let base = &daemon.base_url;

    // Setup: save workflow + create session + launch workflow
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

    // Wait for question
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

    let request_id = question["request_id"].as_str().unwrap();
    let agent_id = question["agent_id"].as_str().unwrap();

    // Answer the question
    let resp = client
        .post(format!("{base}/api/v1/chat/sessions/{session_id}/agents/{agent_id}/interaction"))
        .json(&json!({
            "request_id": request_id,
            "payload": { "type": "answer", "selected_choice": 1, "text": "Blue" }
        }))
        .send()
        .await
        .expect("answer question");
    let answer_status = resp.status();
    let answer_body: Value = resp.json().await.expect("answer json body");
    assert!(answer_status.is_success(), "answer response: {answer_status}");
    assert_eq!(
        answer_body["acknowledged"].as_bool(),
        Some(true),
        "gate should acknowledge the answer. Full body: {answer_body}"
    );

    // Wait for the message to be updated with the answer
    let answered_msg = wait_for(TIMEOUT, DEFAULT_POLL_INTERVAL, || {
        let client = client.clone();
        let url = format!("{base}/api/v1/chat/sessions/{session_id}");
        let rid = request_id.to_string();
        async move {
            let resp = client.get(&url).send().await.ok()?;
            let snapshot: Value = resp.json().await.ok()?;
            let messages = snapshot["messages"].as_array()?;
            messages
                .iter()
                .find(|m| {
                    m["interaction_request_id"].as_str() == Some(&rid)
                        && m["interaction_answer"].is_string()
                })
                .cloned()
        }
    })
    .await
    .expect("question message should be updated with interaction_answer after answering");

    assert_eq!(
        answered_msg["status"].as_str(),
        Some("complete"),
        "status should be 'complete' after answering"
    );
    assert!(
        answered_msg["interaction_answer"].as_str().unwrap().contains("Blue"),
        "interaction_answer should contain the user's answer text"
    );

    daemon.stop().await.expect("stop daemon");
}
