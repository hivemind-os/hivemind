use std::collections::HashMap;
use std::sync::Arc;

use hive_contracts::prompt_sanitize;
use hive_model::{
    CompletionMessage, CompletionRequest, CompletionResponse, MessageBlock, RetryInfo,
    RoutingRequest,
};
use tokio_stream::StreamExt;

use super::super::interaction::UserInteractionGate;
use super::super::journal::{JournalEntry, JournalPhase};
use super::super::parsing::{parse_tool_calls, ToolCall};
use super::super::strategy::{LoopMiddleware, LoopStrategy};
use super::super::streaming::StreamingToolCallFilter;
use super::super::tool_execution::{estimate_request_tokens, execute_tool_batch};
use super::super::types::{BoxFuture, LoopContext, LoopError, LoopEvent, LoopResult};
use super::super::{
    check_preempt, is_budget_exempt, model_router_error_to_loop_error, simple_model_error,
};

#[derive(Default)]
pub struct ReActStrategy;

impl LoopStrategy for ReActStrategy {
    fn run<'a>(
        &'a self,
        context: LoopContext,
        model_router: Arc<hive_model::ModelRouter>,
        middleware: &'a [Arc<dyn LoopMiddleware>],
        event_tx: Option<tokio::sync::mpsc::Sender<LoopEvent>>,
        interaction_gate: Option<Arc<UserInteractionGate>>,
    ) -> BoxFuture<'a, Result<LoopResult, LoopError>> {
        Box::pin(async move {
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

            // Store the routing decision so middleware (e.g. compactor) can
            // look up the correct model limits.
            let mut context = context;
            context.routing.routing_decision = Some(decision.clone());

            // Determine whether the selected provider supports multi-turn tool
            // history (structured assistant+tool messages).
            let use_multi_turn = model_router
                .provider_kind(&decision.selected.provider_id)
                .map(|k| k.supports_tool_history())
                .unwrap_or(false);

            if use_multi_turn {
                tracing::info!(
                    provider_id = %decision.selected.provider_id,
                    model = %decision.selected.model,
                    "ReAct loop: using multi-turn tool history"
                );
            }

            let mut prompt = context.conversation.prompt.clone();
            let mut tool_iterations = context.conversation.initial_tool_iterations;
            // Include multimodal content parts only on the very first LLM call.
            let mut prompt_content_parts = context.conversation.prompt_content_parts.clone();
            // Cumulative count of tool results by tool name across iterations.
            let mut tool_result_counts: HashMap<String, u32> = HashMap::new();

            // Multi-turn tool history: structured assistant+tool messages
            // appended after the initial user message. Only used when the
            // provider supports native tool calling.
            let mut tool_history: Vec<CompletionMessage> = if use_multi_turn {
                // If resuming from a journal, reconstruct multi-turn messages.
                if let Some(ref journal) = context.conversation.conversation_journal {
                    journal.lock().reconstruct_multi_turn_messages()
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            };

            // Adaptive tool-call budget
            let mut budget = crate::tool_budget::AdaptiveBudget::new(&context.tool_limits);

            // Stall breaker: track recent ask_user calls to detect repeated
            // identical questions with the same answer (model stuck in a loop).
            // Each entry is (question_prefix, answer).
            let mut ask_user_history: Vec<(String, String)> = Vec::new();

            loop {
                let mut request = if use_multi_turn {
                    // Multi-turn mode: all context in messages, prompt is empty.
                    let mut messages = context.conversation.history.clone();
                    // First user message with the task (with optional multimodal parts).
                    let user_msg = if prompt_content_parts.is_empty() {
                        CompletionMessage {
                            role: "user".into(),
                            content: prompt.clone(),
                            content_parts: vec![],
                            blocks: vec![],
                        }
                    } else {
                        CompletionMessage {
                            role: "user".into(),
                            content: prompt.clone(),
                            content_parts: std::mem::take(&mut prompt_content_parts),
                            blocks: vec![],
                        }
                    };
                    messages.push(user_msg);
                    messages.extend(tool_history.iter().cloned());
                    CompletionRequest {
                        prompt: String::new(),
                        prompt_content_parts: vec![],
                        messages,
                        required_capabilities: context.routing.required_capabilities.clone(),
                        preferred_models: context.routing.preferred_models.clone(),
                        tools: context.tools_ctx.tools.list_definitions(),
                    }
                } else {
                    // Legacy mode: growing prompt with XML tags.
                    CompletionRequest {
                        prompt: prompt.clone(),
                        prompt_content_parts: std::mem::take(&mut prompt_content_parts),
                        messages: context.conversation.history.clone(),
                        required_capabilities: context.routing.required_capabilities.clone(),
                        preferred_models: context.routing.preferred_models.clone(),
                        tools: context.tools_ctx.tools.list_definitions(),
                    }
                };

                // Log tool count and any MCP tools reaching the LLM.
                let mcp_tools: Vec<&str> = request
                    .tools
                    .iter()
                    .filter(|t| t.id.starts_with("mcp."))
                    .map(|t| t.id.as_str())
                    .collect();
                if !mcp_tools.is_empty() {
                    tracing::info!(
                        total = request.tools.len(),
                        mcp_count = mcp_tools.len(),
                        mcp_tools = ?mcp_tools,
                        "tools in CompletionRequest"
                    );
                }

                // Diagnostic: log the full tool list on the first iteration so
                // we can compare shadow vs non-shadow runs.
                if tool_iterations == 0 {
                    let mut all_tool_ids: Vec<&str> =
                        request.tools.iter().map(|t| t.id.as_str()).collect();
                    all_tool_ids.sort();
                    let has_send = all_tool_ids.iter().any(|id| id.contains("send_external"));
                    tracing::info!(
                        shadow_mode = context.security.shadow_mode,
                        tool_count = all_tool_ids.len(),
                        has_send_external_message = has_send,
                        tools = ?all_tool_ids,
                        prompt_len = request.prompt.len(),
                        history_len = request.messages.len(),
                        "DIAGNOSTIC: first model call tool list"
                    );
                }

                // Diagnostic: log prompt hash and tail on every iteration so we
                // can compare model inputs between shadow and non-shadow runs.
                {
                    use std::hash::{Hash, Hasher};
                    let mut hasher = std::collections::hash_map::DefaultHasher::new();
                    request.prompt.hash(&mut hasher);
                    let prompt_hash = hasher.finish();
                    let prompt_tail: String = request
                        .prompt
                        .chars()
                        .rev()
                        .take(300)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect();
                    let msgs_summary: Vec<String> = request
                        .messages
                        .iter()
                        .map(|m| format!("{}:{}", m.role, m.content.len()))
                        .collect();
                    tracing::info!(
                        shadow_mode = context.security.shadow_mode,
                        iteration = tool_iterations,
                        prompt_len = request.prompt.len(),
                        prompt_hash = prompt_hash,
                        prompt_tail = %prompt_tail,
                        messages = ?msgs_summary,
                        "DIAGNOSTIC: model input per iteration"
                    );
                }

                for hook in middleware {
                    request = hook.before_model_call(&context, request)?;
                }

                let response = if let Some(ref tx) = event_tx {
                    let router = Arc::clone(&model_router);
                    let decision_clone = decision.clone();

                    // Signal that we're about to call the model (may trigger
                    // loading a local model into memory — a slow operation).
                    let _ = tx.try_send(LoopEvent::ModelLoading {
                        provider_id: decision_clone.selected.provider_id.clone(),
                        model: decision_clone.selected.model.clone(),
                        tool_result_counts: tool_result_counts.clone(),
                        estimated_tokens: Some(estimate_request_tokens(&request)),
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

                    // Emit fallback notification if the model differs from the originally selected one.
                    if actual_selection != decision_clone.selected
                        && tx
                            .try_send(LoopEvent::ModelFallback {
                                from_provider: decision_clone.selected.provider_id.clone(),
                                from_model: decision_clone.selected.model.clone(),
                                to_provider: actual_selection.provider_id.clone(),
                                to_model: actual_selection.model.clone(),
                            })
                            .is_err()
                    {
                        tracing::warn!(
                            "failed to send ModelFallback event — channel full or closed"
                        );
                    }

                    let mut content = String::new();
                    let provider_id = actual_selection.provider_id.clone();
                    let model = actual_selection.model.clone();
                    let mut streamed_tool_calls = Vec::new();
                    let mut token_filter = StreamingToolCallFilter::new();

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
                                    // Always accumulate full content for parsing.
                                    content.push_str(&chunk.delta);
                                    // Filter out <tool_call> blocks before
                                    // sending tokens to the UI.
                                    let visible = token_filter.feed(&chunk.delta);
                                    if !visible.is_empty() {
                                        let _ = tx.try_send(LoopEvent::Token { delta: visible });
                                    }
                                }
                                // Emit partial tool-call argument snapshots
                                // only for MCP server tools (id pattern: mcp.{server}.{tool},
                                // sanitized to mcp_{server}_{tool}).  Skipping internal
                                // tools like core_ask_user avoids flooding the event log.
                                for d in &chunk.tool_call_arg_deltas {
                                    let is_mcp_tool = d
                                        .name
                                        .as_deref()
                                        .map(|n| {
                                            n.starts_with("mcp_")
                                                || n.starts_with("mcp.")
                                                || n.starts_with("app.")
                                        })
                                        .unwrap_or(false);
                                    if is_mcp_tool {
                                        let _ = tx.try_send(LoopEvent::ToolCallArgDelta {
                                            index: d.index,
                                            call_id: d.call_id.clone(),
                                            tool_name: d.name.clone(),
                                            arguments_so_far: d.arguments_so_far.clone(),
                                        });
                                    }
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
                    // Flush any remaining buffered text that wasn't part of a tag
                    let remaining = token_filter.flush();
                    if !remaining.is_empty() {
                        let _ = tx.try_send(LoopEvent::Token { delta: remaining });
                    }

                    let _ = tx.try_send(LoopEvent::ModelDone {
                        content: content.clone(),
                        provider_id: provider_id.clone(),
                        model: model.clone(),
                    });

                    CompletionResponse {
                        provider_id,
                        model,
                        content,
                        tool_calls: streamed_tool_calls,
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

                // Prefer native structured tool calls from the provider
                let detected_calls: Vec<ToolCall> = if !response.tool_calls.is_empty() {
                    response
                        .tool_calls
                        .iter()
                        .map(|tc| ToolCall {
                            tool_id: tc.name.clone(),
                            input: tc.arguments.clone(),
                        })
                        .collect()
                } else {
                    // Fallback: text-based extraction for providers without native tool calls
                    parse_tool_calls(&response.content)
                };

                // Diagnostic: log every tool call the model produces, plus
                // the model's text content (may reveal reasoning differences).
                {
                    let call_ids: Vec<&str> =
                        detected_calls.iter().map(|c| c.tool_id.as_str()).collect();
                    let response_text_preview: String =
                        response.content.chars().take(500).collect();
                    tracing::info!(
                        shadow_mode = context.security.shadow_mode,
                        iteration = tool_iterations,
                        calls = ?call_ids,
                        response_text = %response_text_preview,
                        native_tool_calls = !response.tool_calls.is_empty(),
                        "DIAGNOSTIC: model response"
                    );
                }

                if !detected_calls.is_empty() {
                    let billable_count =
                        detected_calls.iter().filter(|c| !is_budget_exempt(&c.tool_id)).count();
                    // Check adaptive budget BEFORE executing the batch.
                    match budget.check(tool_iterations, billable_count) {
                        crate::tool_budget::BudgetDecision::Allow => { /* proceed */ }
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
                            tracing::info!(
                                new_budget,
                                extensions_granted,
                                "tool-call budget extended — agent is making progress"
                            );
                        }
                        crate::tool_budget::BudgetDecision::HardStop { ceiling } => {
                            return Err(LoopError::HardCeilingReached { ceiling });
                        }
                    }

                    let (mut tool_results, journal_tool_calls) = execute_tool_batch(
                        &detected_calls,
                        &context,
                        middleware,
                        event_tx.as_ref(),
                        interaction_gate.as_deref(),
                        Some(&response.content),
                    )
                    .await;

                    for jtc in &journal_tool_calls {
                        *tool_result_counts.entry(jtc.tool_id.clone()).or_insert(0) += 1;
                    }

                    // ── Stall breaker: detect repeated ask_user loops ────
                    // Track ask_user question+answer pairs. If the model asks
                    // the same question 3+ times and gets the same answer, it's
                    // stuck in a confirmation loop. Inject a nudge into the
                    // prompt so the model proceeds instead of re-asking.
                    for jtc in &journal_tool_calls {
                        if jtc.tool_id == "core.ask_user" {
                            let question_prefix: String =
                                serde_json::from_str::<serde_json::Value>(&jtc.input)
                                    .ok()
                                    .and_then(|v| {
                                        v.get("question")
                                            .and_then(|q| q.as_str())
                                            .map(|s| s.chars().take(120).collect())
                                    })
                                    .unwrap_or_default();
                            let answer: String =
                                serde_json::from_str::<serde_json::Value>(&jtc.output)
                                    .ok()
                                    .and_then(|v| {
                                        v.get("answer").and_then(|a| a.as_str()).map(String::from)
                                    })
                                    .unwrap_or_default();
                            ask_user_history.push((question_prefix, answer));
                        } else {
                            // A non-ask_user tool call breaks the streak
                            ask_user_history.clear();
                        }
                    }

                    const STALL_THRESHOLD: usize = 2;
                    if ask_user_history.len() >= STALL_THRESHOLD {
                        let last = &ask_user_history[ask_user_history.len() - 1];
                        let repeats = ask_user_history
                            .iter()
                            .rev()
                            .take_while(|(q, a)| q == &last.0 && a == &last.1)
                            .count();
                        if repeats >= STALL_THRESHOLD {
                            tracing::info!(
                                repeats,
                                answer = %last.1,
                                "stall breaker: injecting nudge after repeated identical ask_user"
                            );
                            tool_results.push_str(
                                "\n\n[System: The user has already confirmed this exact request. \
                                 Do NOT ask again. Proceed immediately with the appropriate \
                                 action to fulfill the user's confirmed request.]",
                            );
                        }
                    }
                    // ── End stall breaker ─────────────────────────────────

                    if use_multi_turn {
                        // Multi-turn mode: build structured assistant + tool messages.
                        let mut assistant_blocks = Vec::new();
                        if !response.content.is_empty() {
                            assistant_blocks
                                .push(MessageBlock::Text { text: response.content.clone() });
                        }

                        // Zip native tool calls with journal entries to get call IDs.
                        let mut journal_with_ids = journal_tool_calls.clone();
                        for (i, jtc) in journal_with_ids.iter_mut().enumerate() {
                            if let Some(tc) = response.tool_calls.get(i) {
                                jtc.tool_call_id = Some(tc.id.clone());
                                assistant_blocks.push(MessageBlock::ToolUse {
                                    id: tc.id.clone(),
                                    name: jtc.tool_id.clone(),
                                    input: serde_json::from_str(&jtc.input)
                                        .unwrap_or(serde_json::Value::Null),
                                });
                            } else {
                                // Text-parsed tool calls don't have IDs — generate synthetic ones.
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

                        // Push assistant message.
                        tool_history.push(CompletionMessage {
                            role: "assistant".into(),
                            content: response.content.clone(),
                            content_parts: vec![],
                            blocks: assistant_blocks,
                        });

                        // Push tool result messages.
                        for jtc in &journal_with_ids {
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

                        // Inject stall-breaker nudge as a system message if needed.
                        if !tool_results.contains("[System: The user has already confirmed") {
                            // No nudge needed.
                        } else {
                            tool_history.push(CompletionMessage {
                                role: "system".into(),
                                content:
                                    "[System: The user has already confirmed this exact request. \
                                    Do NOT ask again. Proceed immediately with the appropriate \
                                    action to fulfill the user's confirmed request.]"
                                        .to_string(),
                                content_parts: vec![],
                                blocks: vec![],
                            });
                        }

                        // Count individual tool calls.
                        tool_iterations +=
                            detected_calls.iter().filter(|c| !is_budget_exempt(&c.tool_id)).count();

                        if let Some(ref journal) = context.conversation.conversation_journal {
                            let mut j = journal.lock();
                            j.record(JournalEntry {
                                phase: JournalPhase::ToolCycle,
                                turn: tool_iterations,
                                tool_calls: journal_with_ids,
                                assistant_content: Some(response.content.clone()),
                            });
                        }
                    } else {
                        // Legacy mode: append XML to prompt.
                        prompt = format!("{prompt}{tool_results}");
                        tool_iterations +=
                            detected_calls.iter().filter(|c| !is_budget_exempt(&c.tool_id)).count();

                        if let Some(ref journal) = context.conversation.conversation_journal {
                            let mut j = journal.lock();
                            j.record(JournalEntry {
                                phase: JournalPhase::ToolCycle,
                                turn: tool_iterations,
                                tool_calls: journal_tool_calls,
                                assistant_content: None,
                            });
                        }
                    }

                    // Check if a new user message is waiting — yield at this checkpoint.
                    if let Some(result) = check_preempt(
                        &context.preempt_signal,
                        &context.conversation.conversation_journal,
                        &decision,
                        &response.provider_id,
                        &response.model,
                        event_tx.as_ref(),
                    )
                    .await
                    {
                        return Ok(result);
                    }

                    continue;
                }

                if let Some(ref tx) = event_tx {
                    // Done is a critical event — use blocking send to avoid silent loss.
                    let _ = tx
                        .send(LoopEvent::Done {
                            content: response.content.clone(),
                            provider_id: response.provider_id.clone(),
                            model: response.model.clone(),
                        })
                        .await;
                }

                return Ok(LoopResult {
                    content: response.content,
                    provider_id: response.provider_id,
                    model: response.model,
                    decision,
                    preempted: false,
                });
            }
        })
    }
}
