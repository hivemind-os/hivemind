//! Backend abstraction traits for the workflow engine.
//!
//! These traits define the interfaces that callers must implement to plug in
//! their own model provider, tool executor, and event sink. They have zero
//! dependencies on any hive-* crate types.

use serde::{Deserialize, Serialize};

use crate::error::WorkflowResult;

// ---------------------------------------------------------------------------
// Message types (self-contained; no dependency on state.rs)
// ---------------------------------------------------------------------------

/// Role of a message in the conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

/// A single message in the conversation history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: MessageRole,
    pub content: String,
}

// ---------------------------------------------------------------------------
// Model types
// ---------------------------------------------------------------------------

/// Request to send to a model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRequest {
    /// The messages to send (conversation history).
    pub messages: Vec<Message>,
    /// Tool definitions available to the model (JSON schema format).
    pub tools: Vec<ToolSchema>,
}

/// A tool definition that the model can call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSchema {
    /// Tool name.
    pub name: String,
    /// Tool description.
    pub description: String,
    /// JSON Schema for the tool's parameters.
    pub parameters: serde_json::Value,
}

/// Response from a model call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelResponse {
    /// Text content from the model.
    pub content: String,
    /// Structured tool calls the model wants to make.
    pub tool_calls: Vec<ToolCall>,
    /// Provider-specific metadata (tokens used, model name, etc.).
    pub metadata: serde_json::Map<String, serde_json::Value>,
}

/// A tool call requested by the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// Unique ID for this tool call (for correlating results).
    pub id: String,
    /// Tool name to invoke.
    pub name: String,
    /// Arguments as JSON.
    pub arguments: serde_json::Value,
}

/// Result of executing a tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    /// The tool call ID this result is for.
    pub call_id: String,
    /// Tool name.
    pub name: String,
    /// Output content.
    pub content: String,
    /// Whether the tool execution failed.
    pub is_error: bool,
}

// ---------------------------------------------------------------------------
// Backend traits
// ---------------------------------------------------------------------------

/// Backend for making LLM calls.
#[async_trait::async_trait]
pub trait ModelBackend: Send + Sync {
    /// Send a request to the model and get a response.
    async fn complete(&self, request: &ModelRequest) -> WorkflowResult<ModelResponse>;
}

/// Backend for executing tools.
#[async_trait::async_trait]
pub trait ToolBackend: Send + Sync {
    /// List available tools and their schemas.
    async fn list_tools(&self) -> WorkflowResult<Vec<ToolSchema>>;

    /// Execute a tool call and return the result.
    async fn execute(&self, call: &ToolCall) -> WorkflowResult<ToolResult>;
}

// ---------------------------------------------------------------------------
// Workflow events
// ---------------------------------------------------------------------------

/// Events emitted during workflow execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkflowEvent {
    /// Workflow execution started.
    Started { run_id: String, workflow_name: String },
    /// A step is starting.
    StepStarted { run_id: String, step_id: String },
    /// A step completed.
    StepCompleted { run_id: String, step_id: String },
    /// A step failed.
    StepFailed { run_id: String, step_id: String, error: String },
    /// Model call made.
    ModelCallStarted { run_id: String },
    /// Model response received.
    ModelCallCompleted { run_id: String, content_preview: String },
    /// Tool execution started.
    ToolCallStarted { run_id: String, tool_name: String },
    /// Tool execution completed.
    ToolCallCompleted { run_id: String, tool_name: String, is_error: bool },
    /// A token was streamed from the model.
    TokenDelta { run_id: String, delta: String },
    /// Workflow completed successfully.
    Completed { run_id: String, result: String },
    /// Workflow failed.
    Failed {
        run_id: String,
        error: String,
        /// Classified error kind (e.g. "rate_limited", "server_error").
        error_code: Option<String>,
        /// HTTP status code from the provider, if available.
        http_status: Option<u16>,
        /// Provider that produced the error.
        provider_id: Option<String>,
        /// Model that produced the error.
        model: Option<String>,
    },
    /// A variable was set.
    VariableSet { run_id: String, name: String },
    /// Log message from workflow.
    Log { run_id: String, level: String, message: String },
    /// A transient LLM error triggered a retry with backoff.
    ModelRetry {
        run_id: String,
        provider_id: String,
        model: String,
        attempt: u32,
        max_attempts: u32,
        error_kind: String,
        http_status: Option<u16>,
        backoff_ms: u64,
    },
}

/// Sink for workflow execution events (observability).
#[async_trait::async_trait]
pub trait WorkflowEventSink: Send + Sync {
    /// Emit a workflow event.
    async fn emit(&self, event: WorkflowEvent);
}

/// A no-op event sink that discards all events.
pub struct NullEventSink;

#[async_trait::async_trait]
impl WorkflowEventSink for NullEventSink {
    async fn emit(&self, _event: WorkflowEvent) {}
}
