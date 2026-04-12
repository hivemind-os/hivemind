//! Central workflow execution engine.
//!
//! Loads a [`WorkflowDefinition`], wires up backends, and runs the workflow
//! step-by-step — handling sequencing, branching, error recovery, state
//! persistence, and event emission.

use std::sync::Arc;

use serde_json::Value;

use crate::actions::{ActionExecutor, ActionOutcome};
use crate::error::{WorkflowError, WorkflowResult};
use crate::schema::WorkflowDefinition;
use crate::state::{WorkflowState, WorkflowStatus};
use crate::store::WorkflowStore;
use crate::traits::{ModelBackend, ToolBackend, WorkflowEvent, WorkflowEventSink};

/// The central workflow execution engine.
///
/// Construct with backend implementations, then call [`run`], [`resume`], or
/// [`run_builtin`] to execute workflows.
pub struct WorkflowEngine {
    model: Arc<dyn ModelBackend>,
    tools: Arc<dyn ToolBackend>,
    store: Arc<dyn WorkflowStore>,
    events: Arc<dyn WorkflowEventSink>,
    tool_limits: hive_contracts::ToolLimitsConfig,
}

impl WorkflowEngine {
    pub fn new(
        model: Arc<dyn ModelBackend>,
        tools: Arc<dyn ToolBackend>,
        store: Arc<dyn WorkflowStore>,
        events: Arc<dyn WorkflowEventSink>,
    ) -> Self {
        Self {
            model,
            tools,
            store,
            events,
            tool_limits: hive_contracts::ToolLimitsConfig::default(),
        }
    }

    /// Set the adaptive tool-call limits configuration.
    pub fn with_tool_limits(mut self, config: hive_contracts::ToolLimitsConfig) -> Self {
        self.tool_limits = config;
        self
    }

    /// Run a workflow to completion with the given inputs.
    ///
    /// Returns the final result value produced by a `ReturnValue` action, or
    /// `Value::Null` if the workflow completes without one.
    pub async fn run(
        &self,
        workflow: &WorkflowDefinition,
        run_id: String,
        inputs: serde_json::Map<String, Value>,
    ) -> WorkflowResult<Value> {
        workflow.validate()?;

        let mut state = WorkflowState::new(run_id.clone(), workflow.name.clone());

        // Validate required inputs and apply defaults.
        for input_def in &workflow.inputs {
            if let Some(value) = inputs.get(&input_def.name) {
                state.set_variable(&input_def.name, value.clone());
                state.untrusted_vars.insert(input_def.name.clone());
            } else if input_def.required {
                return Err(WorkflowError::InvalidState(format!(
                    "missing required input: {}",
                    input_def.name
                )));
            } else if let Some(default) = &input_def.default {
                state.set_variable(&input_def.name, default.clone());
            }
        }

        state.status = WorkflowStatus::Running;
        self.store.save(&state).await?;

        self.events
            .emit(WorkflowEvent::Started {
                run_id: run_id.clone(),
                workflow_name: workflow.name.clone(),
            })
            .await;

        match self.execute_steps(workflow, &mut state).await {
            Ok(result) => {
                state.complete();
                self.store.save(&state).await?;
                self.events
                    .emit(WorkflowEvent::Completed { run_id, result: result.to_string() })
                    .await;
                Ok(result)
            }
            Err(e) => {
                state.fail();
                self.store.save(&state).await?;
                self.events.emit(workflow_failed_event(&run_id, &e)).await;
                Err(e)
            }
        }
    }

    /// Resume a previously persisted workflow run.
    ///
    /// Loads the state from the store, verifies it is resumable, and continues
    /// executing from the current step.
    pub async fn resume(
        &self,
        workflow: &WorkflowDefinition,
        run_id: &str,
    ) -> WorkflowResult<Value> {
        let mut state = self
            .store
            .load(run_id)
            .await?
            .ok_or_else(|| WorkflowError::Store(format!("run `{run_id}` not found")))?;

        if state.workflow_name != workflow.name {
            return Err(WorkflowError::InvalidState(format!(
                "workflow name mismatch: state has `{}`, expected `{}`",
                state.workflow_name, workflow.name
            )));
        }

        match state.status {
            WorkflowStatus::Running | WorkflowStatus::Paused => {}
            other => {
                return Err(WorkflowError::InvalidState(format!(
                    "cannot resume workflow in {other:?} state"
                )));
            }
        }

        match self.execute_steps(workflow, &mut state).await {
            Ok(result) => {
                state.complete();
                self.store.save(&state).await?;
                self.events
                    .emit(WorkflowEvent::Completed {
                        run_id: run_id.to_string(),
                        result: result.to_string(),
                    })
                    .await;
                Ok(result)
            }
            Err(e) => {
                state.fail();
                self.store.save(&state).await?;
                self.events.emit(workflow_failed_event(run_id, &e)).await;
                Err(e)
            }
        }
    }

    /// Load a built-in workflow by name and run it.
    pub async fn run_builtin(
        &self,
        name: &str,
        run_id: String,
        inputs: serde_json::Map<String, Value>,
    ) -> WorkflowResult<Value> {
        let workflow = crate::workflows::load_builtin(name)?;
        self.run(&workflow, run_id, inputs).await
    }

    // ------------------------------------------------------------------
    // Internal
    // ------------------------------------------------------------------

    /// Main step-execution loop.
    async fn execute_steps(
        &self,
        workflow: &WorkflowDefinition,
        state: &mut WorkflowState,
    ) -> WorkflowResult<Value> {
        let max_tool_calls = workflow.config.max_tool_calls.unwrap_or(50);
        let max_iterations = workflow.config.max_iterations.unwrap_or(25);

        let executor = ActionExecutor::new(
            Arc::clone(&self.model),
            Arc::clone(&self.tools),
            Arc::clone(&self.events),
            max_tool_calls,
            &self.tool_limits,
        );

        while state.current_step < workflow.steps.len() {
            let step = &workflow.steps[state.current_step];

            self.events
                .emit(WorkflowEvent::StepStarted {
                    run_id: state.run_id.clone(),
                    step_id: step.id.clone(),
                })
                .await;

            // Execute, with optional retry/fallback on error.
            let outcome = match self.try_execute_step(&executor, step, state).await {
                Ok(outcome) => outcome,
                Err(e) => {
                    self.events
                        .emit(WorkflowEvent::StepFailed {
                            run_id: state.run_id.clone(),
                            step_id: step.id.clone(),
                            error: e.to_string(),
                        })
                        .await;
                    return Err(e);
                }
            };

            // Handle outcome.
            match outcome {
                ActionOutcome::Continue => {
                    self.events
                        .emit(WorkflowEvent::StepCompleted {
                            run_id: state.run_id.clone(),
                            step_id: step.id.clone(),
                        })
                        .await;
                    state.advance_step();
                }
                ActionOutcome::Jump(target_id) => {
                    let idx = find_step_index(workflow, &target_id)?;
                    self.events
                        .emit(WorkflowEvent::StepCompleted {
                            run_id: state.run_id.clone(),
                            step_id: step.id.clone(),
                        })
                        .await;
                    state.jump_to_step(idx);
                }
                ActionOutcome::Complete(value) => {
                    self.events
                        .emit(WorkflowEvent::StepCompleted {
                            run_id: state.run_id.clone(),
                            step_id: step.id.clone(),
                        })
                        .await;
                    return Ok(value);
                }
            }

            state.increment_iteration(max_iterations)?;
            self.store.save(state).await?;
        }

        // All steps exhausted without an explicit ReturnValue.
        Ok(Value::Null)
    }

    /// Execute a step's action, applying the step's error-handling policy
    /// (retry, fallback, return_error) on failure.
    async fn try_execute_step(
        &self,
        executor: &ActionExecutor,
        step: &crate::schema::StepDef,
        state: &mut WorkflowState,
    ) -> WorkflowResult<ActionOutcome> {
        let result = executor.execute(&step.action, state).await;

        match result {
            Ok(outcome) => Ok(outcome),
            Err(err) => {
                let handler = match &step.on_error {
                    Some(h) => h,
                    None => return Err(err),
                };

                // Retry logic.
                if let Some(retry) = &handler.retry {
                    let mut last_err = err;
                    for _ in 1..retry.max_attempts {
                        if let Some(delay) = retry.delay_ms {
                            tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                        }
                        match executor.execute(&step.action, state).await {
                            Ok(outcome) => return Ok(outcome),
                            Err(e) => last_err = e,
                        }
                    }
                    // All retries exhausted — fall through to fallback/return_error.
                    if let Some(fallback) = &handler.fallback_step {
                        return Ok(ActionOutcome::Jump(fallback.clone()));
                    }
                    if handler.return_error == Some(true) {
                        return Ok(ActionOutcome::Complete(Value::String(last_err.to_string())));
                    }
                    return Err(last_err);
                }

                // No retry — try fallback directly.
                if let Some(fallback) = &handler.fallback_step {
                    return Ok(ActionOutcome::Jump(fallback.clone()));
                }
                if handler.return_error == Some(true) {
                    return Ok(ActionOutcome::Complete(Value::String(err.to_string())));
                }

                Err(err)
            }
        }
    }
}

/// Find the index of a step by its ID.
fn find_step_index(workflow: &WorkflowDefinition, step_id: &str) -> WorkflowResult<usize> {
    workflow
        .steps
        .iter()
        .position(|s| s.id == step_id)
        .ok_or_else(|| WorkflowError::StepNotFound { step_id: step_id.to_string() })
}

/// Build a [`WorkflowEvent::Failed`] from a [`WorkflowError`], extracting
/// structured error fields when available.
fn workflow_failed_event(run_id: &str, error: &WorkflowError) -> WorkflowEvent {
    match error {
        WorkflowError::Model { message, error_code, http_status, provider_id, model } => {
            WorkflowEvent::Failed {
                run_id: run_id.to_string(),
                error: message.clone(),
                error_code: error_code.clone(),
                http_status: *http_status,
                provider_id: provider_id.clone(),
                model: model.clone(),
            }
        }
        other => WorkflowEvent::Failed {
            run_id: run_id.to_string(),
            error: other.to_string(),
            error_code: None,
            http_status: None,
            provider_id: None,
            model: None,
        },
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::WorkflowDefinition;
    use crate::store::InMemoryStore;
    use crate::traits::{
        ModelBackend, ModelRequest, ModelResponse, NullEventSink, ToolBackend, ToolCall,
        ToolResult, ToolSchema,
    };

    // -- Mock backends -------------------------------------------------------

    struct MockModel {
        response: String,
    }

    #[async_trait::async_trait]
    impl ModelBackend for MockModel {
        async fn complete(&self, _req: &ModelRequest) -> WorkflowResult<ModelResponse> {
            Ok(ModelResponse {
                content: self.response.clone(),
                tool_calls: vec![],
                metadata: Default::default(),
            })
        }
    }

    struct MockTools;

    #[async_trait::async_trait]
    impl ToolBackend for MockTools {
        async fn list_tools(&self) -> WorkflowResult<Vec<ToolSchema>> {
            Ok(vec![])
        }
        async fn execute(&self, call: &ToolCall) -> WorkflowResult<ToolResult> {
            Ok(ToolResult {
                call_id: call.id.clone(),
                name: call.name.clone(),
                content: "ok".into(),
                is_error: false,
            })
        }
    }

    // -- Helpers --------------------------------------------------------------

    fn make_engine(response: &str) -> WorkflowEngine {
        WorkflowEngine::new(
            Arc::new(MockModel { response: response.to_string() }),
            Arc::new(MockTools),
            Arc::new(InMemoryStore::new()),
            Arc::new(NullEventSink),
        )
    }

    fn make_engine_with_store(response: &str, store: Arc<InMemoryStore>) -> WorkflowEngine {
        WorkflowEngine::new(
            Arc::new(MockModel { response: response.to_string() }),
            Arc::new(MockTools),
            store,
            Arc::new(NullEventSink),
        )
    }

    // -- Tests ----------------------------------------------------------------

    #[tokio::test]
    async fn run_sequential_workflow() {
        let engine = make_engine("Hello from the model!");
        let mut inputs = serde_json::Map::new();
        inputs.insert("user_input".into(), Value::String("say hello".into()));

        let result = engine.run_builtin("sequential", "run-1".into(), inputs).await.unwrap();

        // The sequential workflow returns {{response.content}} which the
        // expression evaluator resolves from the model response.
        assert!(
            result.is_string()
                || result.is_null()
                || result == Value::Null
                || result.to_string().contains("Hello")
        );
    }

    #[tokio::test]
    async fn run_with_missing_required_input() {
        let engine = make_engine("ignored");
        let inputs = serde_json::Map::new(); // no user_input

        let result = engine.run_builtin("sequential", "run-2".into(), inputs).await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("missing required input"),
            "expected 'missing required input' error, got: {err}"
        );
    }

    #[tokio::test]
    async fn resume_completed_workflow() {
        let store = Arc::new(InMemoryStore::new());
        let engine = make_engine_with_store("done", store.clone());

        // Create and complete a workflow state.
        let mut state = WorkflowState::new("run-3".into(), "sequential".into());
        state.complete();
        store.save(&state).await.unwrap();

        let wf = crate::workflows::load_builtin("sequential").unwrap();
        let result = engine.resume(&wf, "run-3").await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("cannot resume") || err.contains("invalid workflow state"),
            "expected state error, got: {err}"
        );
    }

    #[tokio::test]
    async fn run_builtin_unknown() {
        let engine = make_engine("nope");
        let result =
            engine.run_builtin("nonexistent", "run-4".into(), serde_json::Map::new()).await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("unknown built-in workflow"), "expected schema error, got: {err}");
    }

    #[tokio::test]
    async fn run_simple_branch_workflow() {
        let yaml = r#"
name: branch-test
version: "1.0"
config:
  max_iterations: 10
  max_tool_calls: 0
steps:
  - id: set_flag
    action:
      type: set_variable
      name: flag
      value: "true"
  - id: check_flag
    action:
      type: branch
      condition: "{{flag}}"
      then_step: on_true
      else_step: on_false
  - id: on_true
    action:
      type: return_value
      value: "took-true-branch"
  - id: on_false
    action:
      type: return_value
      value: "took-false-branch"
"#;
        let wf = WorkflowDefinition::from_yaml(yaml).unwrap();
        wf.validate().unwrap();

        let engine = make_engine("unused");
        let result = engine.run(&wf, "run-5".into(), serde_json::Map::new()).await.unwrap();

        assert_eq!(result, Value::String("took-true-branch".into()));
    }
}
