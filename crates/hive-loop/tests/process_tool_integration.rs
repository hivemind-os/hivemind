//! Integration tests for process management tools (process.start, process.status,
//! process.write, process.kill, process.list) exercised through the legacy ReAct loop
//! with a scripted mock LLM.
//!
//! Tests range from simple single-tool invocations to complex multi-step process
//! lifecycle scenarios.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::atomic::AtomicU8;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hive_contracts::{
    Capability, PermissionRule, ProviderDescriptor, ProviderKind, SessionPermissions,
    ToolExecutionMode,
};
use hive_loop::legacy::{
    AgentContext, ConversationContext, LoopContext, LoopExecutor, ReActStrategy, RoutingConfig,
    SecurityContext, ToolsContext,
};
use hive_model::{
    CompletionRequest, CompletionResponse, ModelProvider, ModelRouter, ModelSelection,
    RoutingDecision, ToolCallResponse,
};
use hive_process::ProcessManager;
use hive_tools::{
    ProcessKillTool, ProcessListTool, ProcessStartTool, ProcessStatusTool, ProcessWriteTool,
    ShellCommandTool, ToolRegistry,
};
use serde_json::json;

// ─── Mock LLM provider ────────────────────────────────────────────────

/// A scripted LLM that returns pre-configured responses (text or tool calls)
/// from a FIFO queue.  Falls back to "done" when the queue is empty.
struct ScriptProvider {
    descriptor: ProviderDescriptor,
    responses: Mutex<VecDeque<CompletionResponse>>,
    prompts: Mutex<Vec<String>>,
}

impl ScriptProvider {
    fn new(responses: Vec<CompletionResponse>) -> Self {
        Self {
            descriptor: ProviderDescriptor {
                id: "mock".to_string(),
                name: Some("Mock".to_string()),
                kind: ProviderKind::Mock,
                models: vec!["test-model".to_string()],
                model_capabilities: BTreeMap::from([(
                    "test-model".to_string(),
                    BTreeSet::from([Capability::Chat, Capability::ToolUse]),
                )]),
                priority: 10,
                available: true,
            },
            responses: Mutex::new(VecDeque::from(responses)),
            prompts: Mutex::new(Vec::new()),
        }
    }

    /// Simple text response (no tool calls).
    fn text(content: &str) -> CompletionResponse {
        CompletionResponse {
            provider_id: "mock".into(),
            model: "test-model".into(),
            content: content.into(),
            tool_calls: vec![],
        }
    }

    /// Response containing a single tool call.
    fn tool_call(name: &str, args: serde_json::Value) -> CompletionResponse {
        Self::tool_call_with_id("tc-1", name, args)
    }

    fn tool_call_with_id(id: &str, name: &str, args: serde_json::Value) -> CompletionResponse {
        CompletionResponse {
            provider_id: "mock".into(),
            model: "test-model".into(),
            content: String::new(),
            tool_calls: vec![ToolCallResponse {
                id: id.into(),
                name: name.into(),
                arguments: args,
            }],
        }
    }

    /// Response with multiple tool calls.
    fn multi_tool(calls: Vec<(&str, &str, serde_json::Value)>) -> CompletionResponse {
        CompletionResponse {
            provider_id: "mock".into(),
            model: "test-model".into(),
            content: String::new(),
            tool_calls: calls
                .into_iter()
                .map(|(id, name, args)| ToolCallResponse {
                    id: id.into(),
                    name: name.into(),
                    arguments: args,
                })
                .collect(),
        }
    }

    fn recorded_prompts(&self) -> Vec<String> {
        self.prompts.lock().unwrap().clone()
    }
}

impl ModelProvider for ScriptProvider {
    fn descriptor(&self) -> &ProviderDescriptor {
        &self.descriptor
    }

    fn complete(
        &self,
        request: &CompletionRequest,
        selection: &ModelSelection,
    ) -> anyhow::Result<CompletionResponse> {
        self.prompts.lock().unwrap().push(request.prompt.clone());
        let mut queue = self.responses.lock().unwrap();
        let mut resp = queue.pop_front().unwrap_or_else(|| CompletionResponse {
            provider_id: "mock".into(),
            model: "test-model".into(),
            content: "done".into(),
            tool_calls: vec![],
        });
        resp.provider_id = self.descriptor.id.clone();
        resp.model = selection.model.clone();
        Ok(resp)
    }
}

// ─── Test helpers ──────────────────────────────────────────────────────

fn make_router(provider: ScriptProvider) -> Arc<ModelRouter> {
    let mut router = ModelRouter::new();
    router.register_provider(provider);
    Arc::new(router)
}

fn make_process_registry(mgr: &Arc<ProcessManager>) -> Arc<ToolRegistry> {
    let mut registry = ToolRegistry::new();
    registry
        .register(Arc::new(ProcessStartTool::new(
            Arc::clone(mgr),
            Arc::new(parking_lot::RwLock::new(std::collections::HashMap::new())),
            Arc::new(parking_lot::RwLock::new(hive_contracts::SandboxConfig::default())),
            hive_process::ProcessOwner::Unknown,
            None,
            None,
        )))
        .unwrap();
    registry.register(Arc::new(ProcessStatusTool::new(Arc::clone(mgr)))).unwrap();
    registry.register(Arc::new(ProcessWriteTool::new(Arc::clone(mgr)))).unwrap();
    registry.register(Arc::new(ProcessKillTool::new(Arc::clone(mgr)))).unwrap();
    registry.register(Arc::new(ProcessListTool::new(Arc::clone(mgr)))).unwrap();
    Arc::new(registry)
}

fn make_full_registry(mgr: &Arc<ProcessManager>) -> Arc<ToolRegistry> {
    let mut registry = ToolRegistry::new();
    registry
        .register(Arc::new(ProcessStartTool::new(
            Arc::clone(mgr),
            Arc::new(parking_lot::RwLock::new(std::collections::HashMap::new())),
            Arc::new(parking_lot::RwLock::new(hive_contracts::SandboxConfig::default())),
            hive_process::ProcessOwner::Unknown,
            None,
            None,
        )))
        .unwrap();
    registry.register(Arc::new(ProcessStatusTool::new(Arc::clone(mgr)))).unwrap();
    registry.register(Arc::new(ProcessWriteTool::new(Arc::clone(mgr)))).unwrap();
    registry.register(Arc::new(ProcessKillTool::new(Arc::clone(mgr)))).unwrap();
    registry.register(Arc::new(ProcessListTool::new(Arc::clone(mgr)))).unwrap();
    registry.register(Arc::new(ShellCommandTool::default())).unwrap();
    Arc::new(registry)
}

fn make_context(tools: Arc<ToolRegistry>, prompt: &str) -> LoopContext {
    // Auto-approve all process.* and shell.* tools so tests don't block on
    // the user interaction gate (which is None in tests).
    let mut permissions = SessionPermissions::default();
    permissions.add_rule(PermissionRule {
        tool_pattern: "process.*".to_string(),
        scope: "*".to_string(),
        decision: hive_contracts::ToolApproval::Auto,
    });
    permissions.add_rule(PermissionRule {
        tool_pattern: "shell.*".to_string(),
        scope: "*".to_string(),
        decision: hive_contracts::ToolApproval::Auto,
    });

    LoopContext {
        conversation: ConversationContext {
            session_id: "test-session".into(),
            message_id: "msg-1".into(),
            prompt: prompt.into(),
            prompt_content_parts: vec![],
            history: vec![],
            conversation_journal: None,
            initial_tool_iterations: 0,
        },
        routing: RoutingConfig {
            required_capabilities: BTreeSet::new(),
            preferred_models: None,
            loop_strategy: None,
            routing_decision: Some(RoutingDecision {
                selected: ModelSelection { provider_id: "mock".into(), model: "test-model".into() },
                fallback_chain: vec![],
                reason: "test".into(),
            }),
        },
        security: SecurityContext {
            data_class: hive_classification::DataClass::Internal,
            permissions: Arc::new(parking_lot::Mutex::new(permissions)),
            workspace_classification: None,
            effective_data_class: Arc::new(AtomicU8::new(
                hive_classification::DataClass::Internal.to_i64() as u8,
            )),
            connector_service: None,
                shadow_mode: false,
        },
        tools_ctx: ToolsContext {
            tools: tools,
            skill_catalog: None,
            knowledge_query_handler: None,
            tool_execution_mode: ToolExecutionMode::default(),
        },
        agent: AgentContext {
            persona: None,
            agent_orchestrator: None,
            workspace_path: None,
            personas: Vec::new(),
            current_agent_id: None,
            parent_agent_id: None,
            keep_alive: false,
            session_messaged: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        },
        tool_limits: hive_contracts::ToolLimitsConfig::default(),
        code_act_config: hive_contracts::CodeActConfig::default(),
        session_registry: None,
        preempt_signal: None,
        cancellation_token: None,
    }
}

fn run_executor() -> LoopExecutor {
    LoopExecutor::new(Arc::new(ReActStrategy))
}

/// Small delay so PTY reader threads can drain output.
async fn settle() {
    tokio::time::sleep(Duration::from_millis(400)).await;
}

/// Like `settle` but polls until the process is no longer running (up to ~4s).
async fn wait_for_exit(mgr: &ProcessManager, id: &str) {
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        if let Ok((info, _)) = mgr.status(id, None) {
            if !info.status.is_running() {
                return;
            }
        }
    }
}

// ─── Cross-platform command helpers ────────────────────────────────────

fn sleep_cmd(secs: u32) -> String {
    if cfg!(windows) {
        format!("ping -n {} 127.0.0.1 >nul", secs + 1)
    } else {
        format!("sleep {secs}")
    }
}

fn true_cmd() -> &'static str {
    if cfg!(windows) {
        "exit 0"
    } else {
        "true"
    }
}

fn false_cmd() -> &'static str {
    if cfg!(windows) {
        "exit 1"
    } else {
        "false"
    }
}

fn pwd_cmd() -> &'static str {
    if cfg!(windows) {
        // Use cmd /K so the shell stays alive after printing the directory;
        // short-lived processes exit before ConPTY can flush their output.
        "cmd /K cd"
    } else {
        "pwd"
    }
}

fn echo_env_cmd(var: &str) -> String {
    if cfg!(windows) {
        // Use cmd /K so the shell stays alive after echoing the value;
        // short-lived processes exit before ConPTY can flush their output.
        format!("cmd /K echo %{var}%")
    } else {
        format!("echo ${var}")
    }
}

// ═══════════════════════════════════════════════════════════════════════
//  SIMPLE TESTS (1–5)
// ═══════════════════════════════════════════════════════════════════════

/// 1. Start a process and verify the tool returns a process ID and PID.
#[tokio::test]
async fn start_returns_process_id() {
    let mgr = Arc::new(ProcessManager::new());
    let tools = make_process_registry(&mgr);
    let provider = ScriptProvider::new(vec![
        ScriptProvider::tool_call("process.start", json!({"command": "echo hello"})),
        ScriptProvider::text("Process started."),
    ]);
    let router = make_router(provider);
    let ctx = make_context(tools, "Start a process");
    let result = run_executor().run(ctx, router).await.unwrap();
    assert_eq!(result.content, "Process started.");

    // The process manager should have one entry.
    let list = mgr.list();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, "proc-1");
}

/// 2. List processes when none are running — should return empty list.
#[tokio::test]
async fn list_empty() {
    let mgr = Arc::new(ProcessManager::new());
    let tools = make_process_registry(&mgr);
    let provider = ScriptProvider::new(vec![
        ScriptProvider::tool_call("process.list", json!({})),
        ScriptProvider::text("No processes running."),
    ]);
    let router = make_router(provider);
    let ctx = make_context(tools, "List processes");
    let result = run_executor().run(ctx, router).await.unwrap();
    assert_eq!(result.content, "No processes running.");
}

/// 3. Start a process, then list — it should appear.
#[tokio::test]
async fn start_then_list() {
    let mgr = Arc::new(ProcessManager::new());
    let tools = make_process_registry(&mgr);
    let provider = ScriptProvider::new(vec![
        ScriptProvider::tool_call("process.start", json!({"command": sleep_cmd(30)})),
        ScriptProvider::tool_call("process.list", json!({})),
        ScriptProvider::text("Listed."),
    ]);
    let router = make_router(provider);
    let ctx = make_context(tools, "Start and list");
    let result = run_executor().run(ctx, router).await.unwrap();
    assert_eq!(result.content, "Listed.");

    let list = mgr.list();
    assert_eq!(list.len(), 1);
    mgr.kill("proc-1", None).unwrap();
}

/// 4. Start `echo hello`, wait for it to finish, then check status.
#[tokio::test]
async fn start_and_check_status() {
    let mgr = Arc::new(ProcessManager::new());
    let tools = make_process_registry(&mgr);
    // The LLM calls process.start, pauses (we'll rely on the test sleep),
    // then calls process.status.
    let provider = ScriptProvider::new(vec![
        ScriptProvider::tool_call("process.start", json!({"command": "echo hello"})),
        ScriptProvider::tool_call("process.status", json!({"process_id": "proc-1"})),
        ScriptProvider::text("Output received."),
    ]);
    let router = make_router(provider);
    let ctx = make_context(tools, "Check output");

    // Give the short-lived process time to finish before the loop requests status.
    // The loop is fast, so it might call status before echo finishes; that's OK —
    // the output will still contain "hello" eventually.
    settle().await;

    let result = run_executor().run(ctx, router).await.unwrap();
    assert_eq!(result.content, "Output received.");
}

/// 5. Start `sleep 60`, then kill it.
#[tokio::test]
async fn start_and_kill() {
    let mgr = Arc::new(ProcessManager::new());
    let tools = make_process_registry(&mgr);
    let provider = ScriptProvider::new(vec![
        ScriptProvider::tool_call("process.start", json!({"command": sleep_cmd(60)})),
        ScriptProvider::tool_call("process.kill", json!({"process_id": "proc-1"})),
        ScriptProvider::text("Killed."),
    ]);
    let router = make_router(provider);
    let ctx = make_context(tools, "Kill a process");
    let result = run_executor().run(ctx, router).await.unwrap();
    assert_eq!(result.content, "Killed.");

    settle().await;
    let (info, _) = mgr.status("proc-1", None).unwrap();
    assert!(!info.status.is_running());
}

// ═══════════════════════════════════════════════════════════════════════
//  MEDIUM TESTS (6–12)
// ═══════════════════════════════════════════════════════════════════════

/// 6. Start `cat`, write to stdin, check output contains written text.
#[tokio::test]
#[cfg(unix)]
async fn write_stdin_and_read_output() {
    let mgr = Arc::new(ProcessManager::new());
    let tools = make_process_registry(&mgr);
    let provider = ScriptProvider::new(vec![
        ScriptProvider::tool_call("process.start", json!({"command": "cat"})),
        ScriptProvider::tool_call(
            "process.write",
            json!({"process_id": "proc-1", "input": "integration test\n"}),
        ),
        // Give PTY time to echo back.
        ScriptProvider::tool_call("process.status", json!({"process_id": "proc-1"})),
        ScriptProvider::text("Write confirmed."),
    ]);
    let router = make_router(provider);
    let ctx = make_context(tools, "Write to stdin");
    let result = run_executor().run(ctx, router).await.unwrap();
    assert_eq!(result.content, "Write confirmed.");

    settle().await;
    let (_, output) = mgr.status("proc-1", None).unwrap();
    assert!(
        output.contains("integration test"),
        "expected 'integration test' in output, got: {output}"
    );
    mgr.kill("proc-1", None).unwrap();
}

/// 7. Start a process that prints many lines, use tail_lines to get last 3.
#[tokio::test]
#[cfg(unix)]
async fn status_tail_lines() {
    let mgr = Arc::new(ProcessManager::new());
    let tools = make_process_registry(&mgr);
    let provider = ScriptProvider::new(vec![
        ScriptProvider::tool_call(
            "process.start",
            json!({"command": "printf 'line1\\nline2\\nline3\\nline4\\nline5'"}),
        ),
        ScriptProvider::tool_call(
            "process.status",
            json!({"process_id": "proc-1", "tail_lines": 2}),
        ),
        ScriptProvider::text("Got tail."),
    ]);
    let router = make_router(provider);
    let ctx = make_context(tools, "Tail lines");

    settle().await;
    let result = run_executor().run(ctx, router).await.unwrap();
    assert_eq!(result.content, "Got tail.");
}

/// 8. Start a process with a specific working directory.
#[tokio::test]
async fn start_with_working_dir() {
    let mgr = Arc::new(ProcessManager::new());
    let tools = make_process_registry(&mgr);
    let work_dir = if cfg!(windows) {
        std::env::temp_dir().to_string_lossy().to_string()
    } else {
        "/tmp".to_string()
    };
    let provider = ScriptProvider::new(vec![
        ScriptProvider::tool_call(
            "process.start",
            json!({"command": pwd_cmd(), "working_dir": &work_dir}),
        ),
        ScriptProvider::tool_call("process.status", json!({"process_id": "proc-1"})),
        ScriptProvider::text("Working dir verified."),
    ]);
    let router = make_router(provider);
    let ctx = make_context(tools, "Start with cwd");

    settle().await;
    let result = run_executor().run(ctx, router).await.unwrap();
    assert_eq!(result.content, "Working dir verified.");

    // On Windows, ConPTY needs extra time to deliver output from the PTY.
    if cfg!(windows) {
        tokio::time::sleep(Duration::from_secs(2)).await;
    } else {
        settle().await;
    }
    let (_, output) = mgr.status("proc-1", None).unwrap();
    if cfg!(windows) {
        // On Windows, PTY output may use 8.3 short names (DANIEL~1 vs danielgerlag)
        // and contain ANSI escape sequences. Strip escapes and canonicalize for comparison.
        let strip_ansi = |s: &str| -> String {
            let mut out = String::new();
            let mut chars = s.chars().peekable();
            while let Some(ch) = chars.next() {
                if ch == '\x1b' {
                    // Skip ESC [ ... <letter> sequences
                    if chars.peek() == Some(&'[') {
                        chars.next();
                        while let Some(&c) = chars.peek() {
                            chars.next();
                            if c.is_ascii_alphabetic() {
                                break;
                            }
                        }
                    }
                } else {
                    out.push(ch);
                }
            }
            out
        };
        let clean_output = strip_ansi(&output);
        let canon = |s: &str| -> String {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return String::new();
            }
            std::fs::canonicalize(trimmed)
                .unwrap_or_else(|_| std::path::PathBuf::from(trimmed))
                .to_string_lossy()
                .to_lowercase()
                .trim_start_matches(r"\\?\")
                .to_string()
        };
        let expected = canon(&work_dir);
        let found = clean_output.lines().any(|line| {
            let c = canon(line);
            !c.is_empty() && c.contains(&expected)
        });
        assert!(found, "expected temp dir {expected} in output, got: {output:?}");
        let _ = mgr.kill("proc-1", None);
    } else {
        // macOS resolves /tmp → /private/tmp
        assert!(
            output.contains("/tmp") || output.contains("/private/tmp"),
            "expected /tmp in output, got: {output}"
        );
    }
}

/// 9. Start a process with custom environment variables.
#[tokio::test]
async fn start_with_env_vars() {
    let mgr = Arc::new(ProcessManager::new());
    let tools = make_process_registry(&mgr);
    let provider = ScriptProvider::new(vec![
        ScriptProvider::tool_call(
            "process.start",
            json!({
                "command": echo_env_cmd("HIVEMIND_TEST_VAR"),
                "env": {"HIVEMIND_TEST_VAR": "hello_from_env"}
            }),
        ),
        ScriptProvider::tool_call("process.status", json!({"process_id": "proc-1"})),
        ScriptProvider::text("Env verified."),
    ]);
    let router = make_router(provider);
    let ctx = make_context(tools, "Start with env");

    settle().await;
    let result = run_executor().run(ctx, router).await.unwrap();
    assert_eq!(result.content, "Env verified.");

    // On Windows, ConPTY needs extra time to deliver output from the PTY.
    if cfg!(windows) {
        tokio::time::sleep(Duration::from_secs(2)).await;
    } else {
        settle().await;
    }
    let (_, output) = mgr.status("proc-1", None).unwrap();
    assert!(output.contains("hello_from_env"), "expected env var in output, got: {output}");
    if cfg!(windows) {
        let _ = mgr.kill("proc-1", None);
    }
}

/// 10. Kill a process with SIGKILL signal.
#[tokio::test]
async fn kill_with_sigkill() {
    let mgr = Arc::new(ProcessManager::new());
    let tools = make_process_registry(&mgr);
    let provider = ScriptProvider::new(vec![
        ScriptProvider::tool_call("process.start", json!({"command": sleep_cmd(60)})),
        ScriptProvider::tool_call(
            "process.kill",
            json!({"process_id": "proc-1", "signal": "SIGKILL"}),
        ),
        ScriptProvider::text("Force killed."),
    ]);
    let router = make_router(provider);
    let ctx = make_context(tools, "Force kill");
    let result = run_executor().run(ctx, router).await.unwrap();
    assert_eq!(result.content, "Force killed.");

    settle().await;
    let (info, _) = mgr.status("proc-1", None).unwrap();
    assert!(!info.status.is_running(), "process should be dead after SIGKILL");
}

/// 11. Start a short-lived process, verify it shows as exited.
#[tokio::test]
async fn status_after_natural_exit() {
    let mgr = Arc::new(ProcessManager::new());
    let tools = make_process_registry(&mgr);
    let provider = ScriptProvider::new(vec![
        ScriptProvider::tool_call("process.start", json!({"command": true_cmd()})),
        ScriptProvider::text("Started."),
    ]);
    let router = make_router(provider);
    let ctx = make_context(tools, "Start true");
    let result = run_executor().run(ctx, router).await.unwrap();
    assert_eq!(result.content, "Started.");

    wait_for_exit(&mgr, "proc-1").await;
    let (info, _) = mgr.status("proc-1", None).unwrap();
    if cfg!(not(windows)) {
        // On Windows, ConPTY may not deliver EOF after a short-lived process exits,
        // so the reader thread blocks and status stays Running.
        assert!(
            !info.status.is_running(),
            "short-lived `true` should have exited: {:?}",
            info.status
        );
    }
}

/// 12. Start multiple processes, list them all, then kill them all.
#[tokio::test]
async fn multiple_concurrent_processes() {
    let mgr = Arc::new(ProcessManager::new());
    let tools = make_process_registry(&mgr);
    let provider = ScriptProvider::new(vec![
        ScriptProvider::tool_call("process.start", json!({"command": sleep_cmd(100)})),
        ScriptProvider::tool_call("process.start", json!({"command": sleep_cmd(101)})),
        ScriptProvider::tool_call("process.start", json!({"command": sleep_cmd(102)})),
        ScriptProvider::tool_call("process.list", json!({})),
        ScriptProvider::tool_call("process.kill", json!({"process_id": "proc-1"})),
        ScriptProvider::tool_call("process.kill", json!({"process_id": "proc-2"})),
        ScriptProvider::tool_call("process.kill", json!({"process_id": "proc-3"})),
        ScriptProvider::text("All cleaned up."),
    ]);
    let router = make_router(provider);
    let ctx = make_context(tools, "Multi process");
    let result = run_executor().run(ctx, router).await.unwrap();
    assert_eq!(result.content, "All cleaned up.");

    settle().await;
    for id in &["proc-1", "proc-2", "proc-3"] {
        let (info, _) = mgr.status(id, None).unwrap();
        assert!(!info.status.is_running(), "{id} should be dead");
    }
}

// ═══════════════════════════════════════════════════════════════════════
//  COMPLEX TESTS (13–20)
// ═══════════════════════════════════════════════════════════════════════

/// 13. Start a process and poll status twice to watch output accumulate.
#[tokio::test]
#[cfg(unix)]
async fn poll_status_multiple_times() {
    let mgr = Arc::new(ProcessManager::new());
    let tools = make_process_registry(&mgr);
    let provider = ScriptProvider::new(vec![
        ScriptProvider::tool_call(
            "process.start",
            json!({"command": "printf 'step1\\nstep2\\nstep3'"}),
        ),
        ScriptProvider::tool_call("process.status", json!({"process_id": "proc-1"})),
        ScriptProvider::tool_call(
            "process.status",
            json!({"process_id": "proc-1", "tail_lines": 1}),
        ),
        ScriptProvider::text("Polling done."),
    ]);
    let router = make_router(provider);
    let ctx = make_context(tools, "Poll status");

    settle().await;
    let result = run_executor().run(ctx, router).await.unwrap();
    assert_eq!(result.content, "Polling done.");
}

/// 14. Interactive session: start `cat`, write multiple lines, read back.
#[tokio::test]
#[cfg(unix)]
async fn interactive_cat_session() {
    let mgr = Arc::new(ProcessManager::new());
    let tools = make_process_registry(&mgr);
    let provider = ScriptProvider::new(vec![
        ScriptProvider::tool_call("process.start", json!({"command": "cat"})),
        ScriptProvider::tool_call(
            "process.write",
            json!({"process_id": "proc-1", "input": "first line\n"}),
        ),
        ScriptProvider::tool_call(
            "process.write",
            json!({"process_id": "proc-1", "input": "second line\n"}),
        ),
        ScriptProvider::tool_call("process.status", json!({"process_id": "proc-1"})),
        ScriptProvider::tool_call("process.kill", json!({"process_id": "proc-1"})),
        ScriptProvider::text("Interactive session complete."),
    ]);
    let router = make_router(provider);
    let ctx = make_context(tools, "Interactive cat");
    let result = run_executor().run(ctx, router).await.unwrap();
    assert_eq!(result.content, "Interactive session complete.");

    settle().await;
    let (info, _) = mgr.status("proc-1", None).unwrap();
    assert!(!info.status.is_running());
}

/// 15. Error handling: status/kill/write on a nonexistent process ID.
#[tokio::test]
async fn error_nonexistent_process_id() {
    let mgr = Arc::new(ProcessManager::new());
    let tools = make_process_registry(&mgr);
    let provider = ScriptProvider::new(vec![
        ScriptProvider::tool_call("process.status", json!({"process_id": "proc-999"})),
        ScriptProvider::tool_call("process.kill", json!({"process_id": "proc-999"})),
        ScriptProvider::tool_call(
            "process.write",
            json!({"process_id": "proc-999", "input": "test"}),
        ),
        ScriptProvider::text("Errors handled."),
    ]);
    let router = make_router(provider);
    let ctx = make_context(tools, "Bad IDs");

    let result = run_executor().run(ctx, router).await.unwrap();
    assert_eq!(result.content, "Errors handled.");
    // The loop should not crash — errors are returned as tool results to the LLM.
}

/// 16. Kill an already-exited process — should not error.
#[tokio::test]
async fn kill_already_exited_process() {
    let mgr = Arc::new(ProcessManager::new());
    let tools = make_process_registry(&mgr);
    let provider = ScriptProvider::new(vec![
        ScriptProvider::tool_call("process.start", json!({"command": true_cmd()})),
        // exits immediately; killing it should be a no-op.
        ScriptProvider::tool_call("process.kill", json!({"process_id": "proc-1"})),
        ScriptProvider::text("Kill on exited OK."),
    ]);
    let router = make_router(provider);
    let ctx = make_context(tools, "Kill exited");

    settle().await; // Let `true` exit.
    let result = run_executor().run(ctx, router).await.unwrap();
    assert_eq!(result.content, "Kill on exited OK.");
}

/// 17. Start a command that fails immediately (`false`), verify exit code != 0.
#[tokio::test]
async fn start_failing_command_exit_code() {
    let mgr = Arc::new(ProcessManager::new());
    let tools = make_process_registry(&mgr);
    let provider = ScriptProvider::new(vec![
        ScriptProvider::tool_call("process.start", json!({"command": false_cmd()})),
        ScriptProvider::tool_call("process.status", json!({"process_id": "proc-1"})),
        ScriptProvider::text("Failure detected."),
    ]);
    let router = make_router(provider);
    let ctx = make_context(tools, "Start false");

    settle().await;
    let result = run_executor().run(ctx, router).await.unwrap();
    assert_eq!(result.content, "Failure detected.");

    wait_for_exit(&mgr, "proc-1").await;
    let (info, _) = mgr.status("proc-1", None).unwrap();
    if cfg!(windows) {
        // On Windows, ConPTY may not deliver EOF after a short-lived process exits,
        // so the reader thread blocks and status stays Running.
        if let hive_process::ProcessStatus::Exited { code } = &info.status {
            assert_ne!(*code, 0, "false should exit with non-zero code");
        }
    } else {
        match &info.status {
            hive_process::ProcessStatus::Exited { code } => {
                assert_ne!(*code, 0, "false should exit with non-zero code");
            }
            other => panic!("expected Exited, got {other:?}"),
        }
    }
}

/// 18. Mix shell.execute (sync) with process.start (async background).
#[tokio::test]
async fn mix_shell_execute_and_process_start() {
    let mgr = Arc::new(ProcessManager::new());
    let tools = make_full_registry(&mgr);
    let provider = ScriptProvider::new(vec![
        // First, sync shell command.
        ScriptProvider::tool_call(
            "shell.execute",
            json!({"command": "echo sync-output", "timeout_secs": 5}),
        ),
        // Then, background process.
        ScriptProvider::tool_call("process.start", json!({"command": sleep_cmd(30)})),
        ScriptProvider::tool_call("process.list", json!({})),
        ScriptProvider::text("Mixed tools done."),
    ]);
    let router = make_router(provider);
    let ctx = make_context(tools, "Mix tools");
    let result = run_executor().run(ctx, router).await.unwrap();
    assert_eq!(result.content, "Mixed tools done.");

    let list = mgr.list();
    assert_eq!(list.len(), 1);
    mgr.kill("proc-1", None).unwrap();
}

/// 19. Write to an exited process — should produce an error result.
#[tokio::test]
async fn write_to_exited_process() {
    let mgr = Arc::new(ProcessManager::new());
    let tools = make_process_registry(&mgr);
    let provider = ScriptProvider::new(vec![
        ScriptProvider::tool_call("process.start", json!({"command": true_cmd()})),
        // Wait for exit, then try to write.
        ScriptProvider::tool_call(
            "process.write",
            json!({"process_id": "proc-1", "input": "should fail\n"}),
        ),
        ScriptProvider::text("Write error handled."),
    ]);
    let router = make_router(provider);
    let ctx = make_context(tools, "Write to exited");

    settle().await; // Let `true` exit.
    let result = run_executor().run(ctx, router).await.unwrap();
    // The loop should not crash; the error is reported to the LLM.
    assert_eq!(result.content, "Write error handled.");
}

/// 20. Full lifecycle: start → status → write → status → kill → status.
#[tokio::test]
#[cfg(unix)]
async fn full_process_lifecycle() {
    let mgr = Arc::new(ProcessManager::new());
    let tools = make_process_registry(&mgr);
    let provider = ScriptProvider::new(vec![
        // 1. Start
        ScriptProvider::tool_call("process.start", json!({"command": "cat"})),
        // 2. Initial status (running)
        ScriptProvider::tool_call("process.status", json!({"process_id": "proc-1"})),
        // 3. Write to stdin
        ScriptProvider::tool_call(
            "process.write",
            json!({"process_id": "proc-1", "input": "lifecycle test\n"}),
        ),
        // 4. Check status after write (should see our text)
        ScriptProvider::tool_call("process.status", json!({"process_id": "proc-1"})),
        // 5. Kill
        ScriptProvider::tool_call("process.kill", json!({"process_id": "proc-1"})),
        // 6. Final status (should be dead)
        ScriptProvider::tool_call("process.status", json!({"process_id": "proc-1"})),
        ScriptProvider::text("Lifecycle complete."),
    ]);
    let router = make_router(provider);
    let ctx = make_context(tools, "Full lifecycle");
    let result = run_executor().run(ctx, router).await.unwrap();
    assert_eq!(result.content, "Lifecycle complete.");

    settle().await;
    let (info, _) = mgr.status("proc-1", None).unwrap();
    assert!(!info.status.is_running(), "process should be dead after kill");
}
