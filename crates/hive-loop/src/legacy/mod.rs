mod interaction;
pub use interaction::UserInteractionGate;

mod journal;
pub use journal::{ConversationJournal, JournalEntry, JournalPhase, JournalToolCall};

pub(crate) mod streaming;

pub(crate) mod parsing;
pub use parsing::ToolCall;
pub use parsing::{parse_tool_call, parse_tool_calls, strip_xml_tool_blocks};

pub(crate) mod types;
pub use types::{
    AgentContext, AgentOrchestrator, BoxFuture, CodeExecutionPhase, ConversationContext,
    KnowledgeQueryHandler, LoopContext, LoopError, LoopEvent, LoopResult, RoutingConfig,
    SecurityContext, ToolsContext,
};

pub(crate) mod strategy;
pub use strategy::{LoopExecutor, LoopMiddleware, LoopStrategy, StrategyKind};

pub(crate) mod tool_execution;
#[allow(unused_imports)] // Used by tests via `super::*`
use tool_execution::{
    execute_tool_batch, execute_tool_call, estimate_request_tokens, run_single_tool_call,
    truncate_str,
};

pub(crate) mod tool_handlers;
use tool_handlers::*;

pub(crate) mod strategies;
pub use strategies::{CodeActStrategy, PlanThenExecuteStrategy, ReActStrategy, SequentialStrategy};

use hive_model::{ModelRouterError, RoutingDecision};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::Arc;

use parking_lot::Mutex;

/// Build a human-readable summary of journal tool calls for a preempted turn.
/// Truncates individual tool outputs to keep the summary compact.
pub(crate) fn build_preemption_summary(journal: &ConversationJournal) -> String {
    let mut lines = Vec::new();
    lines.push("[Turn paused to process a new message]\n".to_string());
    lines.push("Progress so far:".to_string());

    let mut call_num = 0usize;
    for entry in &journal.entries {
        for tc in &entry.tool_calls {
            call_num += 1;
            let truncated_output = if tc.output.len() > 200 {
                format!("{}…", &tc.output[..200])
            } else {
                tc.output.clone()
            };
            lines.push(format!("{}. Called `{}` → {}", call_num, tc.tool_id, truncated_output));
        }
    }

    if call_num == 0 {
        lines.push("(no tool calls completed)".to_string());
    }

    lines.join("\n")
}

/// Check the preempt signal and, if set, build a preempted `LoopResult`.
/// Returns `Some(LoopResult)` when the loop should yield.
pub(crate) async fn check_preempt(
    signal: &Option<Arc<AtomicBool>>,
    journal: &Option<Arc<Mutex<ConversationJournal>>>,
    decision: &RoutingDecision,
    provider_id: &str,
    model: &str,
    event_tx: Option<&tokio::sync::mpsc::Sender<LoopEvent>>,
) -> Option<LoopResult> {
    let sig = signal.as_ref()?;
    if !sig.load(AtomicOrdering::Acquire) {
        return None;
    }

    let content = if let Some(ref j) = journal {
        build_preemption_summary(&j.lock())
    } else {
        "[Turn paused to process a new message]".to_string()
    };

    if let Some(tx) = event_tx {
        if tx.send(LoopEvent::Preempted).await.is_err() {
            tracing::warn!("failed to send Preempted event — loop receiver dropped");
        }
    }

    Some(LoopResult {
        content,
        provider_id: provider_id.to_string(),
        model: model.to_string(),
        decision: decision.clone(),
        preempted: true,
    })
}

/// Tools that are exempt from the adaptive tool-call budget.
/// These are lightweight status/polling tools that don't represent
/// forward progress and shouldn't consume the agent's budget.
pub(crate) fn is_budget_exempt(tool_id: &str) -> bool {
    matches!(
        tool_id,
        "core.list_agents"
            | "core.get_agent_result"
            | "core.wait_for_agent"
            | "process.status"
            | "process.list"
    )
}

/// Convert a [`ModelRouterError`] into a [`LoopError::ModelExecution`] with
/// structured error fields extracted from the router error.
pub(crate) fn model_router_error_to_loop_error(error: ModelRouterError) -> LoopError {
    match &error {
        ModelRouterError::ProviderExecutionFailed { error_kind, http_status, .. } => {
            LoopError::ModelExecution {
                error_code: error_kind.map(|k| format!("{k:?}").to_lowercase()),
                http_status: *http_status,
                provider_id: None,
                model: None,
                message: error.to_string(),
            }
        }
        _ => LoopError::ModelExecution {
            message: error.to_string(),
            error_code: None,
            http_status: None,
            provider_id: None,
            model: None,
        },
    }
}

/// Build a simple [`LoopError::ModelExecution`] from a string (for non-router
/// errors like mid-stream failures or join errors).
pub(crate) fn simple_model_error(message: String) -> LoopError {
    LoopError::ModelExecution {
        message,
        error_code: None,
        http_status: None,
        provider_id: None,
        model: None,
    }
}

#[cfg(test)]
mod tests;
