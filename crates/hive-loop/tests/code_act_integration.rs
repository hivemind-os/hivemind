//! End-to-end integration tests for the CodeAct strategy.
//!
//! These tests exercise the full CodeAct loop with:
//! - A mock LLM (ScriptProvider) that returns scripted responses
//! - Real Python subprocess execution
//! - The actual LoopExecutor and CodeActStrategy
//!
//! Each test verifies that the loop processes model responses correctly,
//! extracts code blocks, executes them, feeds observations back, and
//! terminates properly.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU8};
use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::{json, Value};
use tempfile::TempDir;

use hive_classification::{ChannelClass, DataClass};
use hive_contracts::{
    CodeActConfig, LoopStrategy as ConfigLoopStrategy, PermissionRule, Persona,
    SessionPermissions, ToolAnnotations, ToolApproval, ToolDefinition, ToolExecutionMode,
    ToolLimitsConfig,
};
use hive_loop::{
    AgentContext, CodeActStrategy, ConversationContext, LoopContext, LoopError,
    LoopEvent, LoopExecutor, RoutingConfig, SecurityContext, ToolsContext,
};
use hive_model::{
    Capability, CompletionChunk, CompletionMessage, CompletionRequest, CompletionResponse,
    CompletionStream, FinishReason, MessageBlock, ModelProvider, ModelRouter, ModelSelection,
    ProviderDescriptor, ProviderKind, RoutingDecision, ToolCallResponse,
};
use hive_tools::{Tool, ToolError, ToolRegistry, ToolResult};

/// Returns true if the WASM Python runtime is available for integration tests.
fn wasm_available() -> bool {
    match (
        std::env::var("PYTHON_WASM_PATH"),
        std::env::var("PYTHON_WASM_STDLIB"),
    ) {
        (Ok(p), Ok(s)) => {
            std::path::Path::new(&p).exists() && std::path::Path::new(&s).exists()
        }
        _ => false,
    }
}

/// Skips the current test if WASM Python runtime is not available.
macro_rules! require_wasm {
    () => {
        if !wasm_available() {
            eprintln!("SKIPPED: WASM Python runtime not available (set PYTHON_WASM_PATH and PYTHON_WASM_STDLIB)");
            return;
        }
    };
}

// ═══════════════════════════════════════════════════════════════════════
//  TEST INFRASTRUCTURE
// ═══════════════════════════════════════════════════════════════════════

/// Queue-based mock provider that records requests for assertions.
struct ScriptProvider {
    responses: Mutex<Vec<CompletionResponse>>,
    recorded_requests: Arc<Mutex<Vec<CompletionRequest>>>,
    descriptor: ProviderDescriptor,
}

impl ScriptProvider {
    fn new(responses: Vec<CompletionResponse>) -> Self {
        Self {
            responses: Mutex::new(responses),
            recorded_requests: Arc::new(Mutex::new(Vec::new())),
            descriptor: ProviderDescriptor {
                id: "test-provider".to_string(),
                name: Some("Test Provider".to_string()),
                kind: ProviderKind::Mock,
                models: vec!["test-model".to_string()],
                model_capabilities: BTreeMap::from([(
                    "test-model".to_string(),
                    [Capability::Chat, Capability::ToolUse].into_iter().collect(),
                )]),
                priority: 100,
                available: true,
            },
        }
    }

    /// Create a provider with OpenAiCompatible kind so supports_tool_history() = true.
    fn new_multi_turn(responses: Vec<CompletionResponse>) -> Self {
        Self {
            responses: Mutex::new(responses),
            recorded_requests: Arc::new(Mutex::new(Vec::new())),
            descriptor: ProviderDescriptor {
                id: "test-provider".to_string(),
                name: Some("Test Provider".to_string()),
                kind: ProviderKind::OpenAiCompatible,
                models: vec!["test-model".to_string()],
                model_capabilities: BTreeMap::from([(
                    "test-model".to_string(),
                    [Capability::Chat, Capability::ToolUse].into_iter().collect(),
                )]),
                priority: 100,
                available: true,
            },
        }
    }

    fn recorder(&self) -> Arc<Mutex<Vec<CompletionRequest>>> {
        Arc::clone(&self.recorded_requests)
    }

    fn text(content: &str) -> CompletionResponse {
        CompletionResponse {
            provider_id: "test-provider".to_string(),
            model: "test-model".to_string(),
            content: content.to_string(),
            tool_calls: vec![],
        }
    }

    /// A response containing a Python code block in the content (CodeAct style).
    fn code_response(text_before: &str, python_code: &str, text_after: &str) -> CompletionResponse {
        let content = format!(
            "{text_before}\n```python\n{python_code}\n```\n{text_after}"
        );
        CompletionResponse {
            provider_id: "test-provider".to_string(),
            model: "test-model".to_string(),
            content,
            tool_calls: vec![],
        }
    }

    /// A response with both code and a native tool call.
    fn code_and_tool(python_code: &str, tool_name: &str, tool_args: Value) -> CompletionResponse {
        let content = format!("```python\n{python_code}\n```");
        CompletionResponse {
            provider_id: "test-provider".to_string(),
            model: "test-model".to_string(),
            content,
            tool_calls: vec![ToolCallResponse {
                id: format!("call-{tool_name}"),
                name: tool_name.to_string(),
                arguments: tool_args,
            }],
        }
    }

    fn tool_call(name: &str, args: Value) -> CompletionResponse {
        CompletionResponse {
            provider_id: "test-provider".to_string(),
            model: "test-model".to_string(),
            content: String::new(),
            tool_calls: vec![ToolCallResponse {
                id: format!("call-{name}"),
                name: name.to_string(),
                arguments: args,
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
        request: &CompletionRequest,
        _selection: &ModelSelection,
    ) -> anyhow::Result<CompletionResponse> {
        self.recorded_requests.lock().push(request.clone());
        let mut responses = self.responses.lock();
        if responses.is_empty() {
            Ok(Self::text("done"))
        } else {
            Ok(responses.remove(0))
        }
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
        };
        Ok(Box::pin(tokio_stream::once(Ok(chunk))))
    }
}

/// Simple mock tool that records inputs and returns a fixed response.
struct MockTool {
    definition: ToolDefinition,
    response: Mutex<Option<Value>>,
    recorded_inputs: Mutex<Vec<Value>>,
}

impl MockTool {
    fn new(id: &str) -> Self {
        Self {
            definition: ToolDefinition {
                id: id.to_string(),
                name: id.to_string(),
                description: format!("Mock tool {id}"),
                input_schema: json!({"type": "object"}),
                output_schema: None,
                channel_class: ChannelClass::Internal,
                side_effects: false,
                approval: ToolApproval::Auto,
                annotations: ToolAnnotations {
                    title: id.to_string(),
                    read_only_hint: Some(true),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(true),
                    open_world_hint: Some(false),
                },
            },
            response: Mutex::new(None),
            recorded_inputs: Mutex::new(Vec::new()),
        }
    }

    fn with_response(self, value: Value) -> Self {
        *self.response.lock() = Some(value);
        self
    }

    fn recorded_inputs(&self) -> Vec<Value> {
        self.recorded_inputs.lock().clone()
    }
}

impl Tool for MockTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(
        &self,
        input: Value,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ToolResult, ToolError>> + Send + '_>,
    > {
        self.recorded_inputs.lock().push(input.clone());
        let response = self.response.lock().clone();
        Box::pin(async move {
            let output = response.unwrap_or(json!({"ok": true}));
            Ok(ToolResult { output, data_class: DataClass::Internal })
        })
    }
}

// ── Helpers ───────────────────────────────────────────────────────────

fn make_router(provider: ScriptProvider) -> Arc<ModelRouter> {
    let mut router = ModelRouter::new();
    router.register_provider(provider);
    Arc::new(router)
}

fn default_routing_decision() -> RoutingDecision {
    RoutingDecision {
        selected: ModelSelection {
            provider_id: "test-provider".to_string(),
            model: "test-model".to_string(),
        },
        fallback_chain: vec![],
        reason: "test".to_string(),
    }
}

fn make_code_act_context(tools: Arc<ToolRegistry>, prompt: &str) -> LoopContext {
    make_code_act_context_with_workspace(tools, prompt, None)
}

fn make_code_act_context_with_workspace(
    tools: Arc<ToolRegistry>,
    prompt: &str,
    workspace: Option<std::path::PathBuf>,
) -> LoopContext {
    let mut permissions = SessionPermissions::new();
    permissions.add_rule(PermissionRule {
        tool_pattern: "*".to_string(),
        scope: "*".to_string(),
        decision: ToolApproval::Auto,
    });

    LoopContext {
        conversation: ConversationContext {
            session_id: "test-codeact-session".to_string(),
            message_id: "test-codeact-msg".to_string(),
            prompt: prompt.to_string(),
            prompt_content_parts: vec![],
            history: Vec::new(),
            conversation_journal: None,
            initial_tool_iterations: 0,
        },
        routing: RoutingConfig {
            required_capabilities: [Capability::Chat].into_iter().collect(),
            preferred_models: None,
            loop_strategy: Some(ConfigLoopStrategy::CodeAct),
            routing_decision: Some(default_routing_decision()),
        },
        security: SecurityContext {
            data_class: DataClass::Internal,
            permissions: Arc::new(Mutex::new(permissions)),
            workspace_classification: None,
            effective_data_class: Arc::new(AtomicU8::new(DataClass::Internal.to_i64() as u8)),
            connector_service: None,
            shadow_mode: false,
        },
        tools_ctx: ToolsContext {
            tools,
            skill_catalog: None,
            knowledge_query_handler: None,
            tool_execution_mode: ToolExecutionMode::default(),
        },
        agent: AgentContext {
            persona: Some(Persona {
                id: "test-codeact-persona".to_string(),
                name: "CodeAct Test".to_string(),
                description: "Test persona for CodeAct".to_string(),
                system_prompt: "You are a CodeAct test agent.".to_string(),
                loop_strategy: ConfigLoopStrategy::CodeAct,
                preferred_models: None,
                allowed_tools: vec![],
                mcp_servers: Vec::new(),
                avatar: None,
                color: None,
                tool_execution_mode: ToolExecutionMode::default(),
                context_map_strategy: hive_contracts::ContextMapStrategy::default(),
                secondary_models: None,
                archived: false,
                bundled: false,
                prompts: Default::default(),
            }),
            agent_orchestrator: None,
            workspace_path: workspace,
            personas: vec![],
            current_agent_id: None,
            parent_agent_id: None,
            keep_alive: false,
            session_messaged: Arc::new(AtomicBool::new(false)),
        },
        tool_limits: ToolLimitsConfig::default(),
        code_act_config: CodeActConfig {
            enabled: true,
            execution_timeout_secs: 10,
            max_output_bytes: 1_048_576,
            idle_timeout_secs: 60,
            max_sessions: 3,
            allow_network: true,
        },
        session_registry: None,
        preempt_signal: None,
        cancellation_token: None,
    }
}

fn make_code_act_executor() -> LoopExecutor {
    LoopExecutor::new(Arc::new(CodeActStrategy))
}

fn registry_with(tools: Vec<Arc<MockTool>>) -> Arc<ToolRegistry> {
    let mut reg = ToolRegistry::new();
    for t in tools {
        reg.register(t as Arc<dyn Tool>).unwrap();
    }
    Arc::new(reg)
}

// ═══════════════════════════════════════════════════════════════════════
//  TESTS
// ═══════════════════════════════════════════════════════════════════════

/// Test 1: Model returns plain text with no code blocks → loop terminates
/// immediately with the text content.
#[tokio::test]
async fn code_act_plain_text_terminates() {
    let provider = ScriptProvider::new(vec![
        ScriptProvider::text("The answer is 42."),
    ]);
    let router = make_router(provider);
    let tools = Arc::new(ToolRegistry::new());
    let ctx = make_code_act_context(tools, "What is the answer?");
    let executor = make_code_act_executor();

    let result = executor.run(ctx, router).await.expect("loop should succeed");
    assert_eq!(result.content, "The answer is 42.");
}

/// Test 2: Model returns a Python code block → code executes → observation
/// fed back → model returns plain text → loop terminates.
/// This is the core CodeAct happy path.
#[tokio::test]
async fn code_act_executes_python_and_returns_result() {
    require_wasm!();
    let provider = ScriptProvider::new(vec![
        // Iteration 0: model writes Python code
        ScriptProvider::code_response(
            "Let me calculate that.",
            "print(2 + 2)",
            "",
        ),
        // Iteration 1: model sees "4" in observation, returns final answer
        ScriptProvider::text("The result is 4."),
    ]);
    let recorder = provider.recorder();
    let router = make_router(provider);
    let tools = Arc::new(ToolRegistry::new());
    let ctx = make_code_act_context(tools, "What is 2+2?");
    let executor = make_code_act_executor();

    let result = executor.run(ctx, router).await.expect("loop should succeed");
    assert_eq!(result.content, "The result is 4.");

    // Verify the observation was fed back in iteration 1
    let requests = recorder.lock();
    assert!(requests.len() >= 2, "Expected at least 2 model calls, got {}", requests.len());
}

/// Test 3: Model writes code that produces an error → error observation
/// fed back → model writes fix → success → terminates.
#[tokio::test]
async fn code_act_handles_execution_error_and_retries() {
    require_wasm!();
    let provider = ScriptProvider::new(vec![
        // Iteration 0: model writes buggy code
        ScriptProvider::code_response(
            "Let me try:",
            "print(1/0)",
            "",
        ),
        // Iteration 1: model sees ZeroDivisionError, writes fix
        ScriptProvider::code_response(
            "I see the error, let me fix:",
            "print('fixed: no division by zero')",
            "",
        ),
        // Iteration 2: model sees success, returns final answer
        ScriptProvider::text("Done, I fixed the division error."),
    ]);
    let recorder = provider.recorder();
    let router = make_router(provider);
    let tools = Arc::new(ToolRegistry::new());
    let ctx = make_code_act_context(tools, "Divide 1 by 0");
    let executor = make_code_act_executor();

    let result = executor.run(ctx, router).await.expect("loop should succeed");
    assert_eq!(result.content, "Done, I fixed the division error.");

    let requests = recorder.lock();
    assert!(requests.len() >= 3, "Expected at least 3 model calls for error+retry");
}

/// Test 4: Code execution instructions include network access section
/// when allow_network is true.
#[tokio::test]
async fn code_act_prompt_includes_network_access() {
    let provider = ScriptProvider::new(vec![
        ScriptProvider::text("I have network access."),
    ]);
    let recorder = provider.recorder();
    let router = make_router(provider);
    let tools = Arc::new(ToolRegistry::new());
    let mut ctx = make_code_act_context(tools, "Do you have network?");
    ctx.code_act_config.allow_network = true;
    let executor = make_code_act_executor();

    let _ = executor.run(ctx, router).await.expect("loop should succeed");

    let requests = recorder.lock();
    assert!(!requests.is_empty());
    // The first request's prompt should contain the network access section
    let first_prompt = &requests[0].prompt;
    let all_messages: String = requests[0].messages.iter().map(|m| m.content.clone()).collect();
    let combined = format!("{first_prompt}\n{all_messages}");
    assert!(
        combined.contains("Network Access"),
        "Prompt should contain Network Access section. Got:\n{}",
        &combined[..combined.len().min(2000)]
    );
    assert!(
        combined.contains("urllib") || combined.contains("not available"),
        "Prompt should mention urllib (either as available or explicitly unavailable)"
    );
}

/// Test 5: Code execution instructions do NOT include network section
/// when allow_network is false.
#[tokio::test]
async fn code_act_prompt_excludes_network_when_disabled() {
    let provider = ScriptProvider::new(vec![
        ScriptProvider::text("No network."),
    ]);
    let recorder = provider.recorder();
    let router = make_router(provider);
    let tools = Arc::new(ToolRegistry::new());
    let mut ctx = make_code_act_context(tools, "Check network");
    ctx.code_act_config.allow_network = false;
    let executor = make_code_act_executor();

    let _ = executor.run(ctx, router).await.expect("loop should succeed");

    let requests = recorder.lock();
    let first_prompt = &requests[0].prompt;
    let all_messages: String = requests[0].messages.iter().map(|m| m.content.clone()).collect();
    let combined = format!("{first_prompt}\n{all_messages}");
    assert!(
        !combined.contains("Network Access"),
        "Prompt should NOT contain Network Access when disabled"
    );
}

/// Test 6: Code act instructions include workspace path when set.
#[tokio::test]
async fn code_act_prompt_includes_workspace_path() {
    let tmp = TempDir::new().unwrap();
    let workspace_path = tmp.path().to_path_buf();

    let provider = ScriptProvider::new(vec![
        ScriptProvider::text("I know where I am."),
    ]);
    let recorder = provider.recorder();
    let router = make_router(provider);
    let tools = Arc::new(ToolRegistry::new());
    let ctx = make_code_act_context_with_workspace(
        tools,
        "Where are you?",
        Some(workspace_path.clone()),
    );
    let executor = make_code_act_executor();

    let _ = executor.run(ctx, router).await.expect("loop should succeed");

    let requests = recorder.lock();
    let first_prompt = &requests[0].prompt;
    let all_messages: String = requests[0].messages.iter().map(|m| m.content.clone()).collect();
    let combined = format!("{first_prompt}\n{all_messages}");
    assert!(
        combined.contains("Working Directory") || combined.contains("/workspace"),
        "Prompt should contain Working Directory section or /workspace path"
    );
    // In WASI mode, the prompt shows /workspace (the guest path), not the host path
    assert!(
        combined.contains("/workspace"),
        "Prompt should contain the WASI workspace path '/workspace'"
    );
}

/// Test 7: CodeAct instructions are included on EVERY iteration, not just
/// the first. This is critical — if the model uses a tool on iteration 0,
/// the code_act_instructions must still be present on iteration 1.
#[tokio::test]
async fn code_act_instructions_persist_across_iterations() {
    require_wasm!();
    let provider = ScriptProvider::new(vec![
        // Iteration 0: model writes code
        ScriptProvider::code_response("", "print('hello')", ""),
        // Iteration 1: model writes more code — instructions must still be present
        ScriptProvider::code_response("", "print('world')", ""),
        // Iteration 2: done
        ScriptProvider::text("All done."),
    ]);
    let recorder = provider.recorder();
    let router = make_router(provider);
    let tools = Arc::new(ToolRegistry::new());
    let ctx = make_code_act_context(tools, "Say hello and world");
    let executor = make_code_act_executor();

    let result = executor.run(ctx, router).await.expect("loop should succeed");
    assert_eq!(result.content, "All done.");

    let requests = recorder.lock();
    assert!(requests.len() >= 3, "Expected at least 3 model calls");

    // Check that iteration 1's request still has the code execution instructions
    // We check for "Bias toward action" which only appears in the CodeAct instructions,
    // NOT in observation text (which contains "[Code Execution Output]").
    let second_prompt = &requests[1].prompt;
    let second_messages: String = requests[1].messages.iter().map(|m| m.content.clone()).collect();
    let combined = format!("{second_prompt}\n{second_messages}");
    assert!(
        combined.contains("Bias toward action") || combined.contains("persistent Python environment")
            || combined.contains("fresh environment"),
        "Iteration 1 should still have CodeAct instructions in the prompt.\nGot:\n{}",
        &combined[..combined.len().min(2000)]
    );
}

/// Test 8: Code writes a file to a temp workspace and the file actually exists.
/// Verifies end-to-end: model response → code extraction → subprocess execution
/// → real file I/O.
#[tokio::test]
async fn code_act_writes_file_to_workspace() {
    require_wasm!();
    let tmp = TempDir::new().unwrap();
    let workspace_path = tmp.path().to_path_buf();
    let file_path = workspace_path.join("output.txt");

    // The Python code writes a file to the workspace
    let python_code = format!(
        "with open('output.txt', 'w') as f:\n    f.write('hello from codeact')\nprint('file written')"
    );

    let provider = ScriptProvider::new(vec![
        ScriptProvider::code_response("Writing file:", &python_code, ""),
        ScriptProvider::text("File has been written."),
    ]);
    let router = make_router(provider);
    let tools = Arc::new(ToolRegistry::new());
    let ctx = make_code_act_context_with_workspace(
        tools,
        "Write hello to output.txt",
        Some(workspace_path.clone()),
    );
    let executor = make_code_act_executor();

    let result = executor.run(ctx, router).await.expect("loop should succeed");
    assert_eq!(result.content, "File has been written.");

    // Verify the file was actually created in the workspace
    assert!(
        file_path.exists(),
        "Expected output.txt to exist in workspace at {:?}",
        file_path
    );
    let content = std::fs::read_to_string(&file_path).unwrap();
    assert_eq!(content, "hello from codeact");
}

/// Test 9: Code act with events — verify that CodeExecution events are emitted
/// for both start and completion phases.
#[tokio::test]
async fn code_act_emits_execution_events() {
    require_wasm!();
    let provider = ScriptProvider::new(vec![
        ScriptProvider::code_response("", "print('event test')", ""),
        ScriptProvider::text("Done."),
    ]);
    let router = make_router(provider);
    let tools = Arc::new(ToolRegistry::new());
    let ctx = make_code_act_context(tools, "Run code");
    let executor = make_code_act_executor();

    let (tx, mut rx) = tokio::sync::mpsc::channel::<LoopEvent>(100);
    let _ = executor.run_with_events(ctx, router, tx, None).await.expect("loop should succeed");

    // Collect all events
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }

    // Should have CodeExecution events (Started + Completed)
    let code_exec_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, LoopEvent::CodeExecution { .. }))
        .collect();
    assert!(
        code_exec_events.len() >= 2,
        "Expected at least 2 CodeExecution events (Started + Completed), got {}",
        code_exec_events.len()
    );
}

/// Test 10: Budget enforcement — CodeAct loop respects tool iteration limits.
#[tokio::test]
async fn code_act_respects_budget_limit() {
    require_wasm!();
    // Set up a model that always returns code (never terminates naturally)
    let mut responses = Vec::new();
    for i in 0..50 {
        responses.push(ScriptProvider::code_response(
            "",
            &format!("print('iteration {i}')"),
            "",
        ));
    }
    let provider = ScriptProvider::new(responses);
    let router = make_router(provider);
    let tools = Arc::new(ToolRegistry::new());
    let mut ctx = make_code_act_context(tools, "Loop forever");
    // Set a low ceiling to trigger HardStop
    ctx.tool_limits = ToolLimitsConfig {
        soft_limit: 3,
        hard_ceiling: 5,
        extension_chunk: 1,
        stall_window: 20,
        stall_threshold: 100,
    };
    let executor = make_code_act_executor();

    let result = executor.run(ctx, router).await;
    assert!(
        matches!(result, Err(LoopError::HardCeilingReached { .. })),
        "Expected HardCeilingReached error, got {:?}",
        result
    );
}

/// Test 11: Multiple code blocks in a single model response are all executed.
#[tokio::test]
async fn code_act_executes_multiple_code_blocks() {
    require_wasm!();
    let provider = ScriptProvider::new(vec![
        // Model returns TWO code blocks in one response
        ScriptProvider::text(
            "Let me do two things:\n\
             ```python\nprint('block1')\n```\n\
             And also:\n\
             ```python\nprint('block2')\n```"
        ),
        // Sees both outputs, returns final answer
        ScriptProvider::text("Both blocks executed."),
    ]);
    let recorder = provider.recorder();
    let router = make_router(provider);
    let tools = Arc::new(ToolRegistry::new());
    let ctx = make_code_act_context(tools, "Do two things");
    let executor = make_code_act_executor();

    let result = executor.run(ctx, router).await.expect("loop should succeed");
    assert_eq!(result.content, "Both blocks executed.");

    // Check that the observation in iteration 1 contains both outputs
    let requests = recorder.lock();
    assert!(requests.len() >= 2);
    let second = &requests[1];
    let all_content: String = second.messages.iter().map(|m| m.content.clone()).collect();
    let combined = format!("{}\n{}", second.prompt, all_content);
    assert!(
        combined.contains("block1") && combined.contains("block2"),
        "Observation should contain output from both blocks"
    );
}

/// Test 12: The "bias toward action" instruction is present in the prompt.
#[tokio::test]
async fn code_act_prompt_has_action_bias() {
    let provider = ScriptProvider::new(vec![
        ScriptProvider::text("OK."),
    ]);
    let recorder = provider.recorder();
    let router = make_router(provider);
    let tools = Arc::new(ToolRegistry::new());
    let ctx = make_code_act_context(tools, "test");
    let executor = make_code_act_executor();

    let _ = executor.run(ctx, router).await.expect("loop should succeed");

    let requests = recorder.lock();
    let first_prompt = &requests[0].prompt;
    let all_messages: String = requests[0].messages.iter().map(|m| m.content.clone()).collect();
    let combined = format!("{first_prompt}\n{all_messages}");
    assert!(
        combined.contains("Bias toward action") || combined.contains("act immediately")
            || combined.contains("Act immediately"),
        "Prompt should contain action bias instruction"
    );
}

/// Test 13: Completion rules tell the agent not to present follow-up menus.
#[tokio::test]
async fn code_act_prompt_forbids_followup_menus() {
    let provider = ScriptProvider::new(vec![
        ScriptProvider::text("OK."),
    ]);
    let recorder = provider.recorder();
    let router = make_router(provider);
    let tools = Arc::new(ToolRegistry::new());
    let ctx = make_code_act_context(tools, "test");
    let executor = make_code_act_executor();

    let _ = executor.run(ctx, router).await.expect("loop should succeed");

    let requests = recorder.lock();
    let first_prompt = &requests[0].prompt;
    let all_messages: String = requests[0].messages.iter().map(|m| m.content.clone()).collect();
    let combined = format!("{first_prompt}\n{all_messages}");
    assert!(
        combined.contains("do next") || combined.contains("follow-up menus")
            || combined.contains("present follow-up"),
        "Completion rules should mention not asking 'what next'"
    );
}

/// Test 14: Multi-turn native tool calls produce proper ToolUse/ToolResult blocks.
/// When the model makes a native tool call (e.g., ask_user), the next iteration
/// must see structured ToolUse blocks in the assistant message and ToolResult
/// blocks in the tool message — NOT a bare "user" message with raw XML.
/// This is the bug that caused the ask_user infinite loop with OpenAI models.
#[tokio::test]
async fn code_act_native_tool_call_produces_structured_history() {
    // Use multi-turn provider (OpenAiCompatible → supports_tool_history = true)
    let provider = ScriptProvider::new_multi_turn(vec![
        // Iteration 0: model makes a native tool call
        ScriptProvider::tool_call("mock_question_tool", json!({
            "question": "What format?",
            "choices": ["JSON", "Text"],
        })),
        // Iteration 1: model sees the structured tool result and finishes
        ScriptProvider::text("OK, I'll use JSON format."),
    ]);
    let recorder = provider.recorder();
    let router = make_router(provider);

    // Register a mock tool that returns an answer
    let ask_tool = Arc::new(
        MockTool::new("mock_question_tool").with_response(json!({"answer": "JSON"})),
    );
    let tools = registry_with(vec![ask_tool]);
    let ctx = make_code_act_context(tools, "Save weather to file");
    let executor = make_code_act_executor();

    let result = executor.run(ctx, router).await.expect("loop should succeed");
    assert_eq!(result.content, "OK, I'll use JSON format.");

    // Verify the second request has proper structured messages
    let requests = recorder.lock();
    assert!(requests.len() >= 2, "Expected at least 2 model calls, got {}", requests.len());

    let second_request = &requests[1];
    // In multi-turn mode, messages should contain:
    // 1. The user prompt message
    // 2. An assistant message with ToolUse blocks
    // 3. A tool message with ToolResult blocks

    let has_tool_use = second_request.messages.iter().any(|m| {
        m.blocks.iter().any(|b| matches!(b, MessageBlock::ToolUse { name, .. } if name == "mock_question_tool"))
    });
    assert!(
        has_tool_use,
        "Second request should have an assistant message with ToolUse block for mock_question_tool.\nMessages: {:?}",
        second_request.messages.iter().map(|m| (&m.role, &m.blocks)).collect::<Vec<_>>()
    );

    let has_tool_result = second_request.messages.iter().any(|m| {
        m.role == "tool" && m.blocks.iter().any(|b| matches!(b, MessageBlock::ToolResult { .. }))
    });
    assert!(
        has_tool_result,
        "Second request should have a tool message with ToolResult block.\nMessages: {:?}",
        second_request.messages.iter().map(|m| (&m.role, &m.blocks)).collect::<Vec<_>>()
    );

    // The tool result content should contain the user's answer
    let tool_result_content: String = second_request.messages.iter()
        .filter(|m| m.role == "tool")
        .map(|m| m.content.clone())
        .collect();
    assert!(
        tool_result_content.contains("JSON"),
        "Tool result should contain the user's answer 'JSON'. Got: {tool_result_content}"
    );
}
