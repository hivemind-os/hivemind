use thiserror::Error;

#[derive(Debug, Error)]
pub enum WorkflowError {
    #[error("Definition not found: {name} v{version}")]
    DefinitionNotFound { name: String, version: String },

    #[error("Instance not found: {id}")]
    InstanceNotFound { id: i64 },

    #[error("Invalid definition: {reason}")]
    InvalidDefinition { reason: String },

    #[error("Expression error: {0}")]
    Expression(String),

    #[error("Step execution failed: step={step_id}, reason={reason}")]
    StepFailed { step_id: String, reason: String },

    #[error("Workflow is in invalid state for this operation: {status}")]
    InvalidState { status: String },

    #[error("Cycle detected in workflow graph involving step: {step_id}")]
    CycleDetected { step_id: String },

    #[error("Store error: {0}")]
    Store(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Step not found: {step_id}")]
    StepNotFound { step_id: String },

    #[error("Workflow killed")]
    Killed,

    #[error("Validation error: {reason}")]
    ValidationError { reason: String },

    #[error("{0}")]
    Other(String),
}

impl From<serde_json::Error> for WorkflowError {
    fn from(e: serde_json::Error) -> Self {
        WorkflowError::Serialization(e.to_string())
    }
}

impl From<serde_yaml::Error> for WorkflowError {
    fn from(e: serde_yaml::Error) -> Self {
        WorkflowError::Serialization(e.to_string())
    }
}

impl From<rusqlite::Error> for WorkflowError {
    fn from(e: rusqlite::Error) -> Self {
        WorkflowError::Store(e.to_string())
    }
}
