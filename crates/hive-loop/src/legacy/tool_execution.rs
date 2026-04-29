//! Tool execution logic: batching, sequential/parallel dispatch, approval gates,
//! and the core `execute_tool_call` function that routes calls through middleware,
//! permission checks, and built-in handler interception.

use std::sync::Arc;

use hive_classification::{ChannelClass, DataClass};
use hive_contracts::{
    infer_scope_with_workspace, InteractionKind, InteractionResponsePayload, ToolExecutionMode,
    UserInteractionResponse,
};
use hive_model::CompletionRequest;
use hive_tools::{ToolApproval, ToolResult};

use super::interaction::UserInteractionGate;
use super::journal::JournalToolCall;
use super::parsing::{strip_xml_tool_blocks, ToolCall};
use super::strategy::LoopMiddleware;
use super::types::{LoopContext, LoopError, LoopEvent};

// ── Constants ───────────────────────────────────────────────────────────

/// Maximum characters for a single tool output before it is truncated in the prompt.
/// ~25K tokens — large enough for most file reads but prevents a single tool from
/// consuming the entire context window.
pub(crate) const MAX_TOOL_OUTPUT_CHARS: usize = 100_000;

// ── Helpers ─────────────────────────────────────────────────────────────

/// Return the largest prefix of `s` whose byte length is ≤ `max_bytes`,
/// without splitting a multi-byte UTF-8 character.
pub(crate) fn truncate_str(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Truncate a tool output string if it exceeds `MAX_TOOL_OUTPUT_CHARS`.
pub(crate) fn cap_tool_output(output: &str) -> String {
    if output.len() <= MAX_TOOL_OUTPUT_CHARS {
        output.to_string()
    } else {
        let keep = MAX_TOOL_OUTPUT_CHARS.saturating_sub(80);
        let truncated = truncate_str(output, keep);
        format!(
            "{}…\n\n[output truncated — {total} chars total, showing first {shown}]",
            truncated,
            total = output.len(),
            shown = truncated.len(),
        )
    }
}

/// Estimate the number of tokens in a completion request.
///
/// Uses the common heuristic of ~4 characters per token (English text).
/// Accounts for prompt, conversation history, and tool definitions.
pub(crate) fn estimate_request_tokens(request: &CompletionRequest) -> u32 {
    let mut chars: usize = request.prompt.len();
    for msg in &request.messages {
        // ~4 tokens overhead per message for role/separators
        chars += msg.role.len() + msg.content.len() + 16;
    }
    for tool in &request.tools {
        chars += tool.id.len() + tool.name.len() + tool.description.len();
        chars += tool.input_schema.to_string().len();
    }
    (chars / 4) as u32
}

// ── Internal types ──────────────────────────────────────────────────────

/// Result of a single tool call execution (success or error).
pub(crate) struct ToolCallOutcome {
    pub(super) tool_id: String,
    pub(super) input_str: String,
    pub(super) output: String,
    pub(super) is_error: bool,
    /// The tool's channel classification, used for post-execution
    /// re-verification in parallel batches (TOCTOU mitigation).
    pub(super) channel_class: Option<ChannelClass>,
}

// ── Public entry points ─────────────────────────────────────────────────

/// Execute a batch of tool calls according to the configured
/// [`ToolExecutionMode`].
pub(crate) async fn execute_tool_batch(
    calls: &[ToolCall],
    context: &LoopContext,
    middleware: &[Arc<dyn LoopMiddleware>],
    event_tx: Option<&tokio::sync::mpsc::Sender<LoopEvent>>,
    interaction_gate: Option<&UserInteractionGate>,
    assistant_content: Option<&str>,
) -> (String, Vec<JournalToolCall>) {
    // Strip tool-call XML blocks from assistant_content so that handlers
    // (e.g. core.ask_user) don't leak raw <tool_call> tags into
    // user-visible output such as the question `message` field.
    let cleaned_content = assistant_content.map(strip_xml_tool_blocks);
    let cleaned_ref = cleaned_content.as_deref();

    let mode = context.tools_ctx.tool_execution_mode;
    let outcomes = match mode {
        ToolExecutionMode::Parallel => {
            execute_tools_parallel(
                calls,
                context,
                middleware,
                event_tx,
                interaction_gate,
                cleaned_ref,
            )
            .await
        }
        _ => {
            let stop_on_error = mode == ToolExecutionMode::SequentialPartial;
            execute_tools_sequential(
                calls,
                context,
                middleware,
                event_tx,
                interaction_gate,
                stop_on_error,
                cleaned_ref,
            )
            .await
        }
    };

    // Post-execution re-verification for parallel mode: if the session's
    // effective data classification was escalated during the batch, check
    // that each tool's channel class still permits the new level.
    let outcomes: Vec<ToolCallOutcome> = if mode == ToolExecutionMode::Parallel {
        let final_dc = context.effective_data_class();
        outcomes
            .into_iter()
            .map(|mut o| {
                if let Some(channel_class) = o.channel_class {
                    if !channel_class.allows(final_dc) {
                        tracing::warn!(
                            tool_id = %o.tool_id,
                            channel_class = ?channel_class,
                            effective_dc = ?final_dc,
                            "redacting tool result: classification escalated during parallel batch"
                        );
                        o.output = "Tool result redacted: session data classification was \
                                    escalated during parallel execution, and this tool's channel \
                                    class no longer permits the current classification level."
                            .to_string();
                    }
                }
                o
            })
            .collect()
    } else {
        outcomes
    };

    let mut tool_results = String::new();
    let mut journal_tool_calls = Vec::new();
    for o in outcomes {
        let capped = cap_tool_output(&o.output);
        let safe = hive_contracts::prompt_sanitize::escape_prompt_tags(&capped);
        tool_results.push_str(&format!(
            "\n\n<tool_call>\n{{\"tool\": \"{}\", \"input\": {}}}\n</tool_call>\n<tool_result>\n{}\n</tool_result>",
            o.tool_id, o.input_str, safe
        ));
        journal_tool_calls.push(JournalToolCall {
            tool_id: o.tool_id,
            input: o.input_str,
            output: cap_tool_output(&o.output),
            tool_call_id: None,
            is_error: o.is_error,
        });
    }
    (tool_results, journal_tool_calls)
}

/// Execute a single tool call through the full middleware + permission pipeline.
pub(crate) async fn execute_tool_call(
    context: &LoopContext,
    call: ToolCall,
    middleware: &[Arc<dyn LoopMiddleware>],
    event_tx: Option<&tokio::sync::mpsc::Sender<LoopEvent>>,
    interaction_gate: Option<&UserInteractionGate>,
    assistant_content: Option<&str>,
) -> Result<ToolResult, LoopError> {
    let mut call = call;
    for hook in middleware {
        call = hook.before_tool_call(context, call)?;
    }

    let tool = context
        .tools()
        .get(&call.tool_id)
        .ok_or_else(|| LoopError::ToolUnavailable { tool_id: call.tool_id.clone() })?;
    let definition = tool.definition();

    // Normalize the tool_id to the canonical registry ID so that
    // permission checks, events, and logging use the real name
    // (e.g. `shell.execute` instead of `shell_execute`).
    call.tool_id = definition.id.clone();

    // --- Session permission check (before tool definition approval) ---
    let workspace_str = context.workspace_path().map(|p| p.to_string_lossy().to_string());
    let resource = infer_scope_with_workspace(&call.tool_id, &call.input, workspace_str.as_deref());
    let needs_approval = {
        let perms = context.security.permissions.lock();
        let rules_summary: Vec<String> = perms
            .rules
            .iter()
            .map(|r| format!("({} | {} | {:?})", r.tool_pattern, r.scope, r.decision))
            .collect();
        let approval =
            crate::tool_policy::resolve_tool_approval(&call.tool_id, &resource, definition, &perms);
        tracing::info!(
            tool_id = %call.tool_id,
            resource = %resource,
            rule_count = perms.rules.len(),
            rules = ?rules_summary,
            decision = ?approval,
            "session permission check"
        );
        match approval {
            crate::tool_policy::ResolvedApproval::Auto => false,
            crate::tool_policy::ResolvedApproval::Deny { reason } => {
                return Err(LoopError::ToolDenied { tool_id: call.tool_id.clone(), reason });
            }
            crate::tool_policy::ResolvedApproval::Ask => true,
        }
    };

    // ── Connector destination-rule enforcement ─────────────────────────
    // Evaluate the per-connector destination rules (Deny/Ask/Auto) for
    // comm tools.  Deny blocks immediately; Ask forces approval even if
    // the session-level / tool-level decision was Auto.
    let (needs_approval, connector_rule_reason) = if call.tool_id.starts_with("comm.send") {
        if let Some(ref svc) = context.security.connector_service {
            let connector_id = call.input.get("connector_id").and_then(|v| v.as_str());
            let to = call.input.get("to").and_then(|v| v.as_str());
            if let (Some(cid), Some(dest)) = (connector_id, to) {
                match svc.resolve_destination_approval(cid, dest) {
                    Some(ToolApproval::Deny) => {
                        tracing::info!(
                            connector_id = cid,
                            destination = dest,
                            "connector destination rule: DENY"
                        );
                        return Err(LoopError::ToolDenied {
                            tool_id: call.tool_id.clone(),
                            reason: format!(
                                "destination '{dest}' is denied by a connector rule on '{cid}'"
                            ),
                        });
                    }
                    Some(ToolApproval::Ask) => {
                        tracing::info!(
                            connector_id = cid,
                            destination = dest,
                            "connector destination rule: ASK"
                        );
                        (
                            true,
                            Some(format!(
                                "Connector rule on '{}' requires approval to send to '{}'.",
                                cid, dest
                            )),
                        )
                    }
                    Some(ToolApproval::Auto) => {
                        tracing::info!(
                            connector_id = cid,
                            destination = dest,
                            "connector destination rule: AUTO"
                        );
                        (needs_approval, None)
                    }
                    None => (needs_approval, None),
                }
            } else {
                (needs_approval, None)
            }
        } else {
            (needs_approval, None)
        }
    } else {
        (needs_approval, None)
    };

    // ── Channel-class check ────────────────────────────────────────────
    // The hard-deny case (violation + no approval path) is handled by
    // DataClassificationMiddleware::before_tool_call.  Here we only need
    // to detect the violation to modify the approval dialog's reason text.
    let effective_dc = context.effective_data_class();
    let channel_violation = !definition.channel_class.allows(effective_dc);

    if needs_approval || channel_violation {
        let reason = if channel_violation {
            format!(
                "Tool '{}' operates on {:?} channel but data is classified as {:?}. Approve to proceed anyway.",
                call.tool_id, definition.channel_class, effective_dc
            )
        } else if let Some(ref cr_reason) = connector_rule_reason {
            cr_reason.clone()
        } else {
            format!("Tool '{}' requires user approval before execution.", call.tool_id)
        };

        if let (Some(tx), Some(gate)) = (event_tx.as_ref(), interaction_gate) {
            let request_id = format!("approval-{}-{}", call.tool_id, uuid::Uuid::new_v4());
            let input_str = serde_json::to_string(&call.input).unwrap_or_default();

            let kind = InteractionKind::ToolApproval {
                tool_id: call.tool_id.clone(),
                input: input_str,
                reason: reason.clone(),
                inferred_scope: Some(resource.clone()),
            };
            let rx = gate.create_request(request_id.clone(), kind.clone());
            if tx
                .send(LoopEvent::UserInteractionRequired { request_id: request_id.clone(), kind })
                .await
                .is_err()
            {
                tracing::warn!(
                    "failed to send UserInteractionRequired event — loop receiver dropped"
                );
            }

            match rx.await {
                Ok(UserInteractionResponse {
                    payload: InteractionResponsePayload::ToolApproval { approved: true, .. },
                    ..
                }) => { /* approved, continue to execute */ }
                _ => {
                    return Err(LoopError::ToolDenied {
                        tool_id: call.tool_id.clone(),
                        reason: "User denied the tool execution".to_string(),
                    });
                }
            }
        } else {
            return Err(LoopError::ToolDenied { tool_id: call.tool_id.clone(), reason });
        }
    }

    // ── Connector output-class enforcement ─────────────────────────────
    // For comm.send_external_message, resolve the connector's output class
    // and compare against the effective session data-class.  If the connector
    // cannot handle the data sensitivity, trigger an approval dialog.
    // This must remain inline because it uses the async interaction gate.
    if call.tool_id == "comm.send_external_message" {
        if let Some(ref svc) = context.security.connector_service {
            let connector_id = call.input.get("connector_id").and_then(|v| v.as_str());
            let to = call.input.get("to").and_then(|v| v.as_str());
            if let (Some(cid), Some(dest)) = (connector_id, to) {
                let output_class =
                    svc.resolve_output_class(cid, dest).unwrap_or(DataClass::Internal);
                if output_class < effective_dc {
                    let reason = format!(
                        "Connector '{}' is classified as {} (outbound) but this session \
                         contains {} data. Approve to send anyway.",
                        cid, output_class, effective_dc
                    );

                    if let (Some(tx), Some(gate)) = (event_tx.as_ref(), interaction_gate) {
                        let request_id =
                            format!("approval-class-{}-{}", call.tool_id, uuid::Uuid::new_v4());
                        let input_str = serde_json::to_string(&call.input).unwrap_or_default();
                        let kind = InteractionKind::ToolApproval {
                            tool_id: call.tool_id.clone(),
                            input: input_str,
                            reason: reason.clone(),
                            inferred_scope: None,
                        };
                        let rx = gate.create_request(request_id.clone(), kind.clone());
                        if tx
                            .send(LoopEvent::UserInteractionRequired { request_id, kind })
                            .await
                            .is_err()
                        {
                            tracing::warn!("failed to send UserInteractionRequired event — loop receiver dropped");
                        }
                        match rx.await {
                            Ok(UserInteractionResponse {
                                payload:
                                    InteractionResponsePayload::ToolApproval { approved: true, .. },
                                ..
                            }) => { /* user approved the override */ }
                            _ => {
                                return Err(LoopError::ToolDenied {
                                    tool_id: call.tool_id.clone(),
                                    reason: format!(
                                        "Blocked: cannot send {} data through {} connector '{}'",
                                        effective_dc, output_class, cid
                                    ),
                                });
                            }
                        }
                    } else {
                        return Err(LoopError::ToolDenied {
                            tool_id: call.tool_id.clone(),
                            reason,
                        });
                    }
                }
            }
        }
    }

    // Handle built-in tools that the loop intercepts directly.
    if call.tool_id == "core.ask_user" {
        return super::handle_question_tool(&call, event_tx, interaction_gate, assistant_content)
            .await;
    }
    if call.tool_id == "core.activate_skill" {
        return super::handle_activate_skill(&call, context).await;
    }
    if call.tool_id == "core.spawn_agent" {
        return super::handle_spawn_agent_tool(&call, context).await;
    }
    if call.tool_id == "core.list_agents" {
        return super::handle_list_agents_tool(&call, context).await;
    }
    if call.tool_id == "core.get_agent_result" {
        return super::handle_get_agent_result_tool(&call, context).await;
    }
    if call.tool_id == "core.wait_for_agent" {
        return super::handle_wait_for_agent_tool(&call, context).await;
    }
    if call.tool_id == "core.list_personas" {
        return super::handle_list_personas_tool(&call, context).await;
    }
    if call.tool_id == "core.kill_agent" {
        return super::handle_kill_agent_tool(&call, context).await;
    }
    if call.tool_id == "core.signal_agent" {
        return super::handle_signal_agent_tool(&call, context).await;
    }
    if call.tool_id == "knowledge.query" {
        return super::handle_knowledge_query_tool(&call, context).await;
    }

    // ── Shadow mode interception ──────────────────────────────────────
    // When shadow_mode is active, intercept external side-effecting tools.
    // Built-in orchestration tools (core.*, knowledge.*) are handled above
    // and always pass through.  Read-only tools also pass through so the
    // agent can reason over real data.
    if context.security.shadow_mode {
        let is_read_only =
            definition.annotations.read_only_hint == Some(true) || !definition.side_effects;
        if !is_read_only {
            tracing::info!(
                tool_id = %call.tool_id,
                "shadow mode: intercepting side-effecting tool call"
            );
            // Emit an interception event so callers (e.g. workflow test
            // runner) can record what the agent *would* have done.
            if let Some(tx) = event_tx {
                let input_str = serde_json::to_string(&call.input)
                    .unwrap_or_else(|_| "<unserializable>".to_string());
                let _ = tx.try_send(LoopEvent::ToolCallIntercepted {
                    tool_id: call.tool_id.clone(),
                    input: input_str,
                });
            }
            // Return a clean success so the agent continues normally.
            // Do NOT include "shadow" or explanatory messages — the LLM
            // would interpret them as partial failures and retry/re-ask.
            let synthetic_output = serde_json::json!({
                "success": true,
            });
            let result = hive_tools::ToolResult {
                output: synthetic_output,
                data_class: DataClass::Internal,
            };
            // Still run after_tool_result middleware so classification and
            // other hooks see the synthetic result.
            let mut result = result;
            for hook in middleware {
                result =
                    hook.after_tool_result(context, &call.tool_id, Some(&call.input), result)?;
            }
            return Ok(result);
        }
    }

    // Snapshot the tool input before it's moved into execute()
    let tool_input_snapshot = call.input.clone();

    // Inform the tool of the effective session data-class so that
    // output-channel enforcement can compare the connector's class against
    // the true high-water mark of data the session has touched.
    tool.set_session_data_class(context.effective_data_class());

    let result = if let Some(ref token) = context.cancellation_token {
        tokio::select! {
            biased;
            _ = token.cancelled() => {
                return Err(LoopError::Cancelled);
            }
            result = tool.execute(call.input) => {
                result.map_err(|error| {
                    LoopError::ToolExecutionFailed { tool_id: call.tool_id.clone(), detail: error.to_string() }
                })?
            }
        }
    } else {
        tool.execute(call.input).await.map_err(|error| LoopError::ToolExecutionFailed {
            tool_id: call.tool_id.clone(),
            detail: error.to_string(),
        })?
    };

    // after_tool_result hooks handle classification resolution and
    // effective_data_class escalation (via DataClassificationMiddleware).
    let mut result = result;
    for hook in middleware {
        result =
            hook.after_tool_result(context, &call.tool_id, Some(&tool_input_snapshot), result)?;
    }

    Ok(result)
}

// ── Private dispatch helpers ────────────────────────────────────────────

pub(crate) async fn run_single_tool_call(
    tool_call: &ToolCall,
    context: &LoopContext,
    middleware: &[Arc<dyn LoopMiddleware>],
    event_tx: Option<&tokio::sync::mpsc::Sender<LoopEvent>>,
    interaction_gate: Option<&UserInteractionGate>,
    assistant_content: Option<&str>,
) -> ToolCallOutcome {
    let input_str =
        serde_json::to_string(&tool_call.input).unwrap_or_else(|_| "<unserializable>".to_string());

    if let Some(tx) = event_tx {
        if tx
            .try_send(LoopEvent::ToolCallStart {
                tool_id: tool_call.tool_id.clone(),
                input: input_str.clone(),
            })
            .is_err()
        {
            tracing::warn!(tool_id = %tool_call.tool_id, "failed to send ToolCallStart event");
        }
    }

    tracing::debug!(
        tool_id = %tool_call.tool_id,
        effective_dc_before = %context.effective_data_class(),
        "run_single_tool_call: starting"
    );

    let (output, is_error) = match execute_tool_call(
        context,
        tool_call.clone(),
        middleware,
        event_tx,
        interaction_gate,
        assistant_content,
    )
    .await
    {
        Ok(result) => {
            // Classification resolution and effective_data_class
            // escalation are handled by DataClassificationMiddleware
            // in its after_tool_result hook.
            let output = serde_json::to_string(&result.output)
                .unwrap_or_else(|_| "<unserializable>".to_string());
            (output, false)
        }
        Err(e) => (format!("ERROR: {e}"), true),
    };

    if let Some(tx) = event_tx {
        if tx
            .try_send(LoopEvent::ToolCallResult {
                tool_id: tool_call.tool_id.clone(),
                output: output.clone(),
                is_error,
            })
            .is_err()
        {
            tracing::warn!(tool_id = %tool_call.tool_id, "failed to send ToolCallResult event");
        }
    }

    let channel_class =
        context.tools_ctx.tools.get(&tool_call.tool_id).map(|t| t.definition().channel_class);

    ToolCallOutcome {
        tool_id: tool_call.tool_id.clone(),
        input_str,
        output,
        is_error,
        channel_class,
    }
}

async fn execute_tools_sequential(
    calls: &[ToolCall],
    context: &LoopContext,
    middleware: &[Arc<dyn LoopMiddleware>],
    event_tx: Option<&tokio::sync::mpsc::Sender<LoopEvent>>,
    interaction_gate: Option<&UserInteractionGate>,
    stop_on_error: bool,
    assistant_content: Option<&str>,
) -> Vec<ToolCallOutcome> {
    let mut outcomes = Vec::with_capacity(calls.len());
    for tool_call in calls {
        let outcome = run_single_tool_call(
            tool_call,
            context,
            middleware,
            event_tx,
            interaction_gate,
            assistant_content,
        )
        .await;
        let failed = outcome.is_error;
        outcomes.push(outcome);
        if stop_on_error && failed {
            break;
        }
    }
    outcomes
}

async fn execute_tools_parallel(
    calls: &[ToolCall],
    context: &LoopContext,
    middleware: &[Arc<dyn LoopMiddleware>],
    event_tx: Option<&tokio::sync::mpsc::Sender<LoopEvent>>,
    interaction_gate: Option<&UserInteractionGate>,
    assistant_content: Option<&str>,
) -> Vec<ToolCallOutcome> {
    // Cap concurrency to avoid resource exhaustion from large batches.
    const MAX_CONCURRENT_TOOLS: usize = 10;
    let semaphore = Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_TOOLS));

    let futures: Vec<_> = calls
        .iter()
        .map(|tool_call| {
            let sem = Arc::clone(&semaphore);
            async move {
                let _permit = sem.acquire().await.expect("semaphore closed unexpectedly");
                run_single_tool_call(
                    tool_call,
                    context,
                    middleware,
                    event_tx,
                    interaction_gate,
                    assistant_content,
                )
                .await
            }
        })
        .collect();
    futures_util::future::join_all(futures).await
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_str_ascii() {
        assert_eq!(truncate_str("hello world", 5), "hello");
        assert_eq!(truncate_str("hello", 10), "hello");
        assert_eq!(truncate_str("", 5), "");
    }

    #[test]
    fn truncate_str_multibyte() {
        // "café" is 5 bytes (é is 2 bytes in UTF-8)
        let s = "café";
        assert_eq!(s.len(), 5);
        // Truncating at 4 bytes would split é — should back up to 3
        assert_eq!(truncate_str(s, 4), "caf");
        // Truncating at 5 bytes keeps the whole string
        assert_eq!(truncate_str(s, 5), "café");
    }

    #[test]
    fn cap_tool_output_short() {
        let short = "hello world";
        assert_eq!(cap_tool_output(short), short);
    }

    #[test]
    fn cap_tool_output_long() {
        let long_str = "x".repeat(MAX_TOOL_OUTPUT_CHARS + 100);
        let result = cap_tool_output(&long_str);
        assert!(result.len() < long_str.len());
        assert!(result.contains("[output truncated"));
    }
}
