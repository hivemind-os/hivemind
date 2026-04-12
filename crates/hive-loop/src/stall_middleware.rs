use std::collections::VecDeque;

use parking_lot::Mutex;

use crate::legacy::{LoopContext, LoopError, LoopMiddleware, ToolCall};
use hive_model::CompletionRequest;
use hive_tools::ToolResult;

/// Internal state for the stall detection middleware.
struct StallState {
    /// Sliding window of `(tool_name, canonical_args)` for successful calls only.
    window: VecDeque<(String, String)>,
    window_size: usize,
    threshold: usize,
    /// The `(tool_name, canonical_args)` pair that was warned about.
    /// If the next proposed call matches, it's a hard stall.
    warned_entry: Option<(String, String)>,
    /// Set when a hard stall is confirmed; checked by `before_model_call`.
    hard_stall: bool,
    /// Info about the stall for the error message.
    hard_stall_info: Option<(String, usize)>,
}

impl StallState {
    fn new(window_size: usize, threshold: usize) -> Self {
        Self {
            window: VecDeque::with_capacity(window_size),
            window_size: window_size.max(1),
            threshold: threshold.max(2),
            warned_entry: None,
            hard_stall: false,
            hard_stall_info: None,
        }
    }

    /// Count consecutive identical entries from the tail of the window.
    fn consecutive_streak(&self, entry: &(String, String)) -> usize {
        self.window.iter().rev().take_while(|e| *e == entry).count()
    }

    /// Record a successful tool call in the sliding window.
    fn record(&mut self, tool_name: &str, args: &serde_json::Value) {
        let canonical_args = serde_json::to_string(args).unwrap_or_default();
        if self.window.len() >= self.window_size {
            self.window.pop_front();
        }
        self.window.push_back((tool_name.to_string(), canonical_args));
    }
}

/// Middleware that detects when an agent is stuck calling the same tool with
/// identical arguments. Uses **consecutive** counting (not total-in-window)
/// and a **warning-before-stop** flow:
///
/// 1. After `threshold` consecutive identical *successful* calls, the next
///    proposed identical call triggers a **warning** — the call is blocked
///    and the agent sees an error message.
/// 2. If the agent proposes the same call *again*, a **hard stall** flag is
///    set and `before_model_call` terminates the loop.
/// 3. If the agent proposes a *different* call, the warning is cleared.
pub struct StallDetectionMiddleware {
    state: Mutex<StallState>,
}

impl StallDetectionMiddleware {
    pub fn new(config: &hive_contracts::ToolLimitsConfig) -> Self {
        Self { state: Mutex::new(StallState::new(config.stall_window, config.stall_threshold)) }
    }

    #[cfg(test)]
    fn with_params(window_size: usize, threshold: usize) -> Self {
        Self { state: Mutex::new(StallState::new(window_size, threshold)) }
    }
}

impl LoopMiddleware for StallDetectionMiddleware {
    fn before_model_call(
        &self,
        _context: &LoopContext,
        request: CompletionRequest,
    ) -> Result<CompletionRequest, LoopError> {
        let state = self.state.lock();
        if state.hard_stall {
            let (tool_name, count) =
                state.hard_stall_info.clone().unwrap_or_else(|| ("unknown".into(), 0));
            return Err(LoopError::StallDetected { tool_name, count });
        }
        Ok(request)
    }

    fn before_tool_call(
        &self,
        _context: &LoopContext,
        call: ToolCall,
    ) -> Result<ToolCall, LoopError> {
        let mut state = self.state.lock();
        let canonical_args = serde_json::to_string(&call.input).unwrap_or_default();
        let entry = (call.tool_id.clone(), canonical_args);
        let streak = state.consecutive_streak(&entry);

        if streak >= state.threshold {
            if state.warned_entry.as_ref() == Some(&entry) {
                // Already warned for this exact call — hard stall.
                state.hard_stall = true;
                state.hard_stall_info = Some((call.tool_id.clone(), streak));
                return Err(LoopError::ToolExecutionFailed {
                    tool_id: call.tool_id,
                    detail: format!(
                        "Stall detected — this tool was called {} times consecutively \
                         with identical arguments and you were already warned. \
                         The agent turn will now be terminated.",
                        streak
                    ),
                });
            }
            // First offense — issue a warning.
            state.warned_entry = Some(entry);
            return Err(LoopError::ToolExecutionFailed {
                tool_id: call.tool_id,
                detail: format!(
                    "Stall warning — you have called this tool {} times consecutively \
                     with identical arguments. Try a different approach or different arguments. \
                     If you repeat the same call, the agent turn will be terminated.",
                    streak
                ),
            });
        }

        // Different call from what was warned about → clear warning.
        if state.warned_entry.as_ref() != Some(&entry) {
            state.warned_entry = None;
        }

        Ok(call)
    }

    fn after_tool_result(
        &self,
        _context: &LoopContext,
        tool_id: &str,
        tool_input: Option<&serde_json::Value>,
        result: ToolResult,
    ) -> Result<ToolResult, LoopError> {
        if let Some(input) = tool_input {
            self.state.lock().record(tool_id, input);
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hive_model::CompletionRequest;
    use serde_json::json;

    fn make_context() -> LoopContext {
        // Minimal context for testing — only the fields the middleware touches.
        use hive_classification::DataClass;
        use hive_contracts::{SessionPermissions, ToolLimitsConfig};
        use std::sync::atomic::{AtomicBool, AtomicU8};
        use std::sync::Arc;

        LoopContext {
            conversation: crate::legacy::ConversationContext {
                session_id: "test".into(),
                message_id: "msg".into(),
                prompt: String::new(),
                prompt_content_parts: Vec::new(),
                history: Vec::new(),
                conversation_journal: None,
                initial_tool_iterations: 0,
            },
            routing: crate::legacy::RoutingConfig {
                required_capabilities: Default::default(),
                preferred_models: None,
                routing_decision: None,
                loop_strategy: None,
            },
            security: crate::legacy::SecurityContext {
                data_class: DataClass::Public,
                effective_data_class: Arc::new(AtomicU8::new(DataClass::Public.to_i64() as u8)),
                permissions: Arc::new(parking_lot::Mutex::new(SessionPermissions::default())),
                workspace_classification: None,
                connector_service: None,
            },
            tools_ctx: crate::legacy::ToolsContext {
                tools: Arc::new(hive_tools::ToolRegistry::new()),
                tool_execution_mode: hive_contracts::ToolExecutionMode::SequentialPartial,
                skill_catalog: None,
                knowledge_query_handler: None,
            },
            agent: crate::legacy::AgentContext {
                persona: None,
                personas: vec![],
                current_agent_id: None,
                parent_agent_id: None,
                agent_orchestrator: None,
                workspace_path: None,
                keep_alive: false,
                session_messaged: Arc::new(AtomicBool::new(false)),
            },
            tool_limits: ToolLimitsConfig::default(),
            preempt_signal: None,
        }
    }

    fn make_tool_call(tool_id: &str, input: serde_json::Value) -> ToolCall {
        ToolCall { tool_id: tool_id.to_string(), input }
    }

    fn make_tool_result(output: &str) -> ToolResult {
        ToolResult { output: json!(output), data_class: hive_classification::DataClass::Public }
    }

    /// Simulate a successful tool call through the middleware: before_tool_call → after_tool_result.
    fn simulate_successful_call(
        mw: &StallDetectionMiddleware,
        ctx: &LoopContext,
        tool_id: &str,
        input: &serde_json::Value,
    ) -> Result<(), LoopError> {
        let call = make_tool_call(tool_id, input.clone());
        let call = mw.before_tool_call(ctx, call)?;
        let result = make_tool_result("ok");
        mw.after_tool_result(ctx, &call.tool_id, Some(&call.input), result)?;
        Ok(())
    }

    #[test]
    fn diverse_calls_are_ok() {
        let mw = StallDetectionMiddleware::with_params(10, 3);
        let ctx = make_context();

        simulate_successful_call(&mw, &ctx, "read_file", &json!({"path": "a.rs"})).unwrap();
        simulate_successful_call(&mw, &ctx, "write_file", &json!({"path": "b.rs"})).unwrap();
        simulate_successful_call(&mw, &ctx, "shell", &json!({"cmd": "ls"})).unwrap();
        // No stall
    }

    #[test]
    fn consecutive_identical_calls_trigger_warning() {
        let mw = StallDetectionMiddleware::with_params(10, 3);
        let ctx = make_context();
        let args = json!({"path": "a.rs"});

        // 3 successful identical calls
        simulate_successful_call(&mw, &ctx, "read_file", &args).unwrap();
        simulate_successful_call(&mw, &ctx, "read_file", &args).unwrap();
        simulate_successful_call(&mw, &ctx, "read_file", &args).unwrap();

        // 4th attempt triggers WARNING (before_tool_call returns Err)
        let call = make_tool_call("read_file", args.clone());
        let err = mw.before_tool_call(&ctx, call).unwrap_err();
        assert!(err.to_string().contains("Stall warning"), "got: {err}");
    }

    #[test]
    fn warning_then_same_call_triggers_hard_stall() {
        let mw = StallDetectionMiddleware::with_params(10, 3);
        let ctx = make_context();
        let args = json!({"path": "a.rs"});

        // 3 successful identical calls
        for _ in 0..3 {
            simulate_successful_call(&mw, &ctx, "read_file", &args).unwrap();
        }

        // 4th → warning
        let call = make_tool_call("read_file", args.clone());
        assert!(mw.before_tool_call(&ctx, call).is_err());

        // 5th → hard stall (before_tool_call sets flag)
        let call = make_tool_call("read_file", args.clone());
        let err = mw.before_tool_call(&ctx, call).unwrap_err();
        assert!(err.to_string().contains("Stall detected"), "got: {err}");

        // before_model_call should now return StallDetected
        let req = CompletionRequest {
            prompt: String::new(),
            prompt_content_parts: vec![],
            messages: vec![],
            required_capabilities: Default::default(),
            preferred_models: None,
            tools: vec![],
        };
        let err = mw.before_model_call(&ctx, req).unwrap_err();
        assert!(matches!(err, LoopError::StallDetected { .. }), "got: {err:?}");
    }

    #[test]
    fn warning_clears_on_different_call() {
        let mw = StallDetectionMiddleware::with_params(10, 3);
        let ctx = make_context();
        let args = json!({"path": "a.rs"});

        // 3 identical → warning
        for _ in 0..3 {
            simulate_successful_call(&mw, &ctx, "read_file", &args).unwrap();
        }
        let call = make_tool_call("read_file", args.clone());
        assert!(mw.before_tool_call(&ctx, call).is_err());

        // Different call → clears warning, succeeds
        simulate_successful_call(&mw, &ctx, "write_file", &json!({"path": "b.rs"})).unwrap();

        // Now read_file again → streak is only 0 from tail (write_file broke it), OK
        simulate_successful_call(&mw, &ctx, "read_file", &args).unwrap();
    }

    #[test]
    fn non_consecutive_duplicates_are_ok() {
        let mw = StallDetectionMiddleware::with_params(10, 3);
        let ctx = make_context();
        let args = json!({"path": "a.rs"});

        // read, write, read, write, read → 3 total reads but never consecutive
        simulate_successful_call(&mw, &ctx, "read_file", &args).unwrap();
        simulate_successful_call(&mw, &ctx, "write_file", &json!({})).unwrap();
        simulate_successful_call(&mw, &ctx, "read_file", &args).unwrap();
        simulate_successful_call(&mw, &ctx, "write_file", &json!({})).unwrap();
        simulate_successful_call(&mw, &ctx, "read_file", &args).unwrap();
        // No warning — never 3 consecutive identical calls
    }

    #[test]
    fn different_args_same_tool_no_stall() {
        let mw = StallDetectionMiddleware::with_params(10, 3);
        let ctx = make_context();

        simulate_successful_call(&mw, &ctx, "read_file", &json!({"path": "a.rs"})).unwrap();
        simulate_successful_call(&mw, &ctx, "read_file", &json!({"path": "b.rs"})).unwrap();
        simulate_successful_call(&mw, &ctx, "read_file", &json!({"path": "c.rs"})).unwrap();
        simulate_successful_call(&mw, &ctx, "read_file", &json!({"path": "d.rs"})).unwrap();
        // All different args → no stall
    }

    #[test]
    fn before_model_call_passes_when_no_stall() {
        let mw = StallDetectionMiddleware::with_params(10, 3);
        let ctx = make_context();
        let req = CompletionRequest {
            prompt: String::new(),
            prompt_content_parts: vec![],
            messages: vec![],
            required_capabilities: Default::default(),
            preferred_models: None,
            tools: vec![],
        };
        assert!(mw.before_model_call(&ctx, req).is_ok());
    }

    #[test]
    fn threshold_of_two_works() {
        let mw = StallDetectionMiddleware::with_params(10, 2);
        let ctx = make_context();
        let args = json!({});

        simulate_successful_call(&mw, &ctx, "tool", &args).unwrap();
        simulate_successful_call(&mw, &ctx, "tool", &args).unwrap();

        // 3rd attempt → warning
        let call = make_tool_call("tool", args.clone());
        assert!(mw.before_tool_call(&ctx, call).is_err());
    }

    #[test]
    fn window_eviction_forgets_old_streak() {
        // Window of 3: once old entries evict, streak resets
        let mw = StallDetectionMiddleware::with_params(3, 3);
        let ctx = make_context();
        let args = json!({"path": "a.rs"});

        simulate_successful_call(&mw, &ctx, "read_file", &args).unwrap();
        simulate_successful_call(&mw, &ctx, "read_file", &args).unwrap();
        // Window: [read(a), read(a)]

        // Different call evicts oldest entry once window is full
        simulate_successful_call(&mw, &ctx, "write_file", &json!({})).unwrap();
        // Window: [read(a), read(a), write({})] → but write broke the streak at tail

        simulate_successful_call(&mw, &ctx, "read_file", &args).unwrap();
        // Window: [read(a), write({}), read(a)] → streak = 1
        simulate_successful_call(&mw, &ctx, "read_file", &args).unwrap();
        // Window: [write({}), read(a), read(a)] → streak = 2
        simulate_successful_call(&mw, &ctx, "read_file", &args).unwrap();
        // Window: [read(a), read(a), read(a)] → streak = 3

        // Next attempt → warning
        let call = make_tool_call("read_file", args.clone());
        assert!(mw.before_tool_call(&ctx, call).is_err());
    }
}
