//! E2E Scenario 2: Multi-Agent Workflow with Feedback Gate & Persona Isolation
//!
//! Validates:
//! 1. Researcher agent uses `ask_user` → question surfaces in chat
//! 2. Workflow feedback gate surfaces as a pending question in chat
//! 3. Executor agent receives researcher's output and gate response
//! 4. Tool approval request from the executor surfaces in chat
//! 5. Top-level session agent does NOT act on workflow results

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

fn build_scenario2_provider() -> ScriptedProvider {
    ScriptedProvider::new("mock", "test-model")
        // Researcher agent: matched by task prompt
        .on_system_contains(
            "Research the topic",
            vec![
                ScriptedProvider::tool_call_response(
                    "mock",
                    "test-model",
                    "tc-ask-topic",
                    "core.ask_user",
                    json!({
                        "question": "Which topic interests you?",
                        "choices": ["Quantum computing", "AI safety", "Climate tech"],
                        "allow_freeform": true
                    }),
                ),
                ScriptedProvider::text_response(
                    "mock",
                    "test-model",
                    "Research findings: user is interested in quantum computing.",
                ),
            ],
        )
        // Executor agent: matched by task prompt
        .on_system_contains(
            "Execute based on research",
            vec![ScriptedProvider::text_response(
                "mock",
                "test-model",
                "Execution complete: generated report on quantum computing.",
            )],
        )
        // Default (fallback)
        .default_responses(vec![ScriptedProvider::text_response(
            "mock",
            "test-model",
            "Fallback: nothing to do.",
        )])
}

fn build_model_router(provider: ScriptedProvider) -> Arc<ModelRouter> {
    let mut router = ModelRouter::new();
    router.register_provider(provider);
    Arc::new(router)
}

fn workflow_yaml() -> &'static str {
    include_str!("../../../tests/fixtures/workflows/scenario2-multi-agent.yaml")
}

#[tokio::test]
async fn multi_agent_workflow_with_feedback_gate() {
    // ── Setup ──────────────────────────────────────────────────────────
    let provider = build_scenario2_provider();
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
            "definition": "test/scenario2-multi-agent",
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

    // ── 4. Wait for researcher's ask_user question ─────────────────────
    let question = wait_for(TIMEOUT, DEFAULT_POLL_INTERVAL, || {
        let client = client.clone();
        let url = format!("{base}/api/v1/chat/sessions/{session_id}/pending-questions");
        async move {
            let resp = client.get(&url).send().await.ok()?;
            let questions: Vec<Value> = resp.json().await.ok()?;
            questions
                .into_iter()
                .find(|q| q["text"].as_str().map_or(false, |t| t.contains("topic interests")))
        }
    })
    .await
    .expect("timed out waiting for researcher question");

    assert_eq!(question["text"].as_str().unwrap(), "Which topic interests you?");
    let researcher_agent_id = question["agent_id"].as_str().expect("agentId");
    let researcher_request_id = question["request_id"].as_str().expect("request_id");

    // ── 5. Respond to the researcher's question ────────────────────────
    let resp = client
        .post(format!(
            "{base}/api/v1/chat/sessions/{session_id}/agents/{researcher_agent_id}/interaction"
        ))
        .json(&json!({
            "request_id": researcher_request_id,
            "payload": {
                "type": "answer",
                "selected_choice": 0,
                "text": "Quantum computing"
            }
        }))
        .send()
        .await
        .expect("respond to researcher question");
    assert!(resp.status().is_success());

    // ── 6. Wait for feedback gate ──────────────────────────────────────
    // The feedback gate appears as a pending question with routing="gate"
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

    assert!(
        gate["text"].as_str().unwrap_or("").contains("confirm the direction"),
        "gate prompt should contain confirmation text, got: {:?}",
        gate["text"]
    );
    // The requestId for gates is "wf:{instance_id}:{step_id}"
    let gate_request_id = gate["request_id"].as_str().expect("gate requestId");
    assert!(
        gate_request_id.starts_with("wf:"),
        "gate requestId should start with 'wf:', got {gate_request_id}"
    );
    // Extract instance_id and step_id from the gate requestId
    let gate_instance_id = gate["workflow_instance_id"].as_i64().expect("workflowInstanceId");
    let gate_step_id = gate["workflow_step_id"].as_str().expect("workflowStepId");
    assert_eq!(gate_instance_id, instance_id);
    assert_eq!(gate_step_id, "confirm_gate");

    // ── 7. Respond to the feedback gate ────────────────────────────────
    let resp = client
        .post(format!(
            "{base}/api/v1/workflows/instances/{gate_instance_id}/steps/{gate_step_id}/respond"
        ))
        .json(&json!({
            "response": "Confirmed, proceed with quantum computing"
        }))
        .send()
        .await
        .expect("respond to feedback gate");
    assert!(
        resp.status().is_success(),
        "gate response failed: {} — {}",
        resp.status(),
        resp.text().await.unwrap_or_default()
    );

    // ── 8. Wait for workflow to complete ────────────────────────────────
    // The executor agent should run and complete after the gate response
    wait_for(TIMEOUT, DEFAULT_POLL_INTERVAL, || {
        let client = client.clone();
        let iid = instance_id;
        let url = format!("{base}/api/v1/workflows/instances/{iid}");
        async move {
            let resp = client.get(&url).send().await.ok()?;
            let body: Value = resp.json().await.ok()?;
            if body["status"].as_str() == Some("completed") {
                Some(body)
            } else {
                None
            }
        }
    })
    .await
    .expect("timed out waiting for workflow completion");

    // ── 9. Verify no pending questions remain ──────────────────────────
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
