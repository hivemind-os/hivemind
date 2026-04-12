pub mod action;
mod sqlite_store;
pub mod ssrf;
mod store;

pub use sqlite_store::{InitialSequences, SqliteSchedulerStore};
pub use store::SchedulerStore;

use std::path::PathBuf;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use cron::Schedule;
use hive_contracts::permissions::PermissionRule;
pub use hive_contracts::{
    CreateTaskRequest, ListTasksFilter, ScheduledTask, TaskAction, TaskCompletionNotification,
    TaskRun, TaskRunStatus, TaskSchedule, TaskStatus, UpdateTaskRequest,
};
use hive_core::EventBus;
use serde_json::Value;
use tracing::Instrument;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the scheduler service.
#[derive(Debug, Clone)]
pub struct SchedulerConfig {
    /// How often the background loop checks for due tasks (seconds).
    pub poll_interval_secs: u64,
    /// Maximum tasks to execute concurrently within a single tick.
    pub max_concurrent_tasks: usize,
    /// Maximum run history entries to keep per recurring task.
    pub max_run_history: usize,
    /// Maximum active (pending + running) tasks per session.
    pub max_active_tasks_per_session: usize,
    /// Maximum active (pending + running) tasks globally.
    pub max_active_tasks_global: usize,
    /// Maximum length for task names.
    pub max_task_name_len: usize,
    /// Maximum length for task descriptions.
    pub max_task_description_len: usize,
    /// Aggregate timeout in seconds for composite actions.
    pub composite_action_timeout_secs: u64,
    /// Timeout in seconds for individual action execution.
    pub action_timeout_secs: u64,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            poll_interval_secs: 5,
            max_concurrent_tasks: 10,
            max_run_history: 100,
            max_active_tasks_per_session: 100,
            max_active_tasks_global: 1000,
            max_task_name_len: 256,
            max_task_description_len: 4096,
            composite_action_timeout_secs: 600,
            action_timeout_secs: 300,
        }
    }
}

// ---------------------------------------------------------------------------
// Extension traits — implemented by hive-api to avoid circular deps
// ---------------------------------------------------------------------------

/// Executes tools on behalf of the scheduler (for `CallTool` actions).
#[async_trait]
pub trait SchedulerToolExecutor: Send + Sync {
    /// Execute a tool by ID with the given JSON arguments.
    /// Returns the tool output as a JSON value on success.
    async fn execute_tool(&self, tool_id: &str, arguments: Value) -> Result<Value, String>;

    /// List available tool IDs (for validation at task creation time).
    fn list_tool_ids(&self) -> Vec<String>;

    /// Check whether a tool exists and is not denied.
    fn is_tool_available(&self, tool_id: &str) -> bool;

    /// Resolve a potentially-sanitized or prefixed tool ID back to its canonical form.
    /// Returns `Some(canonical_id)` if the raw ID can be mapped, `None` otherwise.
    ///
    /// Handles:
    /// - Exact match (already canonical)
    /// - OpenAI `functions.` prefix stripping
    /// - Provider-specific sanitization reversal (e.g. `comm_send_external_message` → `comm.send_external_message`)
    fn resolve_tool_id(&self, raw_id: &str) -> Option<String>;

    /// Get the default approval level for a tool by its canonical ID.
    /// Returns `None` if the tool is not found.
    fn get_tool_approval(&self, tool_id: &str) -> Option<hive_contracts::ToolApproval>;
}

/// Runs agents on behalf of the scheduler (for `InvokeAgent` actions).
#[async_trait]
pub trait SchedulerAgentRunner: Send + Sync {
    /// Spawn an agent with the given persona and task, wait for completion
    /// (up to `timeout_secs`), and return the agent's final result.
    /// If the agent times out, it should be killed and an error returned.
    async fn run_agent(
        &self,
        persona_id: &str,
        task: &str,
        friendly_name: Option<String>,
        timeout_secs: u64,
        permissions: Option<Vec<PermissionRule>>,
    ) -> Result<Option<String>, String>;
}

/// Sends notifications to originators when scheduled tasks complete.
#[async_trait]
pub trait SchedulerNotifier: Send + Sync {
    /// Notify the originating agent that a task completed.
    async fn notify_agent(
        &self,
        agent_id: &str,
        notification: TaskCompletionNotification,
    ) -> Result<(), String>;

    /// Notify the originating session that a task completed (via EventBus or SSE).
    async fn notify_session(
        &self,
        session_id: &str,
        notification: TaskCompletionNotification,
    ) -> Result<(), String>;
}

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum SchedulerError {
    #[error("task not found: {id}")]
    TaskNotFound { id: String },

    #[error("database error: {0}")]
    Database(String),

    #[error("internal error: {0}")]
    Internal(String),
}

// ---------------------------------------------------------------------------
// Service
// ---------------------------------------------------------------------------

pub struct SchedulerService {
    store: Arc<dyn SchedulerStore>,
    event_bus: EventBus,
    running: AtomicBool,
    task_seq: std::sync::atomic::AtomicU64,
    run_seq: std::sync::atomic::AtomicU64,
    http_client: reqwest::Client,
    daemon_addr: String,
    tool_executor: Option<Arc<dyn SchedulerToolExecutor>>,
    agent_runner: Option<Arc<dyn SchedulerAgentRunner>>,
    notifier: Option<Arc<dyn SchedulerNotifier>>,
    auth_token: Option<String>,
    /// Signals the background loop to wake up and check the stop flag.
    stop_notify: Arc<tokio::sync::Notify>,
    /// Handle to the background loop task, used to await clean shutdown.
    loop_handle: parking_lot::Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// Scheduler configuration.
    config: SchedulerConfig,
    /// Registry of action executors, lazily initialized on first use.
    action_registry: std::sync::OnceLock<Arc<action::ActionRegistry>>,
    /// Shared semaphore limiting concurrent task execution across ticks.
    task_semaphore: Arc<tokio::sync::Semaphore>,
}

impl SchedulerService {
    pub fn new(
        db_path: PathBuf,
        event_bus: EventBus,
        daemon_addr: String,
        config: SchedulerConfig,
    ) -> Result<Self> {
        let (sqlite_store, seqs) = SqliteSchedulerStore::new(db_path)?;
        Self::from_store(Arc::new(sqlite_store), seqs, event_bus, daemon_addr, config)
    }

    /// Create a service backed by an in-memory database (for tests).
    pub fn in_memory(event_bus: EventBus, config: SchedulerConfig) -> Result<Self> {
        let (sqlite_store, seqs) = SqliteSchedulerStore::in_memory()?;
        Self::from_store(Arc::new(sqlite_store), seqs, event_bus, "127.0.0.1:0".to_string(), config)
    }

    /// Create a service backed by an in-memory database with a custom daemon address.
    pub fn in_memory_with_addr(
        event_bus: EventBus,
        daemon_addr: String,
        config: SchedulerConfig,
    ) -> Result<Self> {
        let (sqlite_store, seqs) = SqliteSchedulerStore::in_memory()?;
        Self::from_store(Arc::new(sqlite_store), seqs, event_bus, daemon_addr, config)
    }

    fn from_store(
        store: Arc<dyn SchedulerStore>,
        seqs: InitialSequences,
        event_bus: EventBus,
        daemon_addr: String,
        config: SchedulerConfig,
    ) -> Result<Self> {
        let http_client = {
            let builder = reqwest::Client::builder().timeout(std::time::Duration::from_secs(30));
            #[cfg(not(feature = "allow-localhost"))]
            let builder = builder.dns_resolver(Arc::new(ssrf::SsrfSafeResolver));
            builder.build().unwrap_or_default()
        };
        Ok(Self {
            store,
            event_bus,
            running: AtomicBool::new(false),
            task_seq: std::sync::atomic::AtomicU64::new(seqs.task_seq),
            run_seq: std::sync::atomic::AtomicU64::new(seqs.run_seq),
            http_client,
            daemon_addr,
            tool_executor: None,
            agent_runner: None,
            notifier: None,
            auth_token: None,
            stop_notify: Arc::new(tokio::sync::Notify::new()),
            loop_handle: parking_lot::Mutex::new(None),
            task_semaphore: Arc::new(tokio::sync::Semaphore::new(config.max_concurrent_tasks)),
            config,
            action_registry: std::sync::OnceLock::new(),
        })
    }

    /// Set the tool executor for `CallTool` actions.
    pub fn set_tool_executor(&mut self, executor: Arc<dyn SchedulerToolExecutor>) {
        self.tool_executor = Some(executor);
    }

    /// Set the agent runner for `InvokeAgent` actions.
    pub fn set_agent_runner(&mut self, runner: Arc<dyn SchedulerAgentRunner>) {
        self.agent_runner = Some(runner);
    }

    /// Set the notifier for originator notifications.
    pub fn set_notifier(&mut self, notifier: Arc<dyn SchedulerNotifier>) {
        self.notifier = Some(notifier);
    }

    /// Set the authentication token for daemon API requests.
    pub fn set_auth_token(&mut self, token: String) {
        self.auth_token = Some(token);
    }

    /// Get a reference to the tool executor (for pre-validation by tools).
    pub fn tool_executor(&self) -> Option<&Arc<dyn SchedulerToolExecutor>> {
        self.tool_executor.as_ref()
    }

    // ----- Validation --------------------------------------------------------

    fn validate_action(&self, action: &TaskAction) -> Result<(), SchedulerError> {
        match action {
            TaskAction::CallTool { tool_id, .. } => {
                if let Some(ref executor) = self.tool_executor {
                    // Try resolving first, then fall back to direct check
                    let resolved = executor.resolve_tool_id(tool_id);
                    let canonical = resolved.as_deref().unwrap_or(tool_id);
                    if !executor.is_tool_available(canonical) {
                        return Err(SchedulerError::Internal(format!(
                            "tool not found: {tool_id}. Use list_tool_ids() to see available tools."
                        )));
                    }
                }
                Ok(())
            }
            TaskAction::CompositeAction { actions, .. } => {
                for sub in actions {
                    if matches!(sub, TaskAction::CompositeAction { .. }) {
                        return Err(SchedulerError::Internal(
                            "nested CompositeAction is not allowed".to_string(),
                        ));
                    }
                    self.validate_action(sub)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    // ----- CRUD -------------------------------------------------------------

    pub fn create_task(&self, request: CreateTaskRequest) -> Result<ScheduledTask, SchedulerError> {
        // Validate action before creating
        self.validate_action(&request.action)?;

        // Input validation
        if request.name.is_empty() || request.name.trim().is_empty() {
            return Err(SchedulerError::Internal("task name cannot be empty".to_string()));
        }
        if request.name.len() > self.config.max_task_name_len {
            return Err(SchedulerError::Internal(format!(
                "task name too long (max {} chars)",
                self.config.max_task_name_len
            )));
        }
        if let Some(ref desc) = request.description {
            if desc.len() > self.config.max_task_description_len {
                return Err(SchedulerError::Internal(format!(
                    "task description too long (max {} chars)",
                    self.config.max_task_description_len
                )));
            }
        }

        // Rate limiting
        let active_global = self.store.count_active_tasks();
        if active_global >= self.config.max_active_tasks_global {
            return Err(SchedulerError::Internal(format!(
                "global task limit reached ({} active tasks, max {})",
                active_global, self.config.max_active_tasks_global
            )));
        }
        if let Some(ref session_id) = request.owner_session_id {
            let active_session = self.store.count_active_tasks_for_session(session_id);
            if active_session >= self.config.max_active_tasks_per_session {
                return Err(SchedulerError::Internal(format!(
                    "per-session task limit reached ({} active tasks, max {})",
                    active_session, self.config.max_active_tasks_per_session
                )));
            }
        }

        let now = now_ms();
        let id = format!("task-{}", self.task_seq.fetch_add(1, Ordering::Relaxed));
        let next_run = compute_next_run(&request.schedule, now)?;
        let schedule_json = serde_json::to_string(&request.schedule)
            .map_err(|e| SchedulerError::Internal(e.to_string()))?;
        let action_json = serde_json::to_string(&request.action)
            .map_err(|e| SchedulerError::Internal(e.to_string()))?;

        let task = ScheduledTask {
            id: id.clone(),
            name: request.name,
            description: request.description.unwrap_or_default(),
            schedule_type: request.schedule.schedule_type().to_string(),
            action_type: request.action.action_type().to_string(),
            schedule: request.schedule,
            action: request.action,
            status: TaskStatus::Pending,
            created_at_ms: now,
            updated_at_ms: now,
            completed_at_ms: None,
            last_run_ms: None,
            next_run_ms: next_run,
            run_count: 0,
            last_error: None,
            owner_session_id: request.owner_session_id,
            owner_agent_id: request.owner_agent_id,
            max_retries: request.max_retries,
            retry_delay_ms: request.retry_delay_ms,
            retry_count: 0,
        };

        self.store.insert_task(&task, &schedule_json, &action_json)?;

        let _ = self.event_bus.publish(
            "scheduler.task.created",
            "scheduler",
            serde_json::json!({ "task_id": task.id }),
        );

        Ok(task)
    }

    pub fn list_tasks(&self) -> Vec<ScheduledTask> {
        self.list_tasks_filtered(&ListTasksFilter::default()).unwrap_or_default()
    }

    pub fn list_tasks_filtered(
        &self,
        filter: &ListTasksFilter,
    ) -> Result<Vec<ScheduledTask>, SchedulerError> {
        self.store.list_tasks_filtered(filter)
    }

    pub fn get_task(&self, task_id: &str) -> Result<ScheduledTask, SchedulerError> {
        self.store.get_task(task_id)
    }

    pub fn cancel_task(&self, task_id: &str) -> Result<ScheduledTask, SchedulerError> {
        let now = now_ms();
        let affected = self.store.cancel_task(task_id, now)?;
        if affected == 0 && !self.store.task_exists(task_id) {
            return Err(SchedulerError::TaskNotFound { id: task_id.to_string() });
        }
        // Task exists but cannot be cancelled (already completed/failed/cancelled) – return as-is
        let task = self.get_task(task_id)?;
        if affected > 0 {
            let _ = self.event_bus.publish(
                "scheduler.task.cancelled",
                "scheduler",
                serde_json::json!({ "task_id": task_id }),
            );
        }
        Ok(task)
    }

    pub fn delete_task(&self, task_id: &str) -> Result<(), SchedulerError> {
        let affected = self.store.delete_task(task_id)?;
        if affected == 0 {
            return Err(SchedulerError::TaskNotFound { id: task_id.to_string() });
        }
        let _ = self.event_bus.publish(
            "scheduler.task.deleted",
            "scheduler",
            serde_json::json!({ "task_id": task_id }),
        );
        Ok(())
    }

    pub fn list_tasks_for_workflow(
        &self,
        definition: &str,
    ) -> Result<Vec<ScheduledTask>, SchedulerError> {
        self.store.list_tasks_for_workflow(definition)
    }

    pub fn update_task(
        &self,
        task_id: &str,
        request: UpdateTaskRequest,
    ) -> Result<ScheduledTask, SchedulerError> {
        // Verify the task exists
        let existing = self.get_task(task_id)?;

        let now = now_ms();

        // Input validation
        if let Some(ref name) = request.name {
            if name.is_empty() || name.trim().is_empty() {
                return Err(SchedulerError::Internal("task name cannot be empty".to_string()));
            }
            if name.len() > self.config.max_task_name_len {
                return Err(SchedulerError::Internal(format!(
                    "task name too long (max {} chars)",
                    self.config.max_task_name_len
                )));
            }
        }
        if let Some(ref desc) = request.description {
            if desc.len() > self.config.max_task_description_len {
                return Err(SchedulerError::Internal(format!(
                    "task description too long (max {} chars)",
                    self.config.max_task_description_len
                )));
            }
        }

        // Pre-validate action before touching the store
        if let Some(ref action) = request.action {
            self.validate_action(action)?;
        }

        // Serialize fields that need it
        let schedule_json = request
            .schedule
            .as_ref()
            .map(|s| serde_json::to_string(s).map_err(|e| SchedulerError::Internal(e.to_string())))
            .transpose()?;
        let next_run = request.schedule.as_ref().map(|s| compute_next_run(s, now)).transpose()?;
        let action_json = request
            .action
            .as_ref()
            .map(|a| serde_json::to_string(a).map_err(|e| SchedulerError::Internal(e.to_string())))
            .transpose()?;

        // Extract denormalized fields
        let schedule_type = request.schedule.as_ref().map(|s| s.schedule_type());
        let cron_expression = request.schedule.as_ref().map(|s| s.cron_expression());
        let run_at_ms = request.schedule.as_ref().map(|s| s.run_at_ms());
        let action_type = request.action.as_ref().map(|a| a.action_type());

        self.store.update_task_atomic(
            task_id,
            request.name.as_deref(),
            request.description.as_deref(),
            schedule_json.as_deref(),
            schedule_type,
            cron_expression,
            run_at_ms,
            next_run,
            action_json.as_deref(),
            action_type,
            now,
            request.max_retries,
            request.retry_delay_ms,
        )?;

        // If the task was failed/completed/cancelled and we changed its schedule, reset to pending
        if request.schedule.is_some()
            && matches!(
                existing.status,
                TaskStatus::Failed | TaskStatus::Completed | TaskStatus::Cancelled
            )
        {
            let _ = self.store.reset_task_to_pending(task_id, now);
        }

        let _ = self.event_bus.publish(
            "scheduler.task.updated",
            "scheduler",
            serde_json::json!({ "task_id": task_id }),
        );

        self.get_task(task_id)
    }

    pub fn list_task_runs(&self, task_id: &str) -> Result<Vec<TaskRun>, SchedulerError> {
        // Verify task exists
        self.get_task(task_id)?;

        self.store.list_task_runs(task_id, self.config.max_run_history)
    }

    // ----- Background loop --------------------------------------------------

    /// Start the background loop that checks for due tasks every 5 seconds.
    pub fn start_background_loop(self: &Arc<Self>) {
        let mut handle_guard = self.loop_handle.lock();
        if self.running.load(Ordering::SeqCst) {
            return; // already running
        }
        *handle_guard = None;
        self.running.store(true, Ordering::SeqCst);

        let svc = Arc::clone(self);
        let stop_notify = Arc::clone(&self.stop_notify);
        let handle = tokio::spawn(async move {
            tracing::info!("scheduler background loop started");
            loop {
                if !svc.running.load(Ordering::SeqCst) {
                    tracing::info!("scheduler background loop stopping");
                    break;
                }
                svc.tick().await;
                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_secs(svc.config.poll_interval_secs)) => {}
                    _ = stop_notify.notified() => {}
                }
            }
        }.instrument(tracing::info_span!("service", service = "scheduler")));
        *handle_guard = Some(handle);
    }

    /// Stop the background loop and wait for it to finish.
    pub async fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
        self.stop_notify.notify_one();
        let handle = self.loop_handle.lock().take();
        if let Some(h) = handle {
            let _ = h.await;
        }
    }

    /// Returns `true` if the background loop is running.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Force all pending tasks to become due immediately (for testing).
    #[cfg(any(test, feature = "test-support"))]
    pub fn force_all_due(&self) {
        self.store.force_all_due();
    }

    /// Single tick — find due tasks and execute them concurrently (bounded).
    pub async fn tick(self: &Arc<Self>) {
        let now = now_ms();
        let due_ids = self.store.get_due_task_ids(now);

        if due_ids.is_empty() {
            return;
        }

        let semaphore = Arc::clone(&self.task_semaphore);
        let mut join_handles = Vec::with_capacity(due_ids.len());

        for task_id in due_ids {
            let permit = match semaphore.clone().acquire_owned().await {
                Ok(p) => p,
                Err(_) => break,
            };
            let svc = Arc::clone(self);
            join_handles.push(tokio::spawn(async move {
                svc.execute_task(&task_id).await;
                drop(permit);
            }));
        }

        for handle in join_handles {
            let _ = handle.await;
        }
    }

    async fn execute_task(&self, task_id: &str) {
        let started_at = now_ms();
        let run_seq = self.run_seq.fetch_add(1, Ordering::Relaxed);
        let run_id = format!("run-{task_id}-{started_at}-{run_seq}");

        // Mark running
        if !self.store.mark_task_running(task_id, started_at) {
            return;
        }

        let task = match self.get_task(task_id) {
            Ok(t) => t,
            Err(_) => return,
        };

        let timeout = std::time::Duration::from_secs(self.config.action_timeout_secs);
        let action_result = match tokio::time::timeout(timeout, self.execute_action(&task.action))
            .await
        {
            Ok(result) => result,
            Err(_) => Err(format!("action timed out after {}s", self.config.action_timeout_secs)),
        };

        let now = now_ms();
        let (is_success, result_value, error_msg) = match &action_result {
            Ok(v) => (true, v.clone(), None),
            Err(e) => (false, None, Some(e.clone())),
        };

        // Record to DB
        self.record_run_result(
            task_id,
            &run_id,
            &task.schedule,
            started_at,
            now,
            is_success,
            &result_value,
            &error_msg,
            task.max_retries,
            task.retry_delay_ms,
            task.retry_count,
        );

        // Notify originator
        self.notify_originator(
            &task,
            &run_id,
            is_success,
            &result_value,
            &error_msg,
            started_at,
            now,
        )
        .await;
    }

    /// Execute a single action and return an optional structured result.
    fn execute_action<'a>(
        &'a self,
        action: &'a TaskAction,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Option<Value>, String>> + Send + 'a>,
    > {
        let registry = self.get_action_registry();
        Box::pin(async move { registry.execute(action).await })
    }

    /// Lazily build and return the action executor registry.
    fn get_action_registry(&self) -> Arc<action::ActionRegistry> {
        self.action_registry
            .get_or_init(|| {
                let registry = Arc::new(action::ActionRegistry::new());
                registry.register(
                    "EmitEvent",
                    Arc::new(action::EmitEventExecutor { event_bus: self.event_bus.clone() }),
                );
                registry.register(
                    "SendMessage",
                    Arc::new(action::SendMessageExecutor {
                        http_client: self.http_client.clone(),
                        daemon_addr: self.daemon_addr.clone(),
                        auth_token: self.auth_token.clone(),
                    }),
                );
                registry.register(
                    "HttpWebhook",
                    Arc::new(action::HttpWebhookExecutor { http_client: self.http_client.clone() }),
                );
                registry.register(
                    "InvokeAgent",
                    Arc::new(action::InvokeAgentExecutor {
                        agent_runner: self.agent_runner.clone(),
                    }),
                );
                registry.register(
                    "CallTool",
                    Arc::new(action::CallToolExecutor {
                        tool_executor: self.tool_executor.clone(),
                    }),
                );
                registry.register(
                    "LaunchWorkflow",
                    Arc::new(action::LaunchWorkflowExecutor {
                        http_client: self.http_client.clone(),
                        daemon_addr: self.daemon_addr.clone(),
                        auth_token: self.auth_token.clone(),
                    }),
                );
                registry.register(
                    "CompositeAction",
                    Arc::new(action::CompositeActionExecutor {
                        registry: registry.clone(),
                        timeout_secs: self.config.composite_action_timeout_secs,
                    }),
                );
                registry
            })
            .clone()
    }

    /// Record a task run result to the database and update task status.
    #[allow(clippy::too_many_arguments)]
    fn record_run_result(
        &self,
        task_id: &str,
        run_id: &str,
        schedule: &TaskSchedule,
        started_at: u64,
        completed_at: u64,
        is_success: bool,
        result_value: &Option<Value>,
        error_msg: &Option<String>,
        max_retries: Option<u32>,
        retry_delay_ms: Option<u64>,
        current_retry_count: u32,
    ) {
        let result_json = result_value.as_ref().and_then(|v| serde_json::to_string(v).ok());
        let result_json_ref = result_json.as_deref();
        let is_recurring = matches!(schedule, TaskSchedule::Cron { .. });

        // Helper: update task and handle cancellation race (0 rows = cancelled externally).
        let do_update = |store: &dyn SchedulerStore,
                         task_id: &str,
                         status: &str,
                         updated_at_ms: u64,
                         last_run_ms: u64,
                         next_run_ms: Option<u64>,
                         last_error: Option<&str>,
                         reset_retry_count: bool,
                         new_retry_count: Option<u32>,
                         completed_at_ms: Option<u64>| {
            match store.update_task_after_run(
                task_id,
                status,
                updated_at_ms,
                last_run_ms,
                next_run_ms,
                last_error,
                reset_retry_count,
                new_retry_count,
                completed_at_ms,
            ) {
                Ok(0) => {
                    tracing::warn!(
                        task_id,
                        attempted_status = status,
                        "task status not updated (0 rows affected — likely cancelled externally)"
                    );
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::error!(task_id, error = %e, "failed to update task after run");
                }
            }
        };

        if is_success {
            let next = if is_recurring {
                match compute_next_run(schedule, completed_at) {
                    Ok(next) => next,
                    Err(err) => {
                        let err = err.to_string();
                        do_update(
                            self.store.as_ref(),
                            task_id,
                            "failed",
                            completed_at,
                            completed_at,
                            None,
                            Some(&err),
                            false,
                            None,
                            Some(completed_at),
                        );
                        if let Err(e) = self.store.insert_task_run(
                            run_id,
                            task_id,
                            started_at,
                            completed_at,
                            "failure",
                            Some(&err),
                            None,
                        ) {
                            tracing::error!(task_id, error = %e, "failed to insert task run");
                        }
                        return;
                    }
                }
            } else {
                None
            };

            let new_status = if is_recurring { "pending" } else { "completed" };
            let terminal_ts = if is_recurring { None } else { Some(completed_at) };
            do_update(
                self.store.as_ref(),
                task_id,
                new_status,
                completed_at,
                completed_at,
                next,
                None,
                true,
                None,
                terminal_ts,
            );
            if let Err(e) = self.store.insert_task_run(
                run_id,
                task_id,
                started_at,
                completed_at,
                "success",
                None,
                result_json_ref,
            ) {
                tracing::error!(task_id, error = %e, "failed to insert task run");
            }
        } else {
            let err = error_msg.as_deref().unwrap_or("unknown error");
            if is_recurring {
                // Cron retry enforcement: respect max_retries for recurring tasks too.
                // max_retries means "max number of retries", so we allow up to max retries
                // (same semantics as non-recurring path).
                let new_retry_count = current_retry_count + 1;
                let exhausted = max_retries.map(|max| new_retry_count > max).unwrap_or(false);

                if exhausted {
                    // Exhausted retries — mark as permanently failed.
                    do_update(
                        self.store.as_ref(),
                        task_id,
                        "failed",
                        completed_at,
                        completed_at,
                        None,
                        Some(err),
                        false,
                        Some(new_retry_count),
                        Some(completed_at),
                    );
                } else {
                    match compute_next_run(schedule, completed_at) {
                        Ok(next) => {
                            do_update(
                                self.store.as_ref(),
                                task_id,
                                "pending",
                                completed_at,
                                completed_at,
                                next,
                                Some(err),
                                false,
                                Some(new_retry_count),
                                None,
                            );
                        }
                        Err(e) => {
                            let combined = format!("{err}; next_run error: {e}");
                            do_update(
                                self.store.as_ref(),
                                task_id,
                                "failed",
                                completed_at,
                                completed_at,
                                None,
                                Some(&combined),
                                false,
                                Some(new_retry_count),
                                Some(completed_at),
                            );
                        }
                    }
                }
            } else {
                // Non-recurring failure: check retry policy
                let can_retry = max_retries.map(|max| current_retry_count < max).unwrap_or(false);
                if can_retry {
                    let delay = retry_delay_ms.unwrap_or(0);
                    let next_retry = completed_at + delay;
                    let new_retry_count = current_retry_count + 1;
                    do_update(
                        self.store.as_ref(),
                        task_id,
                        "pending",
                        completed_at,
                        completed_at,
                        Some(next_retry),
                        Some(err),
                        false,
                        Some(new_retry_count),
                        None,
                    );
                } else {
                    do_update(
                        self.store.as_ref(),
                        task_id,
                        "failed",
                        completed_at,
                        completed_at,
                        None,
                        Some(err),
                        false,
                        None,
                        Some(completed_at),
                    );
                }
            }
            if let Err(e) = self.store.insert_task_run(
                run_id,
                task_id,
                started_at,
                completed_at,
                "failure",
                Some(err),
                result_json_ref,
            ) {
                tracing::error!(task_id, error = %e, "failed to insert task run");
            }
        }

        // Prune old runs for recurring tasks to prevent unbounded growth.
        if is_recurring {
            self.store.prune_old_runs(task_id, self.config.max_run_history);
        }
    }

    /// Notify the originator (agent or session) and publish EventBus event.
    #[allow(clippy::too_many_arguments)]
    async fn notify_originator(
        &self,
        task: &ScheduledTask,
        run_id: &str,
        is_success: bool,
        result_value: &Option<Value>,
        error_msg: &Option<String>,
        started_at: u64,
        completed_at: u64,
    ) {
        let notification = TaskCompletionNotification {
            task_id: task.id.clone(),
            task_name: task.name.clone(),
            run_id: run_id.to_string(),
            status: if is_success { TaskRunStatus::Success } else { TaskRunStatus::Failure },
            result: result_value.clone(),
            error: error_msg.clone(),
            started_at_ms: started_at,
            completed_at_ms: completed_at,
        };

        // Always publish to EventBus
        let topic = if is_success { "scheduler.task.completed" } else { "scheduler.task.failed" };
        let payload = serde_json::to_value(&notification).unwrap_or_default();
        let _ = self.event_bus.publish(topic, "scheduler", payload);

        // Notify originating agent if set
        if let Some(ref agent_id) = task.owner_agent_id {
            if let Some(ref notifier) = self.notifier {
                if let Err(e) = notifier.notify_agent(agent_id, notification.clone()).await {
                    tracing::warn!(
                        task_id = %task.id, %agent_id,
                        "Failed to notify originating agent: {e}"
                    );
                }
            }
        }

        // Notify originating session if set
        if let Some(ref session_id) = task.owner_session_id {
            if let Some(ref notifier) = self.notifier {
                if let Err(e) = notifier.notify_session(session_id, notification.clone()).await {
                    tracing::warn!(
                        task_id = %task.id, %session_id,
                        "Failed to notify originating session: {e}"
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

fn compute_next_run(schedule: &TaskSchedule, now: u64) -> Result<Option<u64>, SchedulerError> {
    match schedule {
        TaskSchedule::Once => Ok(Some(now)),
        TaskSchedule::Scheduled { run_at_ms } => {
            if *run_at_ms > i64::MAX as u64 {
                return Err(SchedulerError::Internal(
                    "run_at_ms exceeds maximum schedulable time".to_string(),
                ));
            }
            Ok(Some(*run_at_ms))
        }
        TaskSchedule::Cron { expression } => {
            let schedule = Schedule::from_str(expression)
                .map_err(|e| SchedulerError::Internal(format!("invalid cron: {e}")))?;
            let now_secs = (now / 1000) as i64;
            let dt = DateTime::<Utc>::from_timestamp(now_secs, 0)
                .ok_or_else(|| SchedulerError::Internal("invalid timestamp".to_string()))?;
            let next = schedule.after(&dt).next().ok_or_else(|| {
                SchedulerError::Internal("cron schedule produced no next run".to_string())
            })?;
            Ok(Some(next.timestamp_millis() as u64))
        }
    }
}

// ---------------------------------------------------------------------------
// SSRF Protection
// ---------------------------------------------------------------------------

/// Validate a webhook URL to prevent SSRF attacks.
///
/// Blocks localhost, private/link-local IPs, cloud metadata endpoints,
/// and `.local` domains.
#[cfg(not(feature = "allow-localhost"))]
pub(crate) fn validate_webhook_url(url: &str) -> Result<(), String> {
    let host = extract_webhook_host(url).ok_or_else(|| format!("invalid webhook URL: {url}"))?;
    let lower = host.to_lowercase();

    // Block localhost variants
    if lower == "localhost"
        || lower == "127.0.0.1"
        || lower == "[::1]"
        || lower == "::1"
        || lower == "0.0.0.0"
    {
        return Err(format!("webhook URL blocked (localhost): {url}"));
    }

    // Block .local domains and cloud metadata
    if lower.ends_with(".local") || lower == "metadata.google.internal" {
        return Err(format!("webhook URL blocked (internal domain): {url}"));
    }

    // Block private/internal IPs (delegates to ssrf module for complete coverage)
    if let Ok(ip) = lower.parse::<std::net::IpAddr>() {
        if ssrf::is_ip_blocked(&ip) {
            return Err(format!("webhook URL blocked (private IP): {url}"));
        }
    }

    Ok(())
}

/// Extract the host portion from a URL string without adding a URL-parsing dependency.
#[cfg(not(feature = "allow-localhost"))]
pub(crate) fn extract_webhook_host(url: &str) -> Option<String> {
    // Strip scheme
    let after_scheme = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?;
    // Take up to the first `/` or `?`
    let authority = after_scheme
        .split('/')
        .next()
        .unwrap_or(after_scheme)
        .split('?')
        .next()
        .unwrap_or(after_scheme);
    // Strip userinfo
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    // Strip port
    let host = if host_port.starts_with('[') {
        // IPv6 in brackets
        host_port.split(']').next().map(|s| format!("{s}]"))
    } else {
        Some(host_port.rsplit(':').next_back().unwrap_or(host_port).to_string())
    };
    host.filter(|h| !h.is_empty())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;

    fn test_service() -> Arc<SchedulerService> {
        let bus = EventBus::new(32);
        Arc::new(
            SchedulerService::in_memory(bus, SchedulerConfig::default())
                .expect("in-memory scheduler"),
        )
    }

    fn emit_request(name: &str) -> CreateTaskRequest {
        CreateTaskRequest {
            name: name.to_string(),
            description: Some("test task".to_string()),
            schedule: TaskSchedule::Once,
            action: TaskAction::EmitEvent {
                topic: "test.topic".to_string(),
                payload: json!({"key": "value"}),
            },
            owner_session_id: None,
            owner_agent_id: None,
            max_retries: None,
            retry_delay_ms: None,
        }
    }

    #[test]
    fn create_and_get_task() {
        let svc = test_service();
        let task = svc.create_task(emit_request("my task")).unwrap();
        assert!(task.id.starts_with("task-"));
        assert_eq!(task.name, "my task");
        assert_eq!(task.status, TaskStatus::Pending);

        let fetched = svc.get_task(&task.id).unwrap();
        assert_eq!(fetched.id, task.id);
        assert_eq!(fetched.name, "my task");
    }

    #[test]
    fn list_shows_created_tasks() {
        let svc = test_service();
        assert!(svc.list_tasks().is_empty());

        svc.create_task(emit_request("alpha")).unwrap();
        svc.create_task(emit_request("beta")).unwrap();

        let tasks = svc.list_tasks();
        assert_eq!(tasks.len(), 2);
        let names: Vec<&str> = tasks.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"alpha"));
        assert!(names.contains(&"beta"));
    }

    #[test]
    fn cancel_task_sets_cancelled() {
        let svc = test_service();
        let task = svc.create_task(emit_request("to cancel")).unwrap();
        let cancelled = svc.cancel_task(&task.id).unwrap();
        assert_eq!(cancelled.status, TaskStatus::Cancelled);
    }

    #[test]
    fn cancel_nonexistent_returns_error() {
        let svc = test_service();
        let result = svc.cancel_task("task-does-not-exist");
        assert!(result.is_err());
    }

    #[test]
    fn delete_removes_task() {
        let svc = test_service();
        let task = svc.create_task(emit_request("to delete")).unwrap();
        svc.delete_task(&task.id).unwrap();
        assert!(svc.get_task(&task.id).is_err());
        assert!(svc.list_tasks().is_empty());
    }

    #[test]
    fn delete_nonexistent_returns_error() {
        let svc = test_service();
        let result = svc.delete_task("task-nope");
        assert!(result.is_err());
    }

    #[test]
    fn get_nonexistent_returns_error() {
        let svc = test_service();
        let result = svc.get_task("task-nope");
        assert!(result.is_err());
    }

    #[test]
    fn task_persistence_round_trip() {
        let svc = test_service();
        let req = CreateTaskRequest {
            name: "webhook task".to_string(),
            description: Some("calls a webhook".to_string()),
            schedule: TaskSchedule::Scheduled { run_at_ms: now_ms() + 60_000 },
            action: TaskAction::HttpWebhook {
                url: "https://example.com/hook".to_string(),
                method: "POST".to_string(),
                body: Some(r#"{"hello":"world"}"#.to_string()),
                headers: None,
            },
            owner_session_id: None,
            owner_agent_id: None,
            max_retries: None,
            retry_delay_ms: None,
        };
        let task = svc.create_task(req).unwrap();
        let fetched = svc.get_task(&task.id).unwrap();
        assert_eq!(
            fetched.schedule,
            TaskSchedule::Scheduled { run_at_ms: task.next_run_ms.unwrap() }
        );
        match &fetched.action {
            TaskAction::HttpWebhook { url, method, body, .. } => {
                assert_eq!(url, "https://example.com/hook");
                assert_eq!(method, "POST");
                assert_eq!(body.as_deref(), Some(r#"{"hello":"world"}"#));
            }
            other => panic!("expected HttpWebhook, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn tick_executes_emit_event_task() {
        let bus = EventBus::new(32);
        let mut rx = bus.subscribe();
        let svc = Arc::new(
            SchedulerService::in_memory(bus, SchedulerConfig::default()).expect("scheduler"),
        );

        svc.create_task(CreateTaskRequest {
            name: "emit once".to_string(),
            description: None,
            schedule: TaskSchedule::Once,
            action: TaskAction::EmitEvent {
                topic: "scheduler.test".to_string(),
                payload: json!({"fired": true}),
            },
            owner_session_id: None,
            owner_agent_id: None,
            max_retries: None,
            retry_delay_ms: None,
        })
        .unwrap();

        svc.tick().await;

        // Drain events — the first may be the "scheduler.task.created" event from create_task.
        // We want the "scheduler.test" event emitted by the EmitEvent action.
        let mut found = false;
        for _ in 0..10 {
            match rx.try_recv() {
                Ok(envelope) if envelope.topic == "scheduler.test" => {
                    assert_eq!(envelope.payload["fired"], true);
                    found = true;
                    break;
                }
                Ok(_) => continue,
                Err(_) => break,
            }
        }
        assert!(found, "expected 'scheduler.test' event");

        // Task should be completed
        let tasks = svc.list_tasks();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].status, TaskStatus::Completed);
        assert_eq!(tasks[0].run_count, 1);
        assert!(tasks[0].last_run_ms.is_some());
    }

    #[tokio::test]
    async fn tick_cron_task_stays_pending() {
        let svc = test_service();
        svc.create_task(CreateTaskRequest {
            name: "cron".to_string(),
            description: None,
            schedule: TaskSchedule::Cron { expression: "0 */5 * * * * *".to_string() },
            action: TaskAction::EmitEvent { topic: "heartbeat".to_string(), payload: json!({}) },
            owner_session_id: None,
            owner_agent_id: None,
            max_retries: None,
            retry_delay_ms: None,
        })
        .unwrap();

        // Manually set next_run_ms to now so it fires on this tick
        svc.force_all_due();

        svc.tick().await;

        let tasks = svc.list_tasks();
        assert_eq!(tasks[0].status, TaskStatus::Pending);
        assert_eq!(tasks[0].run_count, 1);
        assert!(tasks[0].next_run_ms.is_some());
    }

    #[tokio::test]
    async fn tick_skips_cancelled_tasks() {
        let svc = test_service();
        let task = svc.create_task(emit_request("will cancel")).unwrap();
        svc.cancel_task(&task.id).unwrap();
        svc.tick().await;
        let fetched = svc.get_task(&task.id).unwrap();
        assert_eq!(fetched.status, TaskStatus::Cancelled);
        assert_eq!(fetched.run_count, 0);
    }

    #[tokio::test]
    async fn scheduled_task_not_executed_before_time() {
        let svc = test_service();
        svc.create_task(CreateTaskRequest {
            name: "scheduled".to_string(),
            description: None,
            schedule: TaskSchedule::Scheduled { run_at_ms: now_ms() + 9_999_000 },
            action: TaskAction::EmitEvent {
                topic: "scheduled.topic".to_string(),
                payload: json!({}),
            },
            owner_session_id: None,
            owner_agent_id: None,
            max_retries: None,
            retry_delay_ms: None,
        })
        .unwrap();

        svc.tick().await;

        let tasks = svc.list_tasks();
        assert_eq!(tasks[0].status, TaskStatus::Pending);
        assert_eq!(tasks[0].run_count, 0);
    }

    #[test]
    fn full_lifecycle() {
        let svc = test_service();

        // create
        let t1 = svc.create_task(emit_request("lifecycle")).unwrap();
        assert_eq!(t1.status, TaskStatus::Pending);

        // list
        assert_eq!(svc.list_tasks().len(), 1);

        // get
        let t2 = svc.get_task(&t1.id).unwrap();
        assert_eq!(t2.id, t1.id);

        // cancel
        let t3 = svc.cancel_task(&t1.id).unwrap();
        assert_eq!(t3.status, TaskStatus::Cancelled);

        // delete
        svc.delete_task(&t1.id).unwrap();
        assert!(svc.list_tasks().is_empty());
    }

    #[tokio::test]
    async fn send_message_action_posts_to_daemon() {
        use axum::{routing::post, Router};
        use std::sync::atomic::{AtomicUsize, Ordering};

        let call_count = Arc::new(AtomicUsize::new(0));
        let counter = call_count.clone();

        let app = Router::new().route(
            "/api/v1/chat/sessions/{session_id}/messages",
            post(move || async move {
                counter.fetch_add(1, Ordering::SeqCst);
                axum::Json(serde_json::json!({"kind": "queued", "session": {}}))
            }),
        );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let bus = EventBus::new(32);
        let svc = Arc::new(
            SchedulerService::in_memory_with_addr(
                bus,
                addr.to_string(),
                SchedulerConfig::default(),
            )
            .expect("scheduler"),
        );

        svc.create_task(CreateTaskRequest {
            name: "send msg".to_string(),
            description: None,
            schedule: TaskSchedule::Once,
            action: TaskAction::SendMessage {
                session_id: "session-1".to_string(),
                content: "hello from scheduler".to_string(),
            },
            owner_session_id: None,
            owner_agent_id: None,
            max_retries: None,
            retry_delay_ms: None,
        })
        .unwrap();

        svc.tick().await;

        let tasks = svc.list_tasks();
        assert_eq!(tasks[0].status, TaskStatus::Completed);
        assert_eq!(tasks[0].run_count, 1);
        assert!(tasks[0].last_error.is_none());
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn send_message_action_fails_on_server_error() {
        use axum::{routing::post, Router};

        let app = Router::new().route(
            "/api/v1/chat/sessions/{session_id}/messages",
            post(|| async { (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "boom") }),
        );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let bus = EventBus::new(32);
        let svc = Arc::new(
            SchedulerService::in_memory_with_addr(
                bus,
                addr.to_string(),
                SchedulerConfig::default(),
            )
            .expect("scheduler"),
        );

        svc.create_task(CreateTaskRequest {
            name: "send fail".to_string(),
            description: None,
            schedule: TaskSchedule::Once,
            action: TaskAction::SendMessage {
                session_id: "s1".to_string(),
                content: "will fail".to_string(),
            },
            owner_session_id: None,
            owner_agent_id: None,
            max_retries: None,
            retry_delay_ms: None,
        })
        .unwrap();

        svc.tick().await;

        let tasks = svc.list_tasks();
        assert_eq!(tasks[0].status, TaskStatus::Failed);
        assert!(tasks[0].last_error.is_some());
    }

    #[tokio::test]
    async fn http_webhook_action_posts_to_url() {
        use axum::{routing::post, Router};
        use std::sync::atomic::{AtomicUsize, Ordering};

        let call_count = Arc::new(AtomicUsize::new(0));
        let counter = call_count.clone();

        let app = Router::new().route(
            "/hook",
            post(move || async move {
                counter.fetch_add(1, Ordering::SeqCst);
                "ok"
            }),
        );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let bus = EventBus::new(32);
        let svc = Arc::new(
            SchedulerService::in_memory_with_addr(
                bus,
                addr.to_string(),
                SchedulerConfig::default(),
            )
            .expect("scheduler"),
        );

        svc.create_task(CreateTaskRequest {
            name: "webhook".to_string(),
            description: None,
            schedule: TaskSchedule::Once,
            action: TaskAction::HttpWebhook {
                url: format!("http://{addr}/hook"),
                method: "POST".to_string(),
                body: Some(r#"{"event":"task.done"}"#.to_string()),
                headers: None,
            },
            owner_session_id: None,
            owner_agent_id: None,
            max_retries: None,
            retry_delay_ms: None,
        })
        .unwrap();

        svc.tick().await;

        let tasks = svc.list_tasks();
        assert_eq!(tasks[0].status, TaskStatus::Completed);
        assert_eq!(tasks[0].run_count, 1);
        assert!(tasks[0].last_error.is_none());
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn http_webhook_action_handles_server_error() {
        use axum::{routing::post, Router};

        let app = Router::new().route(
            "/hook",
            post(|| async { (axum::http::StatusCode::BAD_GATEWAY, "upstream down") }),
        );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let bus = EventBus::new(32);
        let svc = Arc::new(
            SchedulerService::in_memory_with_addr(
                bus,
                addr.to_string(),
                SchedulerConfig::default(),
            )
            .expect("scheduler"),
        );

        svc.create_task(CreateTaskRequest {
            name: "bad hook".to_string(),
            description: None,
            schedule: TaskSchedule::Once,
            action: TaskAction::HttpWebhook {
                url: format!("http://{addr}/hook"),
                method: "POST".to_string(),
                body: None,
                headers: None,
            },
            owner_session_id: None,
            owner_agent_id: None,
            max_retries: None,
            retry_delay_ms: None,
        })
        .unwrap();

        svc.tick().await;

        let tasks = svc.list_tasks();
        assert_eq!(tasks[0].status, TaskStatus::Failed);
        assert!(tasks[0].last_error.is_some());
    }

    #[tokio::test]
    async fn http_webhook_action_unreachable_url_fails_gracefully() {
        let bus = EventBus::new(32);
        let svc = Arc::new(
            SchedulerService::in_memory_with_addr(
                bus,
                "127.0.0.1:0".to_string(),
                SchedulerConfig::default(),
            )
            .expect("scheduler"),
        );

        svc.create_task(CreateTaskRequest {
            name: "unreachable hook".to_string(),
            description: None,
            schedule: TaskSchedule::Once,
            action: TaskAction::HttpWebhook {
                url: "http://127.0.0.1:1/never".to_string(),
                method: "POST".to_string(),
                body: None,
                headers: None,
            },
            owner_session_id: None,
            owner_agent_id: None,
            max_retries: None,
            retry_delay_ms: None,
        })
        .unwrap();

        svc.tick().await;

        let tasks = svc.list_tasks();
        assert_eq!(tasks[0].status, TaskStatus::Failed);
        assert!(tasks[0].last_error.is_some());
    }

    #[tokio::test]
    async fn http_webhook_respects_method() {
        use axum::{routing::get, Router};
        use std::sync::atomic::{AtomicUsize, Ordering};

        let call_count = Arc::new(AtomicUsize::new(0));
        let counter = call_count.clone();

        let app = Router::new().route(
            "/status",
            get(move || async move {
                counter.fetch_add(1, Ordering::SeqCst);
                "ok"
            }),
        );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let bus = EventBus::new(32);
        let svc = Arc::new(
            SchedulerService::in_memory_with_addr(
                bus,
                addr.to_string(),
                SchedulerConfig::default(),
            )
            .expect("scheduler"),
        );

        svc.create_task(CreateTaskRequest {
            name: "get hook".to_string(),
            description: None,
            schedule: TaskSchedule::Once,
            action: TaskAction::HttpWebhook {
                url: format!("http://{addr}/status"),
                method: "GET".to_string(),
                body: None,
                headers: None,
            },
            owner_session_id: None,
            owner_agent_id: None,
            max_retries: None,
            retry_delay_ms: None,
        })
        .unwrap();

        svc.tick().await;

        let tasks = svc.list_tasks();
        assert_eq!(tasks[0].status, TaskStatus::Completed);
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    // -----------------------------------------------------------------------
    // Cron parsing tests
    // -----------------------------------------------------------------------

    #[test]
    fn cron_valid_expression_produces_future_time() {
        // "0 * * * * *" = every minute at second 0 (7-field cron: sec min hour dom month dow year)
        let now = now_ms();
        let schedule = TaskSchedule::Cron { expression: "0 * * * * *".to_string() };
        let next = compute_next_run(&schedule, now).unwrap();
        assert!(next.is_some(), "valid cron should return Some");
        let next_ms = next.unwrap();
        assert!(next_ms > now, "next run must be in the future (got {next_ms}, now {now})");
    }

    #[test]
    fn cron_invalid_expression_returns_none() {
        let now = now_ms();
        let schedule = TaskSchedule::Cron { expression: "not a cron expression".to_string() };
        assert!(compute_next_run(&schedule, now).is_err(), "invalid cron should return Err");
    }

    #[test]
    fn cron_next_run_is_within_expected_range() {
        // "0 0 * * * *" = every hour at minute 0, second 0
        let now = now_ms();
        let schedule = TaskSchedule::Cron { expression: "0 0 * * * *".to_string() };
        let next = compute_next_run(&schedule, now).unwrap();
        assert!(next.is_some());
        let next_ms = next.unwrap();
        let diff = next_ms - now;
        // Should be at most 1 hour (3_600_000 ms) in the future
        assert!(diff <= 3_600_000, "next run should be within 1 hour, got {diff}ms");
        assert!(diff > 0, "next run must be in the future");
    }

    #[test]
    fn cron_specific_timestamp_produces_correct_next() {
        // 1_705_311_000 seconds = 2024-01-15 09:30:00 UTC
        let now_ms_val: u64 = 1_705_311_000_000;
        // "0 0 11 * * *" = every day at 11:00:00
        let schedule = TaskSchedule::Cron { expression: "0 0 11 * * *".to_string() };
        let next = compute_next_run(&schedule, now_ms_val).unwrap();
        assert!(next.is_some());
        let next_ms = next.unwrap();
        assert!(next_ms > now_ms_val);
        // Next 11:00:00 after 09:30:00 is the same day at 11:00:00 = 1_705_316_400_000
        assert_eq!(next_ms, 1_705_316_400_000);
    }

    #[test]
    fn update_cancelled_task_schedule_resets_to_pending() {
        let svc = test_service();
        let task = svc.create_task(emit_request("will cancel")).unwrap();
        svc.cancel_task(&task.id).unwrap();

        let updated = svc
            .update_task(
                &task.id,
                UpdateTaskRequest {
                    name: None,
                    description: None,
                    schedule: Some(TaskSchedule::Cron {
                        expression: "0 */5 * * * * *".to_string(),
                    }),
                    action: None,
                    max_retries: None,
                    retry_delay_ms: None,
                },
            )
            .unwrap();

        assert_eq!(
            updated.status,
            TaskStatus::Pending,
            "cancelled task should reset to pending when schedule is updated"
        );
    }

    #[tokio::test]
    async fn reset_to_pending_clears_retry_state() {
        use axum::{routing::post, Router};

        // Spin up a local server that always returns 500
        let app = Router::new()
            .route("/fail", post(|| async { axum::http::StatusCode::INTERNAL_SERVER_ERROR }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let bus = EventBus::new(32);
        let svc = Arc::new(
            SchedulerService::in_memory_with_addr(
                bus,
                addr.to_string(),
                SchedulerConfig::default(),
            )
            .expect("scheduler"),
        );

        // Create a task that will always fail (500 error), with max_retries=1
        let task = svc
            .create_task(CreateTaskRequest {
                name: "retry-reset".to_string(),
                description: None,
                schedule: TaskSchedule::Once,
                action: TaskAction::HttpWebhook {
                    url: format!("http://{addr}/fail"),
                    method: "POST".to_string(),
                    body: None,
                    headers: None,
                },
                owner_session_id: None,
                owner_agent_id: None,
                max_retries: Some(1),
                retry_delay_ms: Some(0),
            })
            .unwrap();

        // First tick: executes and fails, retry_count becomes 1, status goes back to pending for retry
        svc.tick().await;
        let t = svc.get_task(&task.id).unwrap();
        assert_eq!(t.status, TaskStatus::Pending, "should be pending for retry");
        assert_eq!(t.retry_count, 1);

        // Force due and tick again: retries, fails again, retry_count=2 > max_retries(1) → Failed
        svc.force_all_due();
        svc.tick().await;
        let t = svc.get_task(&task.id).unwrap();
        assert_eq!(t.status, TaskStatus::Failed, "should be failed after exhausting retries");
        assert!(t.retry_count > 0);
        assert!(t.last_error.is_some());
        assert!(t.completed_at_ms.is_some());

        // Update the task's schedule to reset it to pending
        let updated = svc
            .update_task(
                &task.id,
                UpdateTaskRequest {
                    name: None,
                    description: None,
                    schedule: Some(TaskSchedule::Once),
                    action: None,
                    max_retries: None,
                    retry_delay_ms: None,
                },
            )
            .unwrap();

        // Verify all retry state was cleared
        assert_eq!(updated.status, TaskStatus::Pending);
        assert_eq!(updated.retry_count, 0, "retry_count should be reset to 0");
        assert!(updated.completed_at_ms.is_none(), "completed_at_ms should be cleared");
        assert!(updated.last_error.is_none(), "last_error should be cleared");

        // Verify it actually executes again (run_count increases)
        let run_count_before = updated.run_count;
        svc.force_all_due();
        svc.tick().await;
        let t = svc.get_task(&task.id).unwrap();
        assert!(t.run_count > run_count_before, "task should have executed again");
    }

    #[tokio::test]
    async fn http_webhook_rejects_unsupported_method() {
        use axum::{routing::any, Router};

        let app = Router::new().route("/hook", any(|| async { "ok" }));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let bus = EventBus::new(32);
        let svc = Arc::new(
            SchedulerService::in_memory_with_addr(
                bus,
                addr.to_string(),
                SchedulerConfig::default(),
            )
            .expect("scheduler"),
        );

        svc.create_task(CreateTaskRequest {
            name: "bad method".to_string(),
            description: None,
            schedule: TaskSchedule::Once,
            action: TaskAction::HttpWebhook {
                url: format!("http://{addr}/hook"),
                method: "OPTIONS".to_string(),
                body: None,
                headers: None,
            },
            owner_session_id: None,
            owner_agent_id: None,
            max_retries: None,
            retry_delay_ms: None,
        })
        .unwrap();

        svc.tick().await;

        let tasks = svc.list_tasks();
        assert_eq!(tasks[0].status, TaskStatus::Failed);
        assert!(tasks[0].last_error.as_ref().unwrap().contains("unsupported HTTP method"));
    }
}
