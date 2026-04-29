use std::sync::Arc;

use hive_model::{CompletionRequest, CompletionResponse, RoutingRequest};

use super::super::interaction::UserInteractionGate;
use super::super::journal::{JournalEntry, JournalPhase};
use super::super::parsing::{parse_tool_calls, ToolCall};
use super::super::strategy::{LoopMiddleware, LoopStrategy};
use super::super::tool_execution::execute_tool_batch;
use super::super::types::{BoxFuture, LoopContext, LoopError, LoopEvent, LoopResult};
use super::super::{
    check_preempt, is_budget_exempt, model_router_error_to_loop_error, simple_model_error,
};

const MAX_PLAN_STEPS: usize = 10;

#[derive(Default)]
pub struct PlanThenExecuteStrategy;

impl PlanThenExecuteStrategy {
    pub(crate) fn parse_plan(content: &str) -> Vec<String> {
        content
            .lines()
            .filter_map(|line| {
                let trimmed = line.trim();
                // Match lines like "1. ...", "2) ...", "- ..." etc.
                if let Some(rest) = trimmed.strip_prefix("- ") {
                    Some(rest.trim().to_string())
                } else if let Some(pos) = trimmed.find(". ") {
                    let prefix = &trimmed[..pos];
                    if prefix.chars().all(|c| c.is_ascii_digit()) {
                        Some(trimmed[pos + 2..].trim().to_string())
                    } else {
                        None
                    }
                } else if let Some(pos) = trimmed.find(") ") {
                    let prefix = &trimmed[..pos];
                    if prefix.chars().all(|c| c.is_ascii_digit()) {
                        Some(trimmed[pos + 2..].trim().to_string())
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect()
    }
}

impl LoopStrategy for PlanThenExecuteStrategy {
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

            // Check if we're resuming from a journal
            let (steps, mut accumulated_results, resume_step) = {
                let journal_ref = context.conversation.conversation_journal.as_ref();
                let journal_guard = journal_ref.map(|j| j.lock());

                if let Some(j) = journal_guard.filter(|j| !j.entries.is_empty()) {
                    if let Some(plan_steps) = j.get_plan_steps() {
                        let completed_results = j.get_completed_step_results();
                        let last_step = j.last_completed_step_index().map(|i| i + 1).unwrap_or(0);
                        (Some(plan_steps), completed_results, last_step)
                    } else {
                        (None, Vec::new(), 0)
                    }
                } else {
                    (None, Vec::new(), 0)
                }
            };

            let steps = if let Some(steps) = steps {
                // Resuming with a previously generated plan
                steps
            } else {
                // Phase 1: Ask the model for a plan
                let plan_prompt = format!(
                    "Create a numbered plan (one step per line) to accomplish the following task. \
                     Output ONLY the numbered steps, nothing else.\n\nTask: {}",
                    context.conversation.prompt
                );

                let mut plan_request = CompletionRequest {
                    prompt: plan_prompt,
                    prompt_content_parts: context.conversation.prompt_content_parts.clone(),
                    messages: context.conversation.history.clone(),
                    required_capabilities: context.routing.required_capabilities.clone(),
                    preferred_models: context.routing.preferred_models.clone(),
                    tools: context.tools_ctx.tools.list_definitions(),
                };

                for hook in middleware {
                    plan_request = hook.before_model_call(&context, plan_request)?;
                }

                let router = Arc::clone(&model_router);
                let decision_clone = decision.clone();
                let request_clone = plan_request.clone();
                let blocking_future = tokio::task::spawn_blocking(move || {
                    router.complete_with_decision(&request_clone, &decision_clone)
                });
                let plan_response = if let Some(ref token) = context.cancellation_token {
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
                };

                let mut plan_response = plan_response;
                for hook in middleware {
                    plan_response = hook.after_model_response(&context, plan_response)?;
                }

                let mut parsed_steps = Self::parse_plan(&plan_response.content);
                parsed_steps.truncate(MAX_PLAN_STEPS);

                if parsed_steps.is_empty() {
                    return Ok(LoopResult {
                        content: plan_response.content,
                        provider_id: plan_response.provider_id,
                        model: plan_response.model,
                        decision,
                        preempted: false,
                    });
                }

                // Journal the plan
                if let Some(ref journal) = context.conversation.conversation_journal {
                    let mut j = journal.lock();
                    j.record(JournalEntry {
                        phase: JournalPhase::Plan { steps: parsed_steps.clone() },
                        turn: 0,
                        tool_calls: Vec::new(),
                        assistant_content: None,
                    });
                }

                parsed_steps
            };

            // Phase 2: Execute each step with adaptive tool-call limits
            let mut last_response = CompletionResponse {
                provider_id: String::new(),
                model: String::new(),
                content: String::new(),
                tool_calls: Vec::new(),
            };

            // Shared adaptive budget across all plan steps.
            let mut budget = crate::tool_budget::AdaptiveBudget::new(&context.tool_limits);
            let mut total_tool_calls = 0usize;
            // Stall breaker for the plan-and-execute loop (same as ReAct).
            let mut ask_user_history: Vec<(String, String)> = Vec::new();

            for (step_idx, step) in steps.iter().enumerate() {
                // Skip already-completed steps when resuming
                if step_idx < resume_step {
                    continue;
                }

                let mut step_prompt = format!(
                    "You are executing a plan step by step.\n\n\
                     Original task: {}\n\n\
                     Completed so far:\n{}\n\n\
                     Current step: {}",
                    context.conversation.prompt,
                    accumulated_results.join("\n"),
                    step
                );

                let mut _tool_calls_in_step = 0usize;

                loop {
                    let mut request = CompletionRequest {
                        prompt: step_prompt.clone(),
                        prompt_content_parts: context.conversation.prompt_content_parts.clone(),
                        messages: context.conversation.history.clone(),
                        required_capabilities: context.routing.required_capabilities.clone(),
                        preferred_models: context.routing.preferred_models.clone(),
                        tools: context.tools_ctx.tools.list_definitions(),
                    };

                    for hook in middleware {
                        request = hook.before_model_call(&context, request)?;
                    }

                    let router = Arc::clone(&model_router);
                    let decision_clone = decision.clone();
                    let request_clone = request.clone();
                    let blocking_future = tokio::task::spawn_blocking(move || {
                        router.complete_with_decision(&request_clone, &decision_clone)
                    });
                    let response = if let Some(ref token) = context.cancellation_token {
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
                        parse_tool_calls(&response.content)
                    };

                    if !detected_calls.is_empty() {
                        let billable_count =
                            detected_calls.iter().filter(|c| !is_budget_exempt(&c.tool_id)).count();
                        // Check adaptive budget BEFORE executing the batch.
                        match budget.check(total_tool_calls, billable_count) {
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
                                    step = step_idx,
                                    "tool-call budget extended in plan step"
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

                        if tool_results.is_empty() {
                            return Err(simple_model_error(
                                "tool calls returned no results".to_string(),
                            ));
                        }

                        // ── Stall breaker (plan-and-execute) ─────────
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
                                            v.get("answer")
                                                .and_then(|a| a.as_str())
                                                .map(String::from)
                                        })
                                        .unwrap_or_default();
                                ask_user_history.push((question_prefix, answer));
                            } else {
                                ask_user_history.clear();
                            }
                        }
                        if ask_user_history.len() >= 2 {
                            let last = &ask_user_history[ask_user_history.len() - 1];
                            let repeats = ask_user_history
                                .iter()
                                .rev()
                                .take_while(|(q, a)| q == &last.0 && a == &last.1)
                                .count();
                            if repeats >= 2 {
                                tracing::info!(
                                    repeats,
                                    answer = %last.1,
                                    "stall breaker: injecting nudge after repeated identical ask_user (plan)"
                                );
                                tool_results.push_str(
                                    "\n\n[System: The user has already confirmed this exact request. \
                                     Do NOT ask again. Proceed immediately with the appropriate \
                                     action to fulfill the user's confirmed request.]"
                                );
                            }
                        }
                        // ── End stall breaker ────────────────────────

                        // PlanAndExecute: the step prompt grows with each
                        // tool iteration. Apply same XML/multi-turn split
                        // as ReAct for consistency (legacy XML for now —
                        // PlanAndExecute rebuilds step_prompt each step).
                        step_prompt = format!("{step_prompt}{tool_results}");
                        let billable =
                            detected_calls.iter().filter(|c| !is_budget_exempt(&c.tool_id)).count();
                        _tool_calls_in_step += billable;
                        total_tool_calls += billable;

                        if let Some(ref journal) = context.conversation.conversation_journal {
                            let mut j = journal.lock();
                            j.record(JournalEntry {
                                phase: JournalPhase::ToolCycle,
                                turn: step_idx + 1,
                                tool_calls: journal_tool_calls,
                                assistant_content: Some(response.content.clone()),
                            });
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

                    let step_result = format!("- {}: {}", step, response.content);
                    accumulated_results.push(step_result.clone());
                    last_response = response;

                    // Journal step completion
                    if let Some(ref journal) = context.conversation.conversation_journal {
                        let mut j = journal.lock();
                        j.record(JournalEntry {
                            phase: JournalPhase::StepComplete {
                                step_index: step_idx,
                                result: step_result,
                            },
                            turn: step_idx + 1,
                            tool_calls: Vec::new(),
                            assistant_content: None,
                        });
                    }

                    break;
                }
            }

            Ok(LoopResult {
                content: last_response.content,
                provider_id: last_response.provider_id,
                model: last_response.model,
                decision,
                preempted: false,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_plan_numbered_dot() {
        let input = "1. First step\n2. Second step\n3. Third step";
        let plan = PlanThenExecuteStrategy::parse_plan(input);
        assert_eq!(plan, vec!["First step", "Second step", "Third step"]);
    }

    #[test]
    fn parse_plan_numbered_paren() {
        let input = "1) Do this\n2) Do that";
        let plan = PlanThenExecuteStrategy::parse_plan(input);
        assert_eq!(plan, vec!["Do this", "Do that"]);
    }

    #[test]
    fn parse_plan_dashes() {
        let input = "- Step A\n- Step B\n- Step C";
        let plan = PlanThenExecuteStrategy::parse_plan(input);
        assert_eq!(plan, vec!["Step A", "Step B", "Step C"]);
    }

    #[test]
    fn parse_plan_mixed_with_non_plan_lines() {
        let input = "Here is my plan:\n1. Do X\n2. Do Y\nSome extra text\n3. Do Z";
        let plan = PlanThenExecuteStrategy::parse_plan(input);
        assert_eq!(plan, vec!["Do X", "Do Y", "Do Z"]);
    }
}
