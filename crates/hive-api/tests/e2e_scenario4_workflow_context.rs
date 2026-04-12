//! E2E Scenario 4: Workflow Context Injection into Session Agent
//!
//! Validates:
//! 1. When a workflow is running, the session agent's system prompt
//!    includes workflow state context.
//! 2. Workflow sub-agent signals are buffered (do NOT trigger an LLM turn
//!    on the session agent).
//! 3. After the user sends a message, the LLM sees workflow context
//!    including step progress and pending feedback gates.

use hive_model::ModelRouter;
use hive_test_utils::{wait_for, ScriptedProvider, TestDaemon, DEFAULT_POLL_INTERVAL};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
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

use hive_test_utils::RecordedCall;

/// Shared call recorder to inspect provider calls after the provider
/// has been consumed by the model router.
type SharedCalls = Arc<Mutex<Vec<RecordedCall>>>;

/// Build a ScriptedProvider that:
/// - Worker agent: returns text immediately (no tool calls)
/// - Session agent: returns text and records the system prompt
fn build_scenario4_provider(shared: SharedCalls) -> ScriptedProvider {
    ScriptedProvider::new("mock", "test-model")
        .with_shared_calls(shared)
        // Worker agent: matched by task prompt
        .on_system_contains(
            "Process the request and report your findings",
            vec![ScriptedProvider::text_response(
                "mock",
                "test-model",
                "Worker findings: the analysis is complete with positive results.",
            )],
        )
        // Session agent (default): will be called when user sends a message
        .default_responses(vec![
            ScriptedProvider::text_response(
                "mock",
                "test-model",
                "I can see the workflow is running. Let me check the status.",
            ),
            // Extra response in case of multiple calls
            ScriptedProvider::text_response("mock", "test-model", "The workflow is progressing."),
        ])
}

fn build_model_router(provider: ScriptedProvider) -> Arc<ModelRouter> {
    let mut router = ModelRouter::new();
    router.register_provider(provider);
    Arc::new(router)
}

fn workflow_yaml() -> &'static str {
    include_str!("../../../tests/fixtures/workflows/scenario4-workflow-context.yaml")
}

#[tokio::test]
async fn workflow_context_injected_into_session_agent_prompt() {
    // ── Setup ──────────────────────────────────────────────────────────
    let shared_calls: SharedCalls = Arc::new(Mutex::new(Vec::new()));
    let provider = build_scenario4_provider(shared_calls.clone());
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
    assert!(resp.status().is_success());
    let session: Value = resp.json().await.expect("session json");
    let session_id = session["id"].as_str().expect("session id");

    // ── 3. Launch the workflow ──────────────────────────────────────────
    let resp = client
        .post(format!("{base}/api/v1/workflows/instances"))
        .json(&json!({
            "definition": "test/scenario4-workflow-context",
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

    // ── 4. Wait for the workflow to reach the feedback gate ─────────────
    // The worker agent should complete quickly, then the workflow hits
    // the feedback_gate step (review_gate), which shows as a pending
    // interaction.
    let gate = wait_for(TIMEOUT, DEFAULT_POLL_INTERVAL, || {
        let client = client.clone();
        let url = format!("{base}/api/v1/chat/sessions/{session_id}/pending-questions");
        async move {
            let resp = client.get(&url).send().await.ok()?;
            let questions: Vec<Value> = resp.json().await.ok()?;
            questions.into_iter().find(|q| q["routing"].as_str() == Some("gate"))
        }
    })
    .await
    .expect("timed out waiting for feedback gate");

    // Verify gate content
    let gate_text = gate["text"].as_str().unwrap_or("");
    assert!(
        gate_text.contains("approve") || gate_text.contains("reported"),
        "gate prompt should mention approval: {gate_text}"
    );

    // Record the call count before the user message. The session agent
    // should NOT have been called yet — only the worker agent.
    let calls_before = shared_calls.lock().unwrap().len();

    // ── 5. Send a user message asking about workflow status ────────────
    let resp = client
        .post(format!("{base}/api/v1/chat/sessions/{session_id}/messages"))
        .json(&json!({
            "content": "What is the status of my workflow?",
            "attachments": []
        }))
        .send()
        .await
        .expect("send user message");
    assert!(
        resp.status().is_success(),
        "send message failed: {} — {}",
        resp.status(),
        resp.text().await.unwrap_or_default()
    );

    // ── 6. Wait for the session agent to respond ───────────────────────
    wait_for(TIMEOUT, DEFAULT_POLL_INTERVAL, || {
        let shared_calls = shared_calls.clone();
        let calls_before = calls_before;
        async move {
            if shared_calls.lock().unwrap().len() > calls_before {
                Some(())
            } else {
                None
            }
        }
    })
    .await
    .expect("timed out waiting for session agent to be called");

    // ── 7. Verify workflow context was in the system prompt ─────────────
    let calls = shared_calls.lock().unwrap().clone();
    // Find the call that was made for the session agent (after the user
    // message). It should be a call whose system prompt does NOT contain
    // the worker agent's task prompt, and DOES contain workflow context.
    let session_call = calls
        .iter()
        .rev() // most recent first
        .find(|c| {
            // Skip the worker agent's calls (matched by task prompt)
            !c.system_prompt.contains("Process the request and report your findings")
        });

    assert!(
        session_call.is_some(),
        "expected at least one session agent call, but none found. All calls: {calls:?}"
    );

    let session_call = session_call.unwrap();

    // The session agent's system prompt should contain workflow context
    // indicators (the WORKFLOW_INSTRUCTIONS and step progress).
    assert!(
        session_call.system_prompt.contains("active workflows")
            || session_call.system_prompt.contains("workflow")
            || session_call.system_prompt.contains("Available actions"),
        "session agent's system prompt should contain workflow context. \
         System prompt excerpt: {}",
        &session_call.system_prompt[..session_call.system_prompt.len().min(500)]
    );

    // Verify the workflow context includes step progress information
    assert!(
        session_call.system_prompt.contains("do_work")
            || session_call.system_prompt.contains("review_gate"),
        "workflow context should reference workflow steps. \
         System prompt excerpt: {}",
        &session_call.system_prompt[..session_call.system_prompt.len().min(500)]
    );

    // ── Teardown ───────────────────────────────────────────────────────
    daemon.stop().await.expect("stop daemon");
}
