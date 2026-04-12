use thiserror::Error;

#[derive(Debug, Error)]
pub enum AgentError {
    #[error("Agent not found: {0}")]
    AgentNotFound(String),

    #[error("Agent channel closed: {0}")]
    ChannelClosed(String),

    #[error("Agent execution setup failed: {0}")]
    ExecutionSetup(String),

    #[error("Topology error: {0}")]
    TopologyError(String),

    #[error("Agent already exists: {0}")]
    AlreadyExists(String),

    #[error("Access denied: {0}")]
    AccessDenied(String),
}
