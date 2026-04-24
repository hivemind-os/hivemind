use crate::error::WorkflowError;
use crate::expression::{
    evaluate_condition, is_pure_template, resolve_output_map, resolve_path, resolve_template,
    resolve_template_for_prompt, ExpressionContext,
};
use crate::shadow_executor::{NullToolInfoProvider, ShadowStepExecutor, ToolInfoProvider};
use crate::store::{sha256_hex, WorkflowPersistence};
use crate::types::*;
use crate::validation::validate_definition;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{Mutex as TokioMutex, RwLock, Semaphore};
use tracing::{debug, info, warn, Instrument};

// ---------------------------------------------------------------------------
// Step executor trait — implemented externally for actual task dispatch
// ---------------------------------------------------------------------------

/// Handles execution of task steps. Injected by the service layer.
#[async_trait]
pub trait StepExecutor: Send + Sync {
    /// Execute a tool call and return the result.
    async fn call_tool(
        &self,
        tool_id: &str,
        arguments: Value,
        ctx: &ExecutionContext,
    ) -> Result<Value, String>;

    /// Invoke an agent. Returns the agent result if sync, or agent_id if async.
    /// `step_permissions` overrides context-level permissions when non-empty.
    /// When `existing_agent_id` is `Some`, the executor should resume the
    /// already-alive agent (signal + wait) instead of spawning a new one.
    /// This is used during daemon-restart recovery when the agent was
    /// previously spawned and its `child_agent_id` was persisted.
    #[allow(clippy::too_many_arguments)]
    async fn invoke_agent(
        &self,
        persona_id: &str,
        task: &str,
        async_exec: bool,
        timeout_secs: Option<u64>,
        step_permissions: &[PermissionEntry],
        agent_name: Option<&str>,
        existing_agent_id: Option<&str>,
        ctx: &ExecutionContext,
    ) -> Result<Value, String>;

    /// Signal an agent or session (fire-and-forget).
    async fn signal_agent(
        &self,
        target: &SignalTarget,
        content: &str,
        ctx: &ExecutionContext,
    ) -> Result<Value, String>;

    /// Wait for a previously signalled agent to complete.
    async fn wait_for_agent(
        &self,
        agent_id: &str,
        timeout_secs: Option<u64>,
        ctx: &ExecutionContext,
    ) -> Result<Value, String>;

    /// Create a feedback gate request. Returns the request_id.
    async fn create_feedback_request(
        &self,
        instance_id: i64,
        step_id: &str,
        prompt: &str,
        choices: Option<&[String]>,
        allow_freeform: bool,
        ctx: &ExecutionContext,
    ) -> Result<String, String>;

    /// Register an event gate subscription. Returns a subscription_id.
    async fn register_event_gate(
        &self,
        instance_id: i64,
        step_id: &str,
        topic: &str,
        filter: Option<&str>,
        timeout_secs: Option<u64>,
        ctx: &ExecutionContext,
    ) -> Result<String, String>;

    /// Launch a child workflow. Returns the child instance_id.
    async fn launch_workflow(
        &self,
        workflow_name: &str,
        inputs: Value,
        ctx: &ExecutionContext,
    ) -> Result<i64, String>;

    /// Schedule a task. Returns the task_id.
    async fn schedule_task(
        &self,
        schedule: &ScheduleTaskDef,
        ctx: &ExecutionContext,
    ) -> Result<String, String>;

    /// Render a persona prompt template with the given parameters.
    /// Resolves the persona, finds the prompt, validates & renders it.
    /// Returns the rendered text.
    async fn render_prompt_template(
        &self,
        persona_id: &str,
        prompt_id: &str,
        parameters: Value,
        ctx: &ExecutionContext,
    ) -> Result<String, String>;

    /// Called when an instance is being stopped (paused or killed).
    /// Implementations can override to clean up external resources such as
    /// event gate registrations and pending feedback requests.
    async fn on_instance_stopped(&self, _instance_id: i64) -> Result<(), String> {
        Ok(())
    }
}

/// Receives events emitted during workflow execution.
#[async_trait]
pub trait WorkflowEventEmitter: Send + Sync {
    async fn emit(&self, event: WorkflowEvent);
}

/// Context available to step executors.
#[derive(Debug, Clone)]
pub struct ExecutionContext {
    pub instance_id: i64,
    pub step_id: String,
    pub parent_session_id: String,
    pub parent_agent_id: Option<String>,
    pub workspace_path: Option<String>,
    pub permissions: Vec<PermissionEntry>,
    /// Path to the workflow-level attachments directory.
    pub attachments_dir: Option<String>,
    /// Resolved workflow attachments selected for the current InvokeAgent step.
    pub selected_attachments: Vec<WorkflowAttachment>,
    /// Execution mode: Normal (real) or Shadow (intercepted side effects).
    pub execution_mode: ExecutionMode,
}

// ---------------------------------------------------------------------------
// Null implementations for testing
// ---------------------------------------------------------------------------

/// A no-op step executor that returns empty results. Useful for testing.
pub struct NullStepExecutor;

#[async_trait]
impl StepExecutor for NullStepExecutor {
    async fn call_tool(&self, _: &str, _: Value, _: &ExecutionContext) -> Result<Value, String> {
        Ok(Value::Null)
    }
    async fn invoke_agent(
        &self,
        _: &str,
        _: &str,
        _: bool,
        _: Option<u64>,
        _: &[PermissionEntry],
        _: Option<&str>,
        _: Option<&str>,
        _: &ExecutionContext,
    ) -> Result<Value, String> {
        Ok(Value::Null)
    }
    async fn signal_agent(
        &self,
        _: &SignalTarget,
        _: &str,
        _: &ExecutionContext,
    ) -> Result<Value, String> {
        Ok(Value::Null)
    }
    async fn wait_for_agent(
        &self,
        _: &str,
        _: Option<u64>,
        _: &ExecutionContext,
    ) -> Result<Value, String> {
        Ok(Value::Null)
    }
    async fn create_feedback_request(
        &self,
        _: i64,
        _: &str,
        _: &str,
        _: Option<&[String]>,
        _: bool,
        _: &ExecutionContext,
    ) -> Result<String, String> {
        Ok("mock-request-id".to_string())
    }
    async fn register_event_gate(
        &self,
        _: i64,
        _: &str,
        _: &str,
        _: Option<&str>,
        _: Option<u64>,
        _: &ExecutionContext,
    ) -> Result<String, String> {
        Ok("mock-subscription-id".to_string())
    }
    async fn launch_workflow(
        &self,
        _: &str,
        _: Value,
        _: &ExecutionContext,
    ) -> Result<i64, String> {
        Ok(0)
    }
    async fn schedule_task(
        &self,
        _: &ScheduleTaskDef,
        _: &ExecutionContext,
    ) -> Result<String, String> {
        Ok("mock-task-id".to_string())
    }
    async fn render_prompt_template(
        &self,
        _: &str,
        _: &str,
        _: Value,
        _: &ExecutionContext,
    ) -> Result<String, String> {
        Ok("mock-rendered-prompt".to_string())
    }
}

/// A no-op event emitter.
pub struct NullEventEmitter;

#[async_trait]
impl WorkflowEventEmitter for NullEventEmitter {
    async fn emit(&self, _event: WorkflowEvent) {}
}

// ---------------------------------------------------------------------------
// Workflow Engine
// ---------------------------------------------------------------------------

/// Default maximum concurrent step tasks across all workflow instances.
const DEFAULT_MAX_CONCURRENT_STEPS: usize = 32;

/// Maximum number of GoTo activations before we consider the workflow stuck in a loop.
const MAX_GOTO_ACTIVATIONS: usize = 50;

/// Default safety limit for While loops when `max_iterations` is not specified.
const DEFAULT_WHILE_MAX_ITERATIONS: u32 = 10_000;

pub struct WorkflowEngine {
    store: Arc<dyn WorkflowPersistence>,
    step_executor: Arc<dyn StepExecutor>,
    event_emitter: Arc<dyn WorkflowEventEmitter>,
    /// Bounds the number of concurrently executing step tasks (shared across
    /// all workflow instances on this engine).
    step_semaphore: Arc<Semaphore>,
    /// Active instances that have been paused or killed externally
    killed: Arc<RwLock<HashSet<i64>>>,
    paused: Arc<RwLock<HashSet<i64>>>,
    /// Per-instance mutexes to serialise concurrent operations on the same
    /// workflow instance.
    instance_locks: Arc<RwLock<HashMap<i64, Arc<TokioMutex<()>>>>>,
    /// Base directory for workflow file attachments, if configured.
    attachments_base_dir: Option<std::path::PathBuf>,
    /// Optional tool metadata provider for shadow mode risk classification.
    /// When absent, shadow mode intercepts ALL tool calls (fail-closed).
    tool_info_provider: Option<Arc<dyn ToolInfoProvider>>,
}

impl WorkflowEngine {
    pub fn new(
        store: Arc<dyn WorkflowPersistence>,
        step_executor: Arc<dyn StepExecutor>,
        event_emitter: Arc<dyn WorkflowEventEmitter>,
    ) -> Self {
        Self::with_concurrency(store, step_executor, event_emitter, DEFAULT_MAX_CONCURRENT_STEPS)
    }

    pub fn with_concurrency(
        store: Arc<dyn WorkflowPersistence>,
        step_executor: Arc<dyn StepExecutor>,
        event_emitter: Arc<dyn WorkflowEventEmitter>,
        max_concurrent_steps: usize,
    ) -> Self {
        Self {
            store,
            step_executor,
            event_emitter,
            step_semaphore: Arc::new(Semaphore::new(max_concurrent_steps)),
            killed: Arc::new(RwLock::new(HashSet::new())),
            paused: Arc::new(RwLock::new(HashSet::new())),
            instance_locks: Arc::new(RwLock::new(HashMap::new())),
            attachments_base_dir: None,
            tool_info_provider: None,
        }
    }

    /// Set the base directory for workflow file attachments.
    pub fn set_attachments_base_dir(&mut self, dir: std::path::PathBuf) {
        self.attachments_base_dir = Some(dir);
    }

    /// Set the tool metadata provider used for shadow-mode risk classification.
    /// When set, shadow mode uses this to distinguish safe (read-only) tools
    /// from risky ones. When absent, all tools are intercepted (fail-closed).
    pub fn set_tool_info_provider(&mut self, provider: Arc<dyn ToolInfoProvider>) {
        self.tool_info_provider = Some(provider);
    }

    /// Access the underlying persistence store.
    pub fn store(&self) -> &Arc<dyn WorkflowPersistence> {
        &self.store
    }

    /// Create a lightweight clone of this engine by sharing all Arc-wrapped
    /// internal state. Used for spawning background tasks that need their own
    /// owned engine reference.
    fn clone_engine(&self) -> WorkflowEngine {
        WorkflowEngine {
            store: Arc::clone(&self.store),
            step_executor: Arc::clone(&self.step_executor),
            event_emitter: Arc::clone(&self.event_emitter),
            step_semaphore: Arc::clone(&self.step_semaphore),
            instance_locks: Arc::clone(&self.instance_locks),
            killed: Arc::clone(&self.killed),
            paused: Arc::clone(&self.paused),
            attachments_base_dir: self.attachments_base_dir.clone(),
            tool_info_provider: self.tool_info_provider.clone(),
        }
    }

    /// Obtain (or create) the per-instance mutex for `instance_id`.
    async fn instance_lock(&self, instance_id: i64) -> Arc<TokioMutex<()>> {
        // Try read-only first
        {
            let locks = self.instance_locks.read().await;
            if let Some(lock) = locks.get(&instance_id) {
                return Arc::clone(lock);
            }
        }
        // Need to insert
        let mut locks = self.instance_locks.write().await;
        locks.entry(instance_id).or_insert_with(|| Arc::new(TokioMutex::new(()))).clone()
    }

    /// Create and start a new workflow instance.
    pub async fn launch(
        &self,
        definition: WorkflowDefinition,
        inputs: Value,
        parent_session_id: String,
        parent_agent_id: Option<String>,
        permissions: Vec<PermissionEntry>,
        trigger_step_id: Option<String>,
    ) -> Result<i64, WorkflowError> {
        self.launch_with_id(
            definition,
            inputs,
            parent_session_id,
            parent_agent_id,
            permissions,
            trigger_step_id,
            None,
            ExecutionMode::Normal,
        )
        .await
    }

    /// Create and start a new workflow instance.
    /// Blocks until the workflow reaches a terminal or waiting state.
    #[allow(clippy::too_many_arguments)]
    pub async fn launch_with_id(
        &self,
        definition: WorkflowDefinition,
        inputs: Value,
        parent_session_id: String,
        parent_agent_id: Option<String>,
        permissions: Vec<PermissionEntry>,
        trigger_step_id: Option<String>,
        workspace_path: Option<String>,
        execution_mode: ExecutionMode,
    ) -> Result<i64, WorkflowError> {
        let instance = self
            .setup_instance(
                definition,
                inputs,
                parent_session_id,
                parent_agent_id,
                permissions,
                trigger_step_id,
                workspace_path,
                execution_mode,
            )
            .await?;
        let id = instance.id;

        let lock = self.instance_lock(id).await;
        let _guard = lock.lock().await;

        self.run_loop(instance).await?;

        Ok(id)
    }

    /// Launch a workflow in Shadow mode with per-step output overrides for
    /// unit-test execution.  Blocks until the instance reaches a terminal or
    /// waiting state.
    #[allow(clippy::too_many_arguments)]
    pub async fn launch_test(
        &self,
        definition: WorkflowDefinition,
        inputs: Value,
        parent_session_id: String,
        trigger_step_id: Option<String>,
        shadow_overrides: HashMap<String, serde_json::Value>,
    ) -> Result<i64, WorkflowError> {
        let mut instance = self
            .setup_instance(
                definition,
                inputs,
                parent_session_id,
                None,
                vec![],
                trigger_step_id,
                None,
                ExecutionMode::Shadow,
            )
            .await?;
        instance.shadow_overrides = shadow_overrides;
        // Persist the overrides so they survive re-entry
        self.store.update_instance(&instance)?;
        let id = instance.id;

        let lock = self.instance_lock(id).await;
        let _guard = lock.lock().await;

        self.run_loop(instance).await?;

        Ok(id)
    }

    /// Create and persist a workflow instance, then spawn execution in the
    /// background. Returns the instance ID immediately without waiting for
    /// the workflow to finish.
    #[allow(clippy::too_many_arguments)]
    pub async fn launch_background(
        self: &Arc<Self>,
        definition: WorkflowDefinition,
        inputs: Value,
        parent_session_id: String,
        parent_agent_id: Option<String>,
        permissions: Vec<PermissionEntry>,
        trigger_step_id: Option<String>,
        workspace_path: Option<String>,
        execution_mode: ExecutionMode,
    ) -> Result<i64, WorkflowError> {
        let instance = self
            .setup_instance(
                definition,
                inputs,
                parent_session_id,
                parent_agent_id,
                permissions,
                trigger_step_id,
                workspace_path,
                execution_mode,
            )
            .await?;
        let ret_id = instance.id;

        // Spawn execution in the background
        let engine = Arc::clone(self);
        let inst_id = ret_id;
        tokio::spawn(async move {
            let lock = engine.instance_lock(inst_id).await;
            let _guard = lock.lock().await;
            // Reload instance from store to pick up any changes made between
            // lock release above and lock re-acquisition here (e.g. gate
            // responses that arrived in the gap).
            let instance = match engine.load_instance(inst_id).await {
                Ok(i) => i,
                Err(e) => {
                    warn!(instance_id = %inst_id, error = %e, "failed to load instance for background execution");
                    engine.instance_locks.write().await.remove(&inst_id);
                    return;
                }
            };
            if let Err(e) = engine.run_loop(instance).await {
                warn!(instance_id = %inst_id, error = %e, "workflow execution failed");
                // Transition the instance to Failed so it doesn't remain
                // permanently stuck in Running.
                match engine.store.get_instance(inst_id) {
                    Ok(Some(mut inst)) if !matches!(
                        inst.status,
                        WorkflowStatus::Completed | WorkflowStatus::Failed | WorkflowStatus::Killed
                    ) => {
                        inst.status = WorkflowStatus::Failed;
                        inst.error = Some(format!("{e}"));
                        inst.completed_at_ms = Some(now_ms());
                        inst.updated_at_ms = now_ms();
                        if let Err(persist_err) = engine.store.update_instance(&inst) {
                            warn!(instance_id = %inst_id, error = %persist_err, "failed to persist Failed status after run_loop error");
                        }
                        engine.event_emitter.emit(WorkflowEvent::InstanceFailed {
                            instance_id: inst_id,
                            error: inst.error.clone().unwrap_or_default(),
                        }).await;
                    }
                    _ => {}
                }
                engine.instance_locks.write().await.remove(&inst_id);
            }
        }.instrument(tracing::info_span!("service", service = "workflows")));

        Ok(ret_id)
    }

    /// Internal: create, persist, and set up triggers for a new workflow
    /// instance.
    #[allow(clippy::too_many_arguments)]
    async fn setup_instance(
        &self,
        definition: WorkflowDefinition,
        inputs: Value,
        parent_session_id: String,
        parent_agent_id: Option<String>,
        permissions: Vec<PermissionEntry>,
        trigger_step_id: Option<String>,
        workspace_path: Option<String>,
        execution_mode: ExecutionMode,
    ) -> Result<WorkflowInstance, WorkflowError> {
        validate_definition(&definition)?;

        if let Some(ref selected_trigger_id) = trigger_step_id {
            let selected =
                definition.steps.iter().find(|s| s.id == *selected_trigger_id).ok_or_else(
                    || WorkflowError::InvalidDefinition {
                        reason: format!(
                            "trigger_step_id '{}' does not reference an existing step",
                            selected_trigger_id
                        ),
                    },
                )?;
            if !matches!(selected.step_type, StepType::Trigger { .. }) {
                return Err(WorkflowError::InvalidDefinition {
                    reason: format!(
                        "trigger_step_id '{}' must reference a trigger step",
                        selected_trigger_id
                    ),
                });
            }
        }

        let now = now_ms();

        // Initialize step states
        let mut step_states = HashMap::new();
        for step in &definition.steps {
            step_states.insert(
                step.id.clone(),
                StepState {
                    step_id: step.id.clone(),
                    status: StepStatus::Pending,
                    started_at_ms: None,
                    completed_at_ms: None,
                    outputs: None,
                    error: None,
                    retry_count: 0,
                    retry_delay_secs: None,
                    child_workflow_id: None,
                    child_agent_id: None,
                    interaction_request_id: None,
                    interaction_prompt: None,
                    interaction_choices: None,
                    interaction_allow_freeform: None,
                    resume_at_ms: None,
                },
            );
        }

        // Initialize variables from schema defaults
        let variables = initialize_variables(&definition.variables);

        // Merge definition-level permissions with caller-provided ones.
        // Caller-provided permissions take precedence (appear first).
        let mut merged_permissions = permissions.clone();
        merged_permissions.extend(definition.permissions.clone());

        let mut instance = WorkflowInstance {
            id: 0, // placeholder — assigned by store
            definition,
            status: WorkflowStatus::Running,
            variables,
            step_states,
            parent_session_id: parent_session_id.clone(),
            parent_agent_id: parent_agent_id.clone(),
            trigger_step_id: trigger_step_id.clone(),
            permissions: merged_permissions,
            workspace_path,
            created_at_ms: now,
            updated_at_ms: now,
            completed_at_ms: None,
            output: None,
            error: None,
            resolved_result_message: None,
            goto_activated_steps: HashSet::new(),
            goto_source_steps: HashSet::new(),
            active_loops: HashMap::new(),
            execution_mode,
            shadow_overrides: HashMap::new(),
        };

        // Persist and get the auto-assigned ID
        let assigned_id = self.store.create_instance(&instance)?;
        instance.id = assigned_id;

        self.event_emitter
            .emit(WorkflowEvent::InstanceCreated {
                instance_id: instance.id,
                definition_name: instance.definition.name.clone(),
                parent_session_id: parent_session_id.clone(),
                mode: instance.definition.mode,
                execution_mode: instance.execution_mode,
            })
            .await;

        self.event_emitter.emit(WorkflowEvent::InstanceStarted { instance_id: instance.id }).await;

        // Complete trigger steps.
        // If trigger_step_id is provided, only complete that specific trigger step.
        // Otherwise (backward compat), complete all trigger steps.
        for step in &instance.definition.steps.clone() {
            if let StepType::Trigger { trigger } = &step.step_type {
                let should_complete = match &trigger_step_id {
                    Some(id) => step.id == *id,
                    None => true,
                };

                if !should_complete {
                    continue;
                }

                // Validate inputs against trigger schema
                validate_trigger_inputs(trigger, &inputs)?;

                if let Some(state) = instance.step_states.get_mut(&step.id) {
                    state.status = StepStatus::Completed;
                    state.started_at_ms = Some(now);
                    state.completed_at_ms = Some(now);

                    if step.outputs.is_empty() {
                        state.outputs = Some(inputs.clone());
                    } else {
                        // Apply output mappings with trigger data = raw inputs
                        let expr_ctx = ExpressionContext {
                            variables: instance.variables.clone(),
                            step_outputs: HashMap::new(),
                            trigger_data: inputs.clone(),
                            current_result: inputs.clone(),
                            current_error: None,
                            untrusted_vars: HashSet::from([
                                "trigger".to_string(),
                                "event".to_string(),
                            ]),
                        };
                        match resolve_output_map(&step.outputs, &expr_ctx) {
                            Ok(mapped) => state.outputs = Some(mapped),
                            Err(e) => {
                                tracing::warn!(step_id = %step.id, error = %e, "Trigger output mapping failed, using raw inputs");
                                state.outputs = Some(inputs.clone());
                            }
                        }
                    }
                }
            }
        }

        // Persist after trigger setup
        instance.updated_at_ms = now_ms();
        {
            let store = &self.store;
            store.update_instance(&instance)?;
        }

        Ok(instance)
    }

    /// Resume a paused or waiting workflow.
    pub async fn resume(&self, instance_id: i64) -> Result<(), WorkflowError> {
        let lock = self.instance_lock(instance_id).await;
        let _guard = lock.lock().await;

        self.paused.write().await.remove(&instance_id);

        let mut instance = self.load_instance(instance_id).await?;

        if instance.status != WorkflowStatus::Paused {
            return Err(WorkflowError::InvalidState { status: instance.status.to_string() });
        }

        instance.status = WorkflowStatus::Running;
        instance.updated_at_ms = now_ms();
        {
            let store = &self.store;
            store.update_instance(&instance)?;
        }

        self.event_emitter.emit(WorkflowEvent::InstanceResumed { instance_id }).await;

        // Continue execution in the background (same pattern as respond_to_gate).
        let engine = self.clone_engine();
        let inst_id = instance_id;
        drop(_guard);
        tokio::spawn(async move {
            let lock = engine.instance_lock(inst_id).await;
            let _guard = lock.lock().await;
            let instance = match engine.load_instance(inst_id).await {
                Ok(i) => i,
                Err(e) => {
                    warn!(instance_id = %inst_id, error = %e, "failed to load instance after resume");
                    engine.instance_locks.write().await.remove(&inst_id);
                    return;
                }
            };
            if let Err(e) = engine.run_loop(instance).await {
                warn!(instance_id = %inst_id, error = %e, "workflow execution failed after resume");
                match engine.store.get_instance(inst_id) {
                    Ok(Some(mut inst)) if !matches!(
                        inst.status,
                        WorkflowStatus::Completed | WorkflowStatus::Failed | WorkflowStatus::Killed
                    ) => {
                        inst.status = WorkflowStatus::Failed;
                        inst.error = Some(format!("{e}"));
                        inst.completed_at_ms = Some(now_ms());
                        inst.updated_at_ms = now_ms();
                        if let Err(persist_err) = engine.store.update_instance(&inst) {
                            warn!(instance_id = %inst_id, error = %persist_err, "failed to persist Failed status");
                        }
                        engine.event_emitter.emit(WorkflowEvent::InstanceFailed {
                            instance_id: inst_id,
                            error: inst.error.clone().unwrap_or_default(),
                        }).await;
                    }
                    _ => {}
                }
                engine.instance_locks.write().await.remove(&inst_id);
            }
        }.instrument(tracing::info_span!("service", service = "workflows")));

        Ok(())
    }

    /// Pause a running workflow.
    pub async fn pause(&self, instance_id: i64) -> Result<(), WorkflowError> {
        self.paused.write().await.insert(instance_id);

        let lock = self.instance_lock(instance_id).await;
        let _guard = lock.lock().await;

        let mut instance = match self.load_instance(instance_id).await {
            Ok(instance) => instance,
            Err(e) => {
                self.paused.write().await.remove(&instance_id);
                return Err(e);
            }
        };

        // Reject pause on terminal instances
        if matches!(
            instance.status,
            WorkflowStatus::Completed | WorkflowStatus::Failed | WorkflowStatus::Killed
        ) {
            self.paused.write().await.remove(&instance_id);
            return Err(WorkflowError::InvalidState {
                status: format!("Cannot pause instance in {:?} state", instance.status),
            });
        }

        instance.status = WorkflowStatus::Paused;
        if let Err(e) = self.step_executor.on_instance_stopped(instance_id).await {
            warn!(instance_id = %instance_id, error = %e, "failed to clean up resources on pause");
        }
        instance.updated_at_ms = now_ms();
        {
            let store = &self.store;
            store.update_instance(&instance)?;
        }

        self.event_emitter.emit(WorkflowEvent::InstancePaused { instance_id }).await;
        self.paused.write().await.remove(&instance_id);
        Ok(())
    }

    /// Set the killed flag for an instance so any in-flight `run_loop`
    /// notices at its next iteration and exits early. Called by the service
    /// layer before cascade cleanup to avoid holding the instance lock.
    pub async fn mark_killed(&self, instance_id: i64) {
        self.killed.write().await.insert(instance_id);
    }

    /// Kill a running workflow.
    pub async fn kill(&self, instance_id: i64) -> Result<(), WorkflowError> {
        self.mark_killed(instance_id).await;

        let lock = self.instance_lock(instance_id).await;
        let _guard = lock.lock().await;

        let mut instance = match self.load_instance(instance_id).await {
            Ok(instance) => instance,
            Err(e) => {
                self.killed.write().await.remove(&instance_id);
                return Err(e);
            }
        };

        // Reject kill on already-terminal instances
        if matches!(
            instance.status,
            WorkflowStatus::Completed | WorkflowStatus::Failed | WorkflowStatus::Killed
        ) {
            self.killed.write().await.remove(&instance_id);
            return Err(WorkflowError::InvalidState {
                status: format!("Cannot kill instance in {:?} state", instance.status),
            });
        }

        if let Err(e) = self.step_executor.on_instance_stopped(instance_id).await {
            warn!(instance_id = %instance_id, error = %e, "failed to clean up resources on kill");
        }

        instance.status = WorkflowStatus::Killed;
        instance.updated_at_ms = now_ms();
        instance.completed_at_ms = Some(now_ms());
        {
            let store = &self.store;
            store.update_instance(&instance)?;
        }

        self.event_emitter.emit(WorkflowEvent::InstanceKilled { instance_id }).await;
        self.killed.write().await.remove(&instance_id);
        Ok(())
    }

    /// Respond to a feedback gate on a specific step.
    pub async fn respond_to_gate(
        &self,
        instance_id: i64,
        step_id: &str,
        response: Value,
    ) -> Result<(), WorkflowError> {
        let lock = self.instance_lock(instance_id).await;
        let _guard = lock.lock().await;

        let mut instance = self.load_instance(instance_id).await?;

        if matches!(
            instance.status,
            WorkflowStatus::Completed | WorkflowStatus::Failed | WorkflowStatus::Killed
        ) {
            return Err(WorkflowError::InvalidState {
                status: format!("Cannot respond to gate on instance in {} state", instance.status),
            });
        }

        // Validate step state (immutable borrow, dropped before mutation)
        {
            let state = instance
                .step_states
                .get(step_id)
                .ok_or_else(|| WorkflowError::StepNotFound { step_id: step_id.to_string() })?;
            if state.status != StepStatus::WaitingOnInput {
                return Err(WorkflowError::InvalidState {
                    status: format!("Step {step_id} is not waiting on input"),
                });
            }
        }

        // Check if this is a loop preview pause response
        let is_preview_resume = instance
            .active_loops
            .get(step_id)
            .map_or(false, |ls| ls.preview_paused);

        if is_preview_resume {
            // Parse response — gate responses arrive as {"selected": "...", "text": "..."}
            let selected = response
                .get("selected")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let gate_request_id = instance
                .step_states
                .get(step_id)
                .and_then(|s| s.interaction_request_id.clone());

            if selected == "Abort" {
                // Complete the loop early with partial results
                let processed_count = instance
                    .active_loops
                    .get(step_id)
                    .map(|ls| ls.iteration as u64 + 1)
                    .unwrap_or(0);
                let total_count = instance
                    .active_loops
                    .get(step_id)
                    .and_then(|ls| ls.collection.as_ref().map(|c| c.len() as u64))
                    .unwrap_or(0);
                instance.active_loops.remove(step_id);
                if let Some(state) = instance.step_states.get_mut(step_id) {
                    state.status = StepStatus::Completed;
                    state.completed_at_ms = Some(now_ms());
                    state.outputs = Some(serde_json::json!({
                        "iteration_count": processed_count,
                        "completed": false,
                        "aborted": true,
                        "processed_count": processed_count,
                        "total_count": total_count,
                    }));
                    state.interaction_request_id = None;
                    state.interaction_prompt = None;
                    state.interaction_choices = None;
                    state.interaction_allow_freeform = None;
                }
            } else {
                // "Continue All" — resume the loop by setting LoopWaiting
                // The pre-pass will see body steps as done and reset to Pending.
                if let Some(state) = instance.step_states.get_mut(step_id) {
                    state.status = StepStatus::LoopWaiting;
                    state.interaction_request_id = None;
                    state.interaction_prompt = None;
                    state.interaction_choices = None;
                    state.interaction_allow_freeform = None;
                }
            }

            // Common: update workflow status and persist
            if instance.status == WorkflowStatus::WaitingOnInput {
                instance.status = WorkflowStatus::Running;
            }
            instance.updated_at_ms = now_ms();
            {
                let store = &self.store;
                store.update_instance(&instance)?;
            }

            self.event_emitter
                .emit(WorkflowEvent::InteractionResponded {
                    instance_id,
                    step_id: step_id.to_string(),
                    request_id: gate_request_id,
                    response_text: Some(selected.to_string()),
                })
                .await;

            // Spawn background run_loop continuation
            let engine = self.clone_engine();
            let inst_id = instance_id;
            drop(_guard);
            tokio::spawn(async move {
                let lock = engine.instance_lock(inst_id).await;
                let _guard = lock.lock().await;
                let instance = match engine.load_instance(inst_id).await {
                    Ok(i) => i,
                    Err(e) => {
                        warn!(instance_id = %inst_id, error = %e, "failed to load instance after preview response");
                        engine.instance_locks.write().await.remove(&inst_id);
                        return;
                    }
                };
                if let Err(e) = engine.run_loop(instance).await {
                    warn!(instance_id = %inst_id, error = %e, "workflow execution failed after preview response");
                    match engine.store.get_instance(inst_id) {
                        Ok(Some(mut inst)) if !matches!(
                            inst.status,
                            WorkflowStatus::Completed | WorkflowStatus::Failed | WorkflowStatus::Killed
                        ) => {
                            inst.status = WorkflowStatus::Failed;
                            inst.error = Some(format!("{e}"));
                            inst.completed_at_ms = Some(now_ms());
                            inst.updated_at_ms = now_ms();
                            if let Err(persist_err) = engine.store.update_instance(&inst) {
                                warn!(instance_id = %inst_id, error = %persist_err, "failed to persist Failed status");
                            }
                            engine.event_emitter.emit(WorkflowEvent::InstanceFailed {
                                instance_id: inst_id,
                                error: inst.error.clone().unwrap_or_default(),
                            }).await;
                        }
                        _ => {}
                    }
                    engine.instance_locks.write().await.remove(&inst_id);
                }
            }.instrument(tracing::info_span!("service", service = "workflows")));

            return Ok(());
        }

        // Resolve output mappings (immutable borrow of instance for context)
        let resolved_outputs = {
            let step_def = instance.definition.steps.iter().find(|s| s.id == step_id);
            if let Some(step_def) = step_def {
                if !step_def.outputs.is_empty() {
                    let mut expr_ctx = build_expression_context(&instance, Some(step_id));
                    expr_ctx.current_result = response.clone();
                    resolve_output_map(&step_def.outputs, &expr_ctx).unwrap_or_else(|e| {
                        tracing::warn!(step_id = %step_id, error = %e, "Gate response output mapping failed, using raw response");
                        response
                    })
                } else {
                    response
                }
            } else {
                response
            }
        };

        // Now mutate
        let state = instance.step_states.get_mut(step_id).unwrap();
        let gate_request_id = state.interaction_request_id.clone();
        state.status = StepStatus::Completed;
        state.completed_at_ms = Some(now_ms());
        let response_text = match &resolved_outputs {
            Value::String(s) => Some(s.clone()),
            other => Some(other.to_string()),
        };
        state.outputs = Some(resolved_outputs);
        state.interaction_request_id = None;

        // If workflow was waiting, set it back to running
        if instance.status == WorkflowStatus::WaitingOnInput {
            instance.status = WorkflowStatus::Running;
        }
        instance.updated_at_ms = now_ms();

        {
            let store = &self.store;
            store.update_instance(&instance)?;
        }

        self.event_emitter
            .emit(WorkflowEvent::InteractionResponded {
                instance_id,
                step_id: step_id.to_string(),
                request_id: gate_request_id,
                response_text,
            })
            .await;

        // Continue execution in the background so the caller (HTTP handler)
        // is not blocked while long-running steps (e.g. agent invocations)
        // execute.  The gate response is already persisted above.
        let engine = self.clone_engine();
        let inst_id = instance_id;
        // Drop the lock before spawning — the background task re-acquires it.
        drop(_guard);
        tokio::spawn(async move {
            let lock = engine.instance_lock(inst_id).await;
            let _guard = lock.lock().await;
            // Reload from store to pick up the persisted state.
            let instance = match engine.load_instance(inst_id).await {
                Ok(i) => i,
                Err(e) => {
                    warn!(instance_id = %inst_id, error = %e, "failed to load instance after gate response");
                    engine.instance_locks.write().await.remove(&inst_id);
                    return;
                }
            };
            if let Err(e) = engine.run_loop(instance).await {
                warn!(instance_id = %inst_id, error = %e, "workflow execution failed after gate response");
                match engine.store.get_instance(inst_id) {
                    Ok(Some(mut inst)) if !matches!(
                        inst.status,
                        WorkflowStatus::Completed | WorkflowStatus::Failed | WorkflowStatus::Killed
                    ) => {
                        inst.status = WorkflowStatus::Failed;
                        inst.error = Some(format!("{e}"));
                        inst.completed_at_ms = Some(now_ms());
                        inst.updated_at_ms = now_ms();
                        if let Err(persist_err) = engine.store.update_instance(&inst) {
                            warn!(instance_id = %inst_id, error = %persist_err, "failed to persist Failed status");
                        }
                        engine.event_emitter.emit(WorkflowEvent::InstanceFailed {
                            instance_id: inst_id,
                            error: inst.error.clone().unwrap_or_default(),
                        }).await;
                    }
                    _ => {}
                }
                engine.instance_locks.write().await.remove(&inst_id);
            }
        }.instrument(tracing::info_span!("service", service = "workflows")));

        Ok(())
    }

    /// Respond to an event gate on a specific step.
    pub async fn respond_to_event(
        &self,
        instance_id: i64,
        step_id: &str,
        event_data: Value,
    ) -> Result<(), WorkflowError> {
        let lock = self.instance_lock(instance_id).await;
        let _guard = lock.lock().await;

        let mut instance = self.load_instance(instance_id).await?;

        if matches!(
            instance.status,
            WorkflowStatus::Completed | WorkflowStatus::Failed | WorkflowStatus::Killed
        ) {
            return Err(WorkflowError::InvalidState {
                status: format!("Cannot respond to event on instance in {} state", instance.status),
            });
        }

        // Validate step state (immutable borrow, dropped before mutation)
        {
            let state = instance
                .step_states
                .get(step_id)
                .ok_or_else(|| WorkflowError::StepNotFound { step_id: step_id.to_string() })?;
            if state.status != StepStatus::WaitingOnEvent {
                return Err(WorkflowError::InvalidState {
                    status: format!("Step {step_id} is not waiting on event"),
                });
            }
        }

        // Resolve output mappings (immutable borrow of instance for context)
        let resolved_outputs = {
            let step_def = instance.definition.steps.iter().find(|s| s.id == step_id);
            if let Some(step_def) = step_def {
                if !step_def.outputs.is_empty() {
                    let mut expr_ctx = build_expression_context(&instance, Some(step_id));
                    expr_ctx.current_result = event_data.clone();
                    resolve_output_map(&step_def.outputs, &expr_ctx).unwrap_or_else(|e| {
                        tracing::warn!(step_id = %step_id, error = %e, "Event gate response output mapping failed, using raw event data");
                        event_data
                    })
                } else {
                    event_data
                }
            } else {
                event_data
            }
        };

        // Now mutate
        let state = instance.step_states.get_mut(step_id).unwrap();
        state.status = StepStatus::Completed;
        state.completed_at_ms = Some(now_ms());
        state.outputs = Some(resolved_outputs);
        state.interaction_request_id = None;

        if instance.status == WorkflowStatus::WaitingOnEvent {
            instance.status = WorkflowStatus::Running;
        }
        instance.updated_at_ms = now_ms();

        {
            let store = &self.store;
            store.update_instance(&instance)?;
        }

        self.event_emitter
            .emit(WorkflowEvent::EventGateResolved { instance_id, step_id: step_id.to_string() })
            .await;

        // Continue execution in the background (same pattern as respond_to_gate).
        let engine = self.clone_engine();
        let inst_id = instance_id;
        drop(_guard);
        tokio::spawn(async move {
            let lock = engine.instance_lock(inst_id).await;
            let _guard = lock.lock().await;
            let instance = match engine.load_instance(inst_id).await {
                Ok(i) => i,
                Err(e) => {
                    warn!(instance_id = %inst_id, error = %e, "failed to load instance after event response");
                    engine.instance_locks.write().await.remove(&inst_id);
                    return;
                }
            };
            if let Err(e) = engine.run_loop(instance).await {
                warn!(instance_id = %inst_id, error = %e, "workflow execution failed after event response");
                match engine.store.get_instance(inst_id) {
                    Ok(Some(mut inst)) if !matches!(
                        inst.status,
                        WorkflowStatus::Completed | WorkflowStatus::Failed | WorkflowStatus::Killed
                    ) => {
                        inst.status = WorkflowStatus::Failed;
                        inst.error = Some(format!("{e}"));
                        inst.completed_at_ms = Some(now_ms());
                        inst.updated_at_ms = now_ms();
                        if let Err(persist_err) = engine.store.update_instance(&inst) {
                            warn!(instance_id = %inst_id, error = %persist_err, "failed to persist Failed status");
                        }
                        engine.event_emitter.emit(WorkflowEvent::InstanceFailed {
                            instance_id: inst_id,
                            error: inst.error.clone().unwrap_or_default(),
                        }).await;
                    }
                    _ => {}
                }
                engine.instance_locks.write().await.remove(&inst_id);
            }
        }.instrument(tracing::info_span!("service", service = "workflows")));

        Ok(())
    }

    /// Spawn background timers for any steps in `WaitingForDelay` status.
    /// Each timer sleeps until `resume_at_ms` and then calls `resume_from_delay`.
    fn spawn_delay_timers(&self, instance: &WorkflowInstance) {
        let now = now_ms();
        for (step_id, state) in &instance.step_states {
            if state.status == StepStatus::WaitingForDelay {
                if let Some(resume_at) = state.resume_at_ms {
                    let remaining_ms = resume_at.saturating_sub(now);
                    let engine = self.clone_engine();
                    let inst_id = instance.id;
                    let sid = step_id.clone();
                    tokio::spawn(async move {
                        if remaining_ms > 0 {
                            tokio::time::sleep(std::time::Duration::from_millis(remaining_ms)).await;
                        }
                        if let Err(e) = engine.resume_from_delay(inst_id, &sid).await {
                            warn!(instance_id = %inst_id, step_id = %sid, error = %e, "failed to resume from delay");
                        }
                    }.instrument(tracing::info_span!("service", service = "workflows")));
                }
            }
        }
    }

    /// Resume a workflow step that was waiting for a delay to elapse.
    /// Called by the background timer spawned in `spawn_delay_timers`.
    async fn resume_from_delay(
        &self,
        instance_id: i64,
        step_id: &str,
    ) -> Result<(), WorkflowError> {
        let lock = self.instance_lock(instance_id).await;
        let _guard = lock.lock().await;

        let mut instance = self.load_instance(instance_id).await?;

        if matches!(
            instance.status,
            WorkflowStatus::Completed | WorkflowStatus::Failed | WorkflowStatus::Killed
        ) {
            return Ok(());
        }

        // Verify the step is still waiting for delay (might have been killed/paused)
        let is_waiting = instance
            .step_states
            .get(step_id)
            .is_some_and(|s| s.status == StepStatus::WaitingForDelay);

        if !is_waiting {
            return Ok(());
        }

        // Mark the delay step as completed with output mapping applied
        let step_def = instance.definition.steps.iter().find(|s| s.id == step_id);
        let resolved_outputs = if let Some(step_def) = step_def {
            if !step_def.outputs.is_empty() {
                let mut expr_ctx = build_expression_context(&instance, Some(step_id));
                expr_ctx.current_result = Value::Null;
                resolve_output_map(&step_def.outputs, &expr_ctx).unwrap_or(Value::Null)
            } else {
                Value::Null
            }
        } else {
            Value::Null
        };

        if let Some(state) = instance.step_states.get_mut(step_id) {
            state.status = StepStatus::Completed;
            state.completed_at_ms = Some(now_ms());
            state.outputs = Some(resolved_outputs.clone());
            state.resume_at_ms = None;
        }

        // Resume the workflow
        if matches!(
            instance.status,
            WorkflowStatus::WaitingOnEvent | WorkflowStatus::WaitingOnInput
        ) {
            instance.status = WorkflowStatus::Running;
        }
        instance.updated_at_ms = now_ms();
        self.store.update_instance(&instance)?;

        self.event_emitter
            .emit(WorkflowEvent::StepCompleted {
                instance_id,
                step_id: step_id.to_string(),
                outputs: Some(resolved_outputs),
            })
            .await;

        self.run_loop(instance).await?;
        Ok(())
    }

    async fn run_loop(&self, mut instance: WorkflowInstance) -> Result<(), WorkflowError> {
        // Wrap the step executor for shadow mode — intercept risky actions
        let effective_executor: Arc<dyn StepExecutor> =
            if instance.execution_mode == ExecutionMode::Shadow {
                let tip: Arc<dyn ToolInfoProvider> = self
                    .tool_info_provider
                    .clone()
                    .unwrap_or_else(|| Arc::new(NullToolInfoProvider));
                Arc::new(ShadowStepExecutor::new(
                    self.step_executor.clone(),
                    tip,
                    self.store.clone(),
                ))
            } else {
                self.step_executor.clone()
            };

        let mut goto_activation_count: usize = 0;
        let mut running_recovery_attempts: usize = 0;
        loop {
            // Check kill/pause flags
            if self.killed.read().await.contains(&instance.id) {
                self.killed.write().await.remove(&instance.id);
                self.instance_locks.write().await.remove(&instance.id);
                return Ok(());
            }
            if self.paused.read().await.contains(&instance.id) {
                instance.status = WorkflowStatus::Paused;
                instance.updated_at_ms = now_ms();
                let store = &self.store;
                store.update_instance(&instance)?;
                return Ok(());
            }

            // Pre-pass: if all body steps of an active loop are Completed/Skipped,
            // reset the loop control step from LoopWaiting to Pending so it can be
            // re-evaluated on this iteration of run_loop.
            let active_loop_ids: Vec<String> = instance.active_loops.keys().cloned().collect();
            for loop_step_id in &active_loop_ids {
                let is_loop_waiting = instance
                    .step_states
                    .get(loop_step_id)
                    .is_some_and(|s| s.status == StepStatus::LoopWaiting);
                if !is_loop_waiting {
                    continue;
                }
                if let Some(ls) = instance.active_loops.get(loop_step_id) {
                    let all_body_done = ls.body_step_ids.iter().all(|body_id| {
                        instance.step_states.get(body_id).is_some_and(|s| {
                            matches!(s.status, StepStatus::Completed | StepStatus::Skipped)
                        })
                    });
                    if all_body_done {
                        // Reset to Pending so compute_ready_steps picks it up
                        if let Some(state) = instance.step_states.get_mut(loop_step_id) {
                            state.status = StepStatus::Pending;
                            state.started_at_ms = None;
                            state.completed_at_ms = None;
                        }
                    }
                }
            }

            // Mark unreachable pending steps as Skipped (replaces the old
            // eager skip in BranchTaken and the deadlock-fallback function).
            skip_unreachable_steps(&mut instance);

            // Compute the set of ready steps
            let ready = compute_ready_steps(&instance);
            if ready.is_empty() {
                // Check if any steps are still running (shouldn't happen at this point
                // since we await all handles, but be defensive)
                let has_running =
                    instance.step_states.values().any(|s| s.status == StepStatus::Running);

                if has_running {
                    // Shouldn't happen since we awaited all step handles
                    // above. Reset orphaned Running steps to Pending and
                    // retry, matching the recovery approach.
                    running_recovery_attempts += 1;
                    if running_recovery_attempts > 3 {
                        warn!(
                            instance_id = %instance.id,
                            "steps remain Running after {running_recovery_attempts} recovery attempts; failing workflow"
                        );
                        instance.status = WorkflowStatus::Failed;
                        instance.error = Some(
                            "workflow stuck: steps remain in Running state after recovery attempts"
                                .to_string(),
                        );
                        instance.completed_at_ms = Some(now_ms());
                        instance.updated_at_ms = now_ms();
                        let store = &self.store;
                        store.update_instance(&instance)?;
                        self.event_emitter
                            .emit(WorkflowEvent::InstanceFailed {
                                instance_id: instance.id,
                                error: instance.error.clone().unwrap_or_default(),
                            })
                            .await;
                        self.killed.write().await.remove(&instance.id);
                        self.instance_locks.write().await.remove(&instance.id);
                        return Ok(());
                    }
                    warn!(
                        instance_id = %instance.id,
                        attempt = running_recovery_attempts,
                        "unexpected Running steps after batch completion; resetting to Pending"
                    );
                    for state in instance.step_states.values_mut() {
                        if state.status == StepStatus::Running {
                            state.status = StepStatus::Pending;
                            state.started_at_ms = None;
                        }
                    }
                    continue;
                }

                let has_waiting = instance.step_states.values().any(|s| {
                    matches!(
                        s.status,
                        StepStatus::WaitingOnInput
                            | StepStatus::WaitingOnEvent
                            | StepStatus::WaitingForDelay
                    )
                });

                let has_pending =
                    instance.step_states.values().any(|s| s.status == StepStatus::Pending);

                if has_waiting {
                    // Workflow is blocked on user input, external event, or delay
                    instance.status = if instance
                        .step_states
                        .values()
                        .any(|s| s.status == StepStatus::WaitingOnInput)
                    {
                        WorkflowStatus::WaitingOnInput
                    } else {
                        WorkflowStatus::WaitingOnEvent
                    };
                } else if !has_pending {
                    // All done — no pending, no waiting, no running
                    instance.status = WorkflowStatus::Completed;
                    instance.completed_at_ms = Some(now_ms());
                    // Resolve output
                    if let Some(ref output_map) = instance.definition.output.clone() {
                        let expr_ctx = build_expression_context(&instance, None);
                        match resolve_output_map(output_map, &expr_ctx) {
                            Ok(output) => instance.output = Some(output),
                            Err(e) => {
                                warn!("Failed to resolve workflow output: {}", e);
                            }
                        }
                    }
                    let result_message =
                        instance.definition.result_message.as_ref().and_then(|tmpl| {
                            let expr_ctx = build_expression_context(&instance, None);
                            resolve_template(tmpl, &expr_ctx).ok()
                        });
                    instance.resolved_result_message = result_message.clone();
                    self.event_emitter
                        .emit(WorkflowEvent::InstanceCompleted {
                            instance_id: instance.id,
                            output: instance.output.clone(),
                            result_message,
                        })
                        .await;
                } else {
                    // Pending steps exist but none are ready — all unreachable
                    // steps were already Skipped by skip_unreachable_steps above.
                    // If we still have pending steps with no ready ones, it is a
                    // genuine deadlock.  Mark remaining pending steps as Skipped
                    // and complete the workflow.
                    for state in instance.step_states.values_mut() {
                        if state.status == StepStatus::Pending {
                            state.status = StepStatus::Skipped;
                        }
                    }

                    instance.status = WorkflowStatus::Completed;
                    instance.completed_at_ms = Some(now_ms());
                    if let Some(ref output_map) = instance.definition.output.clone() {
                        let expr_ctx = build_expression_context(&instance, None);
                        if let Ok(output) = resolve_output_map(output_map, &expr_ctx) {
                            instance.output = Some(output);
                        }
                    }
                    let result_message =
                        instance.definition.result_message.as_ref().and_then(|tmpl| {
                            let expr_ctx = build_expression_context(&instance, None);
                            resolve_template(tmpl, &expr_ctx).ok()
                        });
                    instance.resolved_result_message = result_message.clone();
                    self.event_emitter
                        .emit(WorkflowEvent::InstanceCompleted {
                            instance_id: instance.id,
                            output: instance.output.clone(),
                            result_message,
                        })
                        .await;
                }

                instance.updated_at_ms = now_ms();
                let store = &self.store;
                store.update_instance(&instance)?;

                // Record successful normal-mode run for first-run protection.
                if instance.status == WorkflowStatus::Completed
                    && instance.execution_mode == ExecutionMode::Normal
                {
                    let def_json =
                        serde_json::to_string(&instance.definition).unwrap_or_default();
                    let hash = sha256_hex(&def_json);
                    if let Err(e) = store.record_successful_run(
                        &instance.definition.name,
                        &instance.definition.version,
                        &hash,
                        instance.completed_at_ms.unwrap_or_else(now_ms),
                    ) {
                        warn!(
                            instance_id = instance.id,
                            error = %e,
                            "Failed to record successful run metadata"
                        );
                    }
                }

                // Clean up in-memory state for terminal and idle instances.
                // Locks are recreated on-demand when an instance resumes.
                if matches!(
                    instance.status,
                    WorkflowStatus::Completed | WorkflowStatus::Failed | WorkflowStatus::Killed
                ) {
                    self.killed.write().await.remove(&instance.id);
                }
                if !matches!(instance.status, WorkflowStatus::Running) {
                    self.instance_locks.write().await.remove(&instance.id);
                }

                // Spawn background timers for any delay steps that are waiting
                self.spawn_delay_timers(&instance);

                return Ok(());
            }

            // Execute ready steps (concurrently)
            let exec_ctx = ExecutionContext {
                instance_id: instance.id,
                step_id: String::new(), // overridden per-step below
                parent_session_id: instance.parent_session_id.clone(),
                parent_agent_id: instance.parent_agent_id.clone(),
                workspace_path: instance.workspace_path.clone(),
                permissions: instance.permissions.clone(),
                attachments_dir: self.attachments_base_dir.as_ref().map(|base| {
                    let store = crate::attachments::AttachmentStore::new(base);
                    store
                        .version_dir(&instance.definition.id, &instance.definition.version)
                        .to_string_lossy()
                        .to_string()
                }),
                selected_attachments: instance.definition.attachments.clone(),
                execution_mode: instance.execution_mode,
            };

            let mut handles = Vec::new();
            for step_id in &ready {
                let step_def = instance.definition.steps.iter().find(|s| s.id == *step_id).cloned();

                if let Some(step_def) = step_def {
                    // Extract and clear any pending retry delay
                    let retry_delay = if let Some(state) = instance.step_states.get_mut(step_id) {
                        let delay = state.retry_delay_secs.take();
                        state.status = StepStatus::Running;
                        state.started_at_ms = Some(now_ms());
                        delay
                    } else {
                        None
                    };
                    // Capture existing child_agent_id for recovery resume.
                    // When a step is re-dispatched after daemon restart, the
                    // previously-spawned agent's ID is still on the step state.
                    let existing_agent_id =
                        instance.step_states.get(step_id).and_then(|s| s.child_agent_id.clone());
                    // Clear GoTo activation flag once the step starts executing
                    instance.goto_activated_steps.remove(step_id);

                    let executor = effective_executor.clone();
                    let mut ctx = exec_ctx.clone();
                    ctx.step_id = step_def.id.clone();
                    let expr_ctx = build_expression_context(&instance, Some(&step_def.id));
                    let emitter = self.event_emitter.clone();
                    let inst_id = instance.id;
                    // Check for test-case shadow override for this step
                    let shadow_override = instance.shadow_overrides.get(step_id).cloned();
                    // Clone loop state for ForEach/While control flow steps
                    let loop_state = instance.active_loops.get(step_id).cloned();
                    let semaphore = Arc::clone(&self.step_semaphore);

                    // Honor retry delay before acquiring semaphore so we don't
                    // hold a concurrency slot while sleeping.
                    let retry_delay_cloned = retry_delay;

                    handles.push(tokio::spawn(
                        async move {
                            if let Some(secs) = retry_delay_cloned {
                                tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
                            }

                            let permit =
                                semaphore.acquire_owned().await.expect("step semaphore closed");
                            let _permit = permit;

                            emitter
                                .emit(WorkflowEvent::StepStarted {
                                    instance_id: inst_id,
                                    step_id: step_def.id.clone(),
                                })
                                .await;

                            // Note: tokio::time::timeout drops the future on expiry but does
                            // not cancel externally-spawned side effects. StepExecutor
                            // implementations should be cancellation-aware (i.e. use
                            // select! or cancellation tokens) to ensure cleanup on timeout.
                            // If a test-case shadow override is provided for
                            // this step, skip real/shadow execution entirely and
                            // return the override as the step output.
                            let result = if let Some(overridden_output) = shadow_override {
                                StepOutcome::Completed { outputs: overridden_output }
                            } else if let Some(timeout) = step_def.timeout_secs {
                                match tokio::time::timeout(
                                    std::time::Duration::from_secs(timeout),
                                    execute_step(
                                        &step_def,
                                        &expr_ctx,
                                        executor.as_ref(),
                                        &ctx,
                                        loop_state,
                                        existing_agent_id.as_deref(),
                                    ),
                                )
                                .await
                                {
                                    Ok(outcome) => outcome,
                                    Err(_) => StepOutcome::Failed {
                                        error: format!("Step timed out after {timeout}s"),
                                    },
                                }
                            } else {
                                execute_step(
                                    &step_def,
                                    &expr_ctx,
                                    executor.as_ref(),
                                    &ctx,
                                    loop_state,
                                    existing_agent_id.as_deref(),
                                )
                                .await
                            };

                            (step_def.id.clone(), step_def, result)
                        }
                        .instrument(tracing::info_span!("service", service = "workflows")),
                    ));
                }
            }

            // Persist running state
            {
                instance.updated_at_ms = now_ms();
                let store = &self.store;
                store.update_instance(&instance)?;
            }

            // Await all step results
            let mut set_variable_tracker: HashMap<String, Vec<String>> = HashMap::new();
            for handle in handles {
                let (step_id, step_def, result) = handle
                    .await
                    .map_err(|e| WorkflowError::Other(format!("Step task join error: {e}")))?;

                match result {
                    StepOutcome::Completed { outputs } => {
                        // If this is a SetVariable step, merge outputs into instance variables.
                        if let StepType::Task { task: TaskDef::SetVariable { assignments } } =
                            &step_def.step_type
                        {
                            // Detect concurrent SetVariable conflicts within this batch.
                            for a in assignments {
                                set_variable_tracker
                                    .entry(a.variable.clone())
                                    .or_default()
                                    .push(step_id.clone());
                            }
                            for a in assignments {
                                if let Some(writers) = set_variable_tracker.get(&a.variable) {
                                    if writers.len() > 1 {
                                        warn!(
                                            instance_id = %instance.id,
                                            variable = %a.variable,
                                            steps = ?writers,
                                            "concurrent SetVariable conflict: multiple steps in the same batch write to the same variable; last-writer wins"
                                        );
                                    }
                                }
                            }
                            if let Err(e) = apply_variable_assignments(
                                &mut instance.variables,
                                assignments,
                                &outputs,
                            ) {
                                // Treat assignment application failure as a step error.
                                let outcome =
                                    handle_step_error(&step_id, &e, &step_def, &mut instance);
                                self.event_emitter
                                    .emit(WorkflowEvent::StepFailed {
                                        instance_id: instance.id,
                                        step_id: step_id.clone(),
                                        error: e.clone(),
                                    })
                                    .await;
                                match outcome {
                                    ErrorOutcome::WorkflowFailed => {
                                        instance.status = WorkflowStatus::Failed;
                                        instance.error = Some(e);
                                        instance.completed_at_ms = Some(now_ms());
                                        instance.updated_at_ms = now_ms();
                                        self.store.update_instance(&instance)?;
                                        self.event_emitter
                                            .emit(WorkflowEvent::InstanceFailed {
                                                instance_id: instance.id,
                                                error: instance.error.clone().unwrap_or_default(),
                                            })
                                            .await;
                                        self.killed.write().await.remove(&instance.id);
                                        self.instance_locks.write().await.remove(&instance.id);
                                        return Ok(());
                                    }
                                    ErrorOutcome::Retry | ErrorOutcome::Skipped => {}
                                    ErrorOutcome::GoTo => {
                                        goto_activation_count += 1;
                                        if goto_activation_count > MAX_GOTO_ACTIVATIONS {
                                            instance.status = WorkflowStatus::Failed;
                                            instance.error = Some(format!(
                                                "Workflow exceeded maximum GoTo activations ({MAX_GOTO_ACTIVATIONS}), possible infinite loop detected"
                                            ));
                                            instance.completed_at_ms = Some(now_ms());
                                            instance.updated_at_ms = now_ms();
                                            self.store.update_instance(&instance)?;
                                            self.event_emitter
                                                .emit(WorkflowEvent::InstanceFailed {
                                                    instance_id: instance.id,
                                                    error: instance
                                                        .error
                                                        .clone()
                                                        .unwrap_or_default(),
                                                })
                                                .await;
                                            self.killed.write().await.remove(&instance.id);
                                            self.instance_locks.write().await.remove(&instance.id);
                                            return Ok(());
                                        }
                                    }
                                }
                                continue;
                            }
                        }

                        if let Some(state) = instance.step_states.get_mut(&step_id) {
                            state.status = StepStatus::Completed;
                            state.completed_at_ms = Some(now_ms());
                            state.outputs = Some(outputs.clone());

                            // Track child agent spawned by InvokeAgent steps
                            if let StepType::Task {
                                task: TaskDef::InvokeAgent { .. } | TaskDef::InvokePrompt { .. },
                            } = &step_def.step_type
                            {
                                if let Some(agent_id) =
                                    outputs.get("agent_id").and_then(|v| v.as_str())
                                {
                                    state.child_agent_id = Some(agent_id.to_string());
                                }
                            }
                        }
                        self.event_emitter
                            .emit(WorkflowEvent::StepCompleted {
                                instance_id: instance.id,
                                step_id: step_id.clone(),
                                outputs: Some(outputs),
                            })
                            .await;
                    }
                    StepOutcome::WaitingOnInput { request_id, prompt, choices, allow_freeform } => {
                        if let Some(state) = instance.step_states.get_mut(&step_id) {
                            state.status = StepStatus::WaitingOnInput;
                            state.interaction_request_id = Some(request_id);
                            state.interaction_prompt = Some(prompt);
                            state.interaction_choices =
                                if choices.is_empty() { None } else { Some(choices) };
                            state.interaction_allow_freeform = Some(allow_freeform);
                        }
                        self.event_emitter
                            .emit(WorkflowEvent::StepWaiting {
                                instance_id: instance.id,
                                step_id: step_id.clone(),
                                waiting_type: "input".to_string(),
                            })
                            .await;
                    }
                    StepOutcome::WaitingOnEvent { subscription_id } => {
                        if let Some(state) = instance.step_states.get_mut(&step_id) {
                            state.status = StepStatus::WaitingOnEvent;
                            state.interaction_request_id = Some(subscription_id);
                            // Persist the timeout deadline so recovery can use the original
                            // expiry instead of recomputing from scratch.
                            if let StepType::Task {
                                task: TaskDef::EventGate { timeout_secs: Some(secs), .. },
                            } = &step_def.step_type
                            {
                                state.resume_at_ms =
                                    Some(now_ms().saturating_add(secs.saturating_mul(1000)));
                            }
                        }
                        self.event_emitter
                            .emit(WorkflowEvent::StepWaiting {
                                instance_id: instance.id,
                                step_id: step_id.clone(),
                                waiting_type: "event".to_string(),
                            })
                            .await;
                    }
                    StepOutcome::DelayScheduled { resume_at_ms } => {
                        if let Some(state) = instance.step_states.get_mut(&step_id) {
                            state.status = StepStatus::WaitingForDelay;
                            state.resume_at_ms = Some(resume_at_ms);
                        }
                        self.event_emitter
                            .emit(WorkflowEvent::StepWaiting {
                                instance_id: instance.id,
                                step_id: step_id.clone(),
                                waiting_type: "delay".to_string(),
                            })
                            .await;
                    }
                    StepOutcome::Failed { error } => {
                        let outcome = handle_step_error(&step_id, &error, &step_def, &mut instance);

                        self.event_emitter
                            .emit(WorkflowEvent::StepFailed {
                                instance_id: instance.id,
                                step_id: step_id.clone(),
                                error: error.clone(),
                            })
                            .await;

                        match outcome {
                            ErrorOutcome::WorkflowFailed => {
                                instance.status = WorkflowStatus::Failed;
                                instance.error = Some(error);
                                instance.completed_at_ms = Some(now_ms());
                                instance.updated_at_ms = now_ms();
                                let store = &self.store;
                                store.update_instance(&instance)?;
                                self.event_emitter
                                    .emit(WorkflowEvent::InstanceFailed {
                                        instance_id: instance.id,
                                        error: instance.error.clone().unwrap_or_default(),
                                    })
                                    .await;
                                self.killed.write().await.remove(&instance.id);
                                self.instance_locks.write().await.remove(&instance.id);
                                return Ok(());
                            }
                            ErrorOutcome::Retry | ErrorOutcome::Skipped => {
                                // Continue loop
                            }
                            ErrorOutcome::GoTo => {
                                goto_activation_count += 1;
                                if goto_activation_count > MAX_GOTO_ACTIVATIONS {
                                    instance.status = WorkflowStatus::Failed;
                                    instance.error = Some(format!(
                                        "Workflow exceeded maximum GoTo activations ({MAX_GOTO_ACTIVATIONS}), possible infinite loop detected"
                                    ));
                                    instance.completed_at_ms = Some(now_ms());
                                    instance.updated_at_ms = now_ms();
                                    self.store.update_instance(&instance)?;
                                    self.event_emitter
                                        .emit(WorkflowEvent::InstanceFailed {
                                            instance_id: instance.id,
                                            error: instance.error.clone().unwrap_or_default(),
                                        })
                                        .await;
                                    self.killed.write().await.remove(&instance.id);
                                    self.instance_locks.write().await.remove(&instance.id);
                                    return Ok(());
                                }
                            }
                        }
                    }
                    StepOutcome::BranchTaken { targets } => {
                        if let Some(state) = instance.step_states.get_mut(&step_id) {
                            state.status = StepStatus::Completed;
                            state.completed_at_ms = Some(now_ms());
                            state.outputs = Some(serde_json::json!({ "branch_targets": targets }));
                        }

                        // Non-selected targets are handled by skip_unreachable_steps
                        // at the start of the next run_loop iteration.

                        self.event_emitter
                            .emit(WorkflowEvent::StepCompleted {
                                instance_id: instance.id,
                                step_id: step_id.clone(),
                                outputs: None,
                            })
                            .await;
                    }
                    StepOutcome::EndWorkflow => {
                        if let Some(state) = instance.step_states.get_mut(&step_id) {
                            state.status = StepStatus::Completed;
                            state.completed_at_ms = Some(now_ms());
                        }
                        // Mark all pending steps as skipped
                        for state in instance.step_states.values_mut() {
                            if state.status == StepStatus::Pending {
                                state.status = StepStatus::Skipped;
                            }
                        }
                    }
                    StepOutcome::ChildWorkflowLaunched { child_id } => {
                        if let Some(state) = instance.step_states.get_mut(&step_id) {
                            state.status = StepStatus::Completed;
                            state.completed_at_ms = Some(now_ms());
                            state.child_workflow_id = Some(child_id);
                            state.outputs =
                                Some(serde_json::json!({ "child_workflow_id": child_id }));
                        }
                        self.event_emitter
                            .emit(WorkflowEvent::StepCompleted {
                                instance_id: instance.id,
                                step_id: step_id.clone(),
                                outputs: Some(serde_json::json!({ "child_workflow_id": child_id })),
                            })
                            .await;
                    }
                    StepOutcome::LoopIteration { body_steps } => {
                        // Create or update the loop state on the instance
                        let loop_state = if let Some(mut ls) =
                            instance.active_loops.remove(&step_id)
                        {
                            // Snapshot body outputs before resetting them
                            // (accumulate for preview if preview_count is active).
                            if ls.preview_results.is_some() && !ls.preview_paused {
                                let snapshot = collect_body_outputs(&instance, &body_steps);
                                if let Some(ref mut results) = ls.preview_results {
                                    results.push(snapshot);
                                }
                            }
                            // Continuing — increment iteration
                            ls.iteration += 1;
                            ls
                        } else {
                            // First iteration — initialize loop state from the step definition
                            let mut ls = LoopState {
                                iteration: 0,
                                collection: None,
                                item_var: None,
                                body_step_ids: body_steps.clone(),
                                max_iterations: None,
                                preview_paused: false,
                                preview_results: None,
                            };
                            // Populate ForEach-specific fields
                            if let StepType::ControlFlow {
                                control: ControlFlowDef::ForEach { collection, item_var, preview_count, .. },
                            } = &step_def.step_type
                            {
                                let expr_ctx = build_expression_context(&instance, Some(&step_id));
                                // Resolve the collection to store it
                                let coll_value = if is_pure_template(collection) {
                                    let path = collection
                                        .trim()
                                        .trim_start_matches("{{")
                                        .trim_end_matches("}}")
                                        .trim();
                                    resolve_path(path, &expr_ctx).unwrap_or(Value::Null)
                                } else {
                                    resolve_template(collection, &expr_ctx)
                                        .ok()
                                        .and_then(|s| serde_json::from_str(&s).ok())
                                        .unwrap_or(Value::Null)
                                };
                                if let Value::Array(arr) = coll_value {
                                    ls.collection = Some(arr);
                                }
                                ls.item_var = Some(item_var.clone());
                                // Initialize preview results accumulator if preview is active
                                if preview_count.map_or(false, |pc| pc > 0) {
                                    ls.preview_results = Some(Vec::new());
                                }
                            }
                            // Populate While-specific fields
                            if let StepType::ControlFlow {
                                control: ControlFlowDef::While { max_iterations, .. },
                            } = &step_def.step_type
                            {
                                ls.max_iterations = *max_iterations;
                            }
                            ls
                        };

                        // Set per-iteration variables (ForEach item injection)
                        if let Some(ref item_var) = loop_state.item_var {
                            if let Some(ref coll) = loop_state.collection {
                                if let Some(item) = coll.get(loop_state.iteration) {
                                    // Set variables.<item_var> and variables.<item_var>_index
                                    if let Value::Object(ref mut vars) = instance.variables {
                                        vars.insert(item_var.clone(), item.clone());
                                        vars.insert(
                                            format!("{}_index", item_var),
                                            Value::Number(serde_json::Number::from(
                                                loop_state.iteration,
                                            )),
                                        );
                                    }
                                }
                            }
                        }

                        info!(
                            step_id = %step_id,
                            iteration = loop_state.iteration,
                            "Loop iteration starting"
                        );

                        // Store loop state
                        instance.active_loops.insert(step_id.clone(), loop_state);

                        // Reset all body step states to Pending
                        for body_id in &body_steps {
                            if let Some(state) = instance.step_states.get_mut(body_id) {
                                state.status = StepStatus::Pending;
                                state.outputs = None;
                                state.error = None;
                                state.started_at_ms = None;
                                state.completed_at_ms = None;
                                state.retry_count = 0;
                                state.retry_delay_secs = None;
                            }
                        }

                        // Set the loop control step to LoopWaiting
                        if let Some(state) = instance.step_states.get_mut(&step_id) {
                            state.status = StepStatus::LoopWaiting;
                        }
                    }
                    StepOutcome::LoopComplete => {
                        // Read iteration count before removing loop state
                        let iteration_count = instance
                            .active_loops
                            .get(&step_id)
                            .map(|ls| ls.iteration as u64)
                            .unwrap_or(0);

                        // Remove the loop state
                        instance.active_loops.remove(&step_id);

                        // Mark the loop control step as Completed
                        if let Some(state) = instance.step_states.get_mut(&step_id) {
                            state.status = StepStatus::Completed;
                            state.completed_at_ms = Some(now_ms());

                            state.outputs = Some(serde_json::json!({
                                "iteration_count": iteration_count,
                                "completed": true,
                            }));
                        }

                        info!(step_id = %step_id, "Loop completed");

                        self.event_emitter
                            .emit(WorkflowEvent::StepCompleted {
                                instance_id: instance.id,
                                step_id: step_id.clone(),
                                outputs: None,
                            })
                            .await;
                    }
                    StepOutcome::LoopPreviewPause { body_steps, completed, total } => {
                        // Snapshot current body step outputs before pausing
                        let body_snapshot = collect_body_outputs(&instance, &body_steps);

                        // Update loop state: mark preview as fired
                        if let Some(ls) = instance.active_loops.get_mut(&step_id) {
                            ls.preview_paused = true;
                            // Accumulate preview results
                            let results = ls.preview_results.get_or_insert_with(Vec::new);
                            results.push(body_snapshot);
                        }

                        // Create feedback request for user review
                        let remaining = total - completed;
                        let prompt = format!(
                            "Processed {completed}/{total} items. Review the results and decide whether to continue with the remaining {remaining} items.",
                        );
                        let choices_owned = vec![
                            "Continue All".to_string(),
                            "Abort".to_string(),
                        ];

                        match effective_executor
                            .create_feedback_request(
                                instance.id,
                                &step_id,
                                &prompt,
                                Some(&choices_owned),
                                false,
                                &exec_ctx,
                            )
                            .await
                        {
                            Ok(request_id) => {
                                if let Some(state) = instance.step_states.get_mut(&step_id) {
                                    state.status = StepStatus::WaitingOnInput;
                                    state.interaction_request_id = Some(request_id);
                                    state.interaction_prompt = Some(prompt);
                                    state.interaction_choices = Some(vec![
                                        "Continue All".to_string(),
                                        "Abort".to_string(),
                                    ]);
                                    state.interaction_allow_freeform = Some(false);
                                }
                                self.event_emitter
                                    .emit(WorkflowEvent::StepWaiting {
                                        instance_id: instance.id,
                                        step_id: step_id.clone(),
                                        waiting_type: "input".to_string(),
                                    })
                                    .await;
                            }
                            Err(e) => {
                                // Feedback request failed — fall through to continue
                                // the loop rather than stranding the instance.
                                warn!(
                                    step_id = %step_id,
                                    error = %e,
                                    "Preview feedback request failed, continuing loop"
                                );
                                // Reset body steps and set LoopWaiting so the
                                // pre-pass will advance to the next iteration.
                                for body_id in &body_steps {
                                    if let Some(state) = instance.step_states.get_mut(body_id) {
                                        state.status = StepStatus::Pending;
                                        state.outputs = None;
                                        state.error = None;
                                        state.started_at_ms = None;
                                        state.completed_at_ms = None;
                                        state.retry_count = 0;
                                        state.retry_delay_secs = None;
                                    }
                                }
                                if let Some(state) = instance.step_states.get_mut(&step_id) {
                                    state.status = StepStatus::LoopWaiting;
                                }
                            }
                        }
                    }
                }
            }

            // Persist after all steps in this batch
            {
                instance.updated_at_ms = now_ms();
                let store = &self.store;
                store.update_instance(&instance)?;
            }
        }
    }

    async fn load_instance(&self, id: i64) -> Result<WorkflowInstance, WorkflowError> {
        let store = &self.store;
        store.get_instance(id)?.ok_or(WorkflowError::InstanceNotFound { id })
    }

    // SPEC-GAP: Recovery resets running steps to Pending and re-executes.
    // The spec describes event-sourced replay but the implementation uses
    // mutable-state recovery. See crates/hive-workflow/DESIGN_NOTES.md.
    /// Recover orphaned workflow instances after a daemon restart.
    ///
    /// Scans the store for instances whose status is `Running`,
    /// `WaitingOnInput`, or `WaitingOnEvent`.  For `Running` instances any
    /// steps that were mid-execution (status `Running`) are reset to `Pending`
    /// and `run_loop` is re-entered.  For waiting instances the event/input
    /// gates are re-registered via the step executor.
    pub async fn recover_instances(
        &self,
    ) -> Result<Vec<tokio::task::JoinHandle<()>>, WorkflowError> {
        let recoverable_statuses = vec![
            WorkflowStatus::Running,
            WorkflowStatus::WaitingOnInput,
            WorkflowStatus::WaitingOnEvent,
        ];
        let filter = InstanceFilter { statuses: recoverable_statuses, ..Default::default() };
        let result = self.store.list_instances(&filter)?;

        if result.items.is_empty() {
            return Ok(Vec::new());
        }

        info!("recovering {} orphaned workflow instance(s)", result.items.len());

        let mut handles = Vec::new();

        for summary in &result.items {
            let inst_id = summary.id;

            // Skip instances whose lock is already held — they are actively
            // being launched by `launch_background` and haven't entered
            // `run_loop` yet.
            let lock = self.instance_lock(inst_id).await;
            if lock.try_lock().is_err() {
                debug!(instance_id = %inst_id, "instance lock held, skipping recovery (likely mid-launch)");
                continue;
            }
            // We only needed the try_lock to probe; the spawned task will
            // acquire its own lock below.
            drop(lock);

            let mut instance = match self.store.get_instance(inst_id)? {
                Some(i) => i,
                None => {
                    warn!(instance_id = %inst_id, "instance disappeared before recovery");
                    continue;
                }
            };

            match instance.status {
                WorkflowStatus::Running => {
                    // Reset all Running steps to Pending so run_loop
                    // re-dispatches them. The `child_agent_id` field on each
                    // StepState is preserved — when run_loop picks up the
                    // step, it passes `existing_agent_id` into invoke_agent,
                    // which resumes the already-alive agent instead of
                    // spawning a duplicate.
                    for (_step_id, state) in instance.step_states.iter_mut() {
                        if state.status == StepStatus::Running {
                            state.status = StepStatus::Pending;
                            state.started_at_ms = None;
                        }
                    }
                    instance.updated_at_ms = now_ms();
                    self.store.update_instance(&instance)?;

                    info!(
                        instance_id = %inst_id,
                        "re-entering run_loop for recovered instance"
                    );

                    let engine = self.clone_engine();
                    let handle = tokio::spawn(async move {
                        let lock = engine.instance_lock(inst_id).await;
                        let _guard = lock.lock().await;

                        let instance = match engine.load_instance(inst_id).await {
                            Ok(i) => i,
                            Err(e) => {
                                warn!(instance_id = %inst_id, error = %e, "failed to reload instance for recovery");
                                engine.instance_locks.write().await.remove(&inst_id);
                                return;
                            }
                        };
                        if let Err(e) = engine.run_loop(instance).await {
                            warn!(instance_id = %inst_id, error = %e, "recovered instance failed");
                            match engine.store.get_instance(inst_id) {
                                Ok(Some(mut inst)) if !matches!(
                                    inst.status,
                                    WorkflowStatus::Completed | WorkflowStatus::Failed | WorkflowStatus::Killed
                                ) => {
                                    inst.status = WorkflowStatus::Failed;
                                    inst.error = Some(format!("{e}"));
                                    inst.completed_at_ms = Some(now_ms());
                                    inst.updated_at_ms = now_ms();
                                    if let Err(persist_err) = engine.store.update_instance(&inst) {
                                        warn!(instance_id = %inst_id, error = %persist_err, "failed to persist Failed status after recovery error");
                                    }
                                    engine.event_emitter.emit(WorkflowEvent::InstanceFailed {
                                        instance_id: inst_id,
                                        error: inst.error.clone().unwrap_or_default(),
                                    }).await;
                                }
                                _ => {}
                            }
                            engine.instance_locks.write().await.remove(&inst_id);
                        }
                    }.instrument(tracing::info_span!("service", service = "workflows")));
                    handles.push(handle);
                }
                WorkflowStatus::WaitingOnInput | WorkflowStatus::WaitingOnEvent => {
                    // Find the waiting step and log it.
                    let mut has_delay_steps = false;
                    for (step_id, state) in &instance.step_states {
                        if state.status == StepStatus::WaitingOnInput
                            || state.status == StepStatus::WaitingOnEvent
                        {
                            info!(
                                instance_id = %inst_id,
                                step_id = %step_id,
                                "re-registering gate for waiting instance"
                            );
                        }
                        if state.status == StepStatus::WaitingForDelay {
                            has_delay_steps = true;
                            info!(
                                instance_id = %inst_id,
                                step_id = %step_id,
                                resume_at_ms = ?state.resume_at_ms,
                                "re-spawning delay timer for waiting instance"
                            );
                        }
                    }
                    // Re-spawn delay timers for steps that were waiting for a delay
                    if has_delay_steps {
                        self.spawn_delay_timers(&instance);
                    }
                    // No further re-registration is needed here. The TriggerManager
                    // and interaction layer are responsible for scanning the store
                    // for instances in WaitingOnInput/WaitingOnEvent status and
                    // routing incoming events/responses to them via respond_to_gate
                    // and respond_to_event. This depends on those external components
                    // performing a startup scan.
                }
                _ => {}
            }
        }

        Ok(handles)
    }
}

// ---------------------------------------------------------------------------
// Ready-set computation
// ---------------------------------------------------------------------------

/// Forward reachability analysis from trigger steps.
///
/// Returns the set of step IDs that are reachable given the current branch
/// decisions and loop states.  Each step type defines its own edge rules via
/// [`StepDef::reachable_successors`], keeping the traversal generic.
fn compute_reachable_steps(instance: &WorkflowInstance) -> HashSet<String> {
    let def = &instance.definition;
    let mut reachable = HashSet::new();

    // Seed: trigger steps are always reachable
    for step in &def.steps {
        if matches!(step.step_type, StepType::Trigger { .. }) {
            reachable.insert(step.id.clone());
        }
    }

    // GoTo-activated steps (not yet started) are unconditionally reachable
    for id in &instance.goto_activated_steps {
        reachable.insert(id.clone());
    }

    // Steps that have already been reached (Running, Completed, Failed,
    // LoopWaiting, etc.) propagate reachability to their successors.
    // This handles the case where a GoTo target has started executing
    // (and was removed from goto_activated_steps) — it still makes its
    // successors reachable.
    for step in &def.steps {
        if let Some(state) = instance.step_states.get(&step.id) {
            if !matches!(state.status, StepStatus::Pending | StepStatus::Skipped) {
                reachable.insert(step.id.clone());
            }
        }
    }

    // Fixed-point: propagate reachability through open edges
    let mut changed = true;
    while changed {
        changed = false;
        for step in &def.steps {
            if !reachable.contains(&step.id) {
                continue;
            }
            let state = instance.step_states.get(&step.id);
            for successor in step.reachable_successors(state) {
                if reachable.insert(successor.to_string()) {
                    changed = true;
                }
            }
        }
    }

    reachable
}

/// Mark pending steps that are unreachable (given current branch/loop
/// decisions) as Skipped.  Also re-activate previously unreachable steps
/// that became reachable (e.g. via GoTo opening a new path).
///
/// Called once per `run_loop` iteration before computing ready steps.
fn skip_unreachable_steps(instance: &mut WorkflowInstance) {
    let reachable = compute_reachable_steps(instance);
    for (step_id, state) in &mut instance.step_states {
        if state.status == StepStatus::Pending && !reachable.contains(step_id.as_str()) {
            state.status = StepStatus::Skipped;
        } else if state.status == StepStatus::Skipped
            && reachable.contains(step_id.as_str())
            && state.completed_at_ms.is_none()
        {
            // A step that was Skipped as unreachable is now reachable (GoTo
            // activated a new path).  Reset to Pending so it can be scheduled.
            // Steps Skipped by error handling have completed_at_ms set, so
            // they are not affected.
            state.status = StepStatus::Pending;
        }
    }
}

/// Compute steps that are ready to execute.
///
/// **Prerequisite:** [`skip_unreachable_steps`] must have run first so that
/// unreachable pending steps are already Skipped.  This function only needs
/// to check predecessor completion — no branch-reachability filter required.
///
/// For loop body steps, `LoopWaiting` on the loop parent counts as "done"
/// (it means the iteration was kicked off).
fn compute_ready_steps(instance: &WorkflowInstance) -> Vec<String> {
    let def = &instance.definition;

    // Build predecessor map: step_id -> set of predecessor step_ids
    let mut predecessors: HashMap<&str, HashSet<&str>> = HashMap::new();
    for step in &def.steps {
        predecessors.entry(step.id.as_str()).or_default();
    }

    // Track which steps are body steps of a loop
    let mut loop_body_parents: HashMap<&str, &str> = HashMap::new();

    for step in &def.steps {
        // `next` edges
        for next_id in &step.next {
            predecessors.entry(next_id.as_str()).or_default().insert(step.id.as_str());
        }
        // Control flow edges (branch then/else)
        if let StepType::ControlFlow {
            control: ControlFlowDef::Branch { ref then, ref else_branch, .. },
        } = step.step_type
        {
            for id in then.iter().chain(else_branch.iter()) {
                predecessors.entry(id.as_str()).or_default().insert(step.id.as_str());
            }
        }
        // ForEach/While body edges
        if let StepType::ControlFlow {
            control:
                ControlFlowDef::ForEach { ref body, .. } | ControlFlowDef::While { ref body, .. },
        } = step.step_type
        {
            for body_id in body {
                predecessors.entry(body_id.as_str()).or_default().insert(step.id.as_str());
                loop_body_parents.insert(body_id.as_str(), step.id.as_str());
            }
        }
    }

    let mut ready = Vec::new();
    for step in &def.steps {
        let state = instance.step_states.get(&step.id);
        let is_pending = state.is_none_or(|s| s.status == StepStatus::Pending);
        if !is_pending {
            continue;
        }

        // Steps activated by GoTo bypass normal predecessor checks
        if instance.goto_activated_steps.contains(&step.id) {
            ready.push(step.id.clone());
            continue;
        }

        // Check all predecessors are completed (or skipped, or failed-via-GoTo)
        let preds = predecessors.get(step.id.as_str()).cloned().unwrap_or_default();
        let all_done = preds.iter().all(|pred_id| {
            // Steps that failed but triggered GoTo count as "done"
            if instance.goto_source_steps.contains(*pred_id) {
                return true;
            }
            // For loop body steps: LoopWaiting on the loop parent counts as "done"
            if loop_body_parents.get(step.id.as_str()) == Some(pred_id) {
                return instance
                    .step_states
                    .get(*pred_id)
                    .is_some_and(|s| s.status == StepStatus::LoopWaiting);
            }
            instance
                .step_states
                .get(*pred_id)
                .is_some_and(|s| matches!(s.status, StepStatus::Completed | StepStatus::Skipped))
        });

        if all_done || preds.is_empty() {
            ready.push(step.id.clone());
        }
    }

    ready
}

// ---------------------------------------------------------------------------
// Step execution
// ---------------------------------------------------------------------------

enum StepOutcome {
    Completed {
        outputs: Value,
    },
    WaitingOnInput {
        request_id: String,
        prompt: String,
        choices: Vec<String>,
        allow_freeform: bool,
    },
    WaitingOnEvent {
        subscription_id: String,
    },
    /// Delay step should resume at the specified epoch millisecond time.
    DelayScheduled {
        resume_at_ms: u64,
    },
    Failed {
        error: String,
    },
    BranchTaken {
        targets: Vec<String>,
    },
    EndWorkflow,
    ChildWorkflowLaunched {
        child_id: i64,
    },
    /// A loop iteration should begin (or continue). Body steps will be reset to Pending.
    LoopIteration {
        body_steps: Vec<String>,
    },
    /// The loop has finished — proceed to the step's `next` successors.
    LoopComplete,
    /// The loop has reached its preview checkpoint and should pause for user review.
    LoopPreviewPause {
        body_steps: Vec<String>,
        completed: usize,
        total: usize,
    },
}

async fn execute_step(
    step_def: &StepDef,
    expr_ctx: &ExpressionContext,
    executor: &dyn StepExecutor,
    ctx: &ExecutionContext,
    loop_state: Option<LoopState>,
    existing_agent_id: Option<&str>,
) -> StepOutcome {
    match &step_def.step_type {
        StepType::Trigger { .. } => {
            // Triggers are pre-completed at launch time
            StepOutcome::Completed { outputs: Value::Null }
        }
        StepType::Task { task } => {
            execute_task(task, step_def, expr_ctx, executor, ctx, existing_agent_id).await
        }
        StepType::ControlFlow { control } => {
            execute_control_flow(control, step_def, expr_ctx, loop_state).await
        }
    }
}

async fn execute_task(
    task: &TaskDef,
    step_def: &StepDef,
    expr_ctx: &ExpressionContext,
    executor: &dyn StepExecutor,
    ctx: &ExecutionContext,
    existing_agent_id: Option<&str>,
) -> StepOutcome {
    let result = match task {
        TaskDef::CallTool { tool_id, arguments } => {
            let resolved_tool_id = match resolve_template(tool_id, expr_ctx) {
                Ok(v) => v,
                Err(e) => return StepOutcome::Failed { error: e.to_string() },
            };
            // Validate tool_id contains only safe characters
            if !resolved_tool_id
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '-' | ':' | '/'))
            {
                return StepOutcome::Failed {
                    error: format!(
                        "invalid tool_id after template resolution: '{resolved_tool_id}'"
                    ),
                };
            }
            let resolved_args = match resolve_arguments(arguments, expr_ctx) {
                Ok(v) => {
                    tracing::debug!(
                        step_id = %step_def.id,
                        tool_id = %resolved_tool_id,
                        raw_args = ?arguments,
                        resolved_args = %v,
                        "resolved tool call arguments"
                    );
                    v
                }
                Err(e) => return StepOutcome::Failed { error: e.to_string() },
            };
            executor.call_tool(&resolved_tool_id, resolved_args, ctx).await
        }
        TaskDef::InvokeAgent {
            persona_id,
            task,
            async_exec,
            timeout_secs,
            permissions,
            attachments,
            agent_name,
        } => {
            let resolved_task = match resolve_template_for_prompt(task, expr_ctx) {
                Ok(v) => v,
                Err(e) => return StepOutcome::Failed { error: e.to_string() },
            };
            // Resolve selected attachments from the context's definition
            let mut step_ctx = ctx.clone();
            if !attachments.is_empty() {
                let att_ids: std::collections::HashSet<&str> =
                    attachments.iter().map(|s| s.as_str()).collect();
                // selected_attachments on the context are set by the service
                // layer at launch time from the full definition.  Here we just
                // need to filter the full list down to the ones this step wants.
                // If the service layer already populated the full list we filter;
                // otherwise we leave it empty (the service layer will resolve).
                if step_ctx.selected_attachments.is_empty() {
                    // Not pre-populated — nothing more we can do at this layer.
                } else {
                    step_ctx.selected_attachments.retain(|a| att_ids.contains(a.id.as_str()));
                }
            } else {
                // No attachments selected for this step
                let mut step_ctx_inner = step_ctx.clone();
                step_ctx_inner.selected_attachments = vec![];
                step_ctx = step_ctx_inner;
            }
            executor
                .invoke_agent(
                    persona_id,
                    &resolved_task,
                    *async_exec,
                    *timeout_secs,
                    permissions,
                    agent_name.as_deref(),
                    existing_agent_id,
                    &step_ctx,
                )
                .await
        }
        TaskDef::SignalAgent { target, content } => {
            let resolved_content = match resolve_template(content, expr_ctx) {
                Ok(v) => v,
                Err(e) => return StepOutcome::Failed { error: e.to_string() },
            };
            executor.signal_agent(target, &resolved_content, ctx).await
        }
        TaskDef::FeedbackGate { prompt, choices, allow_freeform } => {
            let resolved_prompt = match resolve_template(prompt, expr_ctx) {
                Ok(v) => v,
                Err(e) => return StepOutcome::Failed { error: e.to_string() },
            };
            match executor
                .create_feedback_request(
                    ctx.instance_id,
                    &step_def.id,
                    &resolved_prompt,
                    choices.as_deref(),
                    *allow_freeform,
                    ctx,
                )
                .await
            {
                Ok(request_id) => {
                    return StepOutcome::WaitingOnInput {
                        request_id,
                        prompt: resolved_prompt,
                        choices: choices.clone().unwrap_or_default(),
                        allow_freeform: *allow_freeform,
                    }
                }
                Err(e) => return StepOutcome::Failed { error: e },
            }
        }
        TaskDef::EventGate { topic, filter, timeout_secs } => {
            let resolved_topic = match resolve_template(topic, expr_ctx) {
                Ok(v) => v,
                Err(e) => return StepOutcome::Failed { error: e.to_string() },
            };
            let resolved_filter = match filter {
                Some(f) => match resolve_template(f, expr_ctx) {
                    Ok(v) => Some(v),
                    Err(e) => return StepOutcome::Failed { error: e.to_string() },
                },
                None => None,
            };
            match executor
                .register_event_gate(
                    ctx.instance_id,
                    &step_def.id,
                    &resolved_topic,
                    resolved_filter.as_deref(),
                    *timeout_secs,
                    ctx,
                )
                .await
            {
                Ok(subscription_id) => return StepOutcome::WaitingOnEvent { subscription_id },
                Err(e) => return StepOutcome::Failed { error: e },
            }
        }
        TaskDef::LaunchWorkflow { workflow_name, inputs } => {
            let resolved_inputs = match resolve_arguments(inputs, expr_ctx) {
                Ok(v) => v,
                Err(e) => return StepOutcome::Failed { error: e.to_string() },
            };
            match executor.launch_workflow(workflow_name, resolved_inputs, ctx).await {
                Ok(child_id) => return StepOutcome::ChildWorkflowLaunched { child_id },
                Err(e) => return StepOutcome::Failed { error: e },
            }
        }
        TaskDef::Delay { duration_secs } => {
            let resume_at_ms = now_ms().saturating_add(duration_secs.saturating_mul(1000));
            return StepOutcome::DelayScheduled { resume_at_ms };
        }
        TaskDef::ScheduleTask { schedule } => executor
            .schedule_task(schedule, ctx)
            .await
            .map(|id| serde_json::json!({ "task_id": id })),
        TaskDef::SetVariable { assignments } => {
            // Resolve all assignments and return as outputs.
            // The actual variable merge happens in the step completion handler.
            let mut resolved = serde_json::Map::new();
            for a in assignments {
                let val = if is_pure_template(&a.value) {
                    let path =
                        a.value.trim().trim_start_matches("{{").trim_end_matches("}}").trim();
                    match resolve_path(path, expr_ctx) {
                        Ok(v) => v,
                        Err(e) => {
                            return StepOutcome::Failed {
                                error: format!("Failed to resolve '{}': {}", a.variable, e),
                            }
                        }
                    }
                } else {
                    match resolve_template(&a.value, expr_ctx) {
                        Ok(v) => Value::String(v),
                        Err(e) => {
                            return StepOutcome::Failed {
                                error: format!("Failed to resolve '{}': {}", a.variable, e),
                            }
                        }
                    }
                };
                resolved.insert(
                    a.variable.clone(),
                    serde_json::json!({
                        "value": val,
                        "operation": a.operation,
                    }),
                );
            }
            // Skip the generic output mapping — return directly.
            return StepOutcome::Completed { outputs: Value::Object(resolved) };
        }
        TaskDef::InvokePrompt {
            persona_id,
            prompt_id,
            parameters,
            async_exec,
            timeout_secs,
            permissions,
            target_agent_id,
            auto_create,
            agent_name,
        } => {
            // Resolve parameter values via workflow expressions
            let mut resolved_params = serde_json::Map::new();
            for (k, v) in parameters {
                let resolved = if is_pure_template(v) {
                    let path = v.trim().trim_start_matches("{{").trim_end_matches("}}").trim();
                    match resolve_path(path, expr_ctx) {
                        Ok(val) => val,
                        Err(e) => {
                            return StepOutcome::Failed {
                                error: format!("Failed to resolve parameter '{}': {}", k, e),
                            }
                        }
                    }
                } else {
                    match resolve_template(v, expr_ctx) {
                        Ok(val) => Value::String(val),
                        Err(e) => {
                            return StepOutcome::Failed {
                                error: format!("Failed to resolve parameter '{}': {}", k, e),
                            }
                        }
                    }
                };
                resolved_params.insert(k.clone(), resolved);
            }

            // Render the prompt template via the executor (service layer)
            let rendered = match executor
                .render_prompt_template(persona_id, prompt_id, Value::Object(resolved_params), ctx)
                .await
            {
                Ok(text) => text,
                Err(e) => {
                    return StepOutcome::Failed {
                        error: format!("Failed to render prompt template: {}", e),
                    }
                }
            };

            // Invoke an agent with the rendered text as the task, or signal
            // an existing agent if target_agent_id is set.
            if let Some(target_expr) = target_agent_id {
                let resolved_id = match resolve_template(target_expr, expr_ctx) {
                    Ok(v) => v,
                    Err(e) => {
                        return StepOutcome::Failed {
                            error: format!("Failed to resolve target_agent_id: {}", e),
                        }
                    }
                };
                if resolved_id.trim().is_empty() {
                    if *auto_create {
                        // Empty target with auto_create — skip signal, spawn new agent
                        tracing::info!("target_agent_id is empty, auto-creating new agent");
                        executor
                            .invoke_agent(
                                persona_id,
                                &rendered,
                                *async_exec,
                                *timeout_secs,
                                permissions,
                                agent_name.as_deref(),
                                existing_agent_id,
                                ctx,
                            )
                            .await
                    } else {
                        Err("target_agent_id resolved to an empty string".to_string())
                    }
                } else {
                    let signal_result = executor
                        .signal_agent(
                            &SignalTarget::Agent { agent_id: resolved_id.clone() },
                            &rendered,
                            ctx,
                        )
                        .await;

                    // If signalling failed and auto_create is enabled, fall back
                    // to spawning a fresh agent with the rendered prompt.
                    match signal_result {
                        Ok(_) => {
                            // Signal succeeded — now wait for the agent to
                            // complete before marking the step done.
                            executor.wait_for_agent(&resolved_id, *timeout_secs, ctx).await
                        }
                        Err(e) if *auto_create => {
                            tracing::info!("target agent not found, auto-creating new agent: {e}");
                            executor
                                .invoke_agent(
                                    persona_id,
                                    &rendered,
                                    *async_exec,
                                    *timeout_secs,
                                    permissions,
                                    agent_name.as_deref(),
                                    existing_agent_id,
                                    ctx,
                                )
                                .await
                        }
                        Err(e) => Err(e),
                    }
                }
            } else {
                executor
                    .invoke_agent(
                        persona_id,
                        &rendered,
                        *async_exec,
                        *timeout_secs,
                        permissions,
                        agent_name.as_deref(),
                        existing_agent_id,
                        ctx,
                    )
                    .await
            }
        }
    };

    match result {
        Ok(result_value) => {
            // Resolve output mappings
            let mut output_ctx = expr_ctx.clone();
            output_ctx.current_result = result_value.clone();
            match resolve_output_map(&step_def.outputs, &output_ctx) {
                Ok(outputs) => StepOutcome::Completed {
                    outputs: if step_def.outputs.is_empty() { result_value } else { outputs },
                },
                Err(e) => StepOutcome::Failed { error: format!("Output mapping failed: {e}") },
            }
        }
        Err(e) => StepOutcome::Failed { error: e },
    }
}

async fn execute_control_flow(
    control: &ControlFlowDef,
    _step_def: &StepDef,
    expr_ctx: &ExpressionContext,
    loop_state: Option<LoopState>,
) -> StepOutcome {
    match control {
        ControlFlowDef::Branch { condition, then, else_branch } => {
            match evaluate_condition(condition, expr_ctx) {
                Ok(true) => StepOutcome::BranchTaken { targets: then.clone() },
                Ok(false) => StepOutcome::BranchTaken { targets: else_branch.clone() },
                Err(e) => StepOutcome::Failed { error: format!("Branch condition error: {e}") },
            }
        }
        ControlFlowDef::EndWorkflow => StepOutcome::EndWorkflow,
        ControlFlowDef::ForEach { collection, item_var: _, body, preview_count } => {
            // Resolve the collection on first iteration; reuse from loop_state after.
            // `next_iteration` is the index that will be used IF we proceed.
            let (resolved_collection, next_iteration) = if let Some(ref ls) = loop_state {
                // Continuing — the handler already ran iteration `ls.iteration`.
                // The *next* iteration index is one higher.
                let coll = ls.collection.clone().unwrap_or_default();
                (coll, ls.iteration + 1)
            } else {
                // First evaluation — resolve the collection expression
                let coll_value = if is_pure_template(collection) {
                    let path =
                        collection.trim().trim_start_matches("{{").trim_end_matches("}}").trim();
                    match resolve_path(path, expr_ctx) {
                        Ok(v) => v,
                        Err(e) => {
                            return StepOutcome::Failed {
                                error: format!("ForEach collection resolution error: {e}"),
                            };
                        }
                    }
                } else {
                    match resolve_template(collection, expr_ctx) {
                        Ok(s) => serde_json::from_str(&s).unwrap_or(Value::String(s)),
                        Err(e) => {
                            return StepOutcome::Failed {
                                error: format!("ForEach collection resolution error: {e}"),
                            };
                        }
                    }
                };

                let coll = match coll_value {
                    Value::Array(arr) => arr,
                    Value::Null => Vec::new(),
                    other => {
                        return StepOutcome::Failed {
                            error: format!(
                                "ForEach collection must be an array, got: {}",
                                other_type_name(&other)
                            ),
                        };
                    }
                };
                (coll, 0)
            };

            if next_iteration < resolved_collection.len() {
                // Check preview checkpoint: pause after preview_count items
                // if we haven't paused yet.
                if let Some(pc) = preview_count {
                    if *pc > 0
                        && next_iteration == *pc as usize
                        && !loop_state.as_ref().map_or(false, |ls| ls.preview_paused)
                    {
                        return StepOutcome::LoopPreviewPause {
                            body_steps: body.clone(),
                            completed: *pc as usize,
                            total: resolved_collection.len(),
                        };
                    }
                }
                // There are more items — start/continue iteration
                StepOutcome::LoopIteration { body_steps: body.clone() }
            } else {
                // All items processed (or collection was empty)
                StepOutcome::LoopComplete
            }
        }
        ControlFlowDef::While { condition, max_iterations, body } => {
            let next_iteration = loop_state.as_ref().map_or(0, |ls| ls.iteration + 1);

            // Check max_iterations safety limit (applies default if not specified)
            let effective_max = max_iterations.unwrap_or(DEFAULT_WHILE_MAX_ITERATIONS);
            if next_iteration >= effective_max as usize {
                info!(
                    next_iteration,
                    max_iterations = effective_max,
                    "While loop hit max_iterations limit"
                );
                return StepOutcome::LoopComplete;
            }

            // Evaluate the condition
            match evaluate_condition(condition, expr_ctx) {
                Ok(true) => StepOutcome::LoopIteration { body_steps: body.clone() },
                Ok(false) => StepOutcome::LoopComplete,
                Err(e) => StepOutcome::Failed { error: format!("While condition error: {e}") },
            }
        }
    }
}

fn other_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Collect body step outputs into a single JSON object for preview snapshots.
fn collect_body_outputs(instance: &WorkflowInstance, body_steps: &[String]) -> Value {
    let mut map = serde_json::Map::new();
    for body_id in body_steps {
        if let Some(state) = instance.step_states.get(body_id) {
            if let Some(ref outputs) = state.outputs {
                map.insert(body_id.clone(), outputs.clone());
            }
        }
    }
    Value::Object(map)
}

// ---------------------------------------------------------------------------
// Error handling
// ---------------------------------------------------------------------------

enum ErrorOutcome {
    WorkflowFailed,
    Retry,
    Skipped,
    GoTo,
}

fn handle_step_error(
    step_id: &str,
    error: &str,
    step_def: &StepDef,
    instance: &mut WorkflowInstance,
) -> ErrorOutcome {
    match &step_def.on_error {
        None => {
            if let Some(state) = instance.step_states.get_mut(step_id) {
                state.status = StepStatus::Failed;
                state.error = Some(error.to_string());
                state.completed_at_ms = Some(now_ms());
            }
            ErrorOutcome::WorkflowFailed
        }
        Some(ErrorStrategy::FailWorkflow { message }) => {
            if let Some(state) = instance.step_states.get_mut(step_id) {
                state.status = StepStatus::Failed;
                state.error = Some(message.as_ref().cloned().unwrap_or_else(|| error.to_string()));
                state.completed_at_ms = Some(now_ms());
            }
            ErrorOutcome::WorkflowFailed
        }
        Some(ErrorStrategy::Retry { max_retries, delay_secs }) => {
            if let Some(state) = instance.step_states.get_mut(step_id) {
                if state.retry_count < *max_retries {
                    state.retry_count += 1;
                    state.status = StepStatus::Pending; // will be re-scheduled
                    state.error = None;
                    if *delay_secs > 0 {
                        state.retry_delay_secs = Some(*delay_secs);
                    }
                    info!(
                        "Retrying step {} (attempt {}/{})",
                        step_id, state.retry_count, max_retries
                    );
                    ErrorOutcome::Retry
                } else {
                    state.status = StepStatus::Failed;
                    state.error = Some(error.to_string());
                    state.completed_at_ms = Some(now_ms());
                    ErrorOutcome::WorkflowFailed
                }
            } else {
                ErrorOutcome::WorkflowFailed
            }
        }
        Some(ErrorStrategy::Skip { default_output }) => {
            if let Some(state) = instance.step_states.get_mut(step_id) {
                state.status = StepStatus::Skipped;
                state.outputs = default_output.clone();
                state.completed_at_ms = Some(now_ms());
            }
            ErrorOutcome::Skipped
        }
        Some(ErrorStrategy::GoTo { step_id: target }) => {
            if let Some(state) = instance.step_states.get_mut(step_id) {
                state.status = StepStatus::Failed;
                state.error = Some(error.to_string());
                state.completed_at_ms = Some(now_ms());
            }
            // Mark target as pending so it becomes ready
            if let Some(target_state) = instance.step_states.get_mut(target) {
                target_state.status = StepStatus::Pending;
                target_state.started_at_ms = None;
                target_state.completed_at_ms = None;
                target_state.outputs = None;
                target_state.error = None;
                target_state.retry_count = 0;
                target_state.retry_delay_secs = None;
                target_state.child_workflow_id = None;
                target_state.child_agent_id = None;
                target_state.interaction_request_id = None;
                target_state.interaction_prompt = None;
                target_state.interaction_choices = None;
                target_state.interaction_allow_freeform = None;
            }
            // Record that this step was activated via GoTo so
            // compute_ready_steps bypasses normal predecessor checks.
            instance.goto_activated_steps.insert(target.clone());
            // The failed step's error is handled; treat it as "done" for
            // successor predecessor checks.
            instance.goto_source_steps.insert(step_id.to_string());
            ErrorOutcome::GoTo
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn build_expression_context(
    instance: &WorkflowInstance,
    _current_step_id: Option<&str>,
) -> ExpressionContext {
    let mut step_outputs = HashMap::new();
    for (id, state) in &instance.step_states {
        if let Some(ref outputs) = state.outputs {
            step_outputs.insert(id.clone(), outputs.clone());
        }
    }

    // Get trigger data from the completed trigger step's outputs.
    // Prefer the trigger that actually fired (has Completed status with outputs).
    let trigger_data = instance
        .definition
        .steps
        .iter()
        .filter(|s| matches!(s.step_type, StepType::Trigger { .. }))
        .filter_map(|s| {
            instance.step_states.get(&s.id).and_then(|state| {
                if state.status == StepStatus::Completed {
                    state.outputs.clone()
                } else {
                    None
                }
            })
        })
        .next()
        .unwrap_or(Value::Null);

    tracing::debug!(
        instance_id = %instance.id,
        trigger_data = %trigger_data,
        variables = %instance.variables,
        step_output_keys = ?step_outputs.keys().collect::<Vec<_>>(),
        "built expression context"
    );

    // Mark trigger/event data as untrusted (originates from external sources).
    let mut untrusted_vars = HashSet::new();
    if trigger_data != Value::Null {
        untrusted_vars.insert("trigger".to_string());
        untrusted_vars.insert("event".to_string());
    }

    ExpressionContext {
        variables: instance.variables.clone(),
        step_outputs,
        trigger_data,
        current_result: Value::Null,
        current_error: None,
        untrusted_vars,
    }
}

pub(crate) fn resolve_arguments(
    args: &HashMap<String, String>,
    ctx: &ExpressionContext,
) -> Result<Value, WorkflowError> {
    let mut map = serde_json::Map::new();
    for (key, expr) in args {
        let resolved = resolve_template(expr, ctx)?;
        // Try to parse as JSON, fall back to string
        let value = serde_json::from_str(&resolved).unwrap_or(Value::String(resolved));
        map.insert(key.clone(), value);
    }
    Ok(Value::Object(map))
}

fn initialize_variables(schema: &Value) -> Value {
    // Extract defaults from JSON Schema properties
    if let Some(props) = schema.get("properties").and_then(|p| p.as_object()) {
        let mut vars = serde_json::Map::new();
        for (key, prop_schema) in props {
            if let Some(default) = prop_schema.get("default") {
                vars.insert(key.clone(), default.clone());
            } else {
                vars.insert(key.clone(), Value::Null);
            }
        }
        Value::Object(vars)
    } else {
        serde_json::json!({})
    }
}

/// Apply SetVariable assignments to the instance variable bag.
/// `outputs` is the JSON object produced by execute_task for SetVariable,
/// with shape: `{ "var_name": { "value": ..., "operation": "set"|"append_list"|"merge_map" }, ... }`
fn apply_variable_assignments(
    variables: &mut Value,
    assignments: &[VariableAssignment],
    outputs: &Value,
) -> Result<(), String> {
    let vars = variables
        .as_object_mut()
        .ok_or_else(|| "Workflow variables is not a JSON object".to_string())?;

    for a in assignments {
        let entry =
            outputs.get(&a.variable).and_then(|e| e.get("value")).cloned().unwrap_or(Value::Null);

        match a.operation {
            AssignOp::Set => {
                vars.insert(a.variable.clone(), entry);
            }
            AssignOp::AppendList => {
                let arr = vars.entry(a.variable.clone()).or_insert_with(|| Value::Array(vec![]));
                // Null values happen when the variable was declared without a default.
                if arr.is_null() {
                    *arr = Value::Array(vec![]);
                }
                match arr.as_array_mut() {
                    Some(list) => list.push(entry),
                    None => {
                        return Err(format!(
                            "Cannot append to '{}': existing value is not an array",
                            a.variable
                        ))
                    }
                }
            }
            AssignOp::MergeMap => {
                let target =
                    vars.entry(a.variable.clone()).or_insert_with(|| serde_json::json!({}));
                if target.is_null() {
                    *target = serde_json::json!({});
                }
                match (target.as_object_mut(), entry) {
                    (Some(map), Value::Object(source)) => {
                        for (k, v) in source {
                            map.insert(k, v);
                        }
                    }
                    (Some(_), other) => {
                        return Err(format!(
                            "Cannot merge into '{}': source value is {}, expected object",
                            a.variable,
                            type_name(&other),
                        ))
                    }
                    (None, _) => {
                        return Err(format!(
                            "Cannot merge into '{}': existing value is not an object",
                            a.variable
                        ))
                    }
                }
            }
        }
    }
    Ok(())
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Validate launch inputs against a trigger definition's input schema.
/// Checks that required fields are present. Type checking is only performed
/// when an explicit `input_schema` is set (legacy `inputs` has unreliable types).
fn validate_trigger_inputs(trigger_def: &TriggerDef, inputs: &Value) -> Result<(), WorkflowError> {
    let schema = trigger_def.effective_input_schema();
    let strict_types = trigger_def.has_explicit_schema();

    // If no properties, anything goes
    let props = match schema.get("properties").and_then(|p| p.as_object()) {
        Some(p) if !p.is_empty() => p,
        _ => return Ok(()),
    };

    let input_obj = inputs.as_object();

    // Check required fields
    if let Some(required) = schema.get("required").and_then(|r| r.as_array()) {
        for req in required {
            if let Some(field_name) = req.as_str() {
                let present = input_obj
                    .map(|obj| obj.get(field_name).map(|v| !v.is_null()).unwrap_or(false))
                    .unwrap_or(false);
                if !present {
                    return Err(WorkflowError::InvalidDefinition {
                        reason: format!("Missing required trigger input: '{field_name}'"),
                    });
                }
            }
        }
    }

    // Type validation only for explicit schemas (legacy inputs have unreliable type info)
    if strict_types {
        if let Some(obj) = input_obj {
            for (key, value) in obj {
                if value.is_null() {
                    continue;
                }
                if let Some(prop_schema) = props.get(key) {
                    if let Some(expected_type) = prop_schema.get("type").and_then(|t| t.as_str()) {
                        let type_ok = match expected_type {
                            "string" => value.is_string(),
                            "number" | "integer" => value.is_number(),
                            "boolean" => value.is_boolean(),
                            "object" => value.is_object(),
                            "array" => value.is_array(),
                            _ => true,
                        };
                        if !type_ok {
                            return Err(WorkflowError::InvalidState {
                                status: format!(
                                    "Trigger input '{key}' expected type '{expected_type}', got {value:?}"
                                ),
                            });
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::WorkflowStore;
    use std::sync::atomic::{AtomicU32, Ordering};
    use tokio::sync::Mutex;

    /// Poll the store until the instance reaches a non-Running status (or timeout).
    /// Used by tests that call `respond_to_gate`/`respond_to_event` which now
    /// continue execution in a background task.
    async fn wait_for_settled(
        store: &dyn WorkflowPersistence,
        instance_id: i64,
        timeout: std::time::Duration,
    ) {
        let start = tokio::time::Instant::now();
        loop {
            if let Ok(Some(inst)) = store.get_instance(instance_id) {
                if !matches!(inst.status, WorkflowStatus::Running | WorkflowStatus::Pending) {
                    return;
                }
            }
            if start.elapsed() > timeout {
                panic!("timeout waiting for instance {instance_id} to settle");
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    /// A test executor that tracks calls and returns configurable results.
    struct TestExecutor {
        tool_call_count: AtomicU32,
        tool_results: Mutex<HashMap<String, Result<Value, String>>>,
    }

    impl TestExecutor {
        fn new() -> Self {
            Self { tool_call_count: AtomicU32::new(0), tool_results: Mutex::new(HashMap::new()) }
        }

        async fn set_tool_result(&self, tool_id: &str, result: Result<Value, String>) {
            self.tool_results.lock().await.insert(tool_id.to_string(), result);
        }
    }

    #[async_trait]
    impl StepExecutor for TestExecutor {
        async fn call_tool(
            &self,
            tool_id: &str,
            _args: Value,
            _ctx: &ExecutionContext,
        ) -> Result<Value, String> {
            self.tool_call_count.fetch_add(1, Ordering::SeqCst);
            let results = self.tool_results.lock().await;
            results.get(tool_id).cloned().unwrap_or(Ok(serde_json::json!({"status": "ok"})))
        }
        async fn invoke_agent(
            &self,
            _: &str,
            _: &str,
            _: bool,
            _: Option<u64>,
            _: &[PermissionEntry],
            _: Option<&str>,
            _: Option<&str>,
            _: &ExecutionContext,
        ) -> Result<Value, String> {
            Ok(serde_json::json!({"agent_result": "done"}))
        }
        async fn signal_agent(
            &self,
            _: &SignalTarget,
            _: &str,
            _: &ExecutionContext,
        ) -> Result<Value, String> {
            Ok(Value::Null)
        }
        async fn wait_for_agent(
            &self,
            _: &str,
            _: Option<u64>,
            _: &ExecutionContext,
        ) -> Result<Value, String> {
            Ok(serde_json::json!({"agent_result": "done"}))
        }
        async fn create_feedback_request(
            &self,
            _: i64,
            _: &str,
            _: &str,
            _: Option<&[String]>,
            _: bool,
            _: &ExecutionContext,
        ) -> Result<String, String> {
            Ok("test-request-id".to_string())
        }
        async fn register_event_gate(
            &self,
            _: i64,
            _: &str,
            _: &str,
            _: Option<&str>,
            _: Option<u64>,
            _: &ExecutionContext,
        ) -> Result<String, String> {
            Ok("test-subscription-id".to_string())
        }
        async fn launch_workflow(
            &self,
            _: &str,
            _: Value,
            _: &ExecutionContext,
        ) -> Result<i64, String> {
            Ok(1001)
        }
        async fn schedule_task(
            &self,
            _: &ScheduleTaskDef,
            _: &ExecutionContext,
        ) -> Result<String, String> {
            Ok("task-001".to_string())
        }
        async fn render_prompt_template(
            &self,
            _: &str,
            _: &str,
            _: Value,
            _: &ExecutionContext,
        ) -> Result<String, String> {
            Err("render_prompt_template not implemented in test executor".to_string())
        }
    }

    /// A test emitter that collects events.
    struct TestEmitter {
        events: Mutex<Vec<WorkflowEvent>>,
    }

    impl TestEmitter {
        fn new() -> Self {
            Self { events: Mutex::new(Vec::new()) }
        }

        async fn events(&self) -> Vec<WorkflowEvent> {
            self.events.lock().await.clone()
        }
    }

    #[async_trait]
    impl WorkflowEventEmitter for TestEmitter {
        async fn emit(&self, event: WorkflowEvent) {
            self.events.lock().await.push(event);
        }
    }

    fn linear_workflow() -> WorkflowDefinition {
        WorkflowDefinition {
            id: generate_workflow_id(),
            name: "linear-test".into(),
            version: "1.0".into(),
            description: None,
            variables: serde_json::json!({
                "type": "object",
                "properties": {
                    "result": { "type": "string", "default": "" }
                }
            }),
            steps: vec![
                StepDef {
                    id: "start".into(),
                    step_type: StepType::Trigger {
                        trigger: TriggerDef {
                            trigger_type: TriggerType::Manual {
                                inputs: vec![],
                                input_schema: None,
                            },
                        },
                    },
                    outputs: HashMap::new(),
                    on_error: None,
                    next: vec!["process".into()],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                },
                StepDef {
                    id: "process".into(),
                    step_type: StepType::Task {
                        task: TaskDef::CallTool {
                            tool_id: "test_tool".into(),
                            arguments: HashMap::from([("input".into(), "{{trigger.msg}}".into())]),
                        },
                    },
                    outputs: HashMap::from([("status".into(), "{{result.status}}".into())]),
                    on_error: None,
                    next: vec!["end".into()],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                },
                StepDef {
                    id: "end".into(),
                    step_type: StepType::ControlFlow { control: ControlFlowDef::EndWorkflow },
                    outputs: HashMap::new(),
                    on_error: None,
                    next: vec![],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                },
            ],
            output: Some(HashMap::from([(
                "final_status".into(),
                "{{steps.process.outputs.status}}".into(),
            )])),
            requested_tools: vec![],
            permissions: vec![],
            attachments: vec![],
            tests: vec![],
            mode: WorkflowMode::default(),
            result_message: None,
            bundled: false,
            archived: false,
            triggers_paused: false,
        }
    }

    #[tokio::test]
    async fn test_linear_workflow_execution() {
        let store = Arc::new(WorkflowStore::in_memory().unwrap());
        let executor = Arc::new(TestExecutor::new());
        let emitter = Arc::new(TestEmitter::new());
        let engine = WorkflowEngine::new(store.clone(), executor.clone(), emitter.clone());

        let instance_id = engine
            .launch(
                linear_workflow(),
                serde_json::json!({"msg": "hello"}),
                "session-1".into(),
                None,
                vec![],
                None,
            )
            .await
            .unwrap();

        // Verify instance completed
        let store = &*store;
        let instance = store.get_instance(instance_id).unwrap().unwrap();
        assert_eq!(instance.status, WorkflowStatus::Completed);
        assert!(instance.completed_at_ms.is_some());

        // Verify steps completed
        assert_eq!(instance.step_states["start"].status, StepStatus::Completed);
        assert_eq!(instance.step_states["process"].status, StepStatus::Completed);
        assert_eq!(instance.step_states["end"].status, StepStatus::Completed);

        // Verify tool was called
        assert_eq!(executor.tool_call_count.load(Ordering::SeqCst), 1);

        // Verify output was resolved
        assert!(instance.output.is_some());
        let output = instance.output.unwrap();
        assert_eq!(output["final_status"], "ok");
    }

    #[tokio::test]
    async fn test_branching_workflow() {
        let def = WorkflowDefinition {
            id: generate_workflow_id(),
            name: "branch-test".into(),
            version: "1.0".into(),
            description: None,
            variables: serde_json::json!({"type": "object", "properties": {}}),
            steps: vec![
                StepDef {
                    id: "start".into(),
                    step_type: StepType::Trigger {
                        trigger: TriggerDef {
                            trigger_type: TriggerType::Manual {
                                inputs: vec![],
                                input_schema: None,
                            },
                        },
                    },
                    outputs: HashMap::new(),
                    on_error: None,
                    next: vec!["check".into()],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                },
                StepDef {
                    id: "check".into(),
                    step_type: StepType::ControlFlow {
                        control: ControlFlowDef::Branch {
                            condition: "{{trigger.amount}} > 100".into(),
                            then: vec!["high_path".into()],
                            else_branch: vec!["low_path".into()],
                        },
                    },
                    outputs: HashMap::new(),
                    on_error: None,
                    next: vec![],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                },
                StepDef {
                    id: "high_path".into(),
                    step_type: StepType::Task {
                        task: TaskDef::CallTool {
                            tool_id: "high_tool".into(),
                            arguments: HashMap::new(),
                        },
                    },
                    outputs: HashMap::new(),
                    on_error: None,
                    next: vec!["end".into()],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                },
                StepDef {
                    id: "low_path".into(),
                    step_type: StepType::Task {
                        task: TaskDef::CallTool {
                            tool_id: "low_tool".into(),
                            arguments: HashMap::new(),
                        },
                    },
                    outputs: HashMap::new(),
                    on_error: None,
                    next: vec!["end".into()],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                },
                StepDef {
                    id: "end".into(),
                    step_type: StepType::ControlFlow { control: ControlFlowDef::EndWorkflow },
                    outputs: HashMap::new(),
                    on_error: None,
                    next: vec![],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                },
            ],
            output: None,
            requested_tools: vec![],
            permissions: vec![],
            attachments: vec![],
            tests: vec![],
            mode: WorkflowMode::default(),
            result_message: None,
            bundled: false,
            archived: false,
            triggers_paused: false,
        };

        let store = Arc::new(WorkflowStore::in_memory().unwrap());
        let executor = Arc::new(TestExecutor::new());
        let emitter = Arc::new(TestEmitter::new());
        let engine = WorkflowEngine::new(store.clone(), executor.clone(), emitter.clone());

        let instance_id = engine
            .launch(def, serde_json::json!({"amount": 200}), "session-1".into(), None, vec![], None)
            .await
            .unwrap();

        let store = &*store;
        let instance = store.get_instance(instance_id).unwrap().unwrap();
        assert_eq!(instance.status, WorkflowStatus::Completed);
        assert_eq!(instance.step_states["high_path"].status, StepStatus::Completed);
        // low_path should not have been executed (it stays Pending, then gets Skipped by EndWorkflow)
        assert_eq!(instance.step_states["low_path"].status, StepStatus::Skipped);
    }

    #[tokio::test]
    async fn test_feedback_gate_pauses_workflow() {
        let def = WorkflowDefinition {
            id: generate_workflow_id(),
            name: "feedback-test".into(),
            version: "1.0".into(),
            description: None,
            variables: serde_json::json!({"type": "object", "properties": {}}),
            steps: vec![
                StepDef {
                    id: "start".into(),
                    step_type: StepType::Trigger {
                        trigger: TriggerDef {
                            trigger_type: TriggerType::Manual {
                                inputs: vec![],
                                input_schema: None,
                            },
                        },
                    },
                    outputs: HashMap::new(),
                    on_error: None,
                    next: vec!["gate".into()],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                },
                StepDef {
                    id: "gate".into(),
                    step_type: StepType::Task {
                        task: TaskDef::FeedbackGate {
                            prompt: "Do you approve?".into(),
                            choices: Some(vec!["Yes".into(), "No".into()]),
                            allow_freeform: false,
                        },
                    },
                    outputs: HashMap::new(),
                    on_error: None,
                    next: vec!["end".into()],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                },
                StepDef {
                    id: "end".into(),
                    step_type: StepType::ControlFlow { control: ControlFlowDef::EndWorkflow },
                    outputs: HashMap::new(),
                    on_error: None,
                    next: vec![],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                },
            ],
            output: None,
            requested_tools: vec![],
            permissions: vec![],
            attachments: vec![],
            tests: vec![],
            mode: WorkflowMode::default(),
            result_message: None,
            bundled: false,
            archived: false,
            triggers_paused: false,
        };

        let store = Arc::new(WorkflowStore::in_memory().unwrap());
        let executor = Arc::new(TestExecutor::new());
        let emitter = Arc::new(TestEmitter::new());
        let engine = WorkflowEngine::new(store.clone(), executor.clone(), emitter.clone());

        let instance_id = engine
            .launch(def, serde_json::json!({}), "session-1".into(), None, vec![], None)
            .await
            .unwrap();

        // Workflow should be waiting on input
        {
            let store = &*store;
            let instance = store.get_instance(instance_id).unwrap().unwrap();
            assert_eq!(instance.status, WorkflowStatus::WaitingOnInput);
            assert_eq!(instance.step_states["gate"].status, StepStatus::WaitingOnInput);
        }

        // Respond to the gate
        engine
            .respond_to_gate(
                instance_id,
                "gate",
                serde_json::json!({"selected": "Yes", "text": "Approved!"}),
            )
            .await
            .unwrap();

        // Workflow continues in background — wait for it to settle
        wait_for_settled(&*store, instance_id, std::time::Duration::from_secs(5)).await;

        // Workflow should now be completed
        {
            let store = &*store;
            let instance = store.get_instance(instance_id).unwrap().unwrap();
            assert_eq!(instance.status, WorkflowStatus::Completed);
            assert_eq!(instance.step_states["gate"].status, StepStatus::Completed);
        }
    }

    #[tokio::test]
    async fn test_feedback_gate_output_flows_to_downstream_templates() {
        // Workflow: trigger → feedback_gate → tool_call (uses gate output in template)
        // Verifies that the feedback response is stored as the step's outputs
        // and accessible via {{steps.gate.outputs.selected}} in downstream steps.
        let def = WorkflowDefinition {
            id: generate_workflow_id(),
            name: "feedback-flow-test".into(),
            version: "1.0".into(),
            description: None,
            variables: serde_json::json!({"type": "object", "properties": {}}),
            steps: vec![
                StepDef {
                    id: "start".into(),
                    step_type: StepType::Trigger {
                        trigger: TriggerDef {
                            trigger_type: TriggerType::Manual {
                                inputs: vec![],
                                input_schema: None,
                            },
                        },
                    },
                    outputs: HashMap::new(),
                    on_error: None,
                    next: vec!["gate".into()],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                },
                StepDef {
                    id: "gate".into(),
                    step_type: StepType::Task {
                        task: TaskDef::FeedbackGate {
                            prompt: "Do you approve?".into(),
                            choices: Some(vec!["Yes".into(), "No".into()]),
                            allow_freeform: false,
                        },
                    },
                    outputs: HashMap::new(),
                    on_error: None,
                    next: vec!["notify".into()],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                },
                StepDef {
                    id: "notify".into(),
                    step_type: StepType::Task {
                        task: TaskDef::CallTool {
                            tool_id: "echo_tool".into(),
                            arguments: HashMap::from([(
                                "message".to_string(),
                                "User chose: {{steps.gate.outputs.selected}}".to_string(),
                            )]),
                        },
                    },
                    outputs: HashMap::new(),
                    on_error: None,
                    next: vec!["end".into()],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                },
                StepDef {
                    id: "end".into(),
                    step_type: StepType::ControlFlow { control: ControlFlowDef::EndWorkflow },
                    outputs: HashMap::new(),
                    on_error: None,
                    next: vec![],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                },
            ],
            output: None,
            requested_tools: vec![],
            permissions: vec![],
            attachments: vec![],
            tests: vec![],
            mode: WorkflowMode::default(),
            result_message: None,
            bundled: false,
            archived: false,
            triggers_paused: false,
        };

        let store = Arc::new(WorkflowStore::in_memory().unwrap());
        let executor = Arc::new(TestExecutor::new());
        // echo_tool returns whatever args it receives
        executor.set_tool_result("echo_tool", Ok(serde_json::json!({"echoed": true}))).await;
        let emitter = Arc::new(TestEmitter::new());
        let engine = WorkflowEngine::new(store.clone(), executor.clone(), emitter.clone());

        let instance_id = engine
            .launch(def, serde_json::json!({}), "session-1".into(), None, vec![], None)
            .await
            .unwrap();

        // Should be waiting on feedback
        {
            let instance = store.get_instance(instance_id).unwrap().unwrap();
            assert_eq!(instance.status, WorkflowStatus::WaitingOnInput);
            assert_eq!(instance.step_states["gate"].status, StepStatus::WaitingOnInput);
        }

        // Respond to the feedback gate
        engine
            .respond_to_gate(
                instance_id,
                "gate",
                serde_json::json!({"selected": "Yes", "text": "Looks good!"}),
            )
            .await
            .unwrap();

        // Workflow continues in background — wait for it to settle
        wait_for_settled(&*store, instance_id, std::time::Duration::from_secs(5)).await;

        // Workflow should complete, and the gate outputs should be stored
        let instance = store.get_instance(instance_id).unwrap().unwrap();
        assert_eq!(instance.status, WorkflowStatus::Completed);

        // Verify gate step has the response stored as outputs
        let gate_outputs = instance.step_states["gate"].outputs.as_ref().unwrap();
        assert_eq!(gate_outputs["selected"], "Yes");
        assert_eq!(gate_outputs["text"], "Looks good!");

        // Verify downstream step completed (it would have failed if template didn't resolve)
        assert_eq!(instance.step_states["notify"].status, StepStatus::Completed);
    }

    #[tokio::test]
    async fn test_error_strategy_skip() {
        let def = WorkflowDefinition {
            id: generate_workflow_id(),
            name: "skip-test".into(),
            version: "1.0".into(),
            description: None,
            variables: serde_json::json!({"type": "object", "properties": {}}),
            steps: vec![
                StepDef {
                    id: "start".into(),
                    step_type: StepType::Trigger {
                        trigger: TriggerDef {
                            trigger_type: TriggerType::Manual {
                                inputs: vec![],
                                input_schema: None,
                            },
                        },
                    },
                    outputs: HashMap::new(),
                    on_error: None,
                    next: vec!["failing_step".into()],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                },
                StepDef {
                    id: "failing_step".into(),
                    step_type: StepType::Task {
                        task: TaskDef::CallTool {
                            tool_id: "bad_tool".into(),
                            arguments: HashMap::new(),
                        },
                    },
                    outputs: HashMap::new(),
                    on_error: Some(ErrorStrategy::Skip {
                        default_output: Some(serde_json::json!({"skipped": true})),
                    }),
                    next: vec!["end".into()],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                },
                StepDef {
                    id: "end".into(),
                    step_type: StepType::ControlFlow { control: ControlFlowDef::EndWorkflow },
                    outputs: HashMap::new(),
                    on_error: None,
                    next: vec![],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                },
            ],
            output: None,
            requested_tools: vec![],
            permissions: vec![],
            attachments: vec![],
            tests: vec![],
            mode: WorkflowMode::default(),
            result_message: None,
            bundled: false,
            archived: false,
            triggers_paused: false,
        };

        let store = Arc::new(WorkflowStore::in_memory().unwrap());
        let executor = Arc::new(TestExecutor::new());
        executor.set_tool_result("bad_tool", Err("Tool failed".to_string())).await;
        let emitter = Arc::new(TestEmitter::new());
        let engine = WorkflowEngine::new(store.clone(), executor, emitter);

        let instance_id = engine
            .launch(def, serde_json::json!({}), "session-1".into(), None, vec![], None)
            .await
            .unwrap();

        let store = &*store;
        let instance = store.get_instance(instance_id).unwrap().unwrap();
        assert_eq!(instance.status, WorkflowStatus::Completed);
        assert_eq!(instance.step_states["failing_step"].status, StepStatus::Skipped);
    }

    #[tokio::test]
    async fn test_kill_workflow() {
        let def = WorkflowDefinition {
            id: generate_workflow_id(),
            name: "kill-test".into(),
            version: "1.0".into(),
            description: None,
            variables: serde_json::json!({"type": "object", "properties": {}}),
            steps: vec![
                StepDef {
                    id: "start".into(),
                    step_type: StepType::Trigger {
                        trigger: TriggerDef {
                            trigger_type: TriggerType::Manual {
                                inputs: vec![],
                                input_schema: None,
                            },
                        },
                    },
                    outputs: HashMap::new(),
                    on_error: None,
                    next: vec!["gate".into()],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                },
                StepDef {
                    id: "gate".into(),
                    step_type: StepType::Task {
                        task: TaskDef::FeedbackGate {
                            prompt: "Wait here".into(),
                            choices: None,
                            allow_freeform: true,
                        },
                    },
                    outputs: HashMap::new(),
                    on_error: None,
                    next: vec![],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                },
            ],
            output: None,
            requested_tools: vec![],
            permissions: vec![],
            attachments: vec![],
            tests: vec![],
            mode: WorkflowMode::default(),
            result_message: None,
            bundled: false,
            archived: false,
            triggers_paused: false,
        };

        let store = Arc::new(WorkflowStore::in_memory().unwrap());
        let executor = Arc::new(TestExecutor::new());
        let emitter = Arc::new(TestEmitter::new());
        let engine = WorkflowEngine::new(store.clone(), executor, emitter);

        let instance_id = engine
            .launch(def, serde_json::json!({}), "session-1".into(), None, vec![], None)
            .await
            .unwrap();

        // Kill it
        engine.kill(instance_id).await.unwrap();

        let store = &*store;
        let instance = store.get_instance(instance_id).unwrap().unwrap();
        assert_eq!(instance.status, WorkflowStatus::Killed);
    }

    #[tokio::test]
    async fn test_delay_step() {
        let def = WorkflowDefinition {
            id: generate_workflow_id(),
            name: "delay-test".into(),
            version: "1.0".into(),
            description: None,
            variables: serde_json::json!({"type": "object", "properties": {}}),
            steps: vec![
                StepDef {
                    id: "start".into(),
                    step_type: StepType::Trigger {
                        trigger: TriggerDef {
                            trigger_type: TriggerType::Manual {
                                inputs: vec![],
                                input_schema: None,
                            },
                        },
                    },
                    outputs: HashMap::new(),
                    on_error: None,
                    next: vec!["wait".into()],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                },
                StepDef {
                    id: "wait".into(),
                    step_type: StepType::Task { task: TaskDef::Delay { duration_secs: 0 } },
                    outputs: HashMap::new(),
                    on_error: None,
                    next: vec!["end".into()],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                },
                StepDef {
                    id: "end".into(),
                    step_type: StepType::ControlFlow { control: ControlFlowDef::EndWorkflow },
                    outputs: HashMap::new(),
                    on_error: None,
                    next: vec![],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                },
            ],
            output: None,
            requested_tools: vec![],
            permissions: vec![],
            attachments: vec![],
            tests: vec![],
            mode: WorkflowMode::default(),
            result_message: None,
            bundled: false,
            archived: false,
            triggers_paused: false,
        };

        let store = Arc::new(WorkflowStore::in_memory().unwrap());
        let executor = Arc::new(NullStepExecutor);
        let emitter = Arc::new(NullEventEmitter);
        let engine = WorkflowEngine::new(store.clone(), executor, emitter);

        let instance_id = engine
            .launch(def, serde_json::json!({}), "session-1".into(), None, vec![], None)
            .await
            .unwrap();

        let store = &*store;

        // After launch, the delay step should be in WaitingForDelay and a
        // background timer has been spawned.  With duration_secs=0, the timer
        // fires almost immediately.
        let instance = store.get_instance(instance_id).unwrap().unwrap();
        assert_eq!(instance.step_states["wait"].status, StepStatus::WaitingForDelay);

        // Give the background timer a moment to fire and complete the workflow.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let instance = store.get_instance(instance_id).unwrap().unwrap();
        assert_eq!(instance.status, WorkflowStatus::Completed);
        assert_eq!(instance.step_states["wait"].status, StepStatus::Completed);
    }

    #[tokio::test]
    async fn test_event_gate_pauses_workflow() {
        let def = WorkflowDefinition {
            id: generate_workflow_id(),
            name: "event-gate-test".into(),
            version: "1.0".into(),
            description: None,
            variables: serde_json::json!({"type": "object", "properties": {}}),
            steps: vec![
                StepDef {
                    id: "start".into(),
                    step_type: StepType::Trigger {
                        trigger: TriggerDef {
                            trigger_type: TriggerType::Manual {
                                inputs: vec![],
                                input_schema: None,
                            },
                        },
                    },
                    outputs: HashMap::new(),
                    on_error: None,
                    next: vec!["wait_event".into()],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                },
                StepDef {
                    id: "wait_event".into(),
                    step_type: StepType::Task {
                        task: TaskDef::EventGate {
                            topic: "approval.granted".into(),
                            filter: None,
                            timeout_secs: None,
                        },
                    },
                    outputs: HashMap::new(),
                    on_error: None,
                    next: vec!["end".into()],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                },
                StepDef {
                    id: "end".into(),
                    step_type: StepType::ControlFlow { control: ControlFlowDef::EndWorkflow },
                    outputs: HashMap::new(),
                    on_error: None,
                    next: vec![],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                },
            ],
            output: None,
            requested_tools: vec![],
            permissions: vec![],
            attachments: vec![],
            tests: vec![],
            mode: WorkflowMode::default(),
            result_message: None,
            bundled: false,
            archived: false,
            triggers_paused: false,
        };

        let store = Arc::new(WorkflowStore::in_memory().unwrap());
        let executor = Arc::new(TestExecutor::new());
        let emitter = Arc::new(TestEmitter::new());
        let engine = WorkflowEngine::new(store.clone(), executor.clone(), emitter.clone());

        let instance_id = engine
            .launch(def, serde_json::json!({}), "session-1".into(), None, vec![], None)
            .await
            .unwrap();

        // Workflow should be waiting on event
        {
            let store = &*store;
            let instance = store.get_instance(instance_id).unwrap().unwrap();
            assert_eq!(instance.status, WorkflowStatus::WaitingOnEvent);
            assert_eq!(instance.step_states["wait_event"].status, StepStatus::WaitingOnEvent);
            // subscription_id should be stored
            assert!(instance.step_states["wait_event"].interaction_request_id.is_some());
        }

        // Respond to the event gate
        engine
            .respond_to_event(
                instance_id,
                "wait_event",
                serde_json::json!({"user_id": "123", "approved": true}),
            )
            .await
            .unwrap();

        // Workflow continues in background — wait for it to settle
        wait_for_settled(&*store, instance_id, std::time::Duration::from_secs(5)).await;

        // Workflow should now be completed
        {
            let store = &*store;
            let instance = store.get_instance(instance_id).unwrap().unwrap();
            assert_eq!(instance.status, WorkflowStatus::Completed);
            assert_eq!(instance.step_states["wait_event"].status, StepStatus::Completed);
            // Event data should be stored as step outputs
            let outputs = instance.step_states["wait_event"].outputs.as_ref().unwrap();
            assert_eq!(outputs["user_id"], "123");
            assert_eq!(outputs["approved"], true);
            // interaction_request_id should be cleared
            assert!(instance.step_states["wait_event"].interaction_request_id.is_none());
        }
    }

    #[tokio::test]
    async fn test_event_gate_respond_wrong_status_fails() {
        let def = WorkflowDefinition {
            id: generate_workflow_id(),
            name: "event-gate-err-test".into(),
            version: "1.0".into(),
            description: None,
            variables: serde_json::json!({"type": "object", "properties": {}}),
            steps: vec![
                StepDef {
                    id: "start".into(),
                    step_type: StepType::Trigger {
                        trigger: TriggerDef {
                            trigger_type: TriggerType::Manual {
                                inputs: vec![],
                                input_schema: None,
                            },
                        },
                    },
                    outputs: HashMap::new(),
                    on_error: None,
                    next: vec!["tool_step".into()],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                },
                StepDef {
                    id: "tool_step".into(),
                    step_type: StepType::Task {
                        task: TaskDef::CallTool {
                            tool_id: "echo".into(),
                            arguments: HashMap::new(),
                        },
                    },
                    outputs: HashMap::new(),
                    on_error: None,
                    next: vec![],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                },
            ],
            output: None,
            requested_tools: vec![],
            permissions: vec![],
            attachments: vec![],
            tests: vec![],
            mode: WorkflowMode::default(),
            result_message: None,
            bundled: false,
            archived: false,
            triggers_paused: false,
        };

        let store = Arc::new(WorkflowStore::in_memory().unwrap());
        let executor = Arc::new(TestExecutor::new());
        let emitter = Arc::new(TestEmitter::new());
        let engine = WorkflowEngine::new(store.clone(), executor.clone(), emitter.clone());

        let instance_id = engine
            .launch(def, serde_json::json!({}), "session-1".into(), None, vec![], None)
            .await
            .unwrap();

        // Try to respond to event on a completed step — should fail
        let result = engine.respond_to_event(instance_id, "tool_step", serde_json::json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_event_gate_with_output_mapping() {
        let def = WorkflowDefinition {
            id: generate_workflow_id(),
            name: "event-gate-outputs".into(),
            version: "1.0".into(),
            description: None,
            variables: serde_json::json!({"type": "object", "properties": {}}),
            steps: vec![
                StepDef {
                    id: "start".into(),
                    step_type: StepType::Trigger {
                        trigger: TriggerDef {
                            trigger_type: TriggerType::Manual {
                                inputs: vec![],
                                input_schema: None,
                            },
                        },
                    },
                    outputs: HashMap::new(),
                    on_error: None,
                    next: vec!["wait_event".into()],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                },
                StepDef {
                    id: "wait_event".into(),
                    step_type: StepType::Task {
                        task: TaskDef::EventGate {
                            topic: "data.received".into(),
                            filter: Some("important".into()),
                            timeout_secs: Some(60),
                        },
                    },
                    outputs: HashMap::new(),
                    on_error: None,
                    next: vec!["process".into()],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                },
                StepDef {
                    id: "process".into(),
                    step_type: StepType::Task {
                        task: TaskDef::CallTool {
                            tool_id: "processor".into(),
                            arguments: {
                                let mut m = HashMap::new();
                                m.insert(
                                    "data".to_string(),
                                    "{{steps.wait_event.outputs}}".to_string(),
                                );
                                m
                            },
                        },
                    },
                    outputs: HashMap::new(),
                    on_error: None,
                    next: vec![],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                },
            ],
            output: None,
            requested_tools: vec![],
            permissions: vec![],
            attachments: vec![],
            tests: vec![],
            mode: WorkflowMode::default(),
            result_message: None,
            bundled: false,
            archived: false,
            triggers_paused: false,
        };

        let store = Arc::new(WorkflowStore::in_memory().unwrap());
        let executor = Arc::new(TestExecutor::new());
        let emitter = Arc::new(TestEmitter::new());
        let engine = WorkflowEngine::new(store.clone(), executor.clone(), emitter.clone());

        let instance_id = engine
            .launch(def, serde_json::json!({}), "session-1".into(), None, vec![], None)
            .await
            .unwrap();

        // Should be waiting
        {
            let store = &*store;
            let instance = store.get_instance(instance_id).unwrap().unwrap();
            assert_eq!(instance.status, WorkflowStatus::WaitingOnEvent);
        }

        // Deliver event
        engine
            .respond_to_event(
                instance_id,
                "wait_event",
                serde_json::json!({"payload": "important_data"}),
            )
            .await
            .unwrap();

        // Workflow continues in background — wait for it to settle
        wait_for_settled(&*store, instance_id, std::time::Duration::from_secs(5)).await;

        // Should be completed — process step should have run
        {
            let store = &*store;
            let instance = store.get_instance(instance_id).unwrap().unwrap();
            assert_eq!(instance.status, WorkflowStatus::Completed);
            assert_eq!(instance.step_states["process"].status, StepStatus::Completed);
        }
    }

    #[tokio::test]
    async fn test_launch_rejects_invalid_trigger_step_id() {
        let store = Arc::new(WorkflowStore::in_memory().unwrap());
        let executor = Arc::new(TestExecutor::new());
        let emitter = Arc::new(TestEmitter::new());
        let engine = WorkflowEngine::new(store, executor, emitter);

        let missing_step = engine
            .launch(
                linear_workflow(),
                serde_json::json!({"msg": "hello"}),
                "session-1".into(),
                None,
                vec![],
                Some("does-not-exist".into()),
            )
            .await;
        assert!(matches!(
            missing_step,
            Err(WorkflowError::InvalidDefinition { ref reason })
                if reason.contains("does not reference an existing step")
        ));

        let non_trigger_step = engine
            .launch(
                linear_workflow(),
                serde_json::json!({"msg": "hello"}),
                "session-1".into(),
                None,
                vec![],
                Some("process".into()),
            )
            .await;
        assert!(matches!(
            non_trigger_step,
            Err(WorkflowError::InvalidDefinition { ref reason })
                if reason.contains("must reference a trigger step")
        ));
    }

    #[tokio::test]
    async fn test_respond_to_gate_rejected_after_kill() {
        let def = WorkflowDefinition {
            id: generate_workflow_id(),
            name: "killed-gate-test".into(),
            version: "1.0".into(),
            description: None,
            variables: serde_json::json!({"type": "object", "properties": {}}),
            steps: vec![
                StepDef {
                    id: "start".into(),
                    step_type: StepType::Trigger {
                        trigger: TriggerDef {
                            trigger_type: TriggerType::Manual {
                                inputs: vec![],
                                input_schema: None,
                            },
                        },
                    },
                    outputs: HashMap::new(),
                    on_error: None,
                    next: vec!["gate".into()],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                },
                StepDef {
                    id: "gate".into(),
                    step_type: StepType::Task {
                        task: TaskDef::FeedbackGate {
                            prompt: "approve?".into(),
                            choices: None,
                            allow_freeform: true,
                        },
                    },
                    outputs: HashMap::new(),
                    on_error: None,
                    next: vec!["end".into()],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                },
                StepDef {
                    id: "end".into(),
                    step_type: StepType::ControlFlow { control: ControlFlowDef::EndWorkflow },
                    outputs: HashMap::new(),
                    on_error: None,
                    next: vec![],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                },
            ],
            output: None,
            requested_tools: vec![],
            permissions: vec![],
            attachments: vec![],
            tests: vec![],
            mode: WorkflowMode::default(),
            result_message: None,
            bundled: false,
            archived: false,
            triggers_paused: false,
        };

        let store = Arc::new(WorkflowStore::in_memory().unwrap());
        let executor = Arc::new(TestExecutor::new());
        let emitter = Arc::new(TestEmitter::new());
        let engine = WorkflowEngine::new(store.clone(), executor, emitter);

        let instance_id = engine
            .launch(def, serde_json::json!({}), "session-1".into(), None, vec![], None)
            .await
            .unwrap();

        engine.kill(instance_id).await.unwrap();

        let result =
            engine.respond_to_gate(instance_id, "gate", serde_json::json!({"text": "yes"})).await;
        assert!(matches!(result, Err(WorkflowError::InvalidState { .. })));

        let instance = store.get_instance(instance_id).unwrap().unwrap();
        assert_eq!(instance.status, WorkflowStatus::Killed);
    }

    #[tokio::test]
    async fn test_respond_to_event_rejected_after_kill() {
        let def = WorkflowDefinition {
            id: generate_workflow_id(),
            name: "killed-event-test".into(),
            version: "1.0".into(),
            description: None,
            variables: serde_json::json!({"type": "object", "properties": {}}),
            steps: vec![
                StepDef {
                    id: "start".into(),
                    step_type: StepType::Trigger {
                        trigger: TriggerDef {
                            trigger_type: TriggerType::Manual {
                                inputs: vec![],
                                input_schema: None,
                            },
                        },
                    },
                    outputs: HashMap::new(),
                    on_error: None,
                    next: vec!["wait_event".into()],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                },
                StepDef {
                    id: "wait_event".into(),
                    step_type: StepType::Task {
                        task: TaskDef::EventGate {
                            topic: "approval.granted".into(),
                            filter: None,
                            timeout_secs: None,
                        },
                    },
                    outputs: HashMap::new(),
                    on_error: None,
                    next: vec!["end".into()],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                },
                StepDef {
                    id: "end".into(),
                    step_type: StepType::ControlFlow { control: ControlFlowDef::EndWorkflow },
                    outputs: HashMap::new(),
                    on_error: None,
                    next: vec![],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                },
            ],
            output: None,
            requested_tools: vec![],
            permissions: vec![],
            attachments: vec![],
            tests: vec![],
            mode: WorkflowMode::default(),
            result_message: None,
            bundled: false,
            archived: false,
            triggers_paused: false,
        };

        let store = Arc::new(WorkflowStore::in_memory().unwrap());
        let executor = Arc::new(TestExecutor::new());
        let emitter = Arc::new(TestEmitter::new());
        let engine = WorkflowEngine::new(store.clone(), executor, emitter);

        let instance_id = engine
            .launch(def, serde_json::json!({}), "session-1".into(), None, vec![], None)
            .await
            .unwrap();

        engine.kill(instance_id).await.unwrap();

        let result = engine
            .respond_to_event(instance_id, "wait_event", serde_json::json!({"approved": true}))
            .await;
        assert!(matches!(result, Err(WorkflowError::InvalidState { .. })));

        let instance = store.get_instance(instance_id).unwrap().unwrap();
        assert_eq!(instance.status, WorkflowStatus::Killed);
    }

    #[tokio::test]
    async fn test_delay_timer_does_not_resume_killed_instance() {
        let def = WorkflowDefinition {
            id: generate_workflow_id(),
            name: "killed-delay-test".into(),
            version: "1.0".into(),
            description: None,
            variables: serde_json::json!({"type": "object", "properties": {}}),
            steps: vec![
                StepDef {
                    id: "start".into(),
                    step_type: StepType::Trigger {
                        trigger: TriggerDef {
                            trigger_type: TriggerType::Manual {
                                inputs: vec![],
                                input_schema: None,
                            },
                        },
                    },
                    outputs: HashMap::new(),
                    on_error: None,
                    next: vec!["wait".into()],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                },
                StepDef {
                    id: "wait".into(),
                    step_type: StepType::Task { task: TaskDef::Delay { duration_secs: 1 } },
                    outputs: HashMap::new(),
                    on_error: None,
                    next: vec!["end".into()],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                },
                StepDef {
                    id: "end".into(),
                    step_type: StepType::ControlFlow { control: ControlFlowDef::EndWorkflow },
                    outputs: HashMap::new(),
                    on_error: None,
                    next: vec![],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                },
            ],
            output: None,
            requested_tools: vec![],
            permissions: vec![],
            attachments: vec![],
            tests: vec![],
            mode: WorkflowMode::default(),
            result_message: None,
            bundled: false,
            archived: false,
            triggers_paused: false,
        };

        let store = Arc::new(WorkflowStore::in_memory().unwrap());
        let executor = Arc::new(NullStepExecutor);
        let emitter = Arc::new(NullEventEmitter);
        let engine = WorkflowEngine::new(store.clone(), executor, emitter);

        let instance_id = engine
            .launch(def, serde_json::json!({}), "session-1".into(), None, vec![], None)
            .await
            .unwrap();

        engine.kill(instance_id).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(1300)).await;

        let instance = store.get_instance(instance_id).unwrap().unwrap();
        assert_eq!(instance.status, WorkflowStatus::Killed);
        assert_ne!(instance.step_states["wait"].status, StepStatus::Completed);
    }

    #[tokio::test]
    async fn test_stale_pause_kill_flags_are_cleared_on_not_found() {
        let store = Arc::new(WorkflowStore::in_memory().unwrap());
        let executor = Arc::new(TestExecutor::new());
        let emitter = Arc::new(TestEmitter::new());
        let engine = WorkflowEngine::new(store.clone(), executor, emitter);

        // Attempt to pause a non-existent instance â the stale flag should be cleaned up.
        let stale_pause_id: i64 = 99999;
        let pause_result = engine.pause(stale_pause_id).await;
        assert!(matches!(pause_result, Err(WorkflowError::InstanceNotFound { .. })));

        // A new launch should complete normally (stale flag was cleaned up).
        let launched_pause_id = engine
            .launch(
                linear_workflow(),
                serde_json::json!({"msg": "hello"}),
                "session-1".into(),
                None,
                vec![],
                None,
            )
            .await
            .unwrap();
        let launched_pause_instance = store.get_instance(launched_pause_id).unwrap().unwrap();
        assert_eq!(launched_pause_instance.status, WorkflowStatus::Completed);

        // Attempt to kill a non-existent instance â the stale flag should be cleaned up.
        let stale_kill_id: i64 = 99998;
        let kill_result = engine.kill(stale_kill_id).await;
        assert!(matches!(kill_result, Err(WorkflowError::InstanceNotFound { .. })));

        // A new launch should complete normally.
        let launched_kill_id = engine
            .launch(
                linear_workflow(),
                serde_json::json!({"msg": "hello"}),
                "session-1".into(),
                None,
                vec![],
                None,
            )
            .await
            .unwrap();
        let launched_kill_instance = store.get_instance(launched_kill_id).unwrap().unwrap();
        assert_eq!(launched_kill_instance.status, WorkflowStatus::Completed);
    }

    #[tokio::test]
    async fn test_skip_propagation_keeps_join_with_waiting_predecessor() {
        let def = WorkflowDefinition {
            id: generate_workflow_id(),
            name: "skip-waiting-predecessor".into(),
            version: "1.0".into(),
            description: None,
            variables: serde_json::json!({"type": "object", "properties": {}}),
            steps: vec![
                StepDef {
                    id: "start".into(),
                    step_type: StepType::Trigger {
                        trigger: TriggerDef {
                            trigger_type: TriggerType::Manual {
                                inputs: vec![],
                                input_schema: None,
                            },
                        },
                    },
                    outputs: HashMap::new(),
                    on_error: None,
                    next: vec!["wait_for_input".into(), "maybe_fail".into()],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                },
                StepDef {
                    id: "wait_for_input".into(),
                    step_type: StepType::Task {
                        task: TaskDef::FeedbackGate {
                            prompt: "continue?".into(),
                            choices: None,
                            allow_freeform: true,
                        },
                    },
                    outputs: HashMap::new(),
                    on_error: None,
                    next: vec!["join".into()],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                },
                StepDef {
                    id: "maybe_fail".into(),
                    step_type: StepType::Task {
                        task: TaskDef::CallTool {
                            tool_id: "bad_tool".into(),
                            arguments: HashMap::new(),
                        },
                    },
                    outputs: HashMap::new(),
                    on_error: Some(ErrorStrategy::Skip { default_output: None }),
                    next: vec!["join".into()],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                },
                StepDef {
                    id: "join".into(),
                    step_type: StepType::Task {
                        task: TaskDef::CallTool {
                            tool_id: "join_tool".into(),
                            arguments: HashMap::new(),
                        },
                    },
                    outputs: HashMap::new(),
                    on_error: None,
                    next: vec!["end".into()],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                },
                StepDef {
                    id: "end".into(),
                    step_type: StepType::ControlFlow { control: ControlFlowDef::EndWorkflow },
                    outputs: HashMap::new(),
                    on_error: None,
                    next: vec![],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                },
            ],
            output: None,
            requested_tools: vec![],
            permissions: vec![],
            attachments: vec![],
            tests: vec![],
            mode: WorkflowMode::default(),
            result_message: None,
            bundled: false,
            archived: false,
            triggers_paused: false,
        };

        let store = Arc::new(WorkflowStore::in_memory().unwrap());
        let executor = Arc::new(TestExecutor::new());
        executor.set_tool_result("bad_tool", Err("simulated failure".to_string())).await;
        let emitter = Arc::new(TestEmitter::new());
        let engine = WorkflowEngine::new(store.clone(), executor, emitter);

        let instance_id = engine
            .launch(def, serde_json::json!({}), "session-1".into(), None, vec![], None)
            .await
            .unwrap();

        let waiting_instance = store.get_instance(instance_id).unwrap().unwrap();
        assert_eq!(waiting_instance.status, WorkflowStatus::WaitingOnInput);
        assert_eq!(
            waiting_instance.step_states["wait_for_input"].status,
            StepStatus::WaitingOnInput
        );
        assert_eq!(waiting_instance.step_states["maybe_fail"].status, StepStatus::Skipped);
        assert_eq!(waiting_instance.step_states["join"].status, StepStatus::Pending);

        engine
            .respond_to_gate(instance_id, "wait_for_input", serde_json::json!({"selected": "yes"}))
            .await
            .unwrap();

        // Workflow continues in background — wait for it to settle
        wait_for_settled(&*store, instance_id, std::time::Duration::from_secs(5)).await;

        let completed_instance = store.get_instance(instance_id).unwrap().unwrap();
        assert_eq!(completed_instance.status, WorkflowStatus::Completed);
        assert_eq!(completed_instance.step_states["join"].status, StepStatus::Completed);
    }

    #[tokio::test]
    async fn test_semaphore_bounds_concurrency() {
        use std::sync::atomic::AtomicUsize;

        // A step executor that tracks peak concurrency via atomics.
        struct ConcurrencyTracker {
            current: AtomicUsize,
            peak: AtomicUsize,
        }
        impl ConcurrencyTracker {
            fn new() -> Self {
                Self { current: AtomicUsize::new(0), peak: AtomicUsize::new(0) }
            }
        }
        #[async_trait]
        impl StepExecutor for ConcurrencyTracker {
            async fn call_tool(
                &self,
                _: &str,
                _: Value,
                _: &ExecutionContext,
            ) -> Result<Value, String> {
                let prev = self.current.fetch_add(1, Ordering::SeqCst);
                let now = prev + 1;
                self.peak.fetch_max(now, Ordering::SeqCst);
                // Hold the "slot" briefly to allow overlap detection
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                self.current.fetch_sub(1, Ordering::SeqCst);
                Ok(serde_json::json!({"ok": true}))
            }
            async fn invoke_agent(
                &self,
                _: &str,
                _: &str,
                _: bool,
                _: Option<u64>,
                _: &[PermissionEntry],
                _: Option<&str>,
                _: Option<&str>,
                _: &ExecutionContext,
            ) -> Result<Value, String> {
                Ok(Value::Null)
            }
            async fn signal_agent(
                &self,
                _: &SignalTarget,
                _: &str,
                _: &ExecutionContext,
            ) -> Result<Value, String> {
                Ok(Value::Null)
            }
            async fn wait_for_agent(
                &self,
                _: &str,
                _: Option<u64>,
                _: &ExecutionContext,
            ) -> Result<Value, String> {
                Ok(Value::Null)
            }
            async fn create_feedback_request(
                &self,
                _: i64,
                _: &str,
                _: &str,
                _: Option<&[String]>,
                _: bool,
                _: &ExecutionContext,
            ) -> Result<String, String> {
                Ok(String::new())
            }
            async fn register_event_gate(
                &self,
                _: i64,
                _: &str,
                _: &str,
                _: Option<&str>,
                _: Option<u64>,
                _: &ExecutionContext,
            ) -> Result<String, String> {
                Ok(String::new())
            }
            async fn launch_workflow(
                &self,
                _: &str,
                _: Value,
                _: &ExecutionContext,
            ) -> Result<i64, String> {
                Ok(0)
            }
            async fn schedule_task(
                &self,
                _: &ScheduleTaskDef,
                _: &ExecutionContext,
            ) -> Result<String, String> {
                Ok(String::new())
            }
            async fn render_prompt_template(
                &self,
                _: &str,
                _: &str,
                _: Value,
                _: &ExecutionContext,
            ) -> Result<String, String> {
                Err("render_prompt_template not implemented in test executor".to_string())
            }
        }

        // Build a workflow with 4 parallel task steps (all successors of trigger).
        let def = WorkflowDefinition {
            id: generate_workflow_id(),
            name: "parallel-bounded".into(),
            version: "1.0".into(),
            description: None,
            variables: serde_json::json!({}),
            steps: {
                let mut steps = vec![StepDef {
                    id: "start".into(),
                    step_type: StepType::Trigger {
                        trigger: TriggerDef {
                            trigger_type: TriggerType::Manual {
                                inputs: vec![],
                                input_schema: None,
                            },
                        },
                    },
                    outputs: HashMap::new(),
                    on_error: None,
                    next: (0..4).map(|i| format!("task_{i}")).collect(),
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                }];
                for i in 0..4 {
                    steps.push(StepDef {
                        id: format!("task_{i}"),
                        step_type: StepType::Task {
                            task: TaskDef::CallTool {
                                tool_id: "noop".into(),
                                arguments: HashMap::new(),
                            },
                        },
                        outputs: HashMap::new(),
                        on_error: None,
                        next: vec!["end".into()],
                        timeout_secs: None,
                        designer_x: None,
                        designer_y: None,
                    });
                }
                steps.push(StepDef {
                    id: "end".into(),
                    step_type: StepType::ControlFlow { control: ControlFlowDef::EndWorkflow },
                    outputs: HashMap::new(),
                    on_error: None,
                    next: vec![],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                });
                steps
            },
            output: None,
            requested_tools: vec![],
            permissions: vec![],
            attachments: vec![],
            tests: vec![],
            mode: WorkflowMode::default(),
            result_message: None,
            bundled: false,
            archived: false,
            triggers_paused: false,
        };

        let store = Arc::new(WorkflowStore::in_memory().unwrap());
        let tracker = Arc::new(ConcurrencyTracker::new());
        let emitter = Arc::new(TestEmitter::new());

        // Set semaphore to 2 — only 2 steps should run at a time.
        let engine = WorkflowEngine::with_concurrency(
            store.clone(),
            tracker.clone() as Arc<dyn StepExecutor>,
            emitter,
            2,
        );

        let instance_id = engine
            .launch(def, serde_json::json!({}), "s1".into(), None, vec![], None)
            .await
            .unwrap();

        let instance = store.get_instance(instance_id).unwrap().unwrap();
        assert_eq!(instance.status, WorkflowStatus::Completed);

        // Peak concurrency must not exceed the semaphore bound.
        let peak = tracker.peak.load(Ordering::SeqCst);
        assert!(peak <= 2, "peak concurrency was {peak}, expected <= 2");
    }

    #[tokio::test]
    async fn test_recover_running_instance() {
        let store = Arc::new(WorkflowStore::in_memory().unwrap());
        let executor = Arc::new(TestExecutor::new());
        let emitter = Arc::new(TestEmitter::new());

        // Launch a workflow so we have a definition + instance in the store.
        let engine = WorkflowEngine::new(store.clone(), executor.clone(), emitter.clone());
        let instance_id = engine
            .launch(
                linear_workflow(),
                serde_json::json!({"msg": "test"}),
                "session-1".into(),
                None,
                vec![],
                None,
            )
            .await
            .unwrap();

        // The workflow should have completed normally.
        let instance = store.get_instance(instance_id).unwrap().unwrap();
        assert_eq!(instance.status, WorkflowStatus::Completed);

        // Now simulate a crash: manually set instance status back to Running
        // and one step back to Running (as if the process died mid-execution).
        let mut instance = instance;
        instance.status = WorkflowStatus::Running;
        instance.completed_at_ms = None;
        instance.step_states.get_mut("process").unwrap().status = StepStatus::Running;
        instance.step_states.get_mut("process").unwrap().started_at_ms = Some(now_ms());
        instance.step_states.get_mut("end").unwrap().status = StepStatus::Pending;
        instance.step_states.get_mut("end").unwrap().completed_at_ms = None;
        store.update_instance(&instance).unwrap();

        // Build a fresh engine (simulating daemon restart) and recover.
        let engine2 = WorkflowEngine::new(store.clone(), executor.clone(), emitter.clone());
        let handles = engine2.recover_instances().await.unwrap();
        assert_eq!(handles.len(), 1);

        // Await the recovery task instead of sleeping.
        for handle in handles {
            handle.await.unwrap();
        }

        // The instance should now be completed again.
        let instance = store.get_instance(instance_id).unwrap().unwrap();
        assert_eq!(
            instance.status,
            WorkflowStatus::Completed,
            "instance should be completed after recovery"
        );
        assert_eq!(
            instance.step_states["process"].status,
            StepStatus::Completed,
            "process step should be completed after recovery"
        );
    }

    // ── SetVariable tests ──────────────────────────────────────────────

    fn set_variable_workflow() -> WorkflowDefinition {
        WorkflowDefinition {
            id: generate_workflow_id(),
            name: "set-var-test".into(),
            version: "1.0".into(),
            description: None,
            variables: serde_json::json!({
                "type": "object",
                "properties": {
                    "counter": { "type": "number", "default": 0 },
                    "items": { "type": "array", "default": [] },
                    "meta": { "type": "object", "default": {} }
                }
            }),
            steps: vec![
                StepDef {
                    id: "start".into(),
                    step_type: StepType::Trigger {
                        trigger: TriggerDef {
                            trigger_type: TriggerType::Manual {
                                inputs: vec![],
                                input_schema: None,
                            },
                        },
                    },
                    outputs: HashMap::new(),
                    on_error: None,
                    next: vec!["set_vars".into()],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                },
                StepDef {
                    id: "set_vars".into(),
                    step_type: StepType::Task {
                        task: TaskDef::SetVariable {
                            assignments: vec![
                                VariableAssignment {
                                    variable: "counter".into(),
                                    value: "{{trigger.new_count}}".into(),
                                    operation: AssignOp::Set,
                                },
                                VariableAssignment {
                                    variable: "items".into(),
                                    value: "{{trigger.item}}".into(),
                                    operation: AssignOp::AppendList,
                                },
                                VariableAssignment {
                                    variable: "meta".into(),
                                    value: "{{trigger.extra}}".into(),
                                    operation: AssignOp::MergeMap,
                                },
                            ],
                        },
                    },
                    outputs: HashMap::new(),
                    on_error: None,
                    next: vec!["read_vars".into()],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                },
                StepDef {
                    id: "read_vars".into(),
                    step_type: StepType::Task {
                        task: TaskDef::CallTool {
                            tool_id: "reader".into(),
                            arguments: HashMap::from([(
                                "count".into(),
                                "{{variables.counter}}".into(),
                            )]),
                        },
                    },
                    outputs: HashMap::new(),
                    on_error: None,
                    next: vec![],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                },
            ],
            output: None,
            requested_tools: vec![],
            permissions: vec![],
            attachments: vec![],
            tests: vec![],
            mode: WorkflowMode::default(),
            result_message: None,
            bundled: false,
            archived: false,
            triggers_paused: false,
        }
    }

    #[tokio::test]
    async fn test_set_variable_set_operation() {
        let store = Arc::new(WorkflowStore::in_memory().unwrap());
        let executor = Arc::new(TestExecutor::new());
        let emitter = Arc::new(TestEmitter::new());
        let engine = WorkflowEngine::new(store.clone(), executor.clone(), emitter.clone());

        let instance_id = engine
            .launch(
                set_variable_workflow(),
                serde_json::json!({
                    "new_count": 42,
                    "item": "hello",
                    "extra": { "key": "value" }
                }),
                "session-1".into(),
                None,
                vec![],
                None,
            )
            .await
            .unwrap();

        let instance = store.get_instance(instance_id).unwrap().unwrap();
        assert_eq!(instance.status, WorkflowStatus::Completed);

        // Verify set operation
        assert_eq!(instance.variables["counter"], 42);

        // Verify append_list operation
        let items = instance.variables["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0], "hello");

        // Verify merge_map operation
        assert_eq!(instance.variables["meta"]["key"], "value");
    }

    #[tokio::test]
    async fn test_set_variable_append_list_creates_array() {
        let mut def = set_variable_workflow();
        def.variables = serde_json::json!({
            "type": "object",
            "properties": {
                "counter": { "type": "number", "default": 0 }
            }
        });
        def.steps[1] = StepDef {
            id: "set_vars".into(),
            step_type: StepType::Task {
                task: TaskDef::SetVariable {
                    assignments: vec![VariableAssignment {
                        variable: "new_list".into(),
                        value: "first_item".into(),
                        operation: AssignOp::AppendList,
                    }],
                },
            },
            outputs: HashMap::new(),
            on_error: None,
            next: vec!["read_vars".into()],
            timeout_secs: None,
            designer_x: None,
            designer_y: None,
        };

        let store = Arc::new(WorkflowStore::in_memory().unwrap());
        let executor = Arc::new(TestExecutor::new());
        let emitter = Arc::new(TestEmitter::new());
        let engine = WorkflowEngine::new(store.clone(), executor.clone(), emitter.clone());

        let instance_id = engine
            .launch(def, serde_json::json!({}), "session-1".into(), None, vec![], None)
            .await
            .unwrap();

        let instance = store.get_instance(instance_id).unwrap().unwrap();
        assert_eq!(instance.status, WorkflowStatus::Completed);

        let list = instance.variables["new_list"].as_array().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0], "first_item");
    }

    #[tokio::test]
    async fn test_set_variable_downstream_reads_updated_vars() {
        let store = Arc::new(WorkflowStore::in_memory().unwrap());
        let executor = Arc::new(TestExecutor::new());
        let emitter = Arc::new(TestEmitter::new());
        let engine = WorkflowEngine::new(store.clone(), executor.clone(), emitter.clone());

        let instance_id = engine
            .launch(
                set_variable_workflow(),
                serde_json::json!({
                    "new_count": 99,
                    "item": "x",
                    "extra": {}
                }),
                "session-1".into(),
                None,
                vec![],
                None,
            )
            .await
            .unwrap();

        let instance = store.get_instance(instance_id).unwrap().unwrap();
        assert_eq!(instance.status, WorkflowStatus::Completed);
        assert_eq!(instance.step_states["read_vars"].status, StepStatus::Completed);
        assert_eq!(instance.variables["counter"], 99);
    }
}
