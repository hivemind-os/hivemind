use crate::{ListTasksFilter, ScheduledTask, SchedulerError, TaskRun};

/// Abstraction over the storage backend for the scheduler.
///
/// All methods are synchronous (matching the `parking_lot::Mutex<Connection>` pattern
/// used by the SQLite implementation).  Implementations must be `Send + Sync`.
pub trait SchedulerStore: Send + Sync {
    // ----- Task CRUD --------------------------------------------------------

    /// Persist a newly-created task.
    ///
    /// `schedule_json` and `action_json` are the pre-serialised JSON
    /// representations of the schedule and action fields.
    fn insert_task(
        &self,
        task: &ScheduledTask,
        schedule_json: &str,
        action_json: &str,
    ) -> Result<(), SchedulerError>;

    /// Retrieve a single task by ID.
    fn get_task(&self, task_id: &str) -> Result<ScheduledTask, SchedulerError>;

    /// List tasks, optionally filtered by owner session/agent.
    fn list_tasks_filtered(
        &self,
        filter: &ListTasksFilter,
    ) -> Result<Vec<ScheduledTask>, SchedulerError>;

    /// List tasks whose action targets a specific workflow definition.
    fn list_tasks_for_workflow(
        &self,
        definition: &str,
    ) -> Result<Vec<ScheduledTask>, SchedulerError>;

    /// Cancel a pending/running task.
    ///
    /// Returns the number of rows affected (0 means the task was not in a
    /// cancellable state).
    fn cancel_task(&self, task_id: &str, now: u64) -> Result<usize, SchedulerError>;

    /// Check whether a task with the given ID exists.
    fn task_exists(&self, task_id: &str) -> bool;

    /// Delete a task and all its associated runs.
    ///
    /// Returns the number of deleted task rows (0 = not found).
    fn delete_task(&self, task_id: &str) -> Result<usize, SchedulerError>;

    // ----- Partial updates --------------------------------------------------

    /// Update only the task name.
    fn update_task_name(&self, task_id: &str, name: &str, now: u64) -> Result<(), SchedulerError>;

    /// Update only the task description.
    fn update_task_description(
        &self,
        task_id: &str,
        description: &str,
        now: u64,
    ) -> Result<(), SchedulerError>;

    /// Update the task schedule (serialised JSON) and next run time.
    #[allow(clippy::too_many_arguments)]
    fn update_task_schedule(
        &self,
        task_id: &str,
        schedule_json: &str,
        schedule_type: &str,
        cron_expression: Option<&str>,
        run_at_ms: Option<u64>,
        next_run_ms: Option<u64>,
        now: u64,
    ) -> Result<(), SchedulerError>;

    /// Update the task action (serialised JSON).
    fn update_task_action(
        &self,
        task_id: &str,
        action_json: &str,
        action_type: &str,
        now: u64,
    ) -> Result<(), SchedulerError>;

    /// Reset a task's status to `pending`.
    fn reset_task_to_pending(&self, task_id: &str, now: u64) -> Result<(), SchedulerError>;

    // ----- Runs -------------------------------------------------------------

    /// List the most recent runs for a task (up to `limit` entries).
    fn list_task_runs(&self, task_id: &str, limit: usize) -> Result<Vec<TaskRun>, SchedulerError>;

    /// Update multiple task fields atomically.
    /// Only fields that are `Some` are updated.
    #[allow(clippy::too_many_arguments)]
    fn update_task_atomic(
        &self,
        task_id: &str,
        name: Option<&str>,
        description: Option<&str>,
        schedule_json: Option<&str>,
        schedule_type: Option<&str>,
        cron_expression: Option<Option<&str>>,
        run_at_ms: Option<Option<u64>>,
        next_run_ms: Option<Option<u64>>,
        action_json: Option<&str>,
        action_type: Option<&str>,
        now: u64,
        max_retries: Option<Option<u32>>,
        retry_delay_ms: Option<Option<u64>>,
    ) -> Result<(), SchedulerError>;

    // ----- Background-loop helpers ------------------------------------------

    /// Make all pending tasks immediately due (used for testing).
    #[cfg(any(test, feature = "test-support"))]
    fn force_all_due(&self);

    /// Return the IDs of all tasks that are due to run (pending with
    /// `next_run_ms <= now` or `next_run_ms IS NULL`).
    fn get_due_task_ids(&self, now: u64) -> Vec<String>;

    /// Atomically transition a task from `pending` → `running`.
    ///
    /// Returns `true` if the transition succeeded (the task was pending).
    fn mark_task_running(&self, task_id: &str, now: u64) -> bool;

    /// Update a task row after a run completes.
    ///
    /// * `status` — new task status string (e.g. `"pending"`, `"completed"`, `"failed"`).
    /// * `last_error` — error message to store, or `None` to clear.
    /// * `reset_retry_count` — if `true`, set `retry_count = 0`.
    /// * `new_retry_count` — if `Some(n)`, set `retry_count = n` (takes
    ///   precedence over `reset_retry_count`).
    /// * `completed_at_ms` — if `Some(ts)`, set the terminal-state timestamp.
    #[allow(clippy::too_many_arguments)]
    fn update_task_after_run(
        &self,
        task_id: &str,
        status: &str,
        updated_at_ms: u64,
        last_run_ms: u64,
        next_run_ms: Option<u64>,
        last_error: Option<&str>,
        reset_retry_count: bool,
        new_retry_count: Option<u32>,
        completed_at_ms: Option<u64>,
    ) -> Result<usize, SchedulerError>;

    /// Insert a task-run record.
    #[allow(clippy::too_many_arguments)]
    fn insert_task_run(
        &self,
        run_id: &str,
        task_id: &str,
        started_at_ms: u64,
        completed_at_ms: u64,
        status: &str,
        error: Option<&str>,
        result_json: Option<&str>,
    ) -> Result<(), SchedulerError>;

    /// Delete old runs for a task, keeping only the most recent `keep`.
    fn prune_old_runs(&self, task_id: &str, keep: usize);

    /// Count active (pending + running) tasks globally.
    fn count_active_tasks(&self) -> usize;

    /// Count active (pending + running) tasks for a specific session.
    fn count_active_tasks_for_session(&self, session_id: &str) -> usize;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TaskAction, TaskRunStatus, TaskSchedule, TaskStatus};
    use parking_lot::Mutex;
    use serde_json::Value;
    use std::collections::HashMap;

    struct InMemorySchedulerInner {
        tasks: HashMap<String, ScheduledTask>,
        runs: Vec<TaskRun>,
    }

    pub struct InMemorySchedulerStore {
        inner: Mutex<InMemorySchedulerInner>,
    }

    impl InMemorySchedulerStore {
        pub fn new() -> Self {
            Self {
                inner: Mutex::new(InMemorySchedulerInner {
                    tasks: HashMap::new(),
                    runs: Vec::new(),
                }),
            }
        }
    }

    impl SchedulerStore for InMemorySchedulerStore {
        fn insert_task(
            &self,
            task: &ScheduledTask,
            _schedule_json: &str,
            _action_json: &str,
        ) -> Result<(), SchedulerError> {
            let mut inner = self.inner.lock();
            inner.tasks.insert(task.id.clone(), task.clone());
            Ok(())
        }

        fn get_task(&self, task_id: &str) -> Result<ScheduledTask, SchedulerError> {
            let inner = self.inner.lock();
            inner
                .tasks
                .get(task_id)
                .cloned()
                .ok_or_else(|| SchedulerError::TaskNotFound { id: task_id.to_string() })
        }

        fn list_tasks_filtered(
            &self,
            filter: &ListTasksFilter,
        ) -> Result<Vec<ScheduledTask>, SchedulerError> {
            let inner = self.inner.lock();
            Ok(inner
                .tasks
                .values()
                .filter(|t| {
                    if let Some(ref sid) = filter.session_id {
                        if t.owner_session_id.as_deref() != Some(sid.as_str()) {
                            return false;
                        }
                    }
                    if let Some(ref aid) = filter.agent_id {
                        if t.owner_agent_id.as_deref() != Some(aid.as_str()) {
                            return false;
                        }
                    }
                    if let Some(ref status) = filter.status {
                        if t.status.as_str() != status.as_str() {
                            return false;
                        }
                    }
                    if let Some(ref st) = filter.schedule_type {
                        if t.schedule_type.as_str() != st.as_str() {
                            return false;
                        }
                    }
                    if let Some(ref at) = filter.action_type {
                        if t.action_type.as_str() != at.as_str() {
                            return false;
                        }
                    }
                    true
                })
                .cloned()
                .collect())
        }

        fn list_tasks_for_workflow(
            &self,
            definition: &str,
        ) -> Result<Vec<ScheduledTask>, SchedulerError> {
            let inner = self.inner.lock();
            Ok(inner
                .tasks
                .values()
                .filter(|t| matches!(&t.action, TaskAction::LaunchWorkflow { definition: d, .. } if d == definition))
                .cloned()
                .collect())
        }

        fn cancel_task(&self, task_id: &str, now: u64) -> Result<usize, SchedulerError> {
            let mut inner = self.inner.lock();
            if let Some(task) = inner.tasks.get_mut(task_id) {
                if task.status == TaskStatus::Pending || task.status == TaskStatus::Running {
                    task.status = TaskStatus::Cancelled;
                    task.updated_at_ms = now;
                    task.completed_at_ms = Some(now);
                    return Ok(1);
                }
            }
            Ok(0)
        }

        fn task_exists(&self, task_id: &str) -> bool {
            self.inner.lock().tasks.contains_key(task_id)
        }

        fn delete_task(&self, task_id: &str) -> Result<usize, SchedulerError> {
            let mut inner = self.inner.lock();
            if inner.tasks.remove(task_id).is_some() {
                inner.runs.retain(|r| r.task_id != task_id);
                Ok(1)
            } else {
                Ok(0)
            }
        }

        fn update_task_name(
            &self,
            task_id: &str,
            name: &str,
            now: u64,
        ) -> Result<(), SchedulerError> {
            let mut inner = self.inner.lock();
            let task = inner
                .tasks
                .get_mut(task_id)
                .ok_or_else(|| SchedulerError::TaskNotFound { id: task_id.to_string() })?;
            task.name = name.to_string();
            task.updated_at_ms = now;
            Ok(())
        }

        fn update_task_description(
            &self,
            task_id: &str,
            description: &str,
            now: u64,
        ) -> Result<(), SchedulerError> {
            let mut inner = self.inner.lock();
            let task = inner
                .tasks
                .get_mut(task_id)
                .ok_or_else(|| SchedulerError::TaskNotFound { id: task_id.to_string() })?;
            task.description = description.to_string();
            task.updated_at_ms = now;
            Ok(())
        }

        fn update_task_schedule(
            &self,
            task_id: &str,
            schedule_json: &str,
            _schedule_type: &str,
            _cron_expression: Option<&str>,
            _run_at_ms: Option<u64>,
            next_run_ms: Option<u64>,
            now: u64,
        ) -> Result<(), SchedulerError> {
            let mut inner = self.inner.lock();
            let task = inner
                .tasks
                .get_mut(task_id)
                .ok_or_else(|| SchedulerError::TaskNotFound { id: task_id.to_string() })?;
            let new_schedule: TaskSchedule = serde_json::from_str(schedule_json)
                .map_err(|e| SchedulerError::Internal(e.to_string()))?;
            task.schedule_type = new_schedule.schedule_type().to_string();
            task.schedule = new_schedule;
            task.next_run_ms = next_run_ms;
            task.updated_at_ms = now;
            Ok(())
        }

        fn update_task_action(
            &self,
            task_id: &str,
            action_json: &str,
            _action_type: &str,
            now: u64,
        ) -> Result<(), SchedulerError> {
            let mut inner = self.inner.lock();
            let task = inner
                .tasks
                .get_mut(task_id)
                .ok_or_else(|| SchedulerError::TaskNotFound { id: task_id.to_string() })?;
            let new_action: TaskAction = serde_json::from_str(action_json)
                .map_err(|e| SchedulerError::Internal(e.to_string()))?;
            task.action_type = new_action.action_type().to_string();
            task.action = new_action;
            task.updated_at_ms = now;
            Ok(())
        }

        fn reset_task_to_pending(&self, task_id: &str, now: u64) -> Result<(), SchedulerError> {
            let mut inner = self.inner.lock();
            let task = inner
                .tasks
                .get_mut(task_id)
                .ok_or_else(|| SchedulerError::TaskNotFound { id: task_id.to_string() })?;
            task.status = TaskStatus::Pending;
            task.retry_count = 0;
            task.last_error = None;
            task.completed_at_ms = None;
            task.updated_at_ms = now;
            Ok(())
        }

        fn update_task_atomic(
            &self,
            task_id: &str,
            name: Option<&str>,
            description: Option<&str>,
            schedule_json: Option<&str>,
            schedule_type: Option<&str>,
            _cron_expression: Option<Option<&str>>,
            _run_at_ms: Option<Option<u64>>,
            next_run_ms: Option<Option<u64>>,
            action_json: Option<&str>,
            action_type: Option<&str>,
            now: u64,
            max_retries: Option<Option<u32>>,
            retry_delay_ms: Option<Option<u64>>,
        ) -> Result<(), SchedulerError> {
            let mut inner = self.inner.lock();
            let task = inner
                .tasks
                .get_mut(task_id)
                .ok_or_else(|| SchedulerError::TaskNotFound { id: task_id.to_string() })?;
            if let Some(n) = name {
                task.name = n.to_string();
            }
            if let Some(d) = description {
                task.description = d.to_string();
            }
            if let Some(s) = schedule_json {
                task.schedule =
                    serde_json::from_str(s).map_err(|e| SchedulerError::Internal(e.to_string()))?;
            }
            if let Some(st) = schedule_type {
                task.schedule_type = st.to_string();
            }
            if let Some(next) = next_run_ms {
                task.next_run_ms = next;
            }
            if let Some(a) = action_json {
                task.action =
                    serde_json::from_str(a).map_err(|e| SchedulerError::Internal(e.to_string()))?;
            }
            if let Some(at) = action_type {
                task.action_type = at.to_string();
            }
            if let Some(mr) = max_retries {
                task.max_retries = mr;
            }
            if let Some(rd) = retry_delay_ms {
                task.retry_delay_ms = rd;
            }
            task.updated_at_ms = now;
            Ok(())
        }

        fn list_task_runs(
            &self,
            task_id: &str,
            limit: usize,
        ) -> Result<Vec<TaskRun>, SchedulerError> {
            let inner = self.inner.lock();
            let mut runs: Vec<TaskRun> =
                inner.runs.iter().filter(|r| r.task_id == task_id).cloned().collect();
            runs.sort_by(|a, b| b.started_at_ms.cmp(&a.started_at_ms));
            runs.truncate(limit);
            Ok(runs)
        }

        #[cfg(any(test, feature = "test-support"))]
        fn force_all_due(&self) {
            let mut inner = self.inner.lock();
            for task in inner.tasks.values_mut() {
                if task.status == TaskStatus::Pending {
                    task.next_run_ms = Some(0);
                }
            }
        }

        fn get_due_task_ids(&self, now: u64) -> Vec<String> {
            let inner = self.inner.lock();
            inner
                .tasks
                .values()
                .filter(|t| {
                    t.status == TaskStatus::Pending
                        && match t.next_run_ms {
                            None => true,
                            Some(ms) => ms <= now,
                        }
                })
                .map(|t| t.id.clone())
                .take(200)
                .collect()
        }

        fn mark_task_running(&self, task_id: &str, now: u64) -> bool {
            let mut inner = self.inner.lock();
            if let Some(task) = inner.tasks.get_mut(task_id) {
                if task.status == TaskStatus::Pending {
                    task.status = TaskStatus::Running;
                    task.updated_at_ms = now;
                    return true;
                }
            }
            false
        }

        fn update_task_after_run(
            &self,
            task_id: &str,
            status: &str,
            updated_at_ms: u64,
            last_run_ms: u64,
            next_run_ms: Option<u64>,
            last_error: Option<&str>,
            reset_retry_count: bool,
            new_retry_count: Option<u32>,
            completed_at_ms: Option<u64>,
        ) -> Result<usize, SchedulerError> {
            let mut inner = self.inner.lock();
            if let Some(task) = inner.tasks.get_mut(task_id) {
                // Only update if task is still running (cancellation race guard)
                if task.status != TaskStatus::Running {
                    return Ok(0);
                }
                task.status = TaskStatus::from_str_lossy(status);
                task.updated_at_ms = updated_at_ms;
                task.last_run_ms = Some(last_run_ms);
                task.next_run_ms = next_run_ms;
                task.last_error = last_error.map(|s| s.to_string());
                task.run_count += 1;
                task.completed_at_ms = completed_at_ms;
                if let Some(n) = new_retry_count {
                    task.retry_count = n;
                } else if reset_retry_count {
                    task.retry_count = 0;
                }
                Ok(1)
            } else {
                Ok(0)
            }
        }

        fn insert_task_run(
            &self,
            run_id: &str,
            task_id: &str,
            started_at_ms: u64,
            completed_at_ms: u64,
            status: &str,
            error: Option<&str>,
            result_json: Option<&str>,
        ) -> Result<(), SchedulerError> {
            let mut inner = self.inner.lock();
            inner.runs.push(TaskRun {
                id: run_id.to_string(),
                task_id: task_id.to_string(),
                started_at_ms,
                completed_at_ms: Some(completed_at_ms),
                status: TaskRunStatus::from_str_lossy(status),
                error: error.map(|s| s.to_string()),
                result: result_json.and_then(|s| serde_json::from_str::<Value>(s).ok()),
            });
            Ok(())
        }

        fn prune_old_runs(&self, task_id: &str, keep: usize) {
            let mut inner = self.inner.lock();
            let mut task_runs: Vec<TaskRun> =
                inner.runs.iter().filter(|r| r.task_id == task_id).cloned().collect();
            task_runs.sort_by(|a, b| b.started_at_ms.cmp(&a.started_at_ms));

            let to_remove: std::collections::HashSet<String> =
                task_runs.into_iter().skip(keep).map(|r| r.id.clone()).collect();

            inner.runs.retain(|r| r.task_id != task_id || !to_remove.contains(&r.id));
        }

        fn count_active_tasks(&self) -> usize {
            let inner = self.inner.lock();
            inner
                .tasks
                .values()
                .filter(|t| t.status == TaskStatus::Pending || t.status == TaskStatus::Running)
                .count()
        }

        fn count_active_tasks_for_session(&self, session_id: &str) -> usize {
            let inner = self.inner.lock();
            inner
                .tasks
                .values()
                .filter(|t| {
                    (t.status == TaskStatus::Pending || t.status == TaskStatus::Running)
                        && t.owner_session_id.as_deref() == Some(session_id)
                })
                .count()
        }
    }

    #[test]
    fn test_in_memory_scheduler_store_roundtrip() {
        let store = InMemorySchedulerStore::new();

        let task = ScheduledTask {
            id: "task-1".to_string(),
            name: "My Task".to_string(),
            description: "A test task".to_string(),
            schedule: TaskSchedule::Once,
            action: TaskAction::EmitEvent { topic: "test".to_string(), payload: Value::Null },
            status: TaskStatus::Pending,
            schedule_type: "once".to_string(),
            action_type: "emit_event".to_string(),
            created_at_ms: 1000,
            updated_at_ms: 1000,
            completed_at_ms: None,
            last_run_ms: None,
            next_run_ms: Some(2000),
            run_count: 0,
            last_error: None,
            owner_session_id: Some("sess-1".to_string()),
            owner_agent_id: Some("agent-1".to_string()),
            max_retries: None,
            retry_delay_ms: None,
            retry_count: 0,
        };

        let schedule_json = serde_json::to_string(&task.schedule).unwrap();
        let action_json = serde_json::to_string(&task.action).unwrap();

        // Insert
        store.insert_task(&task, &schedule_json, &action_json).unwrap();
        assert!(store.task_exists("task-1"));

        // Get
        let fetched = store.get_task("task-1").unwrap();
        assert_eq!(fetched.id, "task-1");
        assert_eq!(fetched.name, "My Task");

        // List filtered
        let all = store.list_tasks_filtered(&ListTasksFilter::default()).unwrap();
        assert_eq!(all.len(), 1);

        let by_session = store
            .list_tasks_filtered(&ListTasksFilter {
                session_id: Some("sess-1".to_string()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(by_session.len(), 1);

        let by_wrong_session = store
            .list_tasks_filtered(&ListTasksFilter {
                session_id: Some("other".to_string()),
                ..Default::default()
            })
            .unwrap();
        assert!(by_wrong_session.is_empty());

        // Update name
        store.update_task_name("task-1", "Renamed", 2000).unwrap();
        let updated = store.get_task("task-1").unwrap();
        assert_eq!(updated.name, "Renamed");
        assert_eq!(updated.updated_at_ms, 2000);

        // Delete
        let deleted = store.delete_task("task-1").unwrap();
        assert_eq!(deleted, 1);
        assert!(!store.task_exists("task-1"));

        // Not found after delete
        assert!(store.get_task("task-1").is_err());
    }
}
