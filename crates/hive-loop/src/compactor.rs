//! Context compaction middleware for the agentic loop.
//!
//! [`ContextCompactorMiddleware`] implements [`LoopMiddleware`] and runs
//! **before** each model call.  When the estimated token usage exceeds
//! `trigger_threshold × context_window`, it:
//!
//! 1. Identifies the oldest non-system messages (outside the keep-window).
//! 2. Builds a combined summary of those messages using the model router.
//! 3. Replaces the dropped messages with a compact summary message.
//!
//! This is the "summarize-only" path (SPEC.md §9.12).  KG extraction
//! (`extract-and-summarize`) is a future extension.

use std::sync::Arc;

use hive_contracts::{CompactionStrategy, ContextCompactionConfig};
use hive_core::model_limits::ModelLimitsRegistry;
use hive_model::{
    CompletionMessage, CompletionRequest, ModelRouter, ModelSelection, RoutingDecision,
};
use tracing::{debug, info, warn};

use crate::legacy::{
    simple_model_error, AgentContext, ConversationContext, LoopContext, LoopError, LoopMiddleware,
    RoutingConfig, SecurityContext, ToolsContext,
};
use crate::token_budget::estimate_request_tokens;

/// Marker prefix used for compaction summary messages so they can be
/// identified (and themselves compacted) in future rounds.
const COMPACTION_SUMMARY_PREFIX: &str = "[Compaction Summary";

pub struct ContextCompactorMiddleware {
    limits: Arc<ModelLimitsRegistry>,
    config: Arc<arc_swap::ArcSwap<ContextCompactionConfig>>,
    model_router: Arc<arc_swap::ArcSwap<ModelRouter>>,
}

impl ContextCompactorMiddleware {
    pub fn new(
        limits: Arc<ModelLimitsRegistry>,
        config: Arc<arc_swap::ArcSwap<ContextCompactionConfig>>,
        model_router: Arc<arc_swap::ArcSwap<ModelRouter>>,
    ) -> Self {
        Self { limits, config, model_router }
    }

    /// Build a summarization prompt from the messages being compacted.
    fn build_summary_prompt(messages: &[CompletionMessage]) -> String {
        let mut prompt = String::from(
            "You are a precise summarizer. Below is a section of conversation history that needs \
             to be compacted into a concise summary. Preserve all important facts, decisions, \
             tool results, and context. Be concise but thorough.\n\n\
             --- CONVERSATION TO SUMMARIZE ---\n",
        );
        for msg in messages {
            prompt.push_str(&format!("\n[{}]: {}\n", msg.role, msg.content));
        }
        prompt.push_str(
            "\n--- END ---\n\n\
             Write a concise summary (target: 500-800 tokens) that captures:\n\
             - Key decisions made\n\
             - Important facts and context\n\
             - Tool results that are still relevant\n\
             - Any ongoing tasks or goals\n\n\
             Summary:",
        );
        prompt
    }

    /// Attempt to generate a summary via the model router.
    fn generate_summary(
        &self,
        messages: &[CompletionMessage],
        decision: &RoutingDecision,
        config: &ContextCompactionConfig,
    ) -> Result<String, LoopError> {
        let summary_prompt = Self::build_summary_prompt(messages);
        let request = CompletionRequest {
            prompt: summary_prompt,
            prompt_content_parts: vec![],
            messages: vec![],
            required_capabilities: std::collections::BTreeSet::new(),
            preferred_models: None,
            tools: vec![],
        };

        // If an extraction model is configured (format: "provider_id:model_name"),
        // build a dedicated routing decision for it; otherwise reuse the
        // conversation's existing decision.
        let effective_decision = if let Some(ref spec) = config.extraction_model {
            if let Some((pid, model)) = spec.split_once(':') {
                RoutingDecision {
                    selected: ModelSelection {
                        provider_id: pid.to_string(),
                        model: model.to_string(),
                    },
                    fallback_chain: vec![],
                    reason: "compaction extraction model override".into(),
                }
            } else {
                warn!(
                    extraction_model = %spec,
                    "extraction_model should be 'provider_id:model_name', falling back to conversation model"
                );
                decision.clone()
            }
        } else {
            decision.clone()
        };

        let router = self.model_router.load();
        let response = router
            .complete_with_decision(&request, &effective_decision)
            .map_err(|e| simple_model_error(format!("compaction summary failed: {e}")))?;

        Ok(response.content)
    }

    /// Count existing compaction summaries in the message history.
    fn count_summaries(messages: &[CompletionMessage]) -> usize {
        messages.iter().filter(|m| m.content.starts_with(COMPACTION_SUMMARY_PREFIX)).count()
    }
}

impl LoopMiddleware for ContextCompactorMiddleware {
    fn before_model_call(
        &self,
        context: &LoopContext,
        request: CompletionRequest,
    ) -> Result<CompletionRequest, LoopError> {
        match self.try_compact(context, request.clone()) {
            Ok(compacted) => Ok(compacted),
            Err(e) => {
                warn!("context compaction failed, continuing with uncompacted history: {e}");
                Ok(request)
            }
        }
    }
}

impl ContextCompactorMiddleware {
    fn try_compact(
        &self,
        context: &LoopContext,
        mut request: CompletionRequest,
    ) -> Result<CompletionRequest, LoopError> {
        let config = self.config.load();

        // Skip if strategy is manual.
        if config.strategy == CompactionStrategy::Manual {
            return Ok(request);
        }

        // Get model limits.
        let model_name =
            context.routing_decision().map(|d| d.selected.model.as_str()).unwrap_or("");
        let model_limits = self.limits.lookup(model_name);
        let context_window = model_limits.context_window as usize;

        // Check if we've crossed the threshold.
        let estimated = estimate_request_tokens(&request);
        let threshold = (context_window as f64 * config.trigger_threshold as f64) as usize;

        debug!(
            model = model_name,
            context_window,
            max_output_tokens = model_limits.max_output_tokens,
            estimated,
            threshold,
            trigger_threshold = config.trigger_threshold,
            "compactor evaluating token usage"
        );

        if estimated <= threshold {
            return Ok(request);
        }

        info!(
            estimated,
            threshold,
            context_window,
            model = model_name,
            "context compaction triggered"
        );

        // Find compactable messages: non-system messages outside the keep window.
        let system_count = request.messages.iter().take_while(|m| m.role == "system").count();
        let non_system = request.messages.len().saturating_sub(system_count);

        if non_system <= config.keep_recent_turns {
            // Not enough messages to compact — the budget middleware will handle truncation.
            return Ok(request);
        }

        let compact_end = system_count + non_system - config.keep_recent_turns;
        if compact_end > request.messages.len() {
            warn!("compaction bounds exceeded message length, skipping");
            return Ok(request);
        }
        let to_compact: Vec<CompletionMessage> =
            request.messages[system_count..compact_end].to_vec();

        if to_compact.is_empty() {
            return Ok(request);
        }

        let compact_count = to_compact.len();

        // Generate summary.
        let decision = context.routing_decision().ok_or_else(|| {
            simple_model_error("no routing decision available for compaction".into())
        })?;

        let summary = match self.generate_summary(&to_compact, decision, &config) {
            Ok(summary) => summary,
            Err(e) => {
                warn!("compaction summary generation failed, skipping: {e}");
                // Fall through — the token budget middleware will handle truncation.
                return Ok(request);
            }
        };

        // Replace compacted messages with the summary.
        let summary_count = Self::count_summaries(&request.messages) + 1;
        let summary_message = CompletionMessage {
            role: "system".to_string(),
            content: format!(
                "{COMPACTION_SUMMARY_PREFIX} #{summary_count} — {compact_count} messages compacted]\n\n{summary}"
            ),
            content_parts: vec![],
        };

        // Remove the compacted messages and insert the summary.
        request.messages.drain(system_count..compact_end);
        request.messages.insert(system_count, summary_message);

        let new_estimated = estimate_request_tokens(&request);
        info!(
            before = estimated,
            after = new_estimated,
            compacted = compact_count,
            "context compaction complete"
        );

        // Recursive compaction: if we have too many summaries, merge the oldest.
        if Self::count_summaries(&request.messages) > config.max_summaries_in_context {
            let summary_indices: Vec<usize> = request
                .messages
                .iter()
                .enumerate()
                .filter(|(_, m)| m.content.starts_with(COMPACTION_SUMMARY_PREFIX))
                .map(|(i, _)| i)
                .collect();

            if summary_indices.len() > 1 {
                // Merge the two oldest summaries.
                let oldest_summaries: Vec<CompletionMessage> =
                    summary_indices.iter().take(2).map(|&i| request.messages[i].clone()).collect();

                let epoch_summary = format!(
                    "{COMPACTION_SUMMARY_PREFIX} — epoch summary]\n\n{}\n\n{}",
                    oldest_summaries[0].content, oldest_summaries[1].content
                );

                // Remove the two oldest summaries (remove in reverse order to keep indices valid).
                request.messages.remove(summary_indices[1]);
                request.messages.remove(summary_indices[0]);

                request.messages.insert(
                    summary_indices[0],
                    CompletionMessage {
                        role: "system".to_string(),
                        content: epoch_summary,
                        content_parts: vec![],
                    },
                );
            }
        }

        Ok(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hive_contracts::ToolExecutionMode;
    use hive_model::{ModelSelection, RoutingDecision};

    #[test]
    fn default_config_values() {
        let config = ContextCompactionConfig::default();
        assert_eq!(config.trigger_threshold, 0.75);
        assert_eq!(config.keep_recent_turns, 10);
        assert_eq!(config.summary_max_tokens, 800);
        assert_eq!(config.max_summaries_in_context, 5);
        assert_eq!(config.strategy, CompactionStrategy::SummarizeOnly);
    }

    #[test]
    fn count_summaries_works() {
        let messages = vec![
            CompletionMessage {
                role: "system".into(),
                content: "system prompt".into(),
                content_parts: vec![],
            },
            CompletionMessage {
                role: "system".into(),
                content: "[Compaction Summary #1 — 5 messages]\nstuff".into(),
                content_parts: vec![],
            },
            CompletionMessage {
                role: "user".into(),
                content: "hello".into(),
                content_parts: vec![],
            },
            CompletionMessage {
                role: "system".into(),
                content: "[Compaction Summary #2 — 3 messages]\nmore stuff".into(),
                content_parts: vec![],
            },
        ];
        assert_eq!(ContextCompactorMiddleware::count_summaries(&messages), 2);
    }

    #[test]
    fn build_summary_prompt_includes_all_messages() {
        let messages = vec![
            CompletionMessage {
                role: "user".into(),
                content: "What is X?".into(),
                content_parts: vec![],
            },
            CompletionMessage {
                role: "assistant".into(),
                content: "X is Y.".into(),
                content_parts: vec![],
            },
        ];
        let prompt = ContextCompactorMiddleware::build_summary_prompt(&messages);
        assert!(prompt.contains("[user]: What is X?"));
        assert!(prompt.contains("[assistant]: X is Y."));
        assert!(prompt.contains("concise summary"));
    }

    #[test]
    fn manual_strategy_skips_compaction() {
        let config =
            ContextCompactionConfig { strategy: CompactionStrategy::Manual, ..Default::default() };
        let limits = Arc::new(ModelLimitsRegistry::load());
        let router = Arc::new(arc_swap::ArcSwap::from_pointee(ModelRouter::new()));
        let mw = ContextCompactorMiddleware::new(
            limits,
            Arc::new(arc_swap::ArcSwap::from_pointee(config)),
            router,
        );

        let ctx = make_test_context("gpt-4o");
        let req = CompletionRequest {
            prompt: "x".repeat(200000), // Way over budget
            prompt_content_parts: vec![],
            messages: vec![],
            required_capabilities: std::collections::BTreeSet::new(),
            preferred_models: None,
            tools: vec![],
        };
        let result = mw.before_model_call(&ctx, req.clone());
        assert!(result.is_ok());
        // Should pass through unchanged.
        assert_eq!(result.unwrap().prompt.len(), req.prompt.len());
    }

    fn make_test_context(model: &str) -> LoopContext {
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
                required_capabilities: std::collections::BTreeSet::new(),
                preferred_models: None,
                loop_strategy: None,
                routing_decision: Some(RoutingDecision {
                    selected: ModelSelection {
                        provider_id: "test-provider".into(),
                        model: model.into(),
                    },
                    fallback_chain: vec![],
                    reason: "test".into(),
                }),
            },
            security: SecurityContext {
                data_class: hive_classification::DataClass::Public,
                permissions: Arc::new(parking_lot::Mutex::new(
                    hive_contracts::SessionPermissions::default(),
                )),
                workspace_classification: None,
                effective_data_class: Arc::new(std::sync::atomic::AtomicU8::new(
                    hive_classification::DataClass::Public.to_i64() as u8,
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
            preempt_signal: None,
            cancellation_token: None,
        }
    }
}
