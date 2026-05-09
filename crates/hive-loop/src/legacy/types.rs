use hive_classification::DataClass;
use hive_contracts::{
    CodeActConfig, InteractionKind, LoopStrategy as ConfigLoopStrategy, Persona,
    SessionPermissions, ToolExecutionMode, ToolLimitsConfig, WorkspaceClassification,
};
use hive_model::{Capability, CompletionMessage, ContentPart, RoutingDecision};
use hive_tools::{ToolRegistry, ToolResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering as AtomicOrdering};
use std::sync::Arc;

use parking_lot::Mutex;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use super::journal::ConversationJournal;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Clone)]
pub struct ConversationContext {
    pub session_id: String,
    pub message_id: String,
    pub prompt: String,
    /// Multimodal content parts for the initial user prompt (text + images).
    /// Only populated on the first turn; subsequent tool-loop iterations use
    /// text-only prompts.
    pub prompt_content_parts: Vec<ContentPart>,
    pub history: Vec<CompletionMessage>,
    /// Shared journal for recording tool cycles (used for mid-task resume).
    pub conversation_journal: Option<Arc<Mutex<ConversationJournal>>>,
    /// Number of tool iterations already completed (from a prior journal on resume).
    pub initial_tool_iterations: usize,
}

#[derive(Clone)]
pub struct RoutingConfig {
    pub required_capabilities: BTreeSet<Capability>,
    pub preferred_models: Option<Vec<String>>,
    pub routing_decision: Option<RoutingDecision>,
    pub loop_strategy: Option<ConfigLoopStrategy>,
}

#[derive(Clone)]
pub struct SecurityContext {
    pub data_class: DataClass,
    /// Effective data-class, escalated as tools touch higher-class data.
    /// Initialized from `data_class`; only increases (never decreases).
    /// Shared `Arc<AtomicU8>` so it can be escalated through `&self`.
    pub effective_data_class: Arc<AtomicU8>,
    /// Per-session scoped permissions checked before tool definition approval.
    pub permissions: Arc<Mutex<SessionPermissions>>,
    pub workspace_classification: Option<Arc<WorkspaceClassification>>,
    /// Optional connector service handle for resolving output data-class
    /// when enforcing classification on outbound sends.
    pub connector_service: Option<Arc<dyn hive_connectors::ConnectorServiceHandle>>,
    /// When true, side-effecting external tool calls are intercepted and a
    /// synthetic success response is returned.  Built-in tools (`core.*`,
    /// `knowledge.*`) and read-only tools pass through unchanged.
    pub shadow_mode: bool,
}

#[derive(Clone)]
pub struct ToolsContext {
    pub tools: Arc<ToolRegistry>,
    /// How batched tool calls are executed (sequential-partial, sequential-full, parallel).
    pub tool_execution_mode: ToolExecutionMode,
    pub skill_catalog: Option<Arc<hive_skills::SkillCatalog>>,
    /// Handler for knowledge.query tool calls (provided by the API layer).
    pub knowledge_query_handler: Option<Arc<dyn KnowledgeQueryHandler>>,
}

#[derive(Clone)]
pub struct AgentContext {
    pub persona: Option<Persona>,
    pub personas: Vec<Persona>,
    pub current_agent_id: Option<String>,
    pub parent_agent_id: Option<String>,
    pub agent_orchestrator: Option<Arc<dyn AgentOrchestrator>>,
    /// The workspace directory for this agent. Child agents should inherit
    /// this so they operate in the same workspace as their parent.
    pub workspace_path: Option<PathBuf>,
    /// Whether this agent is one-shot (false) or service (true).
    pub keep_alive: bool,
    /// Set when a one-shot agent has already messaged the session.
    pub session_messaged: Arc<AtomicBool>,
}

pub struct LoopContext {
    pub conversation: ConversationContext,
    pub routing: RoutingConfig,
    pub security: SecurityContext,
    pub tools_ctx: ToolsContext,
    pub agent: AgentContext,
    /// Adaptive tool-call limits and stall detection config.
    /// Defaults to `ToolLimitsConfig::default()` if not explicitly set.
    pub tool_limits: ToolLimitsConfig,
    /// CodeAct executor settings (timeouts, memory limits, network access).
    /// Defaults to `CodeActConfig::default()` if not explicitly set.
    pub code_act_config: CodeActConfig,
    /// Shared session registry for CodeAct code execution.
    /// When set, the CodeAct strategy reuses sessions across conversation turns
    /// instead of creating a fresh executor each time.
    pub session_registry: Option<Arc<hive_code_executor::SessionRegistry>>,
    /// When set, the loop checks this signal after each tool batch.
    /// If `true`, the loop yields early so the next queued message
    /// can be processed at the current checkpoint.
    pub preempt_signal: Option<Arc<AtomicBool>>,
    /// When set, the loop can be cooperatively cancelled (e.g. on agent kill).
    /// Checked before model calls, between streaming chunks, and around tool
    /// execution so that in-flight operations are interrupted promptly.
    pub cancellation_token: Option<CancellationToken>,
}

impl LoopContext {
    /// Return the effective (possibly escalated) session data-class.
    pub fn effective_data_class(&self) -> DataClass {
        let raw = self.security.effective_data_class.load(AtomicOrdering::Acquire);
        DataClass::from_i64(raw as i64).unwrap_or(self.security.data_class)
    }

    /// Escalate the effective data-class if `new_class` is higher.
    pub fn escalate_data_class(&self, new_class: DataClass) {
        let new_val = new_class.to_i64() as u8;
        self.security.effective_data_class.fetch_max(new_val, AtomicOrdering::AcqRel);
    }

    // -- Accessor methods --
    pub fn session_id(&self) -> &str {
        &self.conversation.session_id
    }
    pub fn message_id(&self) -> &str {
        &self.conversation.message_id
    }
    pub fn prompt(&self) -> &str {
        &self.conversation.prompt
    }
    pub fn prompt_content_parts(&self) -> &[ContentPart] {
        &self.conversation.prompt_content_parts
    }
    pub fn history(&self) -> &[CompletionMessage] {
        &self.conversation.history
    }
    pub fn conversation_journal(&self) -> Option<&Arc<Mutex<ConversationJournal>>> {
        self.conversation.conversation_journal.as_ref()
    }
    pub fn initial_tool_iterations(&self) -> usize {
        self.conversation.initial_tool_iterations
    }
    pub fn required_capabilities(&self) -> &BTreeSet<Capability> {
        &self.routing.required_capabilities
    }
    pub fn preferred_models(&self) -> Option<&Vec<String>> {
        self.routing.preferred_models.as_ref()
    }
    pub fn routing_decision(&self) -> Option<&RoutingDecision> {
        self.routing.routing_decision.as_ref()
    }
    pub fn loop_strategy(&self) -> Option<&ConfigLoopStrategy> {
        self.routing.loop_strategy.as_ref()
    }
    pub fn data_class(&self) -> DataClass {
        self.security.data_class
    }
    pub fn permissions(&self) -> &Arc<Mutex<SessionPermissions>> {
        &self.security.permissions
    }
    pub fn workspace_classification(&self) -> Option<&Arc<WorkspaceClassification>> {
        self.security.workspace_classification.as_ref()
    }
    pub fn connector_service(&self) -> Option<&Arc<dyn hive_connectors::ConnectorServiceHandle>> {
        self.security.connector_service.as_ref()
    }
    pub fn tools(&self) -> &Arc<ToolRegistry> {
        &self.tools_ctx.tools
    }
    pub fn tool_execution_mode(&self) -> ToolExecutionMode {
        self.tools_ctx.tool_execution_mode
    }
    pub fn skill_catalog(&self) -> Option<&Arc<hive_skills::SkillCatalog>> {
        self.tools_ctx.skill_catalog.as_ref()
    }
    pub fn knowledge_query_handler(&self) -> Option<&Arc<dyn KnowledgeQueryHandler>> {
        self.tools_ctx.knowledge_query_handler.as_ref()
    }
    pub fn persona(&self) -> Option<&Persona> {
        self.agent.persona.as_ref()
    }
    pub fn personas(&self) -> &[Persona] {
        &self.agent.personas
    }
    pub fn current_agent_id(&self) -> Option<&str> {
        self.agent.current_agent_id.as_deref()
    }
    pub fn parent_agent_id(&self) -> Option<&str> {
        self.agent.parent_agent_id.as_deref()
    }
    pub fn agent_orchestrator(&self) -> Option<&Arc<dyn AgentOrchestrator>> {
        self.agent.agent_orchestrator.as_ref()
    }
    pub fn keep_alive(&self) -> bool {
        self.agent.keep_alive
    }
    pub fn session_messaged(&self) -> &Arc<AtomicBool> {
        &self.agent.session_messaged
    }
    pub fn workspace_path(&self) -> Option<&Path> {
        self.agent.workspace_path.as_deref()
    }
}

#[allow(clippy::too_many_arguments)]
pub trait AgentOrchestrator: Send + Sync {
    fn spawn_agent(
        &self,
        persona: Persona,
        task: String,
        from: Option<String>,
        friendly_name: Option<String>,
        data_class: hive_classification::DataClass,
        parent_model: Option<hive_model::ModelSelection>,
        keep_alive: bool,
        workspace_path: Option<PathBuf>,
    ) -> BoxFuture<'_, Result<String, String>>;

    fn message_agent(
        &self,
        agent_id: String,
        message: String,
        from: String,
    ) -> BoxFuture<'_, Result<(), String>>;

    /// Send a message from an agent back to the parent chat session.
    fn message_session(
        &self,
        message: String,
        from_agent_id: String,
    ) -> BoxFuture<'_, Result<(), String>>;

    /// Send a feedback (non-executing) message to an agent. Unlike `message_agent`,
    /// this does NOT trigger a new task execution — the agent merely logs the content.
    fn feedback_agent(
        &self,
        agent_id: String,
        message: String,
        from: String,
    ) -> BoxFuture<'_, Result<(), String>>;

    #[allow(clippy::type_complexity)]
    fn list_agents(
        &self,
    ) -> BoxFuture<'_, Result<Vec<(String, String, String, String, Option<String>)>, String>>;

    /// Retrieve the final result of a completed agent by ID.
    fn get_agent_result(
        &self,
        agent_id: String,
    ) -> BoxFuture<'_, Result<(String, Option<String>), String>>;

    fn kill_agent(&self, agent_id: String) -> BoxFuture<'_, Result<(), String>>;

    /// Block until the given agent reaches a terminal state (done/error) or timeout.
    /// Returns `(status, result)`.
    fn wait_for_agent(
        &self,
        agent_id: String,
        timeout_secs: Option<u64>,
    ) -> BoxFuture<'_, Result<(String, Option<String>), String>> {
        let _ = (agent_id, timeout_secs);
        Box::pin(async { Err("wait_for_agent is not supported in this context".to_string()) })
    }

    /// Search bots by keyword. Returns (id, name, description) tuples.
    /// Default: no bots available.
    #[allow(clippy::type_complexity)]
    fn search_bots(
        &self,
        _query: String,
    ) -> BoxFuture<'_, Result<Vec<(String, String, String)>, String>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    /// Get the parent agent ID for a given agent.
    /// Returns `Ok(None)` if the agent exists but has no parent (root-level).
    /// Returns `Ok(Some(parent_id))` if the agent has a parent.
    /// Returns `Err` if the agent is not found or not supported.
    fn get_agent_parent(&self, _agent_id: String) -> BoxFuture<'_, Result<Option<String>, String>> {
        Box::pin(async { Err("get_agent_parent is not supported in this context".to_string()) })
    }
}

/// Handler for `knowledge.query` tool calls.
///
/// Implemented by the API layer which has access to the knowledge graph.
/// The loop layer intercepts `knowledge.query` calls and delegates to this
/// trait, similar to `AgentOrchestrator`.
pub trait KnowledgeQueryHandler: Send + Sync {
    /// Execute a knowledge graph query and return the JSON result.
    fn handle_query(&self, input: Value) -> BoxFuture<'_, Result<ToolResult, String>>;
}

#[derive(Debug, Clone)]
pub struct LoopResult {
    pub content: String,
    pub provider_id: String,
    pub model: String,
    pub decision: RoutingDecision,
    /// `true` when the loop yielded early because a new user message
    /// was enqueued (the preempt signal fired). The `content` field
    /// contains a summary of tool work completed so far.
    pub preempted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LoopEvent {
    /// The model is being loaded into memory (local models only)
    ModelLoading {
        provider_id: String,
        model: String,
        /// Count of tool results by tool name included in this model call.
        tool_result_counts: HashMap<String, u32>,
        /// Estimated token count for the outgoing request (prompt + history + tools).
        estimated_tokens: Option<u32>,
    },
    /// A token chunk from the model
    Token { delta: String },
    /// Model finished generating (may include tool call)
    ModelDone {
        content: String,
        provider_id: String,
        model: String,
        /// Provider-reported input token count, if available.
        input_tokens: Option<u32>,
        /// Provider-reported output token count, if available.
        output_tokens: Option<u32>,
        /// Input tokens served from prompt cache, if available.
        cached_input_tokens: Option<u32>,
        /// Input tokens written to prompt cache (Anthropic), if available.
        cache_write_tokens: Option<u32>,
    },
    /// A tool call is starting
    ToolCallStart { tool_id: String, input: String },
    /// A tool call completed
    ToolCallResult { tool_id: String, output: String, is_error: bool },
    /// User interaction required (tool approval, question, etc.)
    UserInteractionRequired { request_id: String, kind: InteractionKind },
    /// The loop is complete with final result
    Done { content: String, provider_id: String, model: String },
    /// An error occurred
    Error {
        message: String,
        /// Classified error kind (e.g. "rate_limited", "server_error").
        error_code: Option<String>,
        /// HTTP status code from the provider, if available.
        http_status: Option<u16>,
        /// Provider that produced the error.
        provider_id: Option<String>,
        /// Model that produced the error.
        model: Option<String>,
    },
    /// A transient LLM error triggered a retry with backoff.
    ModelRetry {
        provider_id: String,
        model: String,
        attempt: u32,
        max_attempts: u32,
        /// Classified error kind (e.g. "rate_limited", "server_error").
        error_kind: String,
        http_status: Option<u16>,
        backoff_ms: u64,
    },
    /// A message was injected into the session by an agent
    AgentSessionMessage { from_agent_id: String, content: String },
    /// The selected model was unavailable; fell back to an alternative.
    ModelFallback {
        from_provider: String,
        from_model: String,
        to_provider: String,
        to_model: String,
    },
    /// The tool-call budget was extended because the agent is making progress.
    BudgetExtended { new_budget: usize, extensions_granted: usize },
    /// The stall detector noticed repeated identical tool calls (warning before stop).
    StallWarning { tool_name: String, repeated_count: usize },
    /// The loop is yielding early because a new user message was enqueued.
    Preempted,
    /// A side-effecting tool call was intercepted in shadow mode.
    ToolCallIntercepted { tool_id: String, input: String },
    /// Partial tool-call argument snapshot during streaming.
    ToolCallArgDelta {
        index: usize,
        call_id: Option<String>,
        tool_name: Option<String>,
        arguments_so_far: String,
    },
    /// CodeAct: a Python code block is being executed or has completed.
    CodeExecution {
        code: String,
        stdout: String,
        stderr: String,
        is_error: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
        phase: CodeExecutionPhase,
    },
}

/// Phase of a CodeAct code execution event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeExecutionPhase {
    /// Code execution has started (output is empty).
    Started,
    /// Code execution has completed (output contains stdout/stderr).
    Completed,
}

#[derive(Debug, Error)]
pub enum LoopError {
    #[error("model routing failed: {0}")]
    ModelRouting(String),
    #[error("model execution failed: {message}")]
    ModelExecution {
        message: String,
        /// Classified error kind (e.g. "rate_limited", "server_error").
        error_code: Option<String>,
        /// HTTP status code from the provider, if available.
        http_status: Option<u16>,
        /// Provider that produced the error.
        provider_id: Option<String>,
        /// Model that produced the error.
        model: Option<String>,
    },
    #[error("model worker join failed: {0}")]
    JoinFailed(String),
    #[error("middleware rejected request: {0}")]
    MiddlewareRejected(String),
    #[error("tool `{tool_id}` is not registered")]
    ToolUnavailable { tool_id: String },
    #[error("tool `{tool_id}` is denied by policy: {reason}")]
    ToolDenied { tool_id: String, reason: String },
    #[error("tool `{tool_id}` requires approval")]
    ToolApprovalRequired { tool_id: String },
    #[error("tool `{tool_id}` failed: {detail}")]
    ToolExecutionFailed { tool_id: String, detail: String },
    #[error("tool call limit reached ({limit})")]
    ToolCallLimit { limit: usize },
    #[error("stall detected: tool `{tool_name}` called {count} times with identical arguments")]
    StallDetected { tool_name: String, count: usize },
    #[error("hard tool call ceiling reached ({ceiling})")]
    HardCeilingReached { ceiling: usize },
    #[error("operation cancelled")]
    Cancelled,
}
