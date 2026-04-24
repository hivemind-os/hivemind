//! Integration tests for multi-agent collaboration via message passing.
//!
//! These tests use a `ScriptProvider` (programmable mock LLM) that returns
//! pre-scripted sequences of responses — some containing `core.signal_agent`
//! tool calls — to simulate realistic agent-to-agent collaboration.

#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

use parking_lot::Mutex;

use arc_swap::ArcSwap;
use hive_agents::{
    AgentMessage, AgentRole, AgentSendHandle, AgentSpec, AgentStatus, AgentSupervisor,
    SupervisorEvent,
};
use hive_contracts::SessionPermissions;
use hive_loop::{AgentOrchestrator, BoxFuture, LoopExecutor, ReActStrategy};
use hive_model::{
    Capability, CompletionChunk, CompletionRequest, CompletionResponse, CompletionStream,
    FinishReason, ModelProvider, ModelRouter, ModelSelection, ProviderDescriptor, ProviderKind,
    ToolCallResponse,
};
use hive_tools::ToolRegistry;
use serde_json::json;
use tempfile::tempdir;
use tokio::sync::broadcast;
use tokio::time::{timeout, Duration};

// ── ScriptProvider ──────────────────────────────────────────────────────────
// A programmable mock LLM that returns responses from a FIFO queue.
// Each response can include tool_calls (e.g. core.signal_agent).
// When the queue is empty, returns a default "done" response.

struct ScriptProvider {
    descriptor: ProviderDescriptor,
    responses: Mutex<VecDeque<CompletionResponse>>,
}

impl ScriptProvider {
    fn new(id: &str, model: &str, responses: Vec<CompletionResponse>) -> Self {
        Self {
            descriptor: ProviderDescriptor {
                id: id.to_string(),
                name: Some(id.to_string()),
                kind: ProviderKind::Mock,
                models: vec![model.to_string()],
                model_capabilities: BTreeMap::from([(
                    model.to_string(),
                    BTreeSet::from([Capability::Chat, Capability::ToolUse]),
                )]),
                priority: 10,
                available: true,
            },
            responses: Mutex::new(VecDeque::from(responses)),
        }
    }

    fn make_response(content: &str) -> CompletionResponse {
        CompletionResponse {
            provider_id: "mock".to_string(),
            model: "test-model".to_string(),
            content: content.to_string(),
            tool_calls: vec![],
        }
    }

    fn make_message_call(
        call_id: &str,
        target_agent_id: &str,
        message: &str,
    ) -> CompletionResponse {
        CompletionResponse {
            provider_id: "mock".to_string(),
            model: "test-model".to_string(),
            content: String::new(),
            tool_calls: vec![ToolCallResponse {
                id: call_id.to_string(),
                name: "core.signal_agent".to_string(),
                arguments: json!({
                    "agent_id": target_agent_id,
                    "content": message,
                }),
            }],
        }
    }
}

impl ModelProvider for ScriptProvider {
    fn descriptor(&self) -> &ProviderDescriptor {
        &self.descriptor
    }

    fn complete(
        &self,
        _request: &CompletionRequest,
        selection: &ModelSelection,
    ) -> anyhow::Result<CompletionResponse> {
        let mut queue = self.responses.lock();
        let mut resp = queue.pop_front().unwrap_or_else(|| CompletionResponse {
            provider_id: self.descriptor.id.clone(),
            model: selection.model.clone(),
            content: "Task complete.".to_string(),
            tool_calls: vec![],
        });
        resp.provider_id = self.descriptor.id.clone();
        resp.model = selection.model.clone();
        Ok(resp)
    }

    fn complete_stream(
        &self,
        request: &CompletionRequest,
        selection: &ModelSelection,
    ) -> anyhow::Result<CompletionStream> {
        let response = self.complete(request, selection)?;
        let finish_reason = if response.tool_calls.is_empty() {
            FinishReason::Stop
        } else {
            FinishReason::ToolCalls
        };
        let chunk = CompletionChunk {
            delta: response.content,
            finish_reason: Some(finish_reason),
            tool_calls: response.tool_calls,
            tool_call_arg_deltas: vec![],
        };// ── TestOrchestrator ────────────────────────────────────────────────────────
// Routes messages through the supervisor's send handle, mimicking the real
// chat.rs orchestrator for inter-agent communication in tests.

struct TestOrchestrator {
    send_handle: AgentSendHandle,
    messages: Mutex<Vec<(String, String, String)>>, // (to, message, from)
    feedbacks: Mutex<Vec<(String, String, String)>>, // (to, message, from)
}

impl TestOrchestrator {
    fn new(send_handle: AgentSendHandle) -> Self {
        Self { send_handle, messages: Mutex::new(Vec::new()), feedbacks: Mutex::new(Vec::new()) }
    }
}

impl AgentOrchestrator for TestOrchestrator {
    fn spawn_agent(
        &self,
        _persona: hive_contracts::Persona,
        _task: String,
        _from: Option<String>,
        _friendly_name: Option<String>,
        _data_class: hive_classification::DataClass,
        _parent_model: Option<ModelSelection>,
        _keep_alive: bool,
        _workspace_path: Option<std::path::PathBuf>,
    ) -> BoxFuture<'_, Result<String, String>> {
        Box::pin(async { Err("spawn not supported in test orchestrator".to_string()) })
    }

    fn message_agent(
        &self,
        agent_id: String,
        message: String,
        from: String,
    ) -> BoxFuture<'_, Result<(), String>> {
        self.messages.lock().push((agent_id.clone(), message.clone(), from.clone()));
        let handle = self.send_handle.clone();
        Box::pin(async move {
            handle
                .send_to_agent(&agent_id, AgentMessage::Task { content: message, from: Some(from) })
                .await
                .map_err(|e| e.to_string())
        })
    }

    fn feedback_agent(
        &self,
        agent_id: String,
        message: String,
        from: String,
    ) -> BoxFuture<'_, Result<(), String>> {
        self.feedbacks.lock().push((agent_id.clone(), message.clone(), from.clone()));
        let handle = self.send_handle.clone();
        Box::pin(async move {
            handle
                .send_to_agent(&agent_id, AgentMessage::Feedback { content: message, from })
                .await
                .map_err(|e| e.to_string())
        })
    }

    fn message_session(
        &self,
        _message: String,
        _from_agent_id: String,
    ) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }

    fn list_agents(
        &self,
    ) -> BoxFuture<'_, Result<Vec<(String, String, String, String, Option<String>)>, String>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn get_agent_result(
        &self,
        _agent_id: String,
    ) -> BoxFuture<'_, Result<(String, Option<String>), String>> {
        Box::pin(async { Ok(("Done".to_string(), None)) })
    }

    fn kill_agent(&self, _agent_id: String) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }

    fn get_agent_parent(&self, _agent_id: String) -> BoxFuture<'_, Result<Option<String>, String>> {
        // All test agents are root-level (no parent), making them siblings.
        Box::pin(async { Ok(None) })
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn make_spec(id: &str, name: &str, keep_alive: bool) -> AgentSpec {
    AgentSpec {
        id: id.to_string(),
        name: name.to_string(),
        friendly_name: name.to_string(),
        description: format!("{name} agent"),
        role: AgentRole::Custom("service".to_string()),
        model: Some("mock:test-model".to_string()),
        preferred_models: None,
        loop_strategy: None,
        tool_execution_mode: None,
        system_prompt: format!("You are {name}"),
        allowed_tools: vec!["core.signal_agent".to_string()],
        avatar: None,
        color: None,
        data_class: hive_classification::DataClass::Public,
        keep_alive,
        idle_timeout_secs: None,
        tool_limits: None,
        persona_id: None,
        workflow_managed: false,
                shadow_mode: false,
    }
}

async fn collect_events(
    rx: &mut broadcast::Receiver<SupervisorEvent>,
    max: usize,
    wait_ms: u64,
) -> Vec<SupervisorEvent> {
    let mut events = Vec::new();
    for _ in 0..max {
        match timeout(Duration::from_millis(wait_ms), rx.recv()).await {
            Ok(Ok(ev)) => events.push(ev),
            _ => break,
        }
    }
    events
}

fn count_message_routed(events: &[SupervisorEvent], from: &str, to: &str) -> usize {
    events
        .iter()
        .filter(|e| {
            matches!(e, SupervisorEvent::MessageRouted { from: f, to: t, .. }
                if f == from && t == to)
        })
        .count()
}

fn count_completed(events: &[SupervisorEvent], agent_id: &str) -> usize {
    events
        .iter()
        .filter(
            |e| matches!(e, SupervisorEvent::AgentCompleted { agent_id: id, .. } if id == agent_id),
        )
        .count()
}

fn find_completed_result(events: &[SupervisorEvent], agent_id: &str) -> Option<String> {
    events.iter().find_map(|e| match e {
        SupervisorEvent::AgentCompleted { agent_id: id, result } if id == agent_id => {
            Some(result.clone())
        }
        _ => None,
    })
}

/// Build a supervisor wired with a ScriptProvider and TestOrchestrator.
/// Returns (supervisor, orchestrator, event_receiver).
fn build_test_env(
    provider: ScriptProvider,
) -> (AgentSupervisor, Arc<TestOrchestrator>, broadcast::Receiver<SupervisorEvent>) {
    let workspace = tempdir().unwrap();
    let mut router = ModelRouter::new();
    router.register_provider(provider);

    let tools = Arc::new({
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(hive_tools::SignalAgentTool::default()))
            .expect("register signal_agent");
        reg
    });

    // Create supervisor without orchestrator first
    let mut sup = AgentSupervisor::with_executor(
        512,
        None,
        Arc::new(LoopExecutor::new(Arc::new(ReActStrategy))),
        Arc::new(ArcSwap::from_pointee(router)),
        tools,
        Arc::new(Mutex::new(SessionPermissions::default())),
        Arc::new(Mutex::new(Vec::new())),
        None,
        "test-session".to_string(),
        workspace.path().to_path_buf(),
        None,
        None,
    );

    // Create orchestrator with send_handle, then set it on the supervisor
    let orch = Arc::new(TestOrchestrator::new(sup.send_handle()));
    sup.set_agent_orchestrator(Some(orch.clone() as Arc<dyn AgentOrchestrator>));

    let rx = sup.subscribe();
    (sup, orch, rx)
}

// ═══════════════════════════════════════════════════════════════════════════
// TESTS
// ═══════════════════════════════════════════════════════════════════════════

// ── 1. Simple one-shot: A sends to B, B responds ───────────────────────────

#[tokio::test]
async fn test_01_simple_one_shot_message() {
    // Agent A sends a message to agent B. B processes and completes.
    // No loop — just verifying basic message routing works.
    let provider = ScriptProvider::new(
        "mock",
        "test-model",
        vec![
            // A's first LLM call: send message to B
            ScriptProvider::make_message_call("tc1", "b", "Hello from A"),
            // A's second LLM call (after tool result): done
            ScriptProvider::make_response("A is done"),
            // B's LLM call (processing A's message): respond
            ScriptProvider::make_response("B received hello"),
        ],
    );

    let (sup, orch, mut rx) = build_test_env(provider);

    // Set orchestrator on supervisor
    let spec_a = make_spec("a", "Alice", false);
    let spec_b = make_spec("b", "Bob", false);

    sup.spawn_agent(spec_a, None, None, None, None).await.unwrap();
    sup.spawn_agent(spec_b, None, None, None, None).await.unwrap();

    // Override orchestrator via supervisor's execution context

    sup.send_to_agent(
        "a",
        AgentMessage::Task {
            content: "Send a greeting to Bob".to_string(),
            from: Some("user".to_string()),
        },
    )
    .await
    .unwrap();

    tokio::time::sleep(Duration::from_millis(500)).await;
    let events = collect_events(&mut rx, 200, 300).await;

    // Verify A completed
    assert!(count_completed(&events, "a") > 0, "Agent A should complete");
    // Verify message was routed A→B
    let msgs = orch.messages.lock();
    assert!(
        msgs.iter().any(|(to, _, from)| to == "b" && from == "a"),
        "Expected message from A to B, got: {msgs:?}"
    );

    sup.kill_all().await.unwrap();
}

// ── 2. Bidirectional: A→B, B auto-replies as feedback to A ──────────────────

#[tokio::test]
async fn test_02_bidirectional_feedback() {
    // A sends task to B. B completes task. B's result is auto-replied
    // as Feedback to A (not Task — so no loop).
    let provider = ScriptProvider::new(
        "mock",
        "test-model",
        vec![
            // A: send message to B
            ScriptProvider::make_message_call("tc1", "b", "Process this data"),
            // A: after tool result, done
            ScriptProvider::make_response("A sent the request"),
            // B: processes the task
            ScriptProvider::make_response("Data processed successfully"),
            // A: processes feedback from B (feedback triggers execute_task)
            ScriptProvider::make_response("A received B's feedback"),
        ],
    );

    let (sup, orch, mut rx) = build_test_env(provider);

    sup.spawn_agent(make_spec("a", "Alice", true), None, None, None, None).await.unwrap();
    sup.spawn_agent(make_spec("b", "Bob", false), None, None, None, None).await.unwrap();

    sup.send_to_agent(
        "a",
        AgentMessage::Task {
            content: "Ask Bob to process data".to_string(),
            from: Some("user".to_string()),
        },
    )
    .await
    .unwrap();

    tokio::time::sleep(Duration::from_millis(800)).await;
    let _events = collect_events(&mut rx, 200, 300).await;

    // B should auto-reply feedback to A
    let feedbacks = orch.feedbacks.lock();
    assert!(
        feedbacks.iter().any(|(to, _, from)| to == "a" && from == "b"),
        "Expected feedback from B to A, got: {feedbacks:?}"
    );

    sup.kill_all().await.unwrap();
}

// ── 3. No infinite loop: feedback doesn't trigger auto-reply ────────────────

#[tokio::test]
async fn test_03_no_infinite_loop() {
    // A→B (task), B auto-replies feedback to A, A processes feedback.
    // Crucially: A's processing of feedback should NOT auto-reply back to B.
    let provider = ScriptProvider::new(
        "mock",
        "test-model",
        vec![
            // A: send to B
            ScriptProvider::make_message_call("tc1", "b", "Do work"),
            // A: after tool result
            ScriptProvider::make_response("A done sending"),
            // B: processes task
            ScriptProvider::make_response("B completed work"),
            // A: processes feedback from B
            ScriptProvider::make_response("A acknowledges B's response"),
        ],
    );

    let (sup, orch, mut rx) = build_test_env(provider);

    sup.spawn_agent(make_spec("a", "Alice", true), None, None, None, None).await.unwrap();
    sup.spawn_agent(make_spec("b", "Bob", true), None, None, None, None).await.unwrap();

    sup.send_to_agent(
        "a",
        AgentMessage::Task { content: "Message Bob".to_string(), from: Some("user".to_string()) },
    )
    .await
    .unwrap();

    tokio::time::sleep(Duration::from_millis(800)).await;
    let _events = collect_events(&mut rx, 200, 300).await;

    // Should have exactly 1 task message (A→B) and 1 feedback (B→A)
    let msgs = orch.messages.lock();
    let a_to_b = msgs.iter().filter(|(to, _, from)| to == "b" && from == "a").count();
    assert_eq!(a_to_b, 1, "Expected exactly 1 message A→B, got {a_to_b}");

    let feedbacks = orch.feedbacks.lock();
    let b_to_a = feedbacks.iter().filter(|(to, _, from)| to == "a" && from == "b").count();
    assert_eq!(b_to_a, 1, "Expected exactly 1 feedback B→A, got {b_to_a}");

    // No feedback from A back to B (would indicate a loop)
    let a_to_b_feedback = feedbacks.iter().filter(|(to, _, from)| to == "b" && from == "a").count();
    assert_eq!(a_to_b_feedback, 0, "A should NOT feedback back to B");

    sup.kill_all().await.unwrap();
}

// ── 4. Service agent chain: A→B→C ──────────────────────────────────────────

#[tokio::test]
async fn test_04_three_agent_chain() {
    // A messages B, B messages C, C completes.
    let provider = ScriptProvider::new(
        "mock",
        "test-model",
        vec![
            // A: message B
            ScriptProvider::make_message_call("tc1", "b", "Forward to C"),
            ScriptProvider::make_response("A forwarded"),
            // B: receives from A, messages C
            ScriptProvider::make_message_call("tc2", "c", "Hello from chain"),
            ScriptProvider::make_response("B forwarded to C"),
            // C: processes message
            ScriptProvider::make_response("C received chain message"),
            // B: processes feedback from C
            ScriptProvider::make_response("B got C's feedback"),
            // A: processes feedback from B
            ScriptProvider::make_response("A got B's feedback"),
        ],
    );

    let (sup, orch, mut rx) = build_test_env(provider);

    sup.spawn_agent(make_spec("a", "Alice", true), None, None, None, None).await.unwrap();
    sup.spawn_agent(make_spec("b", "Bob", true), None, None, None, None).await.unwrap();
    sup.spawn_agent(make_spec("c", "Carol", true), None, None, None, None).await.unwrap();

    sup.send_to_agent(
        "a",
        AgentMessage::Task {
            content: "Start the chain".to_string(),
            from: Some("user".to_string()),
        },
    )
    .await
    .unwrap();

    tokio::time::sleep(Duration::from_millis(1200)).await;
    let _events = collect_events(&mut rx, 300, 300).await;

    let msgs = orch.messages.lock();
    // Expect A→B and B→C messages
    assert!(msgs.iter().any(|(to, _, from)| to == "b" && from == "a"), "Expected message A→B");
    assert!(msgs.iter().any(|(to, _, from)| to == "c" && from == "b"), "Expected message B→C");

    sup.kill_all().await.unwrap();
}

// ── 5. Keep-alive agent processes multiple tasks ────────────────────────────

#[tokio::test]
async fn test_05_keep_alive_multiple_tasks() {
    let provider = ScriptProvider::new(
        "mock",
        "test-model",
        vec![
            ScriptProvider::make_response("Processed task 1"),
            ScriptProvider::make_response("Processed task 2"),
            ScriptProvider::make_response("Processed task 3"),
        ],
    );

    let (sup, _orch, mut rx) = build_test_env(provider);

    sup.spawn_agent(make_spec("a", "Alice", true), None, None, None, None).await.unwrap();

    for i in 1..=3 {
        sup.send_to_agent(
            "a",
            AgentMessage::Task { content: format!("Task {i}"), from: Some("user".to_string()) },
        )
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
    }

    tokio::time::sleep(Duration::from_millis(300)).await;
    let events = collect_events(&mut rx, 300, 300).await;

    let completions = count_completed(&events, "a");
    assert!(completions >= 3, "Expected 3 completions, got {completions}");

    // Agent should still be alive (keep_alive)
    let status = sup.get_agent_status("a");
    assert!(status.is_some(), "Agent A should still exist");

    sup.kill_all().await.unwrap();
}

// ── 6. One-shot agent terminates after task ─────────────────────────────────

#[tokio::test]
async fn test_06_one_shot_terminates() {
    let provider = ScriptProvider::new(
        "mock",
        "test-model",
        vec![ScriptProvider::make_response("Done with my task")],
    );

    let (sup, _orch, mut rx) = build_test_env(provider);

    sup.spawn_agent(make_spec("a", "Alice", false), None, None, None, None).await.unwrap();

    sup.send_to_agent(
        "a",
        AgentMessage::Task { content: "One-shot task".to_string(), from: Some("user".to_string()) },
    )
    .await
    .unwrap();

    tokio::time::sleep(Duration::from_millis(500)).await;
    let events = collect_events(&mut rx, 100, 300).await;

    assert!(count_completed(&events, "a") > 0, "Agent A should complete");

    // Agent should transition to Done status
    let has_done = events.iter().any(|e| {
        matches!(e,
            SupervisorEvent::AgentStatusChanged { agent_id, status: AgentStatus::Done }
            if agent_id == "a"
        )
    });
    assert!(has_done, "One-shot agent should reach Done status");

    sup.kill_all().await.unwrap();
}

// ── 7. Parallel messages: A→B and A→C simultaneously ────────────────────────

#[tokio::test]
async fn test_07_parallel_fan_out() {
    // A sends messages to both B and C in the same LLM response (multi-tool-call)
    let provider = ScriptProvider::new(
        "mock",
        "test-model",
        vec![
            // A: two tool calls in one response
            CompletionResponse {
                provider_id: "mock".to_string(),
                model: "test-model".to_string(),
                content: String::new(),
                tool_calls: vec![
                    ToolCallResponse {
                        id: "tc1".to_string(),
                        name: "core.signal_agent".to_string(),
                        arguments: json!({"agent_id": "b", "content": "Task for B"}),
                    },
                    ToolCallResponse {
                        id: "tc2".to_string(),
                        name: "core.signal_agent".to_string(),
                        arguments: json!({"agent_id": "c", "content": "Task for C"}),
                    },
                ],
            },
            // A: after tool results
            ScriptProvider::make_response("A dispatched to both"),
            // B processes
            ScriptProvider::make_response("B done"),
            // C processes
            ScriptProvider::make_response("C done"),
            // A: processes feedbacks
            ScriptProvider::make_response("A got B feedback"),
            ScriptProvider::make_response("A got C feedback"),
        ],
    );

    let (sup, orch, mut rx) = build_test_env(provider);

    sup.spawn_agent(make_spec("a", "Alice", true), None, None, None, None).await.unwrap();
    sup.spawn_agent(make_spec("b", "Bob", true), None, None, None, None).await.unwrap();
    sup.spawn_agent(make_spec("c", "Carol", true), None, None, None, None).await.unwrap();

    sup.send_to_agent(
        "a",
        AgentMessage::Task {
            content: "Fan out to B and C".to_string(),
            from: Some("user".to_string()),
        },
    )
    .await
    .unwrap();

    tokio::time::sleep(Duration::from_millis(1000)).await;
    let _events = collect_events(&mut rx, 300, 300).await;

    let msgs = orch.messages.lock();
    assert!(msgs.iter().any(|(to, _, from)| to == "b" && from == "a"), "Expected A→B message");
    assert!(msgs.iter().any(|(to, _, from)| to == "c" && from == "a"), "Expected A→C message");

    sup.kill_all().await.unwrap();
}

// ── 8. Message to nonexistent agent returns error ───────────────────────────

#[tokio::test]
async fn test_08_message_nonexistent_agent() {
    // A tries to message "ghost" which doesn't exist.
    // The orchestrator should return an error, which becomes the tool result.
    let provider = ScriptProvider::new(
        "mock",
        "test-model",
        vec![
            ScriptProvider::make_message_call("tc1", "ghost", "Are you there?"),
            ScriptProvider::make_response("Got error, done"),
        ],
    );

    let (sup, _orch, mut rx) = build_test_env(provider);

    sup.spawn_agent(make_spec("a", "Alice", false), None, None, None, None).await.unwrap();

    sup.send_to_agent(
        "a",
        AgentMessage::Task {
            content: "Message ghost agent".to_string(),
            from: Some("user".to_string()),
        },
    )
    .await
    .unwrap();

    tokio::time::sleep(Duration::from_millis(500)).await;
    let events = collect_events(&mut rx, 100, 300).await;

    // A should still complete (error is returned as tool result, not a crash)
    assert!(count_completed(&events, "a") > 0, "Agent A should complete despite error");

    sup.kill_all().await.unwrap();
}

// ── 9. Feedback message includes correct sender ─────────────────────────────

#[tokio::test]
async fn test_09_feedback_preserves_sender() {
    let provider = ScriptProvider::new(
        "mock",
        "test-model",
        vec![
            ScriptProvider::make_message_call("tc1", "b", "Check sender"),
            ScriptProvider::make_response("A done"),
            ScriptProvider::make_response("B done"),
            ScriptProvider::make_response("A got feedback"),
        ],
    );

    let (sup, orch, _rx) = build_test_env(provider);

    sup.spawn_agent(make_spec("a", "Alice", true), None, None, None, None).await.unwrap();
    sup.spawn_agent(make_spec("b", "Bob", false), None, None, None, None).await.unwrap();

    sup.send_to_agent(
        "a",
        AgentMessage::Task { content: "Test sender".to_string(), from: Some("user".to_string()) },
    )
    .await
    .unwrap();

    tokio::time::sleep(Duration::from_millis(600)).await;

    let feedbacks = orch.feedbacks.lock();
    for (to, _msg, from) in feedbacks.iter() {
        if to == "a" {
            assert_eq!(from, "b", "Feedback to A should be from B");
        }
    }

    sup.kill_all().await.unwrap();
}

// ── 10. Telemetry tracks LLM calls per agent ────────────────────────────────

#[tokio::test]
async fn test_10_telemetry_per_agent() {
    let provider = ScriptProvider::new(
        "mock",
        "test-model",
        vec![
            ScriptProvider::make_response("A response 1"),
            ScriptProvider::make_response("B response 1"),
        ],
    );

    let (sup, _orch, _rx) = build_test_env(provider);

    sup.spawn_agent(make_spec("a", "Alice", false), None, None, None, None).await.unwrap();
    sup.spawn_agent(make_spec("b", "Bob", false), None, None, None, None).await.unwrap();

    sup.send_to_agent(
        "a",
        AgentMessage::Task { content: "Task A".to_string(), from: Some("user".to_string()) },
    )
    .await
    .unwrap();
    sup.send_to_agent(
        "b",
        AgentMessage::Task { content: "Task B".to_string(), from: Some("user".to_string()) },
    )
    .await
    .unwrap();

    tokio::time::sleep(Duration::from_millis(500)).await;

    let snapshot = sup.telemetry_snapshot();
    // Both agents should have telemetry recorded
    assert!(snapshot.total.model_calls > 0, "Expected model calls tracked");

    sup.kill_all().await.unwrap();
}

// ── 11. Multiple sequential messages between same pair ──────────────────────

#[tokio::test]
async fn test_11_sequential_messages_same_pair() {
    // A sends two separate messages to B (sequential, not looping)
    let provider = ScriptProvider::new(
        "mock",
        "test-model",
        vec![
            // A first task: message B
            ScriptProvider::make_message_call("tc1", "b", "First message"),
            ScriptProvider::make_response("A sent first"),
            // B processes first
            ScriptProvider::make_response("B got first"),
            // A processes feedback, then sends second message
            ScriptProvider::make_message_call("tc2", "b", "Second message"),
            ScriptProvider::make_response("A sent second"),
            // B processes second
            ScriptProvider::make_response("B got second"),
            // A processes second feedback
            ScriptProvider::make_response("A done"),
        ],
    );

    let (sup, orch, _rx) = build_test_env(provider);

    sup.spawn_agent(make_spec("a", "Alice", true), None, None, None, None).await.unwrap();
    sup.spawn_agent(make_spec("b", "Bob", true), None, None, None, None).await.unwrap();

    sup.send_to_agent(
        "a",
        AgentMessage::Task {
            content: "Send two messages to Bob".to_string(),
            from: Some("user".to_string()),
        },
    )
    .await
    .unwrap();

    tokio::time::sleep(Duration::from_millis(1500)).await;

    let msgs = orch.messages.lock();
    let a_to_b = msgs.iter().filter(|(to, _, from)| to == "b" && from == "a").count();
    assert!(a_to_b >= 1, "Expected at least 1 message A→B, got {a_to_b}");

    sup.kill_all().await.unwrap();
}

// ── 12. Diamond pattern: A→B, A→C, B→D, C→D ────────────────────────────────

#[tokio::test]
async fn test_12_diamond_pattern() {
    let provider = ScriptProvider::new(
        "mock",
        "test-model",
        vec![
            // A: fan out to B and C
            CompletionResponse {
                provider_id: "mock".to_string(),
                model: "test-model".to_string(),
                content: String::new(),
                tool_calls: vec![
                    ToolCallResponse {
                        id: "tc1".to_string(),
                        name: "core.signal_agent".to_string(),
                        arguments: json!({"agent_id": "b", "content": "B's part"}),
                    },
                    ToolCallResponse {
                        id: "tc2".to_string(),
                        name: "core.signal_agent".to_string(),
                        arguments: json!({"agent_id": "c", "content": "C's part"}),
                    },
                ],
            },
            ScriptProvider::make_response("A dispatched"),
            // B: forward to D
            ScriptProvider::make_message_call("tc3", "d", "From B via diamond"),
            ScriptProvider::make_response("B forwarded"),
            // C: forward to D
            ScriptProvider::make_message_call("tc4", "d", "From C via diamond"),
            ScriptProvider::make_response("C forwarded"),
            // D processes B's message
            ScriptProvider::make_response("D got from B"),
            // D processes C's message
            ScriptProvider::make_response("D got from C"),
            // B gets feedback from D
            ScriptProvider::make_response("B got D feedback"),
            // C gets feedback from D
            ScriptProvider::make_response("C got D feedback"),
            // A gets feedbacks
            ScriptProvider::make_response("A got B feedback"),
            ScriptProvider::make_response("A got C feedback"),
        ],
    );

    let (sup, orch, _rx) = build_test_env(provider);

    for (id, name) in [("a", "Alice"), ("b", "Bob"), ("c", "Carol"), ("d", "Dave")] {
        sup.spawn_agent(make_spec(id, name, true), None, None, None, None).await.unwrap();
    }

    sup.send_to_agent(
        "a",
        AgentMessage::Task {
            content: "Diamond dispatch".to_string(),
            from: Some("user".to_string()),
        },
    )
    .await
    .unwrap();

    tokio::time::sleep(Duration::from_millis(2000)).await;

    let msgs = orch.messages.lock();
    assert!(msgs.iter().any(|(to, _, from)| to == "b" && from == "a"), "Expected A→B");
    assert!(msgs.iter().any(|(to, _, from)| to == "c" && from == "a"), "Expected A→C");

    sup.kill_all().await.unwrap();
}

// ── 13. Broadcast doesn't trigger auto-reply ────────────────────────────────

#[tokio::test]
async fn test_13_broadcast_no_auto_reply() {
    let provider = ScriptProvider::new("mock", "test-model", vec![]);

    let (sup, _orch, mut rx) = build_test_env(provider);

    for i in 0..3 {
        sup.spawn_agent(
            make_spec(&format!("a{i}"), &format!("Agent{i}"), true),
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
    }

    sup.broadcast(AgentMessage::Broadcast {
        content: "System announcement".to_string(),
        from: "supervisor".to_string(),
    })
    .await
    .unwrap();

    tokio::time::sleep(Duration::from_millis(300)).await;
    let events = collect_events(&mut rx, 100, 300).await;

    let routed =
        events.iter().filter(|e| matches!(e, SupervisorEvent::MessageRouted { .. })).count();
    assert!(routed >= 3, "Expected 3 message routes for broadcast");

    sup.kill_all().await.unwrap();
}

// ── 14. Keep-alive agent survives after feedback ────────────────────────────

#[tokio::test]
async fn test_14_keep_alive_survives_feedback() {
    let provider = ScriptProvider::new(
        "mock",
        "test-model",
        vec![
            ScriptProvider::make_message_call("tc1", "b", "Hello"),
            ScriptProvider::make_response("A sent"),
            ScriptProvider::make_response("B done"),
            ScriptProvider::make_response("A processed feedback"),
        ],
    );

    let (sup, _orch, _rx) = build_test_env(provider);

    sup.spawn_agent(make_spec("a", "Alice", true), None, None, None, None).await.unwrap();
    sup.spawn_agent(make_spec("b", "Bob", false), None, None, None, None).await.unwrap();

    sup.send_to_agent(
        "a",
        AgentMessage::Task { content: "Talk to Bob".to_string(), from: Some("user".to_string()) },
    )
    .await
    .unwrap();

    tokio::time::sleep(Duration::from_millis(800)).await;

    // A should still be alive (keep_alive=true) even after processing feedback
    let status = sup.get_agent_status("a");
    assert!(status.is_some(), "Keep-alive agent A should still exist after feedback");

    sup.kill_all().await.unwrap();
}

// ── 15. Kill during message processing ──────────────────────────────────────

#[tokio::test]
async fn test_15_kill_during_processing() {
    // Spawn an agent, send it a task, then kill it mid-processing.
    let provider = ScriptProvider::new(
        "mock",
        "test-model",
        vec![ScriptProvider::make_response("Working...")],
    );

    let (sup, _orch, _rx) = build_test_env(provider);

    sup.spawn_agent(make_spec("a", "Alice", true), None, None, None, None).await.unwrap();

    sup.send_to_agent(
        "a",
        AgentMessage::Task { content: "Long task".to_string(), from: Some("user".to_string()) },
    )
    .await
    .unwrap();

    // Kill immediately
    sup.kill_agent("a").await.unwrap();
    assert_eq!(sup.agent_count(), 0);
}

// ── 16. MessageRouted events emitted for both Task and Feedback ─────────────

#[tokio::test]
async fn test_16_message_routed_events() {
    let provider = ScriptProvider::new(
        "mock",
        "test-model",
        vec![
            ScriptProvider::make_message_call("tc1", "b", "Ping"),
            ScriptProvider::make_response("A sent"),
            ScriptProvider::make_response("B pong"),
            ScriptProvider::make_response("A got pong"),
        ],
    );

    let (sup, _orch, mut rx) = build_test_env(provider);

    sup.spawn_agent(make_spec("a", "Alice", true), None, None, None, None).await.unwrap();
    sup.spawn_agent(make_spec("b", "Bob", false), None, None, None, None).await.unwrap();

    sup.send_to_agent(
        "a",
        AgentMessage::Task { content: "Ping Bob".to_string(), from: Some("user".to_string()) },
    )
    .await
    .unwrap();

    tokio::time::sleep(Duration::from_millis(800)).await;
    let events = collect_events(&mut rx, 200, 300).await;

    // Should have MessageRouted for the task A→B
    let task_routed = events.iter().any(|e| {
        matches!(e,
            SupervisorEvent::MessageRouted { from, to, msg_type }
            if from == "a" && to == "b" && msg_type == "task"
        )
    });
    assert!(task_routed, "Expected task MessageRouted from A→B");

    // Should have MessageRouted for feedback B→A
    let feedback_routed = events.iter().any(|e| {
        matches!(e,
            SupervisorEvent::MessageRouted { from, to, msg_type }
            if from == "b" && to == "a" && msg_type == "feedback"
        )
    });
    assert!(feedback_routed, "Expected feedback MessageRouted from B→A");

    sup.kill_all().await.unwrap();
}

// ── 17. Five agents in a ring: A→B→C→D→E ───────────────────────────────────

#[tokio::test]
async fn test_17_five_agent_chain() {
    let provider = ScriptProvider::new(
        "mock",
        "test-model",
        vec![
            // A→B
            ScriptProvider::make_message_call("tc1", "b", "Pass along: hello"),
            ScriptProvider::make_response("A forwarded"),
            // B→C
            ScriptProvider::make_message_call("tc2", "c", "Pass along: hello"),
            ScriptProvider::make_response("B forwarded"),
            // C→D
            ScriptProvider::make_message_call("tc3", "d", "Pass along: hello"),
            ScriptProvider::make_response("C forwarded"),
            // D→E
            ScriptProvider::make_message_call("tc4", "e", "Pass along: hello"),
            ScriptProvider::make_response("D forwarded"),
            // E processes (end of chain)
            ScriptProvider::make_response("E: chain complete"),
            // D gets feedback from E
            ScriptProvider::make_response("D got E feedback"),
            // C gets feedback from D
            ScriptProvider::make_response("C got D feedback"),
            // B gets feedback from C
            ScriptProvider::make_response("B got C feedback"),
            // A gets feedback from B
            ScriptProvider::make_response("A got B feedback"),
        ],
    );

    let (sup, orch, _rx) = build_test_env(provider);

    for (id, name) in [("a", "A"), ("b", "B"), ("c", "C"), ("d", "D"), ("e", "E")] {
        sup.spawn_agent(make_spec(id, name, true), None, None, None, None).await.unwrap();
    }

    sup.send_to_agent(
        "a",
        AgentMessage::Task { content: "Start chain".to_string(), from: Some("user".to_string()) },
    )
    .await
    .unwrap();

    tokio::time::sleep(Duration::from_millis(3000)).await;

    let msgs = orch.messages.lock();
    assert!(msgs.iter().any(|(to, _, from)| to == "b" && from == "a"), "Expected A→B");
    assert!(msgs.iter().any(|(to, _, from)| to == "c" && from == "b"), "Expected B→C");

    sup.kill_all().await.unwrap();
}

// ── 18. Concurrent independent conversations ────────────────────────────────

#[tokio::test]
async fn test_18_concurrent_independent_conversations() {
    // Two independent pairs: A↔B and C↔D, started sequentially but
    // running concurrently (keep_alive agents).
    let provider = ScriptProvider::new(
        "mock",
        "test-model",
        vec![
            // A→B (A starts first)
            ScriptProvider::make_message_call("tc1", "b", "Hello B from A"),
            ScriptProvider::make_response("A sent to B"),
            // B processes A's message
            ScriptProvider::make_response("B got A's message"),
            // A processes B's feedback
            ScriptProvider::make_response("A got B's feedback"),
            // C→D (C starts after A is done)
            ScriptProvider::make_message_call("tc2", "d", "Hello D from C"),
            ScriptProvider::make_response("C sent to D"),
            // D processes C's message
            ScriptProvider::make_response("D got C's message"),
            // C processes D's feedback
            ScriptProvider::make_response("C got D's feedback"),
        ],
    );

    let (sup, orch, _rx) = build_test_env(provider);

    for (id, name) in [("a", "A"), ("b", "B"), ("c", "C"), ("d", "D")] {
        sup.spawn_agent(make_spec(id, name, true), None, None, None, None).await.unwrap();
    }

    // Start A's conversation first
    sup.send_to_agent(
        "a",
        AgentMessage::Task { content: "Talk to B".to_string(), from: Some("user".to_string()) },
    )
    .await
    .unwrap();

    // Wait for A↔B exchange to complete
    tokio::time::sleep(Duration::from_millis(1000)).await;

    // Then start C's conversation
    sup.send_to_agent(
        "c",
        AgentMessage::Task { content: "Talk to D".to_string(), from: Some("user".to_string()) },
    )
    .await
    .unwrap();

    tokio::time::sleep(Duration::from_millis(1000)).await;

    let msgs = orch.messages.lock();
    assert!(msgs.iter().any(|(to, _, from)| to == "b" && from == "a"), "Expected A→B");
    assert!(msgs.iter().any(|(to, _, from)| to == "d" && from == "c"), "Expected C→D");

    sup.kill_all().await.unwrap();
}

// ── 19. Agent receives task from user, not another agent ────────────────────

#[tokio::test]
async fn test_19_user_task_no_auto_reply() {
    // When from="user", the agent should NOT auto-reply
    let provider = ScriptProvider::new(
        "mock",
        "test-model",
        vec![ScriptProvider::make_response("User task completed")],
    );

    let (sup, orch, _rx) = build_test_env(provider);

    sup.spawn_agent(make_spec("a", "Alice", false), None, None, None, None).await.unwrap();

    sup.send_to_agent(
        "a",
        AgentMessage::Task {
            content: "User's direct task".to_string(),
            from: Some("user".to_string()),
        },
    )
    .await
    .unwrap();

    tokio::time::sleep(Duration::from_millis(500)).await;

    // No messages or feedbacks should be sent (from="user" skips auto-reply)
    let msgs = orch.messages.lock();
    assert!(msgs.is_empty(), "No messages should be routed for user tasks");
    let feedbacks = orch.feedbacks.lock();
    assert!(feedbacks.is_empty(), "No feedbacks should be sent for user tasks");

    sup.kill_all().await.unwrap();
}

// ── 20. Stress: 10 agents, coordinator fans out to all ──────────────────────

#[tokio::test]
async fn test_20_stress_fan_out_10_agents() {
    let mut responses = Vec::new();
    // Coordinator sends to all 10 workers
    let mut tool_calls = Vec::new();
    for i in 0..10 {
        tool_calls.push(ToolCallResponse {
            id: format!("tc{i}"),
            name: "core.signal_agent".to_string(),
            arguments: json!({"agent_id": format!("w{}", i), "content": format!("Task {}", i)}),
        });
    }
    responses.push(CompletionResponse {
        provider_id: "mock".to_string(),
        model: "test-model".to_string(),
        content: String::new(),
        tool_calls,
    });
    // Coordinator after tool results
    responses.push(ScriptProvider::make_response("Coordinator dispatched all"));
    // Each worker processes its task
    for i in 0..10 {
        responses.push(ScriptProvider::make_response(&format!("Worker {i} done")));
    }
    // Coordinator processes all feedbacks
    for _ in 0..10 {
        responses.push(ScriptProvider::make_response("Coordinator ack"));
    }

    let provider = ScriptProvider::new("mock", "test-model", responses);
    let (sup, orch, _rx) = build_test_env(provider);

    sup.spawn_agent(make_spec("coord", "Coordinator", true), None, None, None, None).await.unwrap();
    for i in 0..10 {
        sup.spawn_agent(
            make_spec(&format!("w{i}"), &format!("Worker{i}"), true),
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
    }

    sup.send_to_agent(
        "coord",
        AgentMessage::Task {
            content: "Distribute work to all workers".to_string(),
            from: Some("user".to_string()),
        },
    )
    .await
    .unwrap();

    tokio::time::sleep(Duration::from_millis(3000)).await;

    let msgs = orch.messages.lock();
    let coord_msgs = msgs.iter().filter(|(_, _, from)| from == "coord").count();
    assert!(
        coord_msgs >= 10,
        "Coordinator should have sent at least 10 messages, got {coord_msgs}"
    );

    // All workers should still be alive
    assert_eq!(sup.agent_count(), 11, "All 11 agents should be alive");

    sup.kill_all().await.unwrap();
}
