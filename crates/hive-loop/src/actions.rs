//! Executor for individual workflow actions.
//!
//! Each variant of [`ActionDef`] is handled by a dedicated method on
//! [`ActionExecutor`], which mutates [`WorkflowState`] and returns an
//! [`ActionOutcome`] that tells the engine what to do next.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::{json, Value};

use crate::error::{WorkflowError, WorkflowResult};
use crate::expression;
use crate::schema::{ActionDef, LogLevel, StepDef};
use crate::stall_detector::StallDetector;
use crate::state::WorkflowState;
use crate::tool_budget::{AdaptiveBudget, BudgetDecision};
use crate::traits::{
    Message, MessageRole, ModelBackend, ModelRequest, ToolBackend, ToolCall, WorkflowEvent,
    WorkflowEventSink,
};

/// Result of executing an action — tells the engine what to do next.
#[derive(Debug)]
pub enum ActionOutcome {
    /// Continue to the next step sequentially.
    Continue,
    /// Jump to a specific step by ID.
    Jump(String),
    /// The workflow is complete, with this final value.
    Complete(Value),
}

/// Executes individual workflow actions against the provided backends.
pub struct ActionExecutor {
    model: Arc<dyn ModelBackend>,
    tools: Arc<dyn ToolBackend>,
    events: Arc<dyn WorkflowEventSink>,
    max_tool_calls: usize,
    stall_detector: Mutex<StallDetector>,
    budget: Mutex<AdaptiveBudget>,
}

impl ActionExecutor {
    pub fn new(
        model: Arc<dyn ModelBackend>,
        tools: Arc<dyn ToolBackend>,
        events: Arc<dyn WorkflowEventSink>,
        max_tool_calls: usize,
        tool_limits: &hive_contracts::ToolLimitsConfig,
    ) -> Self {
        Self {
            model,
            tools,
            events,
            max_tool_calls,
            stall_detector: Mutex::new(StallDetector::from_config(tool_limits)),
            budget: Mutex::new(AdaptiveBudget::new(tool_limits)),
        }
    }

    /// Execute a single action, mutating state and returning the outcome.
    ///
    /// The return type is boxed because [`ActionDef::Loop`] calls `execute`
    /// recursively on inner steps, and Rust requires indirection to allow
    /// recursive async functions.
    pub fn execute<'a>(
        &'a self,
        action: &'a ActionDef,
        state: &'a mut WorkflowState,
    ) -> Pin<Box<dyn Future<Output = WorkflowResult<ActionOutcome>> + Send + 'a>> {
        Box::pin(async move {
            match action {
                ActionDef::ModelCall { prompt, system_prompt, result_var } => {
                    self.execute_model_call(
                        prompt,
                        system_prompt.as_deref(),
                        result_var.as_deref(),
                        state,
                    )
                    .await
                }
                ActionDef::ToolCall { tool_name, arguments, result_var } => {
                    self.execute_tool_call(
                        tool_name,
                        arguments.as_deref(),
                        result_var.as_deref(),
                        state,
                    )
                    .await
                }
                ActionDef::Branch { condition, then_step, else_step } => {
                    self.execute_branch(condition, then_step, else_step.as_deref(), state).await
                }
                ActionDef::ReturnValue { value } => self.execute_return(value, state).await,
                ActionDef::SetVariable { name, value } => {
                    self.execute_set_variable(name, value, state).await
                }
                ActionDef::Log { message, level } => self.execute_log(message, level, state).await,
                ActionDef::Loop { condition, max_iterations, steps } => {
                    self.execute_loop(condition, *max_iterations, steps, state).await
                }
                ActionDef::ParallelToolCalls { calls, result_var } => {
                    self.execute_parallel_tool_calls(calls, result_var.as_deref(), state).await
                }
            }
        })
    }

    // ------------------------------------------------------------------
    // ModelCall
    // ------------------------------------------------------------------

    async fn execute_model_call(
        &self,
        prompt: &str,
        system_prompt: Option<&str>,
        result_var: Option<&str>,
        state: &mut WorkflowState,
    ) -> WorkflowResult<ActionOutcome> {
        let resolved_prompt =
            expression::resolve_template(prompt, &state.variables, Some(&state.untrusted_vars))?;

        let mut messages: Vec<Message> = Vec::new();

        // Prepend system prompt if provided.
        if let Some(sp) = system_prompt {
            let resolved_sp =
                expression::resolve_template(sp, &state.variables, Some(&state.untrusted_vars))?;
            messages.push(Message { role: MessageRole::System, content: resolved_sp });
        }

        // Copy existing conversation history.
        messages.extend(state.messages.iter().cloned());

        // Append the current user prompt.
        messages.push(Message { role: MessageRole::User, content: resolved_prompt.clone() });

        let tool_schemas = self.tools.list_tools().await?;

        let request = ModelRequest { messages, tools: tool_schemas };

        self.events.emit(WorkflowEvent::ModelCallStarted { run_id: state.run_id.clone() }).await;

        let response = self.model.complete(&request).await?;

        let content_preview = if response.content.len() > 100 {
            let end = {
                let mut e = 100;
                while e > 0 && !response.content.is_char_boundary(e) {
                    e -= 1;
                }
                e
            };
            format!("{}…", &response.content[..end])
        } else {
            response.content.clone()
        };

        self.events
            .emit(WorkflowEvent::ModelCallCompleted {
                run_id: state.run_id.clone(),
                content_preview,
            })
            .await;

        // Record messages in state.
        state.push_message(MessageRole::User, resolved_prompt);
        state.push_message(MessageRole::Assistant, response.content.clone());

        // Store result variable.
        if let Some(var_name) = result_var {
            let tool_calls_json: Vec<Value> = response
                .tool_calls
                .iter()
                .map(|tc| {
                    json!({
                        "id": tc.id,
                        "name": tc.name,
                        "arguments": tc.arguments,
                    })
                })
                .collect();

            state.set_variable(
                var_name,
                json!({
                    "content": response.content,
                    "has_tool_calls": !response.tool_calls.is_empty(),
                    "tool_calls": tool_calls_json,
                }),
            );
        }

        Ok(ActionOutcome::Continue)
    }

    // ------------------------------------------------------------------
    // ToolCall
    // ------------------------------------------------------------------

    async fn execute_tool_call(
        &self,
        tool_name: &str,
        arguments: Option<&str>,
        result_var: Option<&str>,
        state: &mut WorkflowState,
    ) -> WorkflowResult<ActionOutcome> {
        let resolved_name = expression::resolve_template(tool_name, &state.variables, None)?;

        let resolved_args: Value = if let Some(args_template) = arguments {
            let resolved = expression::resolve_template(args_template, &state.variables, None)?;
            serde_json::from_str(&resolved).map_err(|error| {
                WorkflowError::Expression(format!("invalid tool arguments JSON: {error}"))
            })?
        } else {
            Value::Object(Default::default())
        };

        let call_id = format!("call_{}", state.tool_call_count);
        let call =
            ToolCall { id: call_id, name: resolved_name.clone(), arguments: resolved_args.clone() };

        // Check stall detection + adaptive budget before executing.
        {
            let stall_status = self.stall_detector.lock().record(&resolved_name, &resolved_args);
            let decision = self.budget.lock().check(state.tool_call_count, 1);

            // Check stall first, then budget.
            if let crate::stall_detector::StallStatus::Stalled { tool_name, repeated_count } =
                stall_status
            {
                return Err(WorkflowError::StallDetected { tool_name, count: repeated_count });
            }

            match decision {
                BudgetDecision::Allow => {}
                BudgetDecision::Extended { new_budget, extensions_granted } => {
                    self.events
                        .emit(WorkflowEvent::Completed {
                            run_id: state.run_id.clone(),
                            result: format!(
                                "Tool-call budget extended to {new_budget} (extension #{extensions_granted})"
                            ),
                        })
                        .await;
                }
                BudgetDecision::HardStop { ceiling } => {
                    return Err(WorkflowError::LimitExceeded {
                        kind: format!("hard ceiling ({ceiling})"),
                        limit: ceiling,
                    });
                }
            }
        }

        self.events
            .emit(WorkflowEvent::ToolCallStarted {
                run_id: state.run_id.clone(),
                tool_name: resolved_name.clone(),
            })
            .await;

        let result = self.tools.execute(&call).await?;

        self.events
            .emit(WorkflowEvent::ToolCallCompleted {
                run_id: state.run_id.clone(),
                tool_name: resolved_name,
                is_error: result.is_error,
            })
            .await;

        // Record tool result as a Tool message.
        state.push_message(MessageRole::Tool, result.content.clone());

        state.increment_tool_calls(1, self.max_tool_calls)?;

        if let Some(var_name) = result_var {
            state.set_variable(
                var_name,
                json!({
                    "content": result.content,
                    "is_error": result.is_error,
                }),
            );
        }

        Ok(ActionOutcome::Continue)
    }

    // ------------------------------------------------------------------
    // Branch
    // ------------------------------------------------------------------

    async fn execute_branch(
        &self,
        condition: &str,
        then_step: &str,
        else_step: Option<&str>,
        state: &mut WorkflowState,
    ) -> WorkflowResult<ActionOutcome> {
        let result = expression::evaluate_condition(condition, &state.variables)?;

        if result {
            Ok(ActionOutcome::Jump(then_step.to_string()))
        } else if let Some(es) = else_step {
            Ok(ActionOutcome::Jump(es.to_string()))
        } else {
            Ok(ActionOutcome::Continue)
        }
    }

    // ------------------------------------------------------------------
    // ReturnValue
    // ------------------------------------------------------------------

    async fn execute_return(
        &self,
        value: &str,
        state: &mut WorkflowState,
    ) -> WorkflowResult<ActionOutcome> {
        let resolved = expression::resolve_value(value, &state.variables)?;
        Ok(ActionOutcome::Complete(resolved))
    }

    // ------------------------------------------------------------------
    // SetVariable
    // ------------------------------------------------------------------

    async fn execute_set_variable(
        &self,
        name: &str,
        value: &str,
        state: &mut WorkflowState,
    ) -> WorkflowResult<ActionOutcome> {
        let resolved = expression::resolve_value(value, &state.variables)?;
        state.set_variable(name, resolved);

        self.events
            .emit(WorkflowEvent::VariableSet {
                run_id: state.run_id.clone(),
                name: name.to_string(),
            })
            .await;

        Ok(ActionOutcome::Continue)
    }

    // ------------------------------------------------------------------
    // Log
    // ------------------------------------------------------------------

    async fn execute_log(
        &self,
        message: &str,
        level: &LogLevel,
        state: &mut WorkflowState,
    ) -> WorkflowResult<ActionOutcome> {
        let resolved = expression::resolve_template(message, &state.variables, None)?;

        let level_str = match level {
            LogLevel::Debug => "debug",
            LogLevel::Info => "info",
            LogLevel::Warn => "warn",
            LogLevel::Error => "error",
        };

        self.events
            .emit(WorkflowEvent::Log {
                run_id: state.run_id.clone(),
                level: level_str.to_string(),
                message: resolved,
            })
            .await;

        Ok(ActionOutcome::Continue)
    }

    // ------------------------------------------------------------------
    // Loop
    // ------------------------------------------------------------------

    async fn execute_loop(
        &self,
        condition: &str,
        max_iterations: usize,
        steps: &[StepDef],
        state: &mut WorkflowState,
    ) -> WorkflowResult<ActionOutcome> {
        while expression::evaluate_condition(condition, &state.variables)? {
            state.increment_iteration(max_iterations)?;

            for step in steps {
                let outcome = self.execute(&step.action, state).await?;
                match outcome {
                    ActionOutcome::Continue => {}
                    ActionOutcome::Complete(_) => return Ok(outcome),
                    ActionOutcome::Jump(_) => return Ok(outcome),
                }
            }
        }

        Ok(ActionOutcome::Continue)
    }

    // ------------------------------------------------------------------
    // ParallelToolCalls
    // ------------------------------------------------------------------

    async fn execute_parallel_tool_calls(
        &self,
        calls: &str,
        result_var: Option<&str>,
        state: &mut WorkflowState,
    ) -> WorkflowResult<ActionOutcome> {
        let resolved = expression::resolve_value(calls, &state.variables)?;

        let call_defs = resolved.as_array().cloned().unwrap_or_default();

        const MAX_PARALLEL_CALLS: usize = 100;
        if call_defs.len() > MAX_PARALLEL_CALLS {
            return Err(WorkflowError::Expression(format!(
                "parallel tool call count {} exceeds limit of {MAX_PARALLEL_CALLS}",
                call_defs.len()
            )));
        }

        // Check tool-call limit BEFORE execution to avoid side effects from
        // over-limit calls. Record stall detection for the batch.
        {
            let batch: Vec<(String, Value)> = call_defs
                .iter()
                .map(|def| {
                    let name = def.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let arguments =
                        def.get("arguments").cloned().unwrap_or(Value::Object(Default::default()));
                    (name, arguments)
                })
                .collect();
            let stall_status = self.stall_detector.lock().record_batch(&batch);
            let decision = self.budget.lock().check(state.tool_call_count, call_defs.len());

            // Check stall first, then budget.
            if let crate::stall_detector::StallStatus::Stalled { tool_name, repeated_count } =
                stall_status
            {
                return Err(WorkflowError::StallDetected { tool_name, count: repeated_count });
            }

            match decision {
                BudgetDecision::Allow => {}
                BudgetDecision::Extended { new_budget, extensions_granted } => {
                    self.events
                        .emit(WorkflowEvent::Completed {
                            run_id: state.run_id.clone(),
                            result: format!(
                                "Tool-call budget extended to {new_budget} (extension #{extensions_granted})"
                            ),
                        })
                        .await;
                }
                BudgetDecision::HardStop { ceiling } => {
                    return Err(WorkflowError::LimitExceeded {
                        kind: format!("hard ceiling ({ceiling})"),
                        limit: ceiling,
                    });
                }
            }
        }

        // Build tool call structs up-front so state borrows are released
        // before the parallel execution phase.
        let prepared: Vec<(ToolCall, String)> = call_defs
            .iter()
            .enumerate()
            .map(|(i, def)| {
                let id = def.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let name = def.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let arguments =
                    def.get("arguments").cloned().unwrap_or(Value::Object(Default::default()));
                let call = ToolCall {
                    id: if id.is_empty() {
                        format!("call_{}", state.tool_call_count + i)
                    } else {
                        id
                    },
                    name: name.clone(),
                    arguments,
                };
                (call, name)
            })
            .collect();

        // Execute tool calls in parallel using JoinSet with bounded concurrency.
        const MAX_CONCURRENCY: usize = 10;
        let semaphore = Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENCY));
        let mut set = tokio::task::JoinSet::new();

        for (idx, (call, name)) in prepared.into_iter().enumerate() {
            let tools = Arc::clone(&self.tools);
            let events = Arc::clone(&self.events);
            let run_id = state.run_id.clone();
            let sem = Arc::clone(&semaphore);

            set.spawn(async move {
                let _permit = sem.acquire().await.expect("semaphore closed");
                events
                    .emit(WorkflowEvent::ToolCallStarted {
                        run_id: run_id.clone(),
                        tool_name: name.clone(),
                    })
                    .await;

                let result = tools.execute(&call).await;

                let is_error = result.as_ref().map(|r| r.is_error).unwrap_or(true);
                events
                    .emit(WorkflowEvent::ToolCallCompleted { run_id, tool_name: name, is_error })
                    .await;

                (idx, result)
            });
        }

        // Collect results in original order.
        let mut indexed_results: Vec<(usize, _)> = Vec::with_capacity(call_defs.len());
        while let Some(join_result) = set.join_next().await {
            let (idx, tool_result) = join_result.map_err(|e| {
                WorkflowError::Other(anyhow::anyhow!("parallel tool call task panicked: {e}"))
            })?;
            let tool_result = tool_result?;
            indexed_results.push((
                idx,
                json!({
                    "call_id": tool_result.call_id,
                    "name": tool_result.name,
                    "content": tool_result.content,
                    "is_error": tool_result.is_error,
                }),
            ));
        }
        indexed_results.sort_by_key(|(idx, _)| *idx);
        let results: Vec<Value> = indexed_results.into_iter().map(|(_, v)| v).collect();

        state.increment_tool_calls(call_defs.len(), self.max_tool_calls)?;

        if let Some(var_name) = result_var {
            state.set_variable(var_name, Value::Array(results));
        }

        Ok(ActionOutcome::Continue)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::WorkflowResult;
    use crate::traits::{ModelResponse, NullEventSink, ToolResult, ToolSchema};

    // -- Mock backends ---------------------------------------------------

    struct MockModelBackend {
        response: ModelResponse,
    }

    #[async_trait::async_trait]
    impl ModelBackend for MockModelBackend {
        async fn complete(&self, _request: &ModelRequest) -> WorkflowResult<ModelResponse> {
            Ok(self.response.clone())
        }
    }

    struct MockToolBackend {
        result: ToolResult,
        schemas: Vec<ToolSchema>,
    }

    impl MockToolBackend {
        fn new(result: ToolResult) -> Self {
            Self { result, schemas: vec![] }
        }
    }

    #[async_trait::async_trait]
    impl ToolBackend for MockToolBackend {
        async fn list_tools(&self) -> WorkflowResult<Vec<ToolSchema>> {
            Ok(self.schemas.clone())
        }

        async fn execute(&self, _call: &ToolCall) -> WorkflowResult<ToolResult> {
            Ok(self.result.clone())
        }
    }

    // -- Helpers ---------------------------------------------------------

    fn make_executor(model_response: ModelResponse, tool_result: ToolResult) -> ActionExecutor {
        ActionExecutor::new(
            Arc::new(MockModelBackend { response: model_response }),
            Arc::new(MockToolBackend::new(tool_result)),
            Arc::new(NullEventSink),
            100,
            &hive_contracts::ToolLimitsConfig::default(),
        )
    }

    fn default_model_response() -> ModelResponse {
        ModelResponse {
            content: "Hello from model".to_string(),
            tool_calls: vec![],
            metadata: serde_json::Map::new(),
        }
    }

    fn default_tool_result() -> ToolResult {
        ToolResult {
            call_id: "call_0".to_string(),
            name: "test_tool".to_string(),
            content: "tool output".to_string(),
            is_error: false,
        }
    }

    fn new_state() -> WorkflowState {
        WorkflowState::new("test-run".to_string(), "test-workflow".to_string())
    }

    // -- Tests -----------------------------------------------------------

    #[tokio::test]
    async fn model_call_stores_result_variable() {
        let executor = make_executor(default_model_response(), default_tool_result());
        let mut state = new_state();
        state.set_variable("user_input", json!("hi"));

        let action = ActionDef::ModelCall {
            prompt: "{{user_input}}".to_string(),
            system_prompt: None,
            result_var: Some("response".to_string()),
        };

        let outcome = executor.execute(&action, &mut state).await.unwrap();
        assert!(matches!(outcome, ActionOutcome::Continue));

        let var = state.get_variable("response").unwrap();
        assert_eq!(var["content"], "Hello from model");
        assert_eq!(var["has_tool_calls"], false);
    }

    #[tokio::test]
    async fn model_call_with_system_prompt() {
        let executor = make_executor(default_model_response(), default_tool_result());
        let mut state = new_state();

        let action = ActionDef::ModelCall {
            prompt: "hello".to_string(),
            system_prompt: Some("You are helpful.".to_string()),
            result_var: Some("resp".to_string()),
        };

        let outcome = executor.execute(&action, &mut state).await.unwrap();
        assert!(matches!(outcome, ActionOutcome::Continue));
        assert!(state.get_variable("resp").is_some());
        // Two messages recorded: user prompt + assistant response.
        assert_eq!(state.messages.len(), 2);
    }

    #[tokio::test]
    async fn tool_call_stores_result_variable() {
        let executor = make_executor(default_model_response(), default_tool_result());
        let mut state = new_state();

        let action = ActionDef::ToolCall {
            tool_name: "test_tool".to_string(),
            arguments: None,
            result_var: Some("result".to_string()),
        };

        let outcome = executor.execute(&action, &mut state).await.unwrap();
        assert!(matches!(outcome, ActionOutcome::Continue));

        let var = state.get_variable("result").unwrap();
        assert_eq!(var["content"], "tool output");
        assert_eq!(var["is_error"], false);
        assert_eq!(state.tool_call_count, 1);
    }

    #[tokio::test]
    async fn branch_true_jumps_to_then_step() {
        let executor = make_executor(default_model_response(), default_tool_result());
        let mut state = new_state();
        state.set_variable("flag", json!(true));

        let action = ActionDef::Branch {
            condition: "{{flag}}".to_string(),
            then_step: "step_a".to_string(),
            else_step: Some("step_b".to_string()),
        };

        let outcome = executor.execute(&action, &mut state).await.unwrap();
        match outcome {
            ActionOutcome::Jump(target) => assert_eq!(target, "step_a"),
            other => panic!("expected Jump, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn branch_false_jumps_to_else_step() {
        let executor = make_executor(default_model_response(), default_tool_result());
        let mut state = new_state();
        state.set_variable("flag", json!(false));

        let action = ActionDef::Branch {
            condition: "{{flag}}".to_string(),
            then_step: "step_a".to_string(),
            else_step: Some("step_b".to_string()),
        };

        let outcome = executor.execute(&action, &mut state).await.unwrap();
        match outcome {
            ActionOutcome::Jump(target) => assert_eq!(target, "step_b"),
            other => panic!("expected Jump, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn branch_false_no_else_continues() {
        let executor = make_executor(default_model_response(), default_tool_result());
        let mut state = new_state();
        state.set_variable("flag", json!(false));

        let action = ActionDef::Branch {
            condition: "{{flag}}".to_string(),
            then_step: "step_a".to_string(),
            else_step: None,
        };

        let outcome = executor.execute(&action, &mut state).await.unwrap();
        assert!(matches!(outcome, ActionOutcome::Continue));
    }

    #[tokio::test]
    async fn return_value_completes_workflow() {
        let executor = make_executor(default_model_response(), default_tool_result());
        let mut state = new_state();
        state.set_variable("answer", json!("done"));

        let action = ActionDef::ReturnValue { value: "{{answer}}".to_string() };

        let outcome = executor.execute(&action, &mut state).await.unwrap();
        match outcome {
            ActionOutcome::Complete(val) => assert_eq!(val, json!("done")),
            other => panic!("expected Complete, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn set_variable_stores_value() {
        let executor = make_executor(default_model_response(), default_tool_result());
        let mut state = new_state();

        let action = ActionDef::SetVariable {
            name: "greeting".to_string(),
            value: "hello world".to_string(),
        };

        let outcome = executor.execute(&action, &mut state).await.unwrap();
        assert!(matches!(outcome, ActionOutcome::Continue));
        assert_eq!(state.get_variable("greeting"), Some(&json!("hello world")));
    }

    #[tokio::test]
    async fn log_action_continues() {
        let executor = make_executor(default_model_response(), default_tool_result());
        let mut state = new_state();

        let action = ActionDef::Log { message: "hello log".to_string(), level: LogLevel::Info };

        let outcome = executor.execute(&action, &mut state).await.unwrap();
        assert!(matches!(outcome, ActionOutcome::Continue));
    }

    #[tokio::test]
    async fn loop_respects_condition() {
        let executor = make_executor(default_model_response(), default_tool_result());
        let mut state = new_state();
        // Condition is false from the start, so the loop body never runs.
        state.set_variable("running", json!(false));

        let action = ActionDef::Loop {
            condition: "{{running}}".to_string(),
            max_iterations: 5,
            steps: vec![StepDef {
                id: "inner".to_string(),
                description: None,
                action: ActionDef::Log {
                    message: "should not run".to_string(),
                    level: LogLevel::Info,
                },
                on_error: None,
            }],
        };

        let outcome = executor.execute(&action, &mut state).await.unwrap();
        assert!(matches!(outcome, ActionOutcome::Continue));
        assert_eq!(state.iteration_count, 0);
    }

    #[tokio::test]
    async fn loop_exits_on_return_value() {
        let executor = make_executor(default_model_response(), default_tool_result());
        let mut state = new_state();
        state.set_variable("running", json!(true));

        let action = ActionDef::Loop {
            condition: "{{running}}".to_string(),
            max_iterations: 10,
            steps: vec![StepDef {
                id: "inner_return".to_string(),
                description: None,
                action: ActionDef::ReturnValue { value: "loop_result".to_string() },
                on_error: None,
            }],
        };

        let outcome = executor.execute(&action, &mut state).await.unwrap();
        match outcome {
            ActionOutcome::Complete(val) => assert_eq!(val, json!("loop_result")),
            other => panic!("expected Complete, got {other:?}"),
        }
    }
}
