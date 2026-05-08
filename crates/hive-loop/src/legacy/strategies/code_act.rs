use std::collections::HashMap;
use std::sync::Arc;

use hive_contracts::prompt_sanitize;
use hive_model::{
    CompletionMessage, CompletionRequest, CompletionResponse, MessageBlock, RetryInfo,
    RoutingRequest,
};
use tokio_stream::StreamExt;

use super::super::interaction::UserInteractionGate;
use super::super::journal::{JournalEntry, JournalPhase, JournalToolCall};
use super::super::parsing::ToolCall;
use super::super::strategy::{LoopMiddleware, LoopStrategy};
use super::super::streaming::StreamingToolCallFilter;
use super::super::tool_execution::{execute_tool_batch, execute_tool_call};
use super::super::types::{
    BoxFuture, CodeExecutionPhase, LoopContext, LoopError, LoopEvent, LoopResult,
};
use super::super::{model_router_error_to_loop_error, simple_model_error};

// ── BridgedToolCallHandler: dispatches tool calls from Python to the ToolRegistry ──

/// Adapter that implements `ToolCallHandler` by dispatching to the `ToolRegistry`.
/// This is how Python code calls host tools via the bridge protocol.
///
/// Unlike the old implementation that called `tool.execute()` directly, this
/// routes every bridged tool call through `execute_tool_call` — the same
/// pipeline used by native tool calls — so that permission checks, approval
/// gates, middleware hooks, data-class enforcement, shadow-mode interception,
/// and connector rules all apply.
struct BridgedToolCallHandler {
    context: Arc<LoopContext>,
    middleware: Vec<Arc<dyn LoopMiddleware>>,
    event_tx: Option<tokio::sync::mpsc::Sender<LoopEvent>>,
    interaction_gate: Option<Arc<UserInteractionGate>>,
}

#[async_trait::async_trait]
impl hive_code_executor::ToolCallHandler for BridgedToolCallHandler {
    async fn handle_tool_call(
        &self,
        request: hive_code_executor::ToolCallRequest,
    ) -> hive_code_executor::ToolCallResponse {
        let call = ToolCall { tool_id: request.tool_id.clone(), input: request.args.clone() };

        let input_str =
            serde_json::to_string(&request.args).unwrap_or_else(|_| "<unserializable>".to_string());

        // Emit ToolCallStart so the frontend can track bridged tool calls.
        if let Some(ref tx) = self.event_tx {
            let _ = tx.try_send(LoopEvent::ToolCallStart {
                tool_id: request.tool_id.clone(),
                input: input_str,
            });
        }

        let gate_ref = self.interaction_gate.as_deref();
        let tx_ref = self.event_tx.as_ref();

        let response = match execute_tool_call(
            &self.context,
            call,
            &self.middleware,
            tx_ref,
            gate_ref,
            None, // assistant_content — not applicable for bridged calls
        )
        .await
        {
            Ok(result) => {
                let output_str = serde_json::to_string(&result.output)
                    .unwrap_or_else(|_| "<unserializable>".to_string());
                if let Some(ref tx) = self.event_tx {
                    let _ = tx.try_send(LoopEvent::ToolCallResult {
                        tool_id: request.tool_id.clone(),
                        output: output_str,
                        is_error: false,
                    });
                }
                hive_code_executor::ToolCallResponse {
                    request_id: request.request_id,
                    result: Some(serde_json::json!(result.output)),
                    error: None,
                    truncated: false,
                }
            }
            Err(LoopError::ToolDenied { reason, .. }) => {
                let err_msg = format!("Tool denied: {}", reason);
                if let Some(ref tx) = self.event_tx {
                    let _ = tx.try_send(LoopEvent::ToolCallResult {
                        tool_id: request.tool_id.clone(),
                        output: err_msg.clone(),
                        is_error: true,
                    });
                }
                hive_code_executor::ToolCallResponse {
                    request_id: request.request_id,
                    result: None,
                    error: Some(err_msg),
                    truncated: false,
                }
            }
            Err(e) => {
                let err_msg = e.to_string();
                if let Some(ref tx) = self.event_tx {
                    let _ = tx.try_send(LoopEvent::ToolCallResult {
                        tool_id: request.tool_id.clone(),
                        output: err_msg.clone(),
                        is_error: true,
                    });
                }
                hive_code_executor::ToolCallResponse {
                    request_id: request.request_id,
                    result: None,
                    error: Some(err_msg),
                    truncated: false,
                }
            }
        };

        response
    }
}

#[derive(Default)]
pub struct CodeActStrategy;

impl LoopStrategy for CodeActStrategy {
    fn run<'a>(
        &'a self,
        context: LoopContext,
        model_router: Arc<hive_model::ModelRouter>,
        middleware: &'a [Arc<dyn LoopMiddleware>],
        event_tx: Option<tokio::sync::mpsc::Sender<LoopEvent>>,
        interaction_gate: Option<Arc<UserInteractionGate>>,
    ) -> BoxFuture<'a, Result<LoopResult, LoopError>> {
        Box::pin(async move {
            use crate::code_act_prompt::build_code_act_instructions;
            use crate::code_extraction::extract_python_blocks;
            use hive_code_executor::{
                BridgedToolInfo, CodeActToolMode, CodeExecutor, ExecutionOptions, ExecutorConfig,
                Language, WasmExecutor,
            };

            // ── Route the model ──────────────────────────────────────
            let routing_request = RoutingRequest {
                prompt: context.conversation.prompt.clone(),
                required_capabilities: context.routing.required_capabilities.clone(),
                preferred_models: context.routing.preferred_models.clone(),
            };
            let decision = if let Some(decision) = context.routing.routing_decision.clone() {
                decision
            } else {
                model_router
                    .route(&routing_request)
                    .map_err(|error| LoopError::ModelRouting(error.to_string()))?
            };

            let mut context = context;
            context.routing.routing_decision = Some(decision.clone());

            // Wrap in Arc so the BridgedToolCallHandler can share the context
            // (it needs to call execute_tool_call which takes &LoopContext).
            // All mutations to context happen above this point.
            let context = Arc::new(context);

            let use_multi_turn = model_router
                .provider_kind(&decision.selected.provider_id)
                .map(|k| k.supports_tool_history())
                .unwrap_or(false);

            tracing::info!(
                provider_id = %decision.selected.provider_id,
                model = %decision.selected.model,
                use_multi_turn,
                "CodeAct loop starting"
            );

            // ── Classify tools: native vs bridged ────────────────────
            let all_tools = context.tools_ctx.tools.list_definitions();
            let mut bridged_tools = Vec::new();
            let mut native_tool_ids = Vec::new();
            let mut native_tool_defs = Vec::new();

            for def in &all_tools {
                let mode = hive_code_executor::tool_bridge::default_tool_mode(&def.id);
                match mode {
                    CodeActToolMode::Excluded => {
                        // Python handles these natively in WASM — skip entirely
                    }
                    CodeActToolMode::Native => {
                        native_tool_ids.push(def.id.clone());
                        native_tool_defs.push(def.clone());
                    }
                    CodeActToolMode::Bridged | CodeActToolMode::Both => {
                        bridged_tools.push(BridgedToolInfo {
                            tool_id: def.id.clone(),
                            description: def.description.clone(),
                            input_schema: def.input_schema.clone(),
                            mode,
                        });
                        if mode == CodeActToolMode::Both {
                            native_tool_ids.push(def.id.clone());
                            native_tool_defs.push(def.clone());
                        }
                    }
                }
            }

            // ── Build CodeAct system prompt supplement ────────────────
            let has_persistent_session = context.session_registry.is_some();
            let ca_cfg = &context.code_act_config;
            let workspace_str = context.workspace_path().map(|p| p.to_string_lossy().to_string());
            tracing::info!(
                allow_network = ca_cfg.allow_network,
                workspace = ?workspace_str,
                persistent = has_persistent_session,
                bridged_count = bridged_tools.len(),
                native_count = native_tool_ids.len(),
                "CodeAct: building system prompt supplement"
            );
            let code_act_instructions = build_code_act_instructions(
                &bridged_tools,
                &native_tool_ids,
                has_persistent_session,
                ca_cfg.allow_network,
                workspace_str.as_deref(),
            );
            tracing::debug!(
                instructions_len = code_act_instructions.len(),
                has_network_section = code_act_instructions.contains("Network Access"),
                has_workspace_section = code_act_instructions.contains("Working Directory"),
                "CodeAct: prompt supplement built"
            );

            // ── Prepare lazy code executor ─────────────────────────
            // The executor is initialized on-demand when the first code block
            // is encountered, not eagerly. This allows the CodeAct loop to
            // handle plain-text-only responses without requiring WASM.
            let session_id = context.conversation.session_id.clone();
            let using_registry = context.session_registry.is_some();
            let mut executor: Option<Arc<dyn CodeExecutor>> = None;

            // ── Build tool call handler for Python→host tool bridge ───
            let tool_handler = BridgedToolCallHandler {
                context: Arc::clone(&context),
                middleware: middleware.to_vec(),
                event_tx: event_tx.clone(),
                interaction_gate: interaction_gate.clone(),
            };
            let exec_options = ExecutionOptions { tool_call_handler: Some(&tool_handler) };

            // Bridge code is generated once but injected lazily when executor is first created
            let bridge_code = hive_code_executor::tool_bridge::generate_bridge_code(&bridged_tools);

            // ── Main loop ────────────────────────────────────────────
            let mut prompt = context.conversation.prompt.clone();
            let mut prompt_content_parts = context.conversation.prompt_content_parts.clone();
            let mut tool_iterations = context.conversation.initial_tool_iterations;
            let mut tool_history: Vec<CompletionMessage> = Vec::new();
            let mut budget = crate::tool_budget::AdaptiveBudget::new(&context.tool_limits);

            loop {
                // CodeAct instructions go into the SYSTEM message so models
                // treat them as authoritative directives, not user chatter.
                // The user's actual request stays in a clean user message.
                let mut request = if use_multi_turn {
                    let mut messages = context.conversation.history.clone();
                    // Inject CodeAct instructions into the system message.
                    if let Some(sys_msg) = messages.iter_mut().find(|m| m.role == "system") {
                        sys_msg.content =
                            format!("{}\n\n{}", sys_msg.content, code_act_instructions);
                    } else {
                        // No system message in history — add one with just the instructions.
                        messages.insert(
                            0,
                            CompletionMessage {
                                role: "system".into(),
                                content: code_act_instructions.clone(),
                                content_parts: vec![],
                                blocks: vec![],
                            },
                        );
                    }
                    let user_msg = CompletionMessage {
                        role: "user".into(),
                        content: prompt.clone(),
                        content_parts: std::mem::take(&mut prompt_content_parts),
                        blocks: vec![],
                    };
                    messages.push(user_msg);
                    messages.extend(tool_history.iter().cloned());
                    CompletionRequest {
                        prompt: String::new(),
                        prompt_content_parts: vec![],
                        messages,
                        required_capabilities: context.routing.required_capabilities.clone(),
                        preferred_models: context.routing.preferred_models.clone(),
                        tools: native_tool_defs.clone(),
                    }
                } else {
                    // Non-multi-turn: prompt goes as the final user message via
                    // the provider's message builder. Prepend CodeAct instructions
                    // to the system message in history.
                    let mut history = context.conversation.history.clone();
                    if let Some(sys_msg) = history.iter_mut().find(|m| m.role == "system") {
                        sys_msg.content =
                            format!("{}\n\n{}", sys_msg.content, code_act_instructions);
                    } else {
                        history.insert(
                            0,
                            CompletionMessage {
                                role: "system".into(),
                                content: code_act_instructions.clone(),
                                content_parts: vec![],
                                blocks: vec![],
                            },
                        );
                    }
                    CompletionRequest {
                        prompt: prompt.clone(),
                        prompt_content_parts: std::mem::take(&mut prompt_content_parts),
                        messages: history,
                        required_capabilities: context.routing.required_capabilities.clone(),
                        preferred_models: context.routing.preferred_models.clone(),
                        tools: native_tool_defs.clone(),
                    }
                };

                for hook in middleware {
                    request = hook.before_model_call(&context, request)?;
                }

                // ── Call the model (streaming if event_tx is present) ──
                let response = if let Some(ref tx) = event_tx {
                    let router = Arc::clone(&model_router);
                    let decision_clone = decision.clone();

                    let _ = tx.try_send(LoopEvent::ModelLoading {
                        provider_id: decision_clone.selected.provider_id.clone(),
                        model: decision_clone.selected.model.clone(),
                        tool_result_counts: HashMap::new(),
                        estimated_tokens: Some(crate::token_budget::estimate_request_tokens(
                            &request,
                        ) as u32),
                    });

                    let retry_cb = |info: &RetryInfo| {
                        let _ = tx.try_send(LoopEvent::ModelRetry {
                            provider_id: info.provider_id.clone(),
                            model: info.model.clone(),
                            attempt: info.attempt,
                            max_attempts: info.max_attempts,
                            error_kind: format!("{:?}", info.error_kind).to_lowercase(),
                            http_status: info.http_status,
                            backoff_ms: info.backoff_ms,
                        });
                    };

                    let (stream, actual_selection) = router
                        .complete_stream_with_decision_and_callback(
                            &request,
                            &decision_clone,
                            Some(&retry_cb),
                        )
                        .map_err(model_router_error_to_loop_error)?;

                    if actual_selection != decision_clone.selected {
                        let _ = tx.try_send(LoopEvent::ModelFallback {
                            from_provider: decision_clone.selected.provider_id.clone(),
                            from_model: decision_clone.selected.model.clone(),
                            to_provider: actual_selection.provider_id.clone(),
                            to_model: actual_selection.model.clone(),
                        });
                    }

                    let mut content = String::new();
                    let provider_id = actual_selection.provider_id.clone();
                    let model = actual_selection.model.clone();
                    let mut streamed_tool_calls = Vec::new();
                    let mut token_filter = StreamingToolCallFilter::new();
                    let mut streamed_usage: Option<hive_model::CompletionUsage> = None;

                    tokio::pin!(stream);
                    let stream_cancelled;
                    loop {
                        let chunk_result = if let Some(ref token) = context.cancellation_token {
                            tokio::select! {
                                biased;
                                _ = token.cancelled() => {
                                    stream_cancelled = true;
                                    break;
                                }
                                chunk = stream.next() => chunk,
                            }
                        } else {
                            stream.next().await
                        };
                        match chunk_result {
                            Some(Ok(chunk)) => {
                                if !chunk.delta.is_empty() {
                                    content.push_str(&chunk.delta);
                                    let visible = token_filter.feed(&chunk.delta);
                                    if !visible.is_empty() {
                                        let _ = tx.try_send(LoopEvent::Token { delta: visible });
                                    }
                                }
                                // Capture provider-reported usage from final chunk.
                                if let Some(usage) = chunk.usage {
                                    streamed_usage = Some(usage);
                                }
                                if !chunk.tool_calls.is_empty() {
                                    streamed_tool_calls.extend(chunk.tool_calls);
                                }
                            }
                            Some(Err(e)) => return Err(simple_model_error(format!("{:#}", e))),
                            None => {
                                stream_cancelled = false;
                                break;
                            }
                        }
                    }
                    if stream_cancelled {
                        return Err(LoopError::Cancelled);
                    }

                    let remaining = token_filter.flush();
                    if !remaining.is_empty() {
                        let _ = tx.try_send(LoopEvent::Token { delta: remaining });
                    }
                    let _ = tx.try_send(LoopEvent::ModelDone {
                        content: content.clone(),
                        provider_id: provider_id.clone(),
                        model: model.clone(),
                        input_tokens: streamed_usage.as_ref().map(|u| u.input_tokens).filter(|&t| t > 0),
                        output_tokens: streamed_usage.as_ref().map(|u| u.output_tokens).filter(|&t| t > 0),
                    });

                    CompletionResponse {
                        provider_id,
                        model,
                        content,
                        tool_calls: streamed_tool_calls,
                        usage: streamed_usage,
                    }
                } else {
                    let router = Arc::clone(&model_router);
                    let decision_clone = decision.clone();
                    let request_clone = request.clone();
                    let blocking_future = tokio::task::spawn_blocking(move || {
                        router.complete_with_decision(&request_clone, &decision_clone)
                    });
                    if let Some(ref token) = context.cancellation_token {
                        tokio::select! {
                            biased;
                            _ = token.cancelled() => {
                                return Err(LoopError::Cancelled);
                            }
                            result = blocking_future => {
                                result
                                    .map_err(|error| LoopError::JoinFailed(error.to_string()))?
                                    .map_err(model_router_error_to_loop_error)?
                            }
                        }
                    } else {
                        blocking_future
                            .await
                            .map_err(|error| LoopError::JoinFailed(error.to_string()))?
                            .map_err(model_router_error_to_loop_error)?
                    }
                };

                let mut response = response;
                for hook in middleware {
                    response = hook.after_model_response(&context, response)?;
                }

                // ── Extract code blocks and native tool calls ────────
                let code_blocks = extract_python_blocks(&response.content);
                let native_calls: Vec<ToolCall> = if !response.tool_calls.is_empty() {
                    response
                        .tool_calls
                        .iter()
                        .map(|tc| ToolCall {
                            tool_id: tc.name.clone(),
                            input: tc.arguments.clone(),
                        })
                        .collect()
                } else {
                    Vec::new()
                };

                tracing::info!(
                    iteration = tool_iterations,
                    code_blocks = code_blocks.len(),
                    native_calls = native_calls.len(),
                    "CodeAct: model response processed"
                );

                // If no code blocks and no native tool calls → done
                if code_blocks.is_empty() && native_calls.is_empty() {
                    // Only shutdown if we created a one-shot executor (no registry).
                    if !using_registry {
                        if let Some(ref exec) = executor {
                            let _ = exec.shutdown().await;
                        }
                    }
                    if let Some(ref tx) = event_tx {
                        let _ = tx.try_send(LoopEvent::Done {
                            content: response.content.clone(),
                            provider_id: response.provider_id.clone(),
                            model: response.model.clone(),
                        });
                    }
                    return Ok(LoopResult {
                        content: response.content,
                        provider_id: response.provider_id.clone(),
                        model: response.model.clone(),
                        decision: decision.clone(),
                        preempted: false,
                    });
                }

                // ── Budget check ─────────────────────────────────────
                let action_count = code_blocks.len() + native_calls.len();
                match budget.check(tool_iterations, action_count) {
                    crate::tool_budget::BudgetDecision::Allow => {}
                    crate::tool_budget::BudgetDecision::Extended {
                        new_budget,
                        extensions_granted,
                    } => {
                        if let Some(ref tx) = event_tx {
                            let _ = tx.try_send(LoopEvent::BudgetExtended {
                                new_budget,
                                extensions_granted,
                            });
                        }
                    }
                    crate::tool_budget::BudgetDecision::HardStop { ceiling } => {
                        if !using_registry {
                            if let Some(ref exec) = executor {
                                let _ = exec.shutdown().await;
                            }
                        }
                        return Err(LoopError::HardCeilingReached { ceiling });
                    }
                }

                let mut observations = Vec::new();

                // ── Lazy executor init on first code block ───────────
                if !code_blocks.is_empty() && executor.is_none() {
                    let exec = if let Some(ref registry) = context.session_registry {
                        let session = registry
                            .get_or_create(&session_id, workspace_str.as_deref())
                            .await
                            .map_err(|e| {
                                simple_model_error(format!("failed to get code session: {e}"))
                            })?;
                        session.executor_arc()
                    } else {
                        let exec_config = ExecutorConfig {
                            execution_timeout_secs: ca_cfg.execution_timeout_secs,
                            max_output_bytes: ca_cfg.max_output_bytes,
                            memory_limit_mb: 256,
                            working_directory: workspace_str.clone(),
                            allow_network: ca_cfg.allow_network,
                        };
                        let wasm_paths = hive_code_executor::resolve_python_wasm(None);
                        match wasm_paths {
                            Some(paths) => {
                                let e = WasmExecutor::new(
                                    exec_config,
                                    &paths.wasm_binary,
                                    &paths.stdlib_dir,
                                )
                                .await
                                .map_err(|e| {
                                    simple_model_error(format!(
                                        "failed to start WASM executor: {e}"
                                    ))
                                })?;
                                tracing::info!(
                                    "CodeAct: using WASM-sandboxed Python executor (one-shot)"
                                );
                                Arc::new(e) as Arc<dyn CodeExecutor>
                            }
                            None => {
                                return Err(simple_model_error(
                                    "CodeAct runtime is still downloading. Please wait a moment and try again.".into()
                                ));
                            }
                        }
                    };

                    // Inject tool bridge code into the fresh executor
                    if !bridge_code.trim().is_empty() {
                        let init_result = exec
                            .execute_with_tools(&bridge_code, Language::Python, &exec_options)
                            .await
                            .map_err(|e| {
                                simple_model_error(format!("failed to initialize tool bridge: {e}"))
                            })?;
                        if init_result.is_error {
                            tracing::warn!(
                                stderr = %init_result.stderr,
                                "tool bridge initialization had errors"
                            );
                        }
                    }

                    executor = Some(exec);
                }

                // ── Executor liveness check + recovery ───────────────
                if let Some(ref exec) = executor {
                    if !exec.is_alive().await {
                        tracing::warn!("CodeAct executor is dead — attempting recovery");
                        if let Err(e) = exec.reset().await {
                            let err_msg = format!("[Code Executor Recovery Failed]\nExecutor died and could not be restarted: {e}");
                            observations.push(err_msg);
                        } else {
                            // Re-inject bridge code after recovery
                            if !bridge_code.trim().is_empty() {
                                match exec
                                    .execute_with_tools(
                                        &bridge_code,
                                        Language::Python,
                                        &exec_options,
                                    )
                                    .await
                                {
                                    Ok(r) if r.is_error => {
                                        tracing::warn!(stderr = %r.stderr, "bridge re-init had errors after recovery");
                                    }
                                    Err(e) => {
                                        tracing::error!(error = %e, "bridge re-init failed after recovery");
                                    }
                                    _ => {
                                        tracing::info!(
                                            "CodeAct executor recovered and bridge re-initialized"
                                        );
                                    }
                                }
                            }
                        }
                    }
                }

                // ── Execute code blocks ──────────────────────────────
                if let Some(ref exec) = executor {
                    for block in &code_blocks {
                        if let Some(ref tx) = event_tx {
                            let _ = tx.try_send(LoopEvent::CodeExecution {
                                code: block.code.clone(),
                                stdout: String::new(),
                                stderr: String::new(),
                                is_error: false,
                                duration_ms: None,
                                phase: CodeExecutionPhase::Started,
                            });
                        }

                        let exec_result = exec
                            .execute_with_tools(&block.code, Language::Python, &exec_options)
                            .await;

                        match exec_result {
                            Ok(result) => {
                                let observation = result.to_observation();
                                tracing::debug!(
                                    is_error = result.is_error,
                                    duration_ms = result.duration_ms,
                                    "CodeAct: code block executed"
                                );

                                if let Some(ref tx) = event_tx {
                                    let _ = tx.try_send(LoopEvent::CodeExecution {
                                        code: block.code.clone(),
                                        stdout: result.stdout.clone(),
                                        stderr: result.stderr.clone(),
                                        is_error: result.is_error,
                                        duration_ms: Some(result.duration_ms),
                                        phase: CodeExecutionPhase::Completed,
                                    });
                                }

                                if result.is_error {
                                    observations
                                        .push(format!("[Code Execution Error]\n{}", observation));
                                } else {
                                    observations
                                        .push(format!("[Code Execution Output]\n{}", observation));
                                }
                            }
                            Err(e) => {
                                let err_msg = format!("[Code Execution Error]\n{e}");
                                if let Some(ref tx) = event_tx {
                                    let _ = tx.try_send(LoopEvent::CodeExecution {
                                        code: block.code.clone(),
                                        stdout: String::new(),
                                        stderr: err_msg.clone(),
                                        is_error: true,
                                        duration_ms: None,
                                        phase: CodeExecutionPhase::Completed,
                                    });
                                }
                                observations.push(err_msg);
                            }
                        }
                    }
                }

                // ── Execute native tool calls ────────────────────────
                let mut native_journal_calls = Vec::new();
                if !native_calls.is_empty() {
                    let (tool_result_text, journal_calls) = execute_tool_batch(
                        &native_calls,
                        &context,
                        middleware,
                        event_tx.as_ref(),
                        interaction_gate.as_deref(),
                        Some(&response.content),
                    )
                    .await;
                    native_journal_calls = journal_calls;

                    if !tool_result_text.is_empty() {
                        observations.push(tool_result_text);
                    }
                }

                // ── Feed observations back to the model ──────────────
                let observation_text = observations.join("\n\n");

                // ── Record in conversation journal for mid-task resume ──
                if let Some(ref journal) = context.conversation.conversation_journal {
                    let journal_calls: Vec<JournalToolCall> = code_blocks
                        .iter()
                        .enumerate()
                        .map(|(i, b)| {
                            let output = observations.get(i).cloned().unwrap_or_default();
                            JournalToolCall {
                                tool_id: "code_execution".to_string(),
                                input: b.code.clone(),
                                output,
                                tool_call_id: None,
                                is_error: false,
                            }
                        })
                        .collect();
                    let mut j = journal.lock();
                    j.record(JournalEntry {
                        phase: JournalPhase::CodeExecution,
                        turn: tool_iterations,
                        tool_calls: journal_calls,
                        assistant_content: Some(response.content.clone()),
                    });
                }

                if use_multi_turn {
                    // Build assistant message with proper ToolUse blocks for
                    // native tool calls so the model sees what it called.
                    let mut assistant_blocks = Vec::new();
                    if !response.content.is_empty() {
                        assistant_blocks
                            .push(MessageBlock::Text { text: response.content.clone() });
                    }

                    // Attach ToolUse blocks for each native tool call, matching
                    // the call IDs from the original response so providers can
                    // pair them with ToolResult messages.
                    let mut native_journal_with_ids = native_journal_calls.clone();
                    for (i, jtc) in native_journal_with_ids.iter_mut().enumerate() {
                        if let Some(tc) = response.tool_calls.get(i) {
                            jtc.tool_call_id = Some(tc.id.clone());
                            assistant_blocks.push(MessageBlock::ToolUse {
                                id: tc.id.clone(),
                                name: jtc.tool_id.clone(),
                                input: serde_json::from_str(&jtc.input)
                                    .unwrap_or(serde_json::Value::Null),
                            });
                        } else {
                            let synthetic_id = format!("tool-call-{}", uuid::Uuid::new_v4());
                            jtc.tool_call_id = Some(synthetic_id.clone());
                            assistant_blocks.push(MessageBlock::ToolUse {
                                id: synthetic_id,
                                name: jtc.tool_id.clone(),
                                input: serde_json::from_str(&jtc.input)
                                    .unwrap_or(serde_json::Value::Null),
                            });
                        }
                    }

                    tool_history.push(CompletionMessage {
                        role: "assistant".into(),
                        content: response.content.clone(),
                        content_parts: vec![],
                        blocks: assistant_blocks,
                    });

                    // Push structured tool result messages for native calls.
                    for jtc in &native_journal_with_ids {
                        if let Some(ref call_id) = jtc.tool_call_id {
                            let safe_output = prompt_sanitize::escape_prompt_tags(&jtc.output);
                            tool_history.push(CompletionMessage {
                                role: "tool".into(),
                                content: safe_output.clone(),
                                content_parts: vec![],
                                blocks: vec![MessageBlock::ToolResult {
                                    tool_use_id: call_id.clone(),
                                    content: safe_output,
                                    is_error: jtc.is_error,
                                }],
                            });
                        }
                    }

                    // Push code execution observations as user messages
                    // (these don't have tool call IDs to match).
                    if !code_blocks.is_empty() {
                        let code_observation: String = observations
                            .iter()
                            .filter(|o| o.starts_with("[Code Execution"))
                            .cloned()
                            .collect::<Vec<_>>()
                            .join("\n\n");
                        if !code_observation.is_empty() {
                            tool_history.push(CompletionMessage {
                                role: "user".into(),
                                content: code_observation,
                                content_parts: vec![],
                                blocks: vec![],
                            });
                        }
                    }
                } else {
                    // Legacy mode: append to prompt
                    prompt.push_str(&format!(
                        "\n\n<assistant>\n{}\n</assistant>\n\n<observation>\n{}\n</observation>\n",
                        response.content, observation_text
                    ));
                }

                tool_iterations += 1;
            }
        })
    }
}
