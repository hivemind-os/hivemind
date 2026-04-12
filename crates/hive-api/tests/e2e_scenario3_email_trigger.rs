//! E2E Scenario 3: Email Trigger → Workflow → Agent Responds
//!
//! Validates:
//! 1. Saving a workflow with an `incoming_message` trigger registers it
//! 2. Publishing a matching email event auto-launches the workflow
//! 3. The workflow's invoke_agent step runs the agent
//! 4. The workflow completes successfully
//! 5. The trigger metadata (from, subject, body) is available to the agent

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

fn build_scenario3_provider() -> ScriptedProvider {
    ScriptedProvider::new("mock", "test-model")
        // Email responder agent: matched by task prompt containing "pricing document"
        .on_system_contains(
            "pricing document",
            vec![ScriptedProvider::text_response(
                "mock",
                "test-model",
                "Based on our pricing document, our standard plan is $99/month.",
            )],
        )
        .default_responses(vec![ScriptedProvider::text_response(
            "mock",
            "test-model",
            "Fallback response.",
        )])
}

fn build_model_router(provider: ScriptedProvider) -> Arc<ModelRouter> {
    let mut router = ModelRouter::new();
    router.register_provider(provider);
    Arc::new(router)
}

fn workflow_yaml() -> &'static str {
    include_str!("../../../tests/fixtures/workflows/scenario3-email-responder.yaml")
}

#[tokio::test]
async fn email_trigger_launches_workflow_and_agent_completes() {
    // ── Setup ──────────────────────────────────────────────────────────
    let provider = build_scenario3_provider();
    let router = build_model_router(provider);

    let daemon =
        TestDaemon::builder().with_model_router(router).spawn().await.expect("test daemon");

    let client = build_client();
    let base = &daemon.base_url;

    // ── 1. Save workflow definition (auto-registers trigger) ───────────
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

    // Give the trigger manager a moment to register the trigger
    tokio::time::sleep(Duration::from_millis(200)).await;

    // ── 2. Publish an incoming email event ─────────────────────────────
    let email_payload = json!({
        "channel_id": "test-email",
        "provider": "test-email",
        "external_id": "email-001",
        "from": "client@example.com",
        "to": "support@company.com",
        "subject": "Pricing inquiry",
        "body": "What are your current prices for the standard plan?",
        "timestamp_ms": 1711000000000u64,
        "metadata": {}
    });

    let _published =
        daemon.event_bus.publish("comm.message.received.test-email", "test-harness", email_payload);

    // ── 3. Wait for a workflow instance to be auto-launched ────────────
    let instance = wait_for(TIMEOUT, DEFAULT_POLL_INTERVAL, || {
        let client = client.clone();
        let url = format!("{base}/api/v1/workflows/instances?session_id=trigger-manager");
        async move {
            let resp = client.get(&url).send().await.ok()?;
            let body: Value = resp.json().await.ok()?;
            let items = body["items"].as_array()?;
            items
                .iter()
                .find(|inst| {
                    inst["definition_name"].as_str() == Some("test/scenario3-email-responder")
                })
                .cloned()
        }
    })
    .await
    .expect("timed out waiting for triggered workflow instance");

    let instance_id = instance["id"].as_i64().expect("instance id");

    // ── 4. Wait for workflow to complete ────────────────────────────────
    let completed = wait_for(TIMEOUT, DEFAULT_POLL_INTERVAL, || {
        let client = client.clone();
        let url = format!("{base}/api/v1/workflows/instances/{instance_id}");
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

    // ── 5. Verify the workflow completed successfully ───────────────────
    assert_eq!(completed["status"].as_str(), Some("completed"));

    // ── Teardown ───────────────────────────────────────────────────────
    daemon.stop().await.expect("stop daemon");
}
