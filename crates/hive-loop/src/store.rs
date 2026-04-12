use std::collections::HashMap;

use tokio::sync::Mutex;

use crate::error::WorkflowResult;
use crate::state::WorkflowState;

/// Persistent store for workflow execution state.
/// Callers can inject their own implementation (SQLite, Redis, etc.)
/// The default InMemoryStore is provided for simple/testing use cases.
#[async_trait::async_trait]
pub trait WorkflowStore: Send + Sync {
    /// Save or update a workflow state.
    async fn save(&self, state: &WorkflowState) -> WorkflowResult<()>;

    /// Load a workflow state by run ID. Returns None if not found.
    async fn load(&self, run_id: &str) -> WorkflowResult<Option<WorkflowState>>;

    /// Delete a workflow state by run ID.
    async fn delete(&self, run_id: &str) -> WorkflowResult<()>;

    /// List all stored run IDs.
    async fn list_runs(&self) -> WorkflowResult<Vec<String>>;
}

/// In-memory workflow store backed by a HashMap behind a Mutex.
/// Suitable for testing and single-session use. Data is lost on drop.
#[derive(Default)]
pub struct InMemoryStore {
    states: Mutex<HashMap<String, WorkflowState>>,
}

impl InMemoryStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait::async_trait]
impl WorkflowStore for InMemoryStore {
    async fn save(&self, state: &WorkflowState) -> WorkflowResult<()> {
        let mut guard = self.states.lock().await;
        guard.insert(state.run_id.clone(), state.clone());
        Ok(())
    }

    async fn load(&self, run_id: &str) -> WorkflowResult<Option<WorkflowState>> {
        let guard = self.states.lock().await;
        Ok(guard.get(run_id).cloned())
    }

    async fn delete(&self, run_id: &str) -> WorkflowResult<()> {
        let mut guard = self.states.lock().await;
        guard.remove(run_id);
        Ok(())
    }

    async fn list_runs(&self) -> WorkflowResult<Vec<String>> {
        let guard = self.states.lock().await;
        Ok(guard.keys().cloned().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::WorkflowState;

    fn make_state(run_id: &str) -> WorkflowState {
        WorkflowState::new(run_id.to_string(), "test-workflow".to_string())
    }

    #[tokio::test]
    async fn save_and_load() {
        let store = InMemoryStore::new();
        let state = make_state("run-1");
        store.save(&state).await.unwrap();

        let loaded = store.load("run-1").await.unwrap().expect("should exist");
        assert_eq!(loaded.run_id, "run-1");
        assert_eq!(loaded.workflow_name, "test-workflow");
        assert_eq!(loaded.current_step, state.current_step);
        assert_eq!(loaded.iteration_count, state.iteration_count);
    }

    #[tokio::test]
    async fn load_nonexistent() {
        let store = InMemoryStore::new();
        let result = store.load("no-such-run").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn delete_removes_state() {
        let store = InMemoryStore::new();
        store.save(&make_state("run-1")).await.unwrap();
        store.delete("run-1").await.unwrap();
        assert!(store.load("run-1").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn list_runs_returns_all() {
        let store = InMemoryStore::new();
        store.save(&make_state("a")).await.unwrap();
        store.save(&make_state("b")).await.unwrap();
        store.save(&make_state("c")).await.unwrap();

        let mut runs = store.list_runs().await.unwrap();
        runs.sort();
        assert_eq!(runs, vec!["a", "b", "c"]);
    }

    #[tokio::test]
    async fn save_overwrites_existing() {
        let store = InMemoryStore::new();
        let mut state = make_state("run-1");
        store.save(&state).await.unwrap();

        state.current_step = 42;
        state.iteration_count = 7;
        store.save(&state).await.unwrap();

        let loaded = store.load("run-1").await.unwrap().expect("should exist");
        assert_eq!(loaded.current_step, 42);
        assert_eq!(loaded.iteration_count, 7);
    }
}
