use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::error::{WorkflowError, WorkflowResult};
use crate::traits::{Message, MessageRole};

/// Execution status of a workflow run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowStatus {
    /// Not yet started
    Pending,
    /// Currently running
    Running,
    /// Completed successfully
    Completed,
    /// Failed with error
    Failed,
    /// Paused (e.g., awaiting approval)
    Paused,
}

/// Runtime state for a single workflow execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowState {
    /// Unique run ID
    pub run_id: String,
    /// Name of the workflow being executed
    pub workflow_name: String,
    /// Current execution status
    pub status: WorkflowStatus,
    /// Current step index (in the top-level steps list)
    pub current_step: usize,
    /// Variable store — all workflow variables
    pub variables: serde_json::Map<String, serde_json::Value>,
    /// Variable names that originate from external/untrusted sources (triggers, user input).
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    pub untrusted_vars: HashSet<String>,
    /// Conversation/message history for model context
    pub messages: Vec<Message>,
    /// Number of iterations completed (for loop limits)
    pub iteration_count: usize,
    /// Number of tool calls made (for tool call limits)
    pub tool_call_count: usize,
    /// Created timestamp (ISO 8601)
    pub created_at: String,
    /// Last updated timestamp
    pub updated_at: String,
}

/// Return the current UTC time formatted as ISO 8601 (e.g. `2025-01-15T08:30:00Z`).
fn now_iso8601() -> String {
    use std::time::SystemTime;

    let duration = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default();

    let total_secs = duration.as_secs();
    let days = total_secs / 86400;
    let day_secs = total_secs % 86400;
    let hours = day_secs / 3600;
    let minutes = (day_secs % 3600) / 60;
    let seconds = day_secs % 60;

    // Convert days-since-epoch to y/m/d using a civil-calendar algorithm.
    let (year, month, day) = epoch_days_to_ymd(days as i64);

    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}Z")
}

/// Convert days since Unix epoch (1970-01-01) to (year, month, day).
/// Uses the algorithm from Howard Hinnant's `chrono`-compatible date library.
fn epoch_days_to_ymd(epoch_days: i64) -> (i64, u32, u32) {
    let z = epoch_days + 719468; // shift to 0000-03-01 epoch
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097) as u64; // day of era [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // year of era [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // day of year [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };
    (y, m as u32, d as u32)
}

impl WorkflowState {
    /// Create a new state for a workflow run.
    pub fn new(run_id: String, workflow_name: String) -> Self {
        let now = now_iso8601();
        Self {
            run_id,
            workflow_name,
            status: WorkflowStatus::Pending,
            current_step: 0,
            variables: serde_json::Map::new(),
            untrusted_vars: HashSet::new(),
            messages: Vec::new(),
            iteration_count: 0,
            tool_call_count: 0,
            created_at: now.clone(),
            updated_at: now,
        }
    }

    /// Set a variable in the store.
    pub fn set_variable(&mut self, name: impl Into<String>, value: serde_json::Value) {
        self.variables.insert(name.into(), value);
        self.touch();
    }

    /// Get a variable from the store.
    pub fn get_variable(&self, name: &str) -> Option<&serde_json::Value> {
        self.variables.get(name)
    }

    /// Add a message to conversation history.
    pub fn push_message(&mut self, role: MessageRole, content: String) {
        self.messages.push(Message { role, content });
        self.touch();
    }

    /// Increment iteration count, return error if over limit.
    pub fn increment_iteration(&mut self, max: usize) -> WorkflowResult<()> {
        self.iteration_count += 1;
        if self.iteration_count > max {
            return Err(WorkflowError::LimitExceeded { kind: "iteration".into(), limit: max });
        }
        self.touch();
        Ok(())
    }

    /// Increment tool call count, return error if over limit.
    pub fn increment_tool_calls(&mut self, count: usize, max: usize) -> WorkflowResult<()> {
        self.tool_call_count += count;
        if self.tool_call_count > max {
            return Err(WorkflowError::LimitExceeded { kind: "tool_call".into(), limit: max });
        }
        self.touch();
        Ok(())
    }

    /// Advance to the next step.
    pub fn advance_step(&mut self) {
        self.current_step += 1;
        self.touch();
    }

    /// Jump to a specific step index.
    pub fn jump_to_step(&mut self, step_index: usize) {
        self.current_step = step_index;
        self.touch();
    }

    /// Mark as completed.
    pub fn complete(&mut self) {
        self.status = WorkflowStatus::Completed;
        self.touch();
    }

    /// Mark as failed.
    pub fn fail(&mut self) {
        self.status = WorkflowStatus::Failed;
        self.touch();
    }

    /// Touch the updated_at timestamp.
    fn touch(&mut self) {
        self.updated_at = now_iso8601();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_state_has_pending_status() {
        let state = WorkflowState::new("run-1".into(), "my-workflow".into());
        assert_eq!(state.status, WorkflowStatus::Pending);
        assert_eq!(state.current_step, 0);
        assert_eq!(state.iteration_count, 0);
        assert_eq!(state.tool_call_count, 0);
        assert!(state.variables.is_empty());
        assert!(state.messages.is_empty());
    }

    #[test]
    fn set_and_get_variable() {
        let mut state = WorkflowState::new("run-1".into(), "wf".into());
        state.set_variable("key", serde_json::json!("value"));
        assert_eq!(state.get_variable("key"), Some(&serde_json::json!("value")));
        assert_eq!(state.get_variable("missing"), None);
    }

    #[test]
    fn push_message_adds_to_history() {
        let mut state = WorkflowState::new("run-1".into(), "wf".into());
        state.push_message(MessageRole::User, "hello".into());
        assert_eq!(state.messages.len(), 1);
        assert_eq!(state.messages[0].content, "hello");
    }

    #[test]
    fn increment_iteration_within_limit() {
        let mut state = WorkflowState::new("run-1".into(), "wf".into());
        assert!(state.increment_iteration(5).is_ok());
        assert_eq!(state.iteration_count, 1);
    }

    #[test]
    fn increment_iteration_exceeds_limit() {
        let mut state = WorkflowState::new("run-1".into(), "wf".into());
        state.iteration_count = 5;
        let result = state.increment_iteration(5);
        assert!(result.is_err());
    }

    #[test]
    fn increment_tool_calls_exceeds_limit() {
        let mut state = WorkflowState::new("run-1".into(), "wf".into());
        let result = state.increment_tool_calls(101, 100);
        assert!(result.is_err());
    }

    #[test]
    fn advance_and_jump_step() {
        let mut state = WorkflowState::new("run-1".into(), "wf".into());
        state.advance_step();
        assert_eq!(state.current_step, 1);
        state.jump_to_step(5);
        assert_eq!(state.current_step, 5);
    }

    #[test]
    fn complete_and_fail() {
        let mut state = WorkflowState::new("run-1".into(), "wf".into());
        state.complete();
        assert_eq!(state.status, WorkflowStatus::Completed);

        let mut state2 = WorkflowState::new("run-2".into(), "wf".into());
        state2.fail();
        assert_eq!(state2.status, WorkflowStatus::Failed);
    }

    #[test]
    fn serde_round_trip() {
        let mut state = WorkflowState::new("run-1".into(), "wf".into());
        state.set_variable("x", serde_json::json!(42));
        state.push_message(MessageRole::Assistant, "hi".into());

        let json = serde_json::to_string(&state).unwrap();
        let restored: WorkflowState = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.run_id, "run-1");
        assert_eq!(restored.get_variable("x"), Some(&serde_json::json!(42)));
        assert_eq!(restored.messages.len(), 1);
    }

    #[test]
    fn status_serializes_as_snake_case() {
        let json = serde_json::to_string(&WorkflowStatus::Running).unwrap();
        assert_eq!(json, "\"running\"");
        let json = serde_json::to_string(&WorkflowStatus::Completed).unwrap();
        assert_eq!(json, "\"completed\"");
    }

    #[test]
    fn timestamp_format_is_valid() {
        let ts = now_iso8601();
        // Basic shape: YYYY-MM-DDTHH:MM:SSZ
        assert_eq!(ts.len(), 20);
        assert!(ts.ends_with('Z'));
        assert_eq!(&ts[4..5], "-");
        assert_eq!(&ts[10..11], "T");
    }
}
