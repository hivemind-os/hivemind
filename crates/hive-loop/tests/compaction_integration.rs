//! Integration tests for context compaction middleware.
//!
//! These tests use a custom `SummarizerProvider` that implements `ModelProvider`
//! to simulate the summarization LLM call without any real API calls.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwap;
use hive_contracts::{
    Capability, CompactionStrategy, ContextCompactionConfig, ProviderDescriptor, ProviderKind,
    SessionPermissions, ToolExecutionMode,
};
use hive_core::model_limits::ModelLimitsRegistry;
use hive_loop::compactor::ContextCompactorMiddleware;
use hive_loop::legacy::{
    AgentContext, ConversationContext, LoopContext, LoopMiddleware, RoutingConfig, SecurityContext,
    ToolsContext,
};
use hive_model::{
    CompletionMessage, CompletionRequest, CompletionResponse, ModelProvider, ModelRouter,
    ModelSelection, RoutingDecision,
};

// ── Mock Summarizer Provider ───────────────────────────────────────────

/// A mock LLM provider that records calls and returns configurable summaries.
#[derive(Debug)]
struct SummarizerProvider {
    descriptor: ProviderDescriptor,
    call_count: AtomicUsize,
    responses: Mutex<Vec<String>>,
    default_response: String,
    fail_on_call: Option<usize>,
}

impl SummarizerProvider {
    fn new(id: &str) -> Self {
        Self {
            descriptor: ProviderDescriptor {
                id: id.to_string(),
                name: Some("Test Summarizer".to_string()),
                kind: ProviderKind::Mock,
                models: vec!["test-model".to_string()],
                model_capabilities: BTreeMap::from([(
                    "test-model".to_string(),
                    [Capability::Chat].into_iter().collect(),
                )]),
                priority: 100,
                available: true,
            },
            call_count: AtomicUsize::new(0),
            responses: Mutex::new(Vec::new()),
            default_response: "Summary: key facts were discussed.".to_string(),
            fail_on_call: None,
        }
    }

    /// Queue a specific response for the Nth call.
    fn with_response(self, response: impl Into<String>) -> Self {
        self.responses.lock().unwrap().push(response.into());
        self
    }

    /// Fail on the Nth call (0-indexed).
    fn with_failure_on(self, n: usize) -> Self {
        Self { fail_on_call: Some(n), ..self }
    }

    fn with_default_response(self, response: impl Into<String>) -> Self {
        Self { default_response: response.into(), ..self }
    }

    fn calls(&self) -> usize {
        self.call_count.load(Ordering::Relaxed)
    }
}

impl ModelProvider for SummarizerProvider {
    fn descriptor(&self) -> &ProviderDescriptor {
        &self.descriptor
    }

    fn complete(
        &self,
        _request: &CompletionRequest,
        selection: &ModelSelection,
    ) -> anyhow::Result<CompletionResponse> {
        let idx = self.call_count.fetch_add(1, Ordering::Relaxed);

        if self.fail_on_call == Some(idx) {
            anyhow::bail!("simulated provider failure on call {idx}");
        }

        let content = {
            let mut resps = self.responses.lock().unwrap();
            if !resps.is_empty() {
                resps.remove(0)
            } else {
                self.default_response.clone()
            }
        };

        Ok(CompletionResponse {
            provider_id: selection.provider_id.clone(),
            model: selection.model.clone(),
            content,
            tool_calls: vec![],
        })
    }
}

// ── Test Helpers ───────────────────────────────────────────────────────

const TEST_PROVIDER: &str = "test-provider";
const TEST_MODEL: &str = "gpt-4o";

fn make_router(provider: SummarizerProvider) -> ModelRouter {
    let mut router = ModelRouter::new();
    router.register_provider(provider);
    router
}

fn make_router_arc(provider: SummarizerProvider) -> Arc<ArcSwap<ModelRouter>> {
    Arc::new(ArcSwap::from_pointee(make_router(provider)))
}

fn make_config(overrides: impl FnOnce(&mut ContextCompactionConfig)) -> ContextCompactionConfig {
    let mut config = ContextCompactionConfig::default();
    overrides(&mut config);
    config
}

fn make_config_swap(
    overrides: impl FnOnce(&mut ContextCompactionConfig),
) -> Arc<ArcSwap<ContextCompactionConfig>> {
    Arc::new(ArcSwap::from_pointee(make_config(overrides)))
}

fn make_middleware(
    config: Arc<ArcSwap<ContextCompactionConfig>>,
    router: Arc<ArcSwap<ModelRouter>>,
) -> ContextCompactorMiddleware {
    let limits = Arc::new(ModelLimitsRegistry::load());
    ContextCompactorMiddleware::new(limits, config, router)
}

fn make_context(model: &str) -> LoopContext {
    LoopContext {
        conversation: ConversationContext {
            session_id: "test-session".into(),
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
                selected: ModelSelection { provider_id: TEST_PROVIDER.into(), model: model.into() },
                fallback_chain: vec![],
                reason: "test".into(),
            }),
        },
        security: SecurityContext {
            data_class: hive_classification::DataClass::Public,
            permissions: Arc::new(parking_lot::Mutex::new(SessionPermissions::default())),
            workspace_classification: None,
            effective_data_class: Arc::new(AtomicU8::new(
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

fn make_request(prompt_chars: usize, messages: Vec<CompletionMessage>) -> CompletionRequest {
    CompletionRequest {
        prompt: "x".repeat(prompt_chars),
        prompt_content_parts: vec![],
        messages,
        required_capabilities: BTreeSet::new(),
        preferred_models: None,
        tools: vec![],
    }
}

fn user_msg(content: &str) -> CompletionMessage {
    CompletionMessage { role: "user".into(), content: content.into(), content_parts: vec![] }
}

fn assistant_msg(content: &str) -> CompletionMessage {
    CompletionMessage { role: "assistant".into(), content: content.into(), content_parts: vec![] }
}

fn system_msg(content: &str) -> CompletionMessage {
    CompletionMessage { role: "system".into(), content: content.into(), content_parts: vec![] }
}

/// Build N user/assistant turn pairs, each with `chars_per_msg` characters.
fn build_conversation(turns: usize, chars_per_msg: usize) -> Vec<CompletionMessage> {
    let mut msgs = vec![system_msg("You are a helpful assistant.")];
    for i in 0..turns {
        msgs.push(user_msg(&format!("Q{i}: {}", "q".repeat(chars_per_msg))));
        msgs.push(assistant_msg(&format!("A{i}: {}", "a".repeat(chars_per_msg))));
    }
    msgs
}

// ── Tests ──────────────────────────────────────────────────────────────

// 1. Below threshold — no compaction occurs
#[test]
fn below_threshold_no_compaction() {
    let provider = SummarizerProvider::new(TEST_PROVIDER);
    let router = make_router_arc(provider);
    let config = make_config_swap(|c| c.trigger_threshold = 0.75);
    let mw = make_middleware(config, router);
    let ctx = make_context(TEST_MODEL);

    // Small request well below gpt-4o's 128K window × 0.75 = 96K tokens
    let msgs = build_conversation(5, 100);
    let req = make_request(100, msgs.clone());
    let result = mw.before_model_call(&ctx, req).unwrap();

    assert_eq!(result.messages.len(), msgs.len(), "messages should be untouched");
}

// 2. Exactly at threshold boundary — no compaction
#[test]
fn at_threshold_boundary_no_compaction() {
    let provider = SummarizerProvider::new(TEST_PROVIDER);
    let router = make_router_arc(provider);
    // Use a very low threshold but small enough data
    let config = make_config_swap(|c| c.trigger_threshold = 0.99);
    let mw = make_middleware(config, router);
    let ctx = make_context(TEST_MODEL);

    let msgs = build_conversation(3, 50);
    let req = make_request(50, msgs.clone());
    let result = mw.before_model_call(&ctx, req).unwrap();
    assert_eq!(result.messages.len(), msgs.len());
}

// 3. Above threshold — compaction triggers and reduces message count
#[test]
fn above_threshold_triggers_compaction() {
    let provider = SummarizerProvider::new(TEST_PROVIDER);
    let router = make_router_arc(provider);
    // gpt-4o = 128K window. threshold = 0.01 → triggers at ~1280 tokens ≈ 5120 chars
    let config = make_config_swap(|c| {
        c.trigger_threshold = 0.01;
        c.keep_recent_turns = 2;
    });
    let mw = make_middleware(config, router);
    let ctx = make_context(TEST_MODEL);

    let msgs = build_conversation(10, 500); // 1 system + 20 messages
    let req = make_request(100, msgs);
    let result = mw.before_model_call(&ctx, req).unwrap();

    // Should have fewer messages: system + 1 summary + 2 recent turns (4 msgs)
    assert!(result.messages.len() < 21, "should have compacted from 21 messages");
    assert!(result.messages.iter().any(|m| m.content.contains("[Compaction Summary")));
}

// 4. System messages are preserved
#[test]
fn system_messages_preserved() {
    let provider = SummarizerProvider::new(TEST_PROVIDER);
    let router = make_router_arc(provider);
    let config = make_config_swap(|c| {
        c.trigger_threshold = 0.01;
        c.keep_recent_turns = 2;
    });
    let mw = make_middleware(config, router);
    let ctx = make_context(TEST_MODEL);

    let mut msgs =
        vec![system_msg("You are a helpful assistant."), system_msg("Additional system context.")];
    for i in 0..15 {
        msgs.push(user_msg(&format!("Q{i}: {}", "q".repeat(500))));
        msgs.push(assistant_msg(&format!("A{i}: {}", "a".repeat(500))));
    }
    let req = make_request(100, msgs);
    let result = mw.before_model_call(&ctx, req).unwrap();

    // Both system messages should still be there
    assert_eq!(result.messages[0].role, "system");
    assert_eq!(result.messages[0].content, "You are a helpful assistant.");
    assert_eq!(result.messages[1].role, "system");
    assert_eq!(result.messages[1].content, "Additional system context.");
}

// 5. Keep-recent-turns are preserved unmodified
#[test]
fn recent_turns_preserved() {
    let provider = SummarizerProvider::new(TEST_PROVIDER);
    let router = make_router_arc(provider);
    let config = make_config_swap(|c| {
        c.trigger_threshold = 0.01;
        c.keep_recent_turns = 4;
    });
    let mw = make_middleware(config, router);
    let ctx = make_context(TEST_MODEL);

    let msgs = build_conversation(10, 500); // 1 system + 20 user/assistant
    let last_four: Vec<_> = msgs[msgs.len() - 4..].to_vec();
    let req = make_request(100, msgs);
    let result = mw.before_model_call(&ctx, req).unwrap();

    // The last 4 non-system messages should be exactly preserved
    let result_tail: Vec<_> = result.messages[result.messages.len() - 4..].to_vec();
    assert_eq!(result_tail, last_four);
}

// 6. Summary message has correct prefix format
#[test]
fn summary_has_correct_prefix() {
    let provider =
        SummarizerProvider::new(TEST_PROVIDER).with_default_response("The user discussed X.");
    let router = make_router_arc(provider);
    let config = make_config_swap(|c| {
        c.trigger_threshold = 0.01;
        c.keep_recent_turns = 2;
    });
    let mw = make_middleware(config, router);
    let ctx = make_context(TEST_MODEL);

    let msgs = build_conversation(10, 500);
    let req = make_request(100, msgs);
    let result = mw.before_model_call(&ctx, req).unwrap();

    let summary = result
        .messages
        .iter()
        .find(|m| m.content.contains("[Compaction Summary"))
        .expect("should have a summary message");
    assert!(summary.content.starts_with("[Compaction Summary #1"));
    assert!(summary.content.contains("messages compacted]"));
    assert!(summary.content.contains("The user discussed X."));
    assert_eq!(summary.role, "system");
}

// 7. Manual strategy skips compaction even when over threshold
#[test]
fn manual_strategy_skips_compaction() {
    let provider = SummarizerProvider::new(TEST_PROVIDER);
    let router = make_router_arc(provider);
    let config = make_config_swap(|c| {
        c.strategy = CompactionStrategy::Manual;
        c.trigger_threshold = 0.01;
    });
    let mw = make_middleware(config, router);
    let ctx = make_context(TEST_MODEL);

    let msgs = build_conversation(10, 500);
    let original_len = msgs.len();
    let req = make_request(100, msgs);
    let result = mw.before_model_call(&ctx, req).unwrap();

    assert_eq!(result.messages.len(), original_len);
}

// 8. Summarizer is actually called with the right messages
#[test]
fn summarizer_receives_correct_messages() {
    let provider = Arc::new(SummarizerProvider::new(TEST_PROVIDER));
    let router = {
        let mut r = ModelRouter::new();
        r.register_provider(ProviderWrapper(Arc::clone(&provider)));
        r
    };
    let router = Arc::new(ArcSwap::from_pointee(router));
    let config = make_config_swap(|c| {
        c.trigger_threshold = 0.01;
        c.keep_recent_turns = 2;
    });
    let mw = make_middleware(config, router);
    let ctx = make_context(TEST_MODEL);

    let msgs = build_conversation(10, 500);
    let req = make_request(100, msgs);
    let _ = mw.before_model_call(&ctx, req).unwrap();

    assert_eq!(provider.calls(), 1, "summarizer should be called exactly once");
}

/// Wrapper to allow using Arc<SummarizerProvider> as ModelProvider
#[derive(Debug)]
struct ProviderWrapper(Arc<SummarizerProvider>);

impl ModelProvider for ProviderWrapper {
    fn descriptor(&self) -> &ProviderDescriptor {
        self.0.descriptor()
    }
    fn complete(
        &self,
        request: &CompletionRequest,
        selection: &ModelSelection,
    ) -> anyhow::Result<CompletionResponse> {
        self.0.complete(request, selection)
    }
}

// 9. Provider failure doesn't break the agent — original request returned
#[test]
fn provider_failure_returns_original() {
    let provider = SummarizerProvider::new(TEST_PROVIDER).with_failure_on(0);
    let router = make_router_arc(provider);
    let config = make_config_swap(|c| {
        c.trigger_threshold = 0.01;
        c.keep_recent_turns = 2;
    });
    let mw = make_middleware(config, router);
    let ctx = make_context(TEST_MODEL);

    let msgs = build_conversation(10, 500);
    let original_len = msgs.len();
    let req = make_request(100, msgs);
    let result = mw.before_model_call(&ctx, req).unwrap();

    // Should return original unmodified on provider failure
    assert_eq!(result.messages.len(), original_len);
    assert!(!result.messages.iter().any(|m| m.content.contains("[Compaction Summary")));
}

// 10. Missing routing decision doesn't crash
#[test]
fn missing_routing_decision_returns_original() {
    let provider = SummarizerProvider::new(TEST_PROVIDER);
    let router = make_router_arc(provider);
    let config = make_config_swap(|c| {
        c.trigger_threshold = 0.01;
        c.keep_recent_turns = 2;
    });
    let mw = make_middleware(config, router);
    let mut ctx = make_context(TEST_MODEL);
    ctx.routing.routing_decision = None; // No routing decision

    let msgs = build_conversation(10, 500);
    let original_len = msgs.len();
    let req = make_request(100, msgs);
    let result = mw.before_model_call(&ctx, req).unwrap();

    assert_eq!(result.messages.len(), original_len, "should pass through unmodified");
}

// 11. Not enough messages to compact (all within keep window)
#[test]
fn too_few_messages_no_compaction() {
    let provider = SummarizerProvider::new(TEST_PROVIDER);
    let router = make_router_arc(provider);
    let config = make_config_swap(|c| {
        c.trigger_threshold = 0.01;
        c.keep_recent_turns = 100; // Keep more than we have
    });
    let mw = make_middleware(config, router);
    let ctx = make_context(TEST_MODEL);

    let msgs = build_conversation(5, 500); // 11 messages total
    let original_len = msgs.len();
    let req = make_request(100, msgs);
    let result = mw.before_model_call(&ctx, req).unwrap();

    assert_eq!(result.messages.len(), original_len);
}

// 12. Token count actually decreases after compaction
#[test]
fn token_count_decreases_after_compaction() {
    let provider = SummarizerProvider::new(TEST_PROVIDER).with_default_response("Short summary.");
    let router = make_router_arc(provider);
    let config = make_config_swap(|c| {
        c.trigger_threshold = 0.01;
        c.keep_recent_turns = 2;
    });
    let mw = make_middleware(config, router);
    let ctx = make_context(TEST_MODEL);

    let msgs = build_conversation(20, 1000);
    let req_before = make_request(100, msgs);
    let chars_before: usize = req_before.prompt.len()
        + req_before.messages.iter().map(|m| m.content.len()).sum::<usize>();
    let result = mw.before_model_call(&ctx, req_before).unwrap();
    let chars_after: usize =
        result.prompt.len() + result.messages.iter().map(|m| m.content.len()).sum::<usize>();

    assert!(
        chars_after < chars_before,
        "chars should decrease: before={chars_before}, after={chars_after}"
    );
}

// 13. Recursive compaction merges oldest summaries when exceeding max_summaries
#[test]
fn recursive_compaction_merges_summaries() {
    let provider = SummarizerProvider::new(TEST_PROVIDER);
    let router = make_router_arc(provider);
    let config = make_config_swap(|c| {
        c.trigger_threshold = 0.01;
        c.keep_recent_turns = 2;
        c.max_summaries_in_context = 1; // Only allow 1 summary
    });
    let mw = make_middleware(config, router);
    let ctx = make_context(TEST_MODEL);

    // Pre-populate with an existing summary + lots of new messages
    let mut msgs = vec![
        system_msg("System prompt"),
        CompletionMessage {
            role: "system".into(),
            content: "[Compaction Summary #1 — 5 messages compacted]\n\nOld summary.".into(),
            content_parts: vec![],
        },
    ];
    for i in 0..15 {
        msgs.push(user_msg(&format!("Q{i}: {}", "q".repeat(500))));
        msgs.push(assistant_msg(&format!("A{i}: {}", "a".repeat(500))));
    }

    let req = make_request(100, msgs);
    let result = mw.before_model_call(&ctx, req).unwrap();

    // Should have merged old summary + new summary into an epoch summary
    let summaries: Vec<_> =
        result.messages.iter().filter(|m| m.content.contains("[Compaction Summary")).collect();
    // With max_summaries_in_context = 1, any extra should be merged
    assert!(summaries.len() <= 2, "summaries should have been merged");
    assert!(
        result.messages.iter().any(|m| m.content.contains("epoch summary")),
        "should have an epoch summary"
    );
}

// 14. Custom extraction model is used when configured
#[test]
fn extraction_model_override() {
    let main_provider = SummarizerProvider::new(TEST_PROVIDER);
    let extraction_provider =
        SummarizerProvider::new("extraction-provider").with_default_response("Extracted summary.");
    let mut router = ModelRouter::new();
    router.register_provider(main_provider);
    router.register_provider(extraction_provider);
    let router = Arc::new(ArcSwap::from_pointee(router));

    let config = make_config_swap(|c| {
        c.trigger_threshold = 0.01;
        c.keep_recent_turns = 2;
        c.extraction_model = Some("extraction-provider:test-model".to_string());
    });
    let mw = make_middleware(config, router);
    let ctx = make_context(TEST_MODEL);

    let msgs = build_conversation(10, 500);
    let req = make_request(100, msgs);
    let result = mw.before_model_call(&ctx, req).unwrap();

    // Should have used the extraction provider's response
    let summary = result
        .messages
        .iter()
        .find(|m| m.content.contains("[Compaction Summary"))
        .expect("should have a summary");
    assert!(summary.content.contains("Extracted summary."));
}

// 15. Invalid extraction model format falls back gracefully
#[test]
fn invalid_extraction_model_falls_back() {
    let provider =
        SummarizerProvider::new(TEST_PROVIDER).with_default_response("Fallback summary from main.");
    let router = make_router_arc(provider);

    let config = make_config_swap(|c| {
        c.trigger_threshold = 0.01;
        c.keep_recent_turns = 2;
        // Invalid format: missing colon separator
        c.extraction_model = Some("no-colon-here".to_string());
    });
    let mw = make_middleware(config, router);
    let ctx = make_context(TEST_MODEL);

    let msgs = build_conversation(10, 500);
    let req = make_request(100, msgs);
    let result = mw.before_model_call(&ctx, req).unwrap();

    // Should still produce a summary using the main provider
    assert!(result.messages.iter().any(|m| m.content.contains("Fallback summary from main.")));
}

// 16. Prompt content is preserved (compaction only modifies messages)
#[test]
fn prompt_preserved_during_compaction() {
    let provider = SummarizerProvider::new(TEST_PROVIDER);
    let router = make_router_arc(provider);
    let config = make_config_swap(|c| {
        c.trigger_threshold = 0.01;
        c.keep_recent_turns = 2;
    });
    let mw = make_middleware(config, router);
    let ctx = make_context(TEST_MODEL);

    let msgs = build_conversation(10, 500);
    let prompt = "This is my important system prompt that must survive.".to_string();
    let req = make_request(0, msgs);
    let req = CompletionRequest { prompt: prompt.clone(), prompt_content_parts: vec![], ..req };
    let result = mw.before_model_call(&ctx, req).unwrap();

    assert_eq!(result.prompt, prompt);
}

// 17. Multiple rounds of compaction produce numbered summaries
#[test]
fn multiple_compaction_rounds_increment_summary_number() {
    let provider = SummarizerProvider::new(TEST_PROVIDER)
        .with_response("First round summary.")
        .with_response("Second round summary.");
    let router = make_router_arc(provider);
    let config = make_config_swap(|c| {
        c.trigger_threshold = 0.01;
        c.keep_recent_turns = 2;
        c.max_summaries_in_context = 10;
    });
    let mw = make_middleware(config, router);
    let ctx = make_context(TEST_MODEL);

    // Round 1
    let msgs = build_conversation(10, 500);
    let req = make_request(100, msgs);
    let result1 = mw.before_model_call(&ctx, req).unwrap();
    assert!(result1.messages.iter().any(|m| m.content.contains("#1")));

    // Round 2: add more messages on top of the compacted result
    let mut msgs2 = result1.messages;
    for i in 0..15 {
        msgs2.push(user_msg(&format!("NewQ{i}: {}", "q".repeat(500))));
        msgs2.push(assistant_msg(&format!("NewA{i}: {}", "a".repeat(500))));
    }
    let req2 = make_request(100, msgs2);
    let result2 = mw.before_model_call(&ctx, req2).unwrap();

    assert!(result2.messages.iter().any(|m| m.content.contains("#2")));
}

// 18. Config hot-reload: changing threshold at runtime takes effect
#[test]
fn config_hot_reload_threshold() {
    let provider = SummarizerProvider::new(TEST_PROVIDER);
    let router = make_router_arc(provider);
    // Start with very high threshold (no compaction)
    let config = make_config_swap(|c| {
        c.trigger_threshold = 0.99;
        c.keep_recent_turns = 2;
    });
    let mw = make_middleware(Arc::clone(&config), router);
    let ctx = make_context(TEST_MODEL);

    let msgs = build_conversation(10, 500);
    let req = make_request(100, msgs.clone());
    let result1 = mw.before_model_call(&ctx, req).unwrap();
    assert_eq!(result1.messages.len(), msgs.len(), "should NOT compact at 0.99 threshold");

    // Hot-reload: lower threshold so compaction triggers
    config.store(Arc::new(ContextCompactionConfig {
        trigger_threshold: 0.01,
        keep_recent_turns: 2,
        ..Default::default()
    }));

    let req2 = make_request(100, msgs);
    let result2 = mw.before_model_call(&ctx, req2).unwrap();
    assert!(
        result2.messages.iter().any(|m| m.content.contains("[Compaction Summary")),
        "should compact after threshold lowered"
    );
}

// 19. Config hot-reload: switching to Manual disables compaction
#[test]
fn config_hot_reload_manual_disables() {
    let provider = SummarizerProvider::new(TEST_PROVIDER);
    let router = make_router_arc(provider);
    let config = make_config_swap(|c| {
        c.trigger_threshold = 0.01;
        c.keep_recent_turns = 2;
    });
    let mw = make_middleware(Arc::clone(&config), router);
    let ctx = make_context(TEST_MODEL);

    // First call should compact
    let msgs = build_conversation(10, 500);
    let req = make_request(100, msgs.clone());
    let result1 = mw.before_model_call(&ctx, req).unwrap();
    assert!(result1.messages.iter().any(|m| m.content.contains("[Compaction Summary")));

    // Switch to manual
    config.store(Arc::new(ContextCompactionConfig {
        strategy: CompactionStrategy::Manual,
        trigger_threshold: 0.01,
        keep_recent_turns: 2,
        ..Default::default()
    }));

    let req2 = make_request(100, msgs.clone());
    let result2 = mw.before_model_call(&ctx, req2).unwrap();
    assert_eq!(result2.messages.len(), msgs.len(), "manual should skip compaction");
}

// 20. Large conversation with mixed roles and tool-like content
#[test]
fn complex_mixed_conversation() {
    let provider = SummarizerProvider::new(TEST_PROVIDER).with_default_response(
        "Complex summary: user requested file edits, tool ran grep, found 3 matches.",
    );
    let router = make_router_arc(provider);
    let config = make_config_swap(|c| {
        c.trigger_threshold = 0.01;
        c.keep_recent_turns = 4;
        c.max_summaries_in_context = 3;
    });
    let mw = make_middleware(config, router);
    let ctx = make_context(TEST_MODEL);

    let mut msgs = vec![
        system_msg("You are a coding assistant."),
        system_msg("Project context: Rust web server"),
    ];
    // Simulate a realistic agentic conversation
    for i in 0..20 {
        msgs.push(user_msg(&format!("Please edit src/main.rs line {i}: {}", "c".repeat(300))));
        msgs.push(assistant_msg(&format!(
            "I'll run the tool.\n<tool_call>grep -n 'pattern{i}' src/main.rs</tool_call>\nResult: {} lines matched. {}",
            i % 5,
            "x".repeat(400)
        )));
    }
    let original_count = msgs.len();
    let req = make_request(200, msgs);
    let result = mw.before_model_call(&ctx, req).unwrap();

    // Should have significantly fewer messages
    assert!(result.messages.len() < original_count);
    // System prompts preserved
    assert_eq!(result.messages[0].content, "You are a coding assistant.");
    assert_eq!(result.messages[1].content, "Project context: Rust web server");
    // Last 4 non-system messages preserved
    let non_system_tail: Vec<_> = result
        .messages
        .iter()
        .filter(|m| !m.content.starts_with("[Compaction Summary") && m.role != "system")
        .collect();
    assert!(non_system_tail.len() >= 4, "should keep at least 4 recent messages");
    // Summary present
    assert!(result.messages.iter().any(|m| m.content.contains("Complex summary")));
}
