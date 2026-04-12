/// Errors that can occur during workflow execution.
#[derive(Debug, thiserror::Error)]
pub enum WorkflowError {
    /// Invalid workflow definition (YAML parsing, schema validation)
    #[error("schema error: {0}")]
    Schema(String),

    /// Template or condition expression evaluation failed
    #[error("expression error: {0}")]
    Expression(String),

    /// A workflow step failed during execution
    #[error("step `{step_id}` failed: {detail}")]
    Step { step_id: String, detail: String },

    /// State persistence error
    #[error("store error: {0}")]
    Store(String),

    /// Tool execution failed
    #[error("tool `{tool_id}` failed: {detail}")]
    Tool { tool_id: String, detail: String },

    /// Tool not found in backend
    #[error("tool `{tool_id}` not found")]
    ToolNotFound { tool_id: String },

    /// Model backend call failed
    #[error("model error: {message}")]
    Model {
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

    /// Iteration or tool call limit exceeded
    #[error("{kind} limit exceeded: {limit}")]
    LimitExceeded { kind: String, limit: usize },

    /// Stall detected — the agent is repeating the same tool call
    #[error("stall detected: tool `{tool_name}` called {count} times with identical arguments")]
    StallDetected { tool_name: String, count: usize },

    /// Workflow step not found
    #[error("step `{step_id}` not found in workflow")]
    StepNotFound { step_id: String },

    /// Workflow is already complete or in invalid state for requested operation
    #[error("invalid workflow state: {0}")]
    InvalidState(String),

    /// Generic/wrapped error
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Convenience Result alias for workflow operations.
pub type WorkflowResult<T> = std::result::Result<T, WorkflowError>;
