use async_trait::async_trait;
use hive_workflow::types::{PermissionEntry, ScheduleTaskDef, SignalTarget, WorkflowAttachment};
use serde_json::Value;

/// A tool call that was intercepted in shadow mode.
#[derive(Debug, Clone)]
pub struct InterceptedToolCall {
    pub tool_id: String,
    pub input: Value,
}

/// Extension trait for executing tools within workflows.
/// Implemented in hive-api to avoid circular dependencies.
#[async_trait]
pub trait WorkflowToolExecutor: Send + Sync {
    /// Execute a tool by ID with the given arguments and permission context.
    async fn execute_tool(
        &self,
        tool_id: &str,
        arguments: Value,
        permissions: &[PermissionEntry],
    ) -> Result<Value, String>;
}

/// Extension trait for invoking agents and sending messages.
/// Implemented in hive-api to avoid circular dependencies.
#[async_trait]
#[allow(clippy::too_many_arguments)]
pub trait WorkflowAgentRunner: Send + Sync {
    /// Spawn an agent and return its ID immediately (does not wait for completion).
    /// When `session_id` is provided, the agent is registered on the per-session
    /// supervisor so its events flow through the session's SSE stream.
    async fn spawn_agent(
        &self,
        persona_id: &str,
        task: &str,
        timeout_secs: Option<u64>,
        workspace_path: Option<&str>,
        permissions: &[PermissionEntry],
        attachments: &[WorkflowAttachment],
        attachments_dir: Option<&str>,
        session_id: Option<&str>,
        agent_name: Option<&str>,
        shadow_mode: bool,
    ) -> Result<String, String>;

    /// Wait for a previously spawned agent to complete and return its result.
    async fn wait_for_agent(
        &self,
        agent_id: &str,
        timeout_secs: Option<u64>,
        session_id: Option<&str>,
    ) -> Result<Value, String>;

    /// Spawn an agent and wait for it to complete in one atomic operation.
    /// This avoids the race condition where `AgentCompleted` fires between
    /// `spawn_agent` and `wait_for_agent`.
    /// The `on_spawned` callback is invoked with the agent_id immediately after
    /// spawning but before waiting, allowing callers to persist the mapping.
    ///
    /// Returns `(agent_id, result_value, intercepted_tool_calls)` where
    /// `intercepted_tool_calls` contains tool calls that were intercepted in
    /// shadow mode (tool_id, input JSON).
    ///
    /// When `auto_respond` is true, any `ask_user` questions and tool approval
    /// requests from the agent are automatically answered, preventing the agent
    /// from blocking indefinitely (used by the workflow test runner).
    async fn spawn_and_wait_agent(
        &self,
        persona_id: &str,
        task: &str,
        timeout_secs: Option<u64>,
        workspace_path: Option<&str>,
        permissions: &[PermissionEntry],
        attachments: &[WorkflowAttachment],
        attachments_dir: Option<&str>,
        session_id: Option<&str>,
        on_spawned: Option<Box<dyn FnOnce(String) + Send + Sync>>,
        agent_name: Option<&str>,
        shadow_mode: bool,
        auto_respond: bool,
    ) -> Result<(String, Value, Vec<InterceptedToolCall>), String>;

    /// Signal an agent or chat session.
    async fn signal_agent(&self, target: &SignalTarget, content: &str) -> Result<Value, String>;

    /// Inject a notification message into a chat session's history.
    /// Used for workflow result messages so the main agent has context.
    async fn inject_session_notification(
        &self,
        session_id: &str,
        source_name: &str,
        message: &str,
    ) -> Result<(), String>;

    /// Inject a question message into a chat session's timeline.
    /// Used for workflow feedback gates so the question appears inline
    /// in the chat thread (rendered as an interactive question card).
    async fn inject_session_question(
        &self,
        session_id: &str,
        request_id: &str,
        prompt: &str,
        choices: &[String],
        allow_freeform: bool,
        workflow_instance_id: i64,
        workflow_step_id: &str,
        workflow_name: &str,
    ) -> Result<(), String>;

    /// Kill an agent by ID. Used for cascade cleanup when a workflow is killed.
    async fn kill_agent(&self, session_id: &str, agent_id: &str) -> Result<(), String>;

    /// Mark a question message as answered in the chat timeline.
    /// Called when a workflow feedback gate is responded to so the
    /// question card in the chat thread shows the answer.
    async fn mark_session_question_answered(
        &self,
        session_id: &str,
        request_id: &str,
        answer_text: &str,
    ) -> Result<(), String>;
}

/// Extension trait for creating user interaction requests (feedback gates).
/// Implemented in hive-api to avoid circular dependencies.
#[async_trait]
pub trait WorkflowInteractionGate: Send + Sync {
    /// Create a feedback/approval request. Returns a request_id that can be
    /// used to match the response when it arrives.
    async fn create_feedback_request(
        &self,
        instance_id: i64,
        step_id: &str,
        prompt: &str,
        choices: Option<&[String]>,
        allow_freeform: bool,
    ) -> Result<String, String>;
}

/// Extension trait for scheduling tasks from within a workflow.
/// Implemented in hive-api to avoid circular dependencies.
#[async_trait]
pub trait WorkflowTaskScheduler: Send + Sync {
    /// Schedule a task using the scheduler service. Returns the task ID.
    async fn schedule_task(
        &self,
        schedule: &ScheduleTaskDef,
        parent_session_id: Option<&str>,
        parent_agent_id: Option<&str>,
    ) -> Result<String, String>;
}

/// Extension trait for registering event gate subscriptions.
/// Implemented by TriggerManager to watch for events that resume waiting steps.
#[async_trait]
pub trait WorkflowEventGateRegistrar: Send + Sync {
    /// Register an event gate subscription. Returns a subscription_id.
    async fn register_event_gate(
        &self,
        instance_id: i64,
        step_id: &str,
        topic: &str,
        filter: Option<&str>,
        timeout_secs: Option<u64>,
    ) -> Result<String, String>;

    /// Unregister all event gates for a given workflow instance.
    async fn unregister_instance_gates(&self, instance_id: i64);
}

/// Extension trait for rendering persona prompt templates within workflows.
/// Implemented in hive-api to avoid circular dependencies.
#[async_trait]
pub trait WorkflowPromptRenderer: Send + Sync {
    /// Resolve a persona's prompt template and render it with the given
    /// parameters. Returns the rendered text.
    async fn render_prompt_template(
        &self,
        persona_id: &str,
        prompt_id: &str,
        parameters: Value,
    ) -> Result<String, String>;
}
