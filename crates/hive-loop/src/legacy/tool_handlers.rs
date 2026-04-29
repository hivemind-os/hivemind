use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::Arc;

use hive_classification::DataClass;
use hive_contracts::{
    InteractionKind, InteractionResponsePayload, Persona, UserInteractionResponse,
};
use hive_tools::ToolResult;

use super::interaction::UserInteractionGate;
use super::parsing::ToolCall;
use super::tool_execution::truncate_str;
use super::types::{AgentOrchestrator, LoopContext, LoopError, LoopEvent};

/// Handle the built-in `core.ask_user` tool by emitting a user interaction
/// event and blocking until the user responds.
pub(super) async fn handle_question_tool(
    call: &ToolCall,
    event_tx: Option<&tokio::sync::mpsc::Sender<LoopEvent>>,
    interaction_gate: Option<&UserInteractionGate>,
    assistant_content: Option<&str>,
) -> Result<ToolResult, LoopError> {
    let text = call.input.get("question").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let choices: Vec<String> = call
        .input
        .get("choices")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let allow_freeform = call.input.get("allow_freeform").and_then(|v| v.as_bool()).unwrap_or(true);
    let multi_select = call.input.get("multi_select").and_then(|v| v.as_bool()).unwrap_or(false);
    let message = assistant_content.filter(|s| !s.is_empty()).map(String::from);

    if let (Some(tx), Some(gate)) = (event_tx, interaction_gate) {
        let request_id = format!("question-{}", uuid::Uuid::new_v4());
        let kind = InteractionKind::Question {
            text: text.clone(),
            choices: choices.clone(),
            allow_freeform,
            multi_select,
            message,
        };
        // Create the gate request FIRST so that the interaction is
        // queryable when the event triggers a snapshot rebuild (e.g.
        // the interactions SSE calls list_pending()).
        let rx = gate.create_request(request_id.clone(), kind.clone());

        if tx.send(LoopEvent::UserInteractionRequired { request_id, kind }).await.is_err() {
            tracing::warn!("failed to send UserInteractionRequired event — loop receiver dropped");
        }
        match rx.await {
            Ok(UserInteractionResponse {
                payload:
                    InteractionResponsePayload::Answer {
                        selected_choice,
                        selected_choices,
                        text: answer_text,
                    },
                ..
            }) => {
                // Build the answer string for the LLM
                let answer = if let Some(ref indices) = selected_choices {
                    // Multi-select: join all selected choice labels
                    let labels: Vec<String> = indices
                        .iter()
                        .map(|&idx| {
                            choices.get(idx).cloned().unwrap_or_else(|| format!("Choice {idx}"))
                        })
                        .collect();
                    if labels.is_empty() {
                        "(no choices selected)".to_string()
                    } else {
                        labels.join(", ")
                    }
                } else if let Some(idx) = selected_choice {
                    choices.get(idx).cloned().unwrap_or_else(|| format!("Choice {idx}"))
                } else if let Some(ref t) = answer_text {
                    t.clone()
                } else {
                    "(no response)".to_string()
                };
                Ok(ToolResult {
                    output: serde_json::json!({ "answer": answer }),
                    data_class: DataClass::Internal,
                })
            }
            _ => Ok(ToolResult {
                output: serde_json::json!({ "answer": "(user did not respond)" }),
                data_class: DataClass::Internal,
            }),
        }
    } else {
        Err(LoopError::ToolExecutionFailed {
            tool_id: call.tool_id.clone(),
            detail: "Question tool requires an active UI connection".to_string(),
        })
    }
}

pub(super) async fn handle_activate_skill(
    call: &ToolCall,
    context: &LoopContext,
) -> Result<ToolResult, LoopError> {
    let name = call.input.get("name").and_then(|value| value.as_str()).ok_or_else(|| {
        LoopError::ToolExecutionFailed {
            tool_id: call.tool_id.clone(),
            detail: "missing required 'name' parameter".to_string(),
        }
    })?;

    if let Some(ref catalog) = context.tools_ctx.skill_catalog {
        match catalog.activate(name) {
            Some(result) => {
                let mut content = result.content;

                // Stage skill resources into the workspace so the model can
                // access them via the sandboxed filesystem tools.
                if let (Some(source_dir), Some(workspace)) =
                    (&result.source_dir, context.workspace_path())
                {
                    let target = workspace.join(".skills").join(name);
                    match hive_skills::stage_skill_resources(source_dir, &target) {
                        Ok(_) => {
                            let abs_str = source_dir.to_string_lossy();
                            let relative = format!(".skills/{name}");
                            content = content.replace(abs_str.as_ref(), &relative);
                        }
                        Err(e) => {
                            tracing::warn!(
                                skill = name,
                                error = %e,
                                "failed to stage skill resources into workspace"
                            );
                        }
                    }
                }

                Ok(ToolResult {
                    output: serde_json::json!({ "content": content }),
                    data_class: hive_classification::DataClass::Internal,
                })
            }
            None => Err(LoopError::ToolExecutionFailed {
                tool_id: call.tool_id.clone(),
                detail: format!("skill '{name}' is not installed or enabled"),
            }),
        }
    } else {
        Err(LoopError::ToolExecutionFailed {
            tool_id: call.tool_id.clone(),
            detail: "no skill catalog available".to_string(),
        })
    }
}

pub(super) async fn handle_spawn_agent_tool(
    call: &ToolCall,
    context: &LoopContext,
) -> Result<ToolResult, LoopError> {
    // Accept "persona" (preferred) or "agent_name" (backward compat)
    let persona_name = call
        .input
        .get("persona")
        .or_else(|| call.input.get("agent_name"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let friendly_name = call
        .input
        .get("friendly_name")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(String::from);
    let keep_alive = match call.input.get("mode").and_then(|v| v.as_str()) {
        Some("idle_after_task") | Some("continuous") => true,
        // Also support legacy "keep_alive" boolean for backward compat.
        _ => call.input.get("keep_alive").and_then(|v| v.as_bool()).unwrap_or(false),
    };
    let task = call
        .input
        .get("task")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| LoopError::ToolExecutionFailed {
            tool_id: call.tool_id.clone(),
            detail: "missing required 'task' parameter".to_string(),
        })?;

    let orchestrator =
        context.agent_orchestrator().ok_or_else(|| LoopError::ToolExecutionFailed {
            tool_id: call.tool_id.clone(),
            detail: "agent orchestration is not available in this context".to_string(),
        })?;

    let definition = persona_name
        .and_then(|name| resolve_persona_by_name(name, context.personas()))
        .cloned()
        .unwrap_or_else(|| {
            context
                .personas()
                .iter()
                .find(|d| d.id == "system/general")
                .cloned()
                .unwrap_or_else(Persona::default_persona)
        });

    let from = current_agent_sender_id(context);
    let parent_model = context.routing_decision().map(|d| d.selected.clone());
    let parent_workspace = context.workspace_path().map(PathBuf::from);
    let agent_id = orchestrator
        .spawn_agent(
            definition,
            task.to_string(),
            from,
            friendly_name,
            context.effective_data_class(),
            parent_model,
            keep_alive,
            parent_workspace,
        )
        .await
        .map_err(|detail| LoopError::ToolExecutionFailed {
            tool_id: call.tool_id.clone(),
            detail,
        })?;

    Ok(ToolResult {
        output: serde_json::json!({ "agent_id": agent_id }),
        data_class: DataClass::Internal,
    })
}

pub(super) async fn handle_list_agents_tool(
    call: &ToolCall,
    context: &LoopContext,
) -> Result<ToolResult, LoopError> {
    let orchestrator =
        context.agent_orchestrator().ok_or_else(|| LoopError::ToolExecutionFailed {
            tool_id: call.tool_id.clone(),
            detail: "agent orchestration is not available in this context".to_string(),
        })?;

    let agents = orchestrator.list_agents().await.map_err(|detail| {
        LoopError::ToolExecutionFailed { tool_id: call.tool_id.clone(), detail }
    })?;

    let entries: Vec<serde_json::Value> = agents
        .into_iter()
        .map(|(id, name, description, status, result)| {
            let mut entry = serde_json::json!({
                "id": id,
                "name": name,
                "description": description,
                "status": status,
            });
            if let Some(ref r) = result {
                let truncated =
                    if r.len() > 200 { format!("{}…", truncate_str(r, 200)) } else { r.clone() };
                entry["result_preview"] = serde_json::json!(truncated);
            }
            entry
        })
        .collect();

    Ok(ToolResult {
        output: serde_json::json!({ "agents": entries }),
        data_class: DataClass::Internal,
    })
}

pub(super) async fn handle_get_agent_result_tool(
    call: &ToolCall,
    context: &LoopContext,
) -> Result<ToolResult, LoopError> {
    let orchestrator =
        context.agent_orchestrator().ok_or_else(|| LoopError::ToolExecutionFailed {
            tool_id: call.tool_id.clone(),
            detail: "agent orchestration is not available in this context".to_string(),
        })?;

    let agent_id = call.input["agent_id"]
        .as_str()
        .ok_or_else(|| LoopError::ToolExecutionFailed {
            tool_id: call.tool_id.clone(),
            detail: "missing required parameter: agent_id".to_string(),
        })?
        .to_string();

    let (status, result) =
        orchestrator.get_agent_result(agent_id.clone()).await.map_err(|detail| {
            LoopError::ToolExecutionFailed { tool_id: call.tool_id.clone(), detail }
        })?;

    let mut output = serde_json::json!({
        "agent_id": agent_id,
        "status": status,
    });
    match result {
        Some(r) => output["result"] = serde_json::json!(r),
        None => output["result"] = serde_json::json!(null),
    }

    Ok(ToolResult { output, data_class: DataClass::Internal })
}

pub(super) async fn handle_wait_for_agent_tool(
    call: &ToolCall,
    context: &LoopContext,
) -> Result<ToolResult, LoopError> {
    let orchestrator =
        context.agent_orchestrator().ok_or_else(|| LoopError::ToolExecutionFailed {
            tool_id: call.tool_id.clone(),
            detail: "agent orchestration is not available in this context".to_string(),
        })?;

    let agent_id = call.input["agent_id"]
        .as_str()
        .ok_or_else(|| LoopError::ToolExecutionFailed {
            tool_id: call.tool_id.clone(),
            detail: "missing required parameter: agent_id".to_string(),
        })?
        .to_string();

    let timeout_secs = call.input.get("timeout_secs").and_then(|v| v.as_u64());

    // Race the actual wait against the preempt signal so that a new user
    // message can interrupt a long wait on a sub-agent (e.g. one that is
    // blocked on a question).
    let preempt = context.preempt_signal.clone();
    let wait_fut = orchestrator.wait_for_agent(agent_id.clone(), timeout_secs);
    tokio::pin!(wait_fut);

    let (status, result) = tokio::select! {
        res = &mut wait_fut => {
            res.map_err(|detail| {
                LoopError::ToolExecutionFailed { tool_id: call.tool_id.clone(), detail }
            })?
        }
        _ = poll_preempt_signal(preempt) => {
            ("preempted".to_string(), Some("A new user message arrived; the wait was interrupted. The sub-agent is still running.".to_string()))
        }
    };

    let mut output = serde_json::json!({
        "agent_id": agent_id,
        "status": status,
    });
    match result {
        Some(r) => output["result"] = serde_json::json!(r),
        None => output["result"] = serde_json::json!(null),
    }

    Ok(ToolResult { output, data_class: DataClass::Internal })
}

/// Poll an `AtomicBool` preempt signal at short intervals.
/// Resolves when the signal is set to `true`, or never if `signal` is `None`.
pub(super) async fn poll_preempt_signal(signal: Option<Arc<AtomicBool>>) {
    match signal {
        Some(sig) => loop {
            if sig.load(AtomicOrdering::Acquire) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        },
        None => std::future::pending::<()>().await,
    }
}

pub(super) async fn handle_list_personas_tool(
    _call: &ToolCall,
    context: &LoopContext,
) -> Result<ToolResult, LoopError> {
    let entries: Vec<serde_json::Value> = context
        .personas()
        .iter()
        .map(|p| {
            serde_json::json!({
                "id": p.id,
                "name": p.name,
                "description": p.description,
            })
        })
        .collect();

    Ok(ToolResult {
        output: serde_json::json!({ "personas": entries }),
        data_class: DataClass::Internal,
    })
}

pub(super) async fn handle_kill_agent_tool(
    call: &ToolCall,
    context: &LoopContext,
) -> Result<ToolResult, LoopError> {
    let agent_id = call
        .input
        .get("agent_id")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| LoopError::ToolExecutionFailed {
            tool_id: call.tool_id.clone(),
            detail: "missing required 'agent_id' parameter".to_string(),
        })?;

    let orchestrator =
        context.agent_orchestrator().ok_or_else(|| LoopError::ToolExecutionFailed {
            tool_id: call.tool_id.clone(),
            detail: "agent orchestration is not available in this context".to_string(),
        })?;

    // Access control: only the direct parent of an agent can kill it.
    // Session-level callers (no current_agent_id) are always allowed.
    // Bot/service-prefixed targets cannot be killed from agent tools.
    if agent_id.starts_with("bot:") || agent_id.starts_with("service:") {
        return Ok(ToolResult {
            output: serde_json::json!({
                "error": "Access denied: cannot kill global bot/service agents."
            }),
            data_class: DataClass::Internal,
        });
    }
    if let Some(caller_id) = context.current_agent_id() {
        // Check that the caller is the direct parent of the target.
        match orchestrator.get_agent_parent(agent_id.to_string()).await {
            Ok(Some(parent)) if parent == caller_id => { /* allowed — caller is parent */ }
            Ok(_) => {
                return Ok(ToolResult {
                    output: serde_json::json!({
                        "error": "Access denied: you can only kill agents that you spawned (your direct children)."
                    }),
                    data_class: DataClass::Internal,
                });
            }
            Err(e) => {
                return Ok(ToolResult {
                    output: serde_json::json!({ "error": format!("Cannot verify agent relationship: {e}") }),
                    data_class: DataClass::Internal,
                });
            }
        }
    }

    orchestrator.kill_agent(agent_id.to_string()).await.map_err(|detail| {
        LoopError::ToolExecutionFailed { tool_id: call.tool_id.clone(), detail }
    })?;

    Ok(ToolResult {
        output: serde_json::json!({ "killed": true }),
        data_class: DataClass::Internal,
    })
}

pub(super) async fn handle_signal_agent_tool(
    call: &ToolCall,
    context: &LoopContext,
) -> Result<ToolResult, LoopError> {
    let requested_target = call
        .input
        .get("agent_id")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| LoopError::ToolExecutionFailed {
            tool_id: call.tool_id.clone(),
            detail: "missing required 'agent_id' parameter".to_string(),
        })?;
    let message = call
        .input
        .get("content")
        .or_else(|| call.input.get("message"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| LoopError::ToolExecutionFailed {
            tool_id: call.tool_id.clone(),
            detail: "missing required 'content' parameter".to_string(),
        })?;

    let orchestrator =
        context.agent_orchestrator().ok_or_else(|| LoopError::ToolExecutionFailed {
            tool_id: call.tool_id.clone(),
            detail: "agent orchestration is not available in this context".to_string(),
        })?;

    let agent_id = if requested_target == "parent" {
        // "parent" resolves to the parent agent, or to "session" if spawned from the chat session
        context.parent_agent_id().map(|s| s.to_string()).unwrap_or_else(|| "session".to_string())
    } else {
        requested_target.to_string()
    };

    let from = current_agent_sender_id(context).unwrap_or_else(|| "unknown".to_string());

    // Access control: validate that the caller can message the target.
    // Skip for session-level callers, "session" target, and bot/service targets.
    if agent_id != "session" && !agent_id.starts_with("bot:") && !agent_id.starts_with("service:") {
        if let Some(caller_id) = context.current_agent_id() {
            if let Err(reason) =
                check_agent_family(orchestrator.as_ref(), caller_id, &agent_id).await
            {
                return Ok(ToolResult {
                    output: serde_json::json!({
                        "error": format!("Access denied: {reason}. You can only message your parent, children, or sibling agents.")
                    }),
                    data_class: DataClass::Internal,
                });
            }
        }
    }

    if agent_id == "session" {
        // For one-shot agents, only allow one message to the session.
        if !context.keep_alive() && context.session_messaged().load(AtomicOrdering::SeqCst) {
            return Ok(ToolResult {
                output: serde_json::json!({
                    "error": "Signal already delivered. You are a one-shot agent — \
                              do not signal again. Produce your final summary now."
                }),
                data_class: DataClass::Internal,
            });
        }
        // Route message back to the parent chat session
        orchestrator.message_session(message.to_string(), from).await.map_err(|detail| {
            LoopError::ToolExecutionFailed { tool_id: call.tool_id.clone(), detail }
        })?;
        // Mark as messaged only after successful delivery so transient
        // failures don't permanently block retry attempts.
        if !context.keep_alive() {
            context.session_messaged().store(true, AtomicOrdering::SeqCst);
        }
    } else {
        orchestrator.message_agent(agent_id.clone(), message.to_string(), from).await.map_err(
            |detail| LoopError::ToolExecutionFailed { tool_id: call.tool_id.clone(), detail },
        )?;
    }

    let output = if agent_id == "session" && !context.keep_alive() {
        serde_json::json!({
            "agent_id": agent_id,
            "delivered": true,
            "hint": "Signal delivered. Your task is complete — produce a brief final summary and stop."
        })
    } else {
        serde_json::json!({ "agent_id": agent_id, "delivered": true })
    };

    Ok(ToolResult { output, data_class: DataClass::Internal })
}

/// Check whether `caller_id` has a family relationship with `target_id` within
/// the same supervisor. Family = parent, child, or sibling (same parent).
/// Returns `Ok(())` if allowed, `Err(reason)` if not.
///
/// Both parents are queried from the orchestrator (the authoritative source)
/// rather than relying on the loop context's `parent_agent_id`, which
/// represents the sender of the current message — not the spawn parent.
async fn check_agent_family(
    orchestrator: &dyn AgentOrchestrator,
    caller_id: &str,
    target_id: &str,
) -> Result<(), String> {
    let caller_parent = orchestrator.get_agent_parent(caller_id.to_string()).await?;
    let target_parent = orchestrator.get_agent_parent(target_id.to_string()).await?;

    // Target is the caller's parent?
    if caller_parent.as_deref() == Some(target_id) {
        return Ok(());
    }

    // Caller is the target's parent?
    if target_parent.as_deref() == Some(caller_id) {
        return Ok(());
    }

    // Siblings — share the same parent (both root-level or same parent agent).
    if caller_parent == target_parent {
        return Ok(());
    }

    Err(format!("agent '{caller_id}' has no family relationship with '{target_id}'"))
}

pub(super) async fn handle_knowledge_query_tool(
    call: &ToolCall,
    context: &LoopContext,
) -> Result<ToolResult, LoopError> {
    let handler =
        context.knowledge_query_handler().ok_or_else(|| LoopError::ToolExecutionFailed {
            tool_id: call.tool_id.clone(),
            detail: "knowledge graph is not available in this context".to_string(),
        })?;

    handler
        .handle_query(call.input.clone())
        .await
        .map_err(|detail| LoopError::ToolExecutionFailed { tool_id: call.tool_id.clone(), detail })
}

pub(super) fn resolve_persona_by_name<'a>(
    agent_name: &str,
    definitions: &'a [Persona],
) -> Option<&'a Persona> {
    definitions
        .iter()
        .find(|definition| definition.id == agent_name || definition.name == agent_name)
        .or_else(|| {
            definitions.iter().find(|definition| {
                definition.id.eq_ignore_ascii_case(agent_name)
                    || definition.name.eq_ignore_ascii_case(agent_name)
            })
        })
}

pub(super) fn current_agent_sender_id(context: &LoopContext) -> Option<String> {
    context.current_agent_id().map(|s| s.to_string())
}
