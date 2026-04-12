use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::permissions::PermissionRule;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl TaskStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "pending" => Self::Pending,
            "running" => Self::Running,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            other => {
                tracing::warn!(
                    status = other,
                    "unknown TaskStatus in database, defaulting to Pending"
                );
                Self::Pending
            }
        }
    }
}

impl std::str::FromStr for TaskStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending" => Ok(Self::Pending),
            "running" => Ok(Self::Running),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(format!("unknown TaskStatus: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskRunStatus {
    Success,
    Failure,
}

impl TaskRunStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
        }
    }

    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "success" => Self::Success,
            other => {
                tracing::warn!(
                    status = other,
                    "unknown TaskRunStatus in database, defaulting to Failure"
                );
                Self::Failure
            }
        }
    }
}

impl std::str::FromStr for TaskRunStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "success" => Ok(Self::Success),
            "failure" => Ok(Self::Failure),
            other => Err(format!("unknown TaskRunStatus: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TaskSchedule {
    Once,
    Scheduled { run_at_ms: u64 },
    Cron { expression: String },
}

impl TaskSchedule {
    /// Returns the schedule type discriminant as stored in the DB.
    pub fn schedule_type(&self) -> &'static str {
        match self {
            Self::Once => "once",
            Self::Scheduled { .. } => "scheduled",
            Self::Cron { .. } => "cron",
        }
    }

    /// Returns the cron expression if this is a Cron schedule.
    pub fn cron_expression(&self) -> Option<&str> {
        match self {
            Self::Cron { expression } => Some(expression.as_str()),
            _ => None,
        }
    }

    /// Returns the run_at_ms if this is a Scheduled schedule.
    pub fn run_at_ms(&self) -> Option<u64> {
        match self {
            Self::Scheduled { run_at_ms } => Some(*run_at_ms),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TaskAction {
    SendMessage {
        session_id: String,
        content: String,
    },
    HttpWebhook {
        url: String,
        method: String,
        body: Option<String>,
        #[serde(default)]
        headers: Option<std::collections::HashMap<String, String>>,
    },
    EmitEvent {
        topic: String,
        payload: Value,
    },
    InvokeAgent {
        persona_id: String,
        task: String,
        #[serde(default)]
        friendly_name: Option<String>,
        #[serde(default)]
        timeout_secs: Option<u64>,
        #[serde(default)]
        permissions: Option<Vec<PermissionRule>>,
    },
    CallTool {
        tool_id: String,
        arguments: Value,
    },
    CompositeAction {
        actions: Vec<TaskAction>,
        #[serde(default)]
        stop_on_failure: bool,
    },
    LaunchWorkflow {
        definition: String,
        #[serde(default)]
        version: Option<String>,
        #[serde(default)]
        inputs: Value,
        #[serde(default)]
        trigger_step_id: Option<String>,
    },
}

impl TaskAction {
    /// Returns the discriminant name for registry lookup.
    pub fn type_name(&self) -> &'static str {
        match self {
            TaskAction::SendMessage { .. } => "SendMessage",
            TaskAction::HttpWebhook { .. } => "HttpWebhook",
            TaskAction::EmitEvent { .. } => "EmitEvent",
            TaskAction::InvokeAgent { .. } => "InvokeAgent",
            TaskAction::CallTool { .. } => "CallTool",
            TaskAction::CompositeAction { .. } => "CompositeAction",
            TaskAction::LaunchWorkflow { .. } => "LaunchWorkflow",
        }
    }

    /// Returns the serde tag value as stored in the DB `action_type` column.
    pub fn action_type(&self) -> &'static str {
        match self {
            TaskAction::SendMessage { .. } => "send_message",
            TaskAction::HttpWebhook { .. } => "http_webhook",
            TaskAction::EmitEvent { .. } => "emit_event",
            TaskAction::InvokeAgent { .. } => "invoke_agent",
            TaskAction::CallTool { .. } => "call_tool",
            TaskAction::CompositeAction { .. } => "composite_action",
            TaskAction::LaunchWorkflow { .. } => "launch_workflow",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledTask {
    pub id: String,
    pub name: String,
    pub description: String,
    pub schedule: TaskSchedule,
    pub action: TaskAction,
    pub status: TaskStatus,
    /// Denormalized schedule type for SQL-level filtering.
    #[serde(default)]
    pub schedule_type: String,
    /// Denormalized action type for SQL-level filtering.
    #[serde(default)]
    pub action_type: String,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    /// Timestamp when the task reached a terminal state (completed/failed/cancelled).
    #[serde(default)]
    pub completed_at_ms: Option<u64>,
    pub last_run_ms: Option<u64>,
    pub next_run_ms: Option<u64>,
    pub run_count: u32,
    pub last_error: Option<String>,
    #[serde(default)]
    pub owner_session_id: Option<String>,
    #[serde(default)]
    pub owner_agent_id: Option<String>,
    /// Maximum number of automatic retries on failure (None = no retries).
    #[serde(default)]
    pub max_retries: Option<u32>,
    /// Delay in milliseconds before retrying a failed task.
    #[serde(default)]
    pub retry_delay_ms: Option<u64>,
    /// Number of retry attempts so far (resets on success).
    #[serde(default)]
    pub retry_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTaskRequest {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub schedule: TaskSchedule,
    pub action: TaskAction,
    #[serde(default)]
    pub owner_session_id: Option<String>,
    #[serde(default)]
    pub owner_agent_id: Option<String>,
    /// Maximum number of automatic retries on failure (None = no retries).
    #[serde(default)]
    pub max_retries: Option<u32>,
    /// Delay in milliseconds before retrying a failed task.
    #[serde(default)]
    pub retry_delay_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateTaskRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub schedule: Option<TaskSchedule>,
    pub action: Option<TaskAction>,
    /// Update max retries. `Some(Some(n))` sets to n, `Some(None)` clears.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<Option<u32>>,
    /// Update retry delay. `Some(Some(ms))` sets to ms, `Some(None)` clears.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_delay_ms: Option<Option<u64>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRun {
    pub id: String,
    pub task_id: String,
    pub started_at_ms: u64,
    pub completed_at_ms: Option<u64>,
    pub status: TaskRunStatus,
    pub error: Option<String>,
    pub result: Option<Value>,
}

/// Notification sent to originators when a scheduled task completes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskCompletionNotification {
    pub task_id: String,
    pub task_name: String,
    pub run_id: String,
    pub status: TaskRunStatus,
    pub result: Option<Value>,
    pub error: Option<String>,
    pub started_at_ms: u64,
    pub completed_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ListTasksFilter {
    pub session_id: Option<String>,
    pub agent_id: Option<String>,
    pub status: Option<String>,
    pub schedule_type: Option<String>,
    pub action_type: Option<String>,
}
