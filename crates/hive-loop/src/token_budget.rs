//! Token-budget enforcement middleware for the agentic loop.
//!
//! [`TokenBudgetMiddleware`] implements [`LoopMiddleware`] and intercepts
//! every model call to ensure the estimated token count stays within the
//! selected model's context window.  When the request is too large it
//! progressively truncates:
//!
//! 1. Oldest conversation history messages (keeping system prompt + recent N).
//! 2. The intra-turn prompt itself (oldest tool-call/result blocks first).
//! 3. If still over budget, returns a clear error instead of a provider 400.

use std::sync::Arc;

use hive_core::model_limits::ModelLimitsRegistry;
use hive_model::{CompletionMessage, CompletionRequest};
use tracing::{debug, warn};

use crate::legacy::{simple_model_error, LoopContext, LoopError, LoopMiddleware};
#[cfg(test)]
use crate::legacy::{
    AgentContext, ConversationContext, RoutingConfig, SecurityContext, ToolsContext,
};

// ── Token estimation ────────────────────────────────────────────────

/// Approximate token count for a string (1 token ≈ 4 characters).
fn estimate_tokens(text: &str) -> usize {
    text.len().div_ceil(4)
}

/// Estimate the total token count for a [`CompletionRequest`].
pub fn estimate_request_tokens(request: &CompletionRequest) -> usize {
    let prompt_tokens = estimate_tokens(&request.prompt);
    let history_tokens: usize = request
        .messages
        .iter()
        .map(|m| {
            let base = estimate_tokens(&m.content) + 4; // role overhead
            if m.blocks.is_empty() {
                return base;
            }
            // When blocks are present, estimate from them instead of content.
            let block_tokens: usize = m
                .blocks
                .iter()
                .map(|b| match b {
                    hive_model::MessageBlock::Text { text } => estimate_tokens(text),
                    hive_model::MessageBlock::ToolUse { id, name, input } => {
                        let args =
                            serde_json::to_string(input).unwrap_or_else(|_| "{}".to_string());
                        estimate_tokens(id) + estimate_tokens(name) + estimate_tokens(&args) + 10
                        // structural overhead
                    }
                    hive_model::MessageBlock::ToolResult { tool_use_id, content, .. } => {
                        estimate_tokens(tool_use_id) + estimate_tokens(content) + 10
                    }
                })
                .sum();
            block_tokens + 4 // role overhead
        })
        .sum();
    let tools_tokens: usize = request
        .tools
        .iter()
        .map(|t| {
            let schema_str = serde_json::to_string(&t.input_schema).unwrap_or_default();
            estimate_tokens(&t.name)
                + estimate_tokens(&t.description)
                + estimate_tokens(&schema_str)
        })
        .sum();
    prompt_tokens + history_tokens + tools_tokens
}

// ── Middleware ───────────────────────────────────────────────────────

/// Number of recent history messages to always preserve during truncation.
const KEEP_RECENT_MESSAGES: usize = 6;

/// Safety margin — leave headroom for the model's reply (fraction of context).
const OUTPUT_RESERVE_FRACTION: f64 = 0.15;

/// Minimum output reserve in tokens (floor).
const OUTPUT_RESERVE_MIN: usize = 2048;

pub struct TokenBudgetMiddleware {
    limits: Arc<ModelLimitsRegistry>,
}

impl TokenBudgetMiddleware {
    pub fn new(limits: Arc<ModelLimitsRegistry>) -> Self {
        Self { limits }
    }

    /// Resolve the model name from the routing decision in the context.
    fn model_name<'a>(&self, context: &'a LoopContext) -> &'a str {
        context.routing_decision().map(|d| d.selected.model.as_str()).unwrap_or("")
    }

    /// Compute the input token budget for the selected model.
    fn input_budget(&self, context: &LoopContext) -> usize {
        let model = self.model_name(context);
        let static_limits = self.limits.lookup(model);
        let context_window = context
            .routing_decision()
            .and_then(|d| d.effective_context_window)
            .unwrap_or(static_limits.context_window) as usize;
        let max_output_tokens = context
            .routing_decision()
            .and_then(|d| d.effective_max_output_tokens)
            .unwrap_or(static_limits.max_output_tokens) as usize;

        // Reserve space for the model's output.
        let output_reserve = (context_window as f64 * OUTPUT_RESERVE_FRACTION) as usize;
        let output_reserve = output_reserve.max(OUTPUT_RESERVE_MIN).min(max_output_tokens);

        let budget = context_window.saturating_sub(output_reserve);
        debug!(
            model,
            context_window,
            max_output_tokens,
            output_reserve,
            input_budget = budget,
            "token budget computed for model"
        );
        budget
    }
}

/// Truncate a [`CompletionRequest`] so its estimated token count fits within
/// the given `budget`. Returns the (possibly modified) request or an error if
/// truncation cannot bring it within budget.
///
/// This is the shared implementation used by both the middleware and the
/// standalone [`enforce_with_limit`] recovery path.
fn truncate_request_to_budget(
    mut request: CompletionRequest,
    budget: usize,
    model_label: &str,
) -> Result<CompletionRequest, LoopError> {
    let mut estimated = estimate_request_tokens(&request);

    if estimated <= budget {
        return Ok(request);
    }

    warn!(model = model_label, estimated, budget, "token budget exceeded — truncating request");

    // ── Step 1: Truncate conversation history (oldest non-system first) ─
    if request.messages.len() > KEEP_RECENT_MESSAGES + 1 {
        let system_count = request.messages.iter().take_while(|m| m.role == "system").count();
        let non_system_count = request.messages.len() - system_count;

        if non_system_count > KEEP_RECENT_MESSAGES {
            let to_remove = non_system_count - KEEP_RECENT_MESSAGES;
            let remove_start = system_count;
            let remove_end = system_count + to_remove;
            let dropped: Vec<CompletionMessage> =
                request.messages.drain(remove_start..remove_end).collect();
            let dropped_count = dropped.len();

            request.messages.insert(
                remove_start,
                CompletionMessage {
                    role: "system".to_string(),
                    content: format!(
                        "[{dropped_count} earlier conversation messages were omitted \
                         to fit within the model's context window]"
                    ),
                    content_parts: vec![],
                    blocks: vec![],
                },
            );

            estimated = estimate_request_tokens(&request);
            if estimated <= budget {
                warn!(estimated, budget, dropped_count, "budget restored after history truncation");
                return Ok(request);
            }
        }
    }

    // ── Step 2: Truncate old tool blocks in the prompt ──────────────────
    let tool_result_end = "</tool_result>";
    let tool_call_start = "<tool_call>";

    let mut last_estimated = estimated;
    while estimated > budget {
        if let Some(tc_start) = request.prompt.find(tool_call_start) {
            if let Some(tr_end) = request.prompt[tc_start..].find(tool_result_end) {
                let block_end = tc_start + tr_end + tool_result_end.len();
                request
                    .prompt
                    .replace_range(tc_start..block_end, "[earlier tool interaction omitted]");
                estimated = estimate_request_tokens(&request);
                if estimated >= last_estimated {
                    break;
                }
                last_estimated = estimated;
                continue;
            }
        }
        break;
    }

    if estimated <= budget {
        warn!(estimated, budget, "budget restored after prompt truncation");
        return Ok(request);
    }

    // ── Step 3: Hard error — still over budget ──────────────────────────
    Err(simple_model_error(format!(
        "request exceeds model context window after truncation \
         (estimated {estimated} tokens, budget {budget} tokens for model '{model_label}'). \
         Consider using a model with a larger context window or reducing tool output size."
    )))
}

/// Truncate a [`CompletionRequest`] to fit within an explicit context window
/// limit (in tokens). This is used by the auto-recovery path when a provider
/// rejects a request with a token-limit error and reports its actual limit.
pub fn enforce_with_limit(
    request: CompletionRequest,
    context_window: u32,
) -> Result<CompletionRequest, LoopError> {
    let context_window = context_window as usize;
    let output_reserve = (context_window as f64 * OUTPUT_RESERVE_FRACTION) as usize;
    let output_reserve = output_reserve.max(OUTPUT_RESERVE_MIN);
    let budget = context_window.saturating_sub(output_reserve);
    truncate_request_to_budget(request, budget, "recovery")
}

impl LoopMiddleware for TokenBudgetMiddleware {
    fn before_model_call(
        &self,
        context: &LoopContext,
        request: CompletionRequest,
    ) -> Result<CompletionRequest, LoopError> {
        let budget = self.input_budget(context);
        let model = self.model_name(context);
        truncate_request_to_budget(request, budget, model)
    }
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use hive_classification::DataClass;
    use hive_contracts::ToolExecutionMode;
    use hive_model::{ModelSelection, RoutingDecision};
    use std::collections::BTreeSet;

    fn make_context_with_model(model: &str) -> LoopContext {
        LoopContext {
            conversation: ConversationContext {
                session_id: "test".into(),
                message_id: "msg-1".into(),
                prompt: String::new(),
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
                    selected: ModelSelection {
                        provider_id: "test-provider".into(),
                        model: model.into(),
                    },
                    fallback_chain: vec![],
                    reason: "test".into(),
                    effective_context_window: None,
                    effective_max_output_tokens: None,
                }),
            },
            security: SecurityContext {
                data_class: DataClass::Public,
                permissions: Arc::new(parking_lot::Mutex::new(
                    hive_contracts::SessionPermissions::default(),
                )),
                workspace_classification: None,
                effective_data_class: Arc::new(std::sync::atomic::AtomicU8::new(
                    DataClass::Public.to_i64() as u8,
                )),
                connector_service: None,
                shadow_mode: false,
            },
            tools_ctx: ToolsContext {
                tools: Arc::new(hive_tools::ToolRegistry::new()),
                skill_catalog: None,
                knowledge_query_handler: None,
                tool_execution_mode: ToolExecutionMode::default(),
            },
            agent: AgentContext {
                persona: None,
                agent_orchestrator: None,
                workspace_path: None,
                personas: vec![],
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

    fn make_request(prompt_size: usize, history_count: usize) -> CompletionRequest {
        let prompt = "x".repeat(prompt_size);
        let messages: Vec<CompletionMessage> = (0..history_count)
            .map(|i| CompletionMessage {
                role: if i % 2 == 0 { "user" } else { "assistant" }.into(),
                content: "y".repeat(400), // ~100 tokens each
                content_parts: vec![],
                blocks: vec![],
            })
            .collect();
        CompletionRequest {
            prompt,
            prompt_content_parts: vec![],
            messages,
            required_capabilities: BTreeSet::new(),
            preferred_models: None,
            tools: vec![],
            temperature: None,
            stop_sequences: None,
            max_tokens: None,
        }
    }

    #[test]
    fn small_request_passes_through() {
        let mw = TokenBudgetMiddleware::new(Arc::new(ModelLimitsRegistry::load()));
        let ctx = make_context_with_model("gpt-4o");
        let req = make_request(1000, 2);
        let result = mw.before_model_call(&ctx, req.clone());
        assert!(result.is_ok());
        let out = result.unwrap();
        assert_eq!(out.prompt, req.prompt);
        assert_eq!(out.messages.len(), req.messages.len());
    }

    #[test]
    fn oversized_history_gets_truncated() {
        let mw = TokenBudgetMiddleware::new(Arc::new(ModelLimitsRegistry::load()));
        // Use an unknown model (default 32768 context → ~27853 input budget).
        let ctx = make_context_with_model("unknown-tiny-model");
        // 120 messages × ~500 tokens each = ~60000 tokens (well over budget)
        let messages: Vec<CompletionMessage> = (0..120)
            .map(|i| CompletionMessage {
                role: if i % 2 == 0 { "user" } else { "assistant" }.into(),
                content: "y".repeat(2000), // ~500 tokens each
                content_parts: vec![],
                blocks: vec![],
            })
            .collect();
        let req = CompletionRequest {
            prompt: "do something".into(),
            prompt_content_parts: vec![],
            messages,
            required_capabilities: BTreeSet::new(),
            preferred_models: None,
            tools: vec![],
            temperature: None,
            stop_sequences: None,
            max_tokens: None,
        };
        let result = mw.before_model_call(&ctx, req);
        assert!(result.is_ok());
        let out = result.unwrap();
        // Should have fewer messages than the original 120.
        assert!(
            out.messages.len() < 120,
            "expected truncation, got {} messages",
            out.messages.len()
        );
    }

    #[test]
    fn tool_blocks_in_prompt_get_truncated() {
        let mw = TokenBudgetMiddleware::new(Arc::new(ModelLimitsRegistry::load()));
        let ctx = make_context_with_model("unknown-tiny-model"); // 32768 context → ~27853 input
        let big_tool_output = "z".repeat(120000); // ~30000 tokens — exceeds budget alone
        let prompt = format!(
            "task description\n\n\
             <tool_call>\n{{\"tool\": \"read_file\", \"input\": \"a.txt\"}}\n</tool_call>\n\
             <tool_result>\n{big_tool_output}\n</tool_result>\n\n\
             Continue working."
        );
        let req = CompletionRequest {
            prompt,
            prompt_content_parts: vec![],
            messages: vec![],
            required_capabilities: BTreeSet::new(),
            preferred_models: None,
            tools: vec![],
            temperature: None,
            stop_sequences: None,
            max_tokens: None,
        };
        let result = mw.before_model_call(&ctx, req);
        assert!(result.is_ok());
        let out = result.unwrap();
        assert!(!out.prompt.contains(&big_tool_output), "expected tool block to be truncated");
        assert!(out.prompt.contains("[earlier tool interaction omitted]"));
    }

    #[test]
    fn estimate_tokens_basic() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
        assert_eq!(estimate_tokens("hello world"), 3); // 11 chars
    }

    #[test]
    fn enforce_with_limit_truncates_history() {
        // 120 messages × ~500 tokens = ~60K tokens.
        // enforce_with_limit(32768) → budget ≈ 27853
        let messages: Vec<CompletionMessage> = (0..120)
            .map(|i| CompletionMessage {
                role: if i % 2 == 0 { "user" } else { "assistant" }.into(),
                content: "y".repeat(2000), // ~500 tokens
                content_parts: vec![],
                blocks: vec![],
            })
            .collect();
        let req = CompletionRequest {
            prompt: "task".into(),
            prompt_content_parts: vec![],
            messages,
            required_capabilities: BTreeSet::new(),
            preferred_models: None,
            tools: vec![],
            temperature: None,
            stop_sequences: None,
            max_tokens: None,
        };
        let result = enforce_with_limit(req, 32768);
        assert!(result.is_ok());
        let out = result.unwrap();
        assert!(out.messages.len() < 120, "expected truncation");
    }

    #[test]
    fn enforce_with_limit_small_request_passes() {
        let req = CompletionRequest {
            prompt: "hello".into(),
            prompt_content_parts: vec![],
            messages: vec![],
            required_capabilities: BTreeSet::new(),
            preferred_models: None,
            tools: vec![],
            temperature: None,
            stop_sequences: None,
            max_tokens: None,
        };
        let result = enforce_with_limit(req.clone(), 128000);
        assert!(result.is_ok());
        let out = result.unwrap();
        assert_eq!(out.prompt, req.prompt);
    }
}
