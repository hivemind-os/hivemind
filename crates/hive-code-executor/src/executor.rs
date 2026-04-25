//! Core trait and types for code execution backends.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Supported languages for code execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    Python,
}

impl std::fmt::Display for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Language::Python => write!(f, "python"),
        }
    }
}

/// Result of executing a code block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    /// Standard output captured during execution.
    pub stdout: String,
    /// Standard error captured during execution.
    pub stderr: String,
    /// Whether the execution resulted in an error (non-zero exit, exception, etc.).
    pub is_error: bool,
    /// Wall-clock duration of the execution.
    pub duration_ms: u64,
}

impl ExecutionResult {
    /// Produce a combined output string suitable for feeding back to the LLM.
    pub fn to_observation(&self) -> String {
        let mut parts = Vec::new();
        if !self.stdout.is_empty() {
            parts.push(self.stdout.clone());
        }
        if !self.stderr.is_empty() {
            if self.is_error {
                parts.push(format!("[stderr]\n{}", self.stderr));
            } else {
                parts.push(format!("[stderr]\n{}", self.stderr));
            }
        }
        if parts.is_empty() {
            "(no output)".to_string()
        } else {
            parts.join("\n")
        }
    }
}

/// Configuration for a code executor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutorConfig {
    /// Maximum wall-clock time for a single code block execution.
    #[serde(default = "default_execution_timeout_secs")]
    pub execution_timeout_secs: u64,
    /// Maximum output size (stdout + stderr combined) in bytes.
    #[serde(default = "default_max_output_bytes")]
    pub max_output_bytes: usize,
    /// Memory limit for the execution environment in MB.
    #[serde(default = "default_memory_limit_mb")]
    pub memory_limit_mb: u64,
    /// Working directory for code execution.
    pub working_directory: Option<String>,
    /// Whether network access is allowed.
    #[serde(default)]
    pub allow_network: bool,
}

fn default_execution_timeout_secs() -> u64 {
    30
}
fn default_max_output_bytes() -> usize {
    1_048_576 // 1 MB
}
fn default_memory_limit_mb() -> u64 {
    256
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            execution_timeout_secs: default_execution_timeout_secs(),
            max_output_bytes: default_max_output_bytes(),
            memory_limit_mb: default_memory_limit_mb(),
            working_directory: None,
            allow_network: false,
        }
    }
}

impl ExecutorConfig {
    pub fn execution_timeout(&self) -> Duration {
        Duration::from_secs(self.execution_timeout_secs)
    }
}

/// Errors from code execution.
#[derive(Debug, thiserror::Error)]
pub enum ExecutorError {
    #[error("execution timed out after {timeout_secs}s")]
    Timeout { timeout_secs: u64 },
    #[error("output exceeded maximum size of {max_bytes} bytes")]
    OutputTooLarge { max_bytes: usize },
    #[error("executor not ready: {0}")]
    NotReady(String),
    #[error("session not found: {0}")]
    SessionNotFound(String),
    #[error("execution failed: {0}")]
    ExecutionFailed(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Core trait for code execution backends.
///
/// Implementations must be `Send + Sync` to be shared across async tasks.
/// Each executor manages a persistent session — state (variables, imports)
/// survives across `execute()` calls until `reset()` or `shutdown()`.
#[async_trait::async_trait]
pub trait CodeExecutor: Send + Sync {
    /// Execute a code block and return the result.
    ///
    /// The execution happens in the context of the current session — previous
    /// variables, imports, and definitions are visible.
    async fn execute(&self, code: &str, language: Language) -> Result<ExecutionResult, ExecutorError>;

    /// Reset the execution environment to a clean state.
    ///
    /// All variables, imports, and side effects are discarded.
    async fn reset(&self) -> Result<(), ExecutorError>;

    /// Shut down the executor and release all resources.
    ///
    /// After shutdown, further calls to `execute()` will return an error.
    async fn shutdown(&self) -> Result<(), ExecutorError>;

    /// Check whether the executor is alive and ready.
    async fn is_alive(&self) -> bool;
}
