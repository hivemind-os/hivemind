use parking_lot::Mutex;
use rusqlite::Connection;
use serde_json::Value;

use crate::store::SchedulerStore;
use crate::{
    ListTasksFilter, ScheduledTask, SchedulerError, TaskAction, TaskRun, TaskRunStatus,
    TaskSchedule, TaskStatus,
};

// ---------------------------------------------------------------------------
// Row helper (moved from lib.rs)
// ---------------------------------------------------------------------------

fn row_to_task(row: &rusqlite::Row<'_>) -> ScheduledTask {
    // Column order: external_id, name, description, schedule_json, action_json, status,
    //   schedule_type, action_type, created_at_ms, updated_at_ms, completed_at_ms,
    //   last_run_ms, next_run_ms, run_count, last_error,
    //   owner_session_id, owner_agent_id, max_retries, retry_delay_ms, retry_count
    let schedule_str: String = row.get(3).unwrap_or_default();
    let action_str: String = row.get(4).unwrap_or_default();
    let status_str: String = row.get(5).unwrap_or_default();

    let (schedule, deser_failed) = match serde_json::from_str(&schedule_str) {
        Ok(s) => (s, false),
        Err(e) => {
            tracing::error!(raw = %schedule_str, error = %e, "schedule deserialization failed — marking task as failed");
            (TaskSchedule::Once, true)
        }
    };
    let (action, action_deser_failed) = match serde_json::from_str(&action_str) {
        Ok(a) => (a, false),
        Err(e) => {
            tracing::error!(raw = %action_str, error = %e, "action deserialization failed — marking task as failed");
            (TaskAction::EmitEvent { topic: String::new(), payload: Value::Null }, true)
        }
    };
    let deserialization_failed = deser_failed || action_deser_failed;

    ScheduledTask {
        id: row.get(0).unwrap_or_default(),
        name: row.get(1).unwrap_or_default(),
        description: row.get(2).unwrap_or_default(),
        schedule,
        action,
        status: if deserialization_failed {
            TaskStatus::Failed
        } else {
            TaskStatus::from_str_lossy(&status_str)
        },
        schedule_type: row.get(6).unwrap_or_default(),
        action_type: row.get(7).unwrap_or_default(),
        created_at_ms: row.get(8).unwrap_or(0),
        updated_at_ms: row.get(9).unwrap_or(0),
        completed_at_ms: row.get(10).ok().and_then(|v: Option<u64>| v),
        last_run_ms: row.get(11).ok().and_then(|v: Option<u64>| v),
        next_run_ms: row.get(12).ok().and_then(|v: Option<u64>| v),
        run_count: row.get(13).unwrap_or(0),
        last_error: if deserialization_failed {
            Some("task data corrupted: schedule or action could not be deserialized".to_string())
        } else {
            row.get(14).ok().and_then(|v: String| if v.is_empty() { None } else { Some(v) })
        },
        owner_session_id: row
            .get(15)
            .ok()
            .and_then(|v: String| if v.is_empty() { None } else { Some(v) }),
        owner_agent_id: row
            .get(16)
            .ok()
            .and_then(|v: String| if v.is_empty() { None } else { Some(v) }),
        max_retries: row.get::<_, Option<u32>>(17).unwrap_or(None),
        retry_delay_ms: row.get::<_, Option<u64>>(18).unwrap_or(None),
        retry_count: row.get(19).unwrap_or(0),
    }
}

// ---------------------------------------------------------------------------
// SqliteSchedulerStore
// ---------------------------------------------------------------------------

/// SQLite-backed implementation of [`SchedulerStore`].
pub struct SqliteSchedulerStore {
    conn: Mutex<Connection>,
}

/// The initial sequence values seeded from the database during construction.
pub struct InitialSequences {
    pub task_seq: u64,
    pub run_seq: u64,
}

impl SqliteSchedulerStore {
    /// Open (or create) a scheduler database at the given path.
    pub fn new(path: std::path::PathBuf) -> anyhow::Result<(Self, InitialSequences)> {
        let conn = Connection::open(&path)
            .map_err(|e| anyhow::anyhow!("open scheduler db at {}: {e}", path.display()))?;
        Self::init(conn)
    }

    /// Create an in-memory store (for tests).
    pub fn in_memory() -> anyhow::Result<(Self, InitialSequences)> {
        let conn = Connection::open_in_memory()?;
        Self::init(conn)
    }

    fn init(conn: Connection) -> anyhow::Result<(Self, InitialSequences)> {
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS tasks (
                id                  INTEGER PRIMARY KEY,
                external_id         TEXT    NOT NULL UNIQUE,
                name                TEXT    NOT NULL,
                description         TEXT    NOT NULL DEFAULT '',
                schedule_json       TEXT    NOT NULL,
                action_json         TEXT    NOT NULL,
                status              TEXT    NOT NULL DEFAULT 'pending',
                schedule_type       TEXT    NOT NULL,
                cron_expression     TEXT,
                run_at_ms           INTEGER,
                action_type         TEXT    NOT NULL,
                created_at_ms       INTEGER NOT NULL,
                updated_at_ms       INTEGER NOT NULL,
                completed_at_ms     INTEGER,
                last_run_ms         INTEGER,
                next_run_ms         INTEGER,
                run_count           INTEGER NOT NULL DEFAULT 0,
                last_error          TEXT,
                owner_session_id    TEXT,
                owner_agent_id      TEXT,
                max_retries         INTEGER,
                retry_delay_ms      INTEGER,
                retry_count         INTEGER NOT NULL DEFAULT 0
            );",
        )?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS task_runs (
                id              INTEGER PRIMARY KEY,
                external_id     TEXT    NOT NULL UNIQUE,
                task_id         INTEGER NOT NULL,
                started_at_ms   INTEGER NOT NULL,
                completed_at_ms INTEGER,
                status          TEXT    NOT NULL DEFAULT 'running',
                error           TEXT,
                result          TEXT,
                FOREIGN KEY (task_id) REFERENCES tasks(id) ON DELETE CASCADE
            );",
        )?;
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_tasks_status_next
                ON tasks(status, next_run_ms);
            CREATE INDEX IF NOT EXISTS idx_tasks_schedule_type
                ON tasks(schedule_type);
            CREATE INDEX IF NOT EXISTS idx_tasks_action_type
                ON tasks(action_type);
            CREATE INDEX IF NOT EXISTS idx_tasks_owner_session
                ON tasks(owner_session_id);
            CREATE INDEX IF NOT EXISTS idx_tasks_owner_agent
                ON tasks(owner_agent_id);
            CREATE INDEX IF NOT EXISTS idx_task_runs_task_started
                ON task_runs(task_id, started_at_ms DESC);",
        )?;
        // Recover stale "running" tasks that were interrupted by a crash.
        let recovered = conn
            .execute("UPDATE tasks SET status = 'pending' WHERE status = 'running'", [])
            .unwrap_or(0);
        if recovered > 0 {
            tracing::warn!(
                count = recovered,
                "recovered stale running tasks to pending after restart"
            );
        }

        // Seed task_seq from existing external IDs to avoid collisions after restart.
        let max_task_seq: u64 = conn
            .query_row(
                "SELECT COALESCE(MAX(CAST(REPLACE(external_id, 'task-', '') AS INTEGER)), 0) FROM tasks WHERE external_id LIKE 'task-%'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        // Seed run_seq from existing run external IDs (trailing numeric portion).
        let max_run_seq: u64 = conn
            .query_row(
                "SELECT COALESCE(MAX(CAST(SUBSTR(external_id, LENGTH(RTRIM(external_id, '0123456789')) + 1) AS INTEGER)), 0) FROM task_runs",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        let seqs = InitialSequences { task_seq: max_task_seq + 1, run_seq: max_run_seq + 1 };

        Ok((Self { conn: Mutex::new(conn) }, seqs))
    }
}

// ---------------------------------------------------------------------------
// Trait implementation
// ---------------------------------------------------------------------------

/// Standard SELECT column list for the `tasks` table, matching `row_to_task()` expectations.
const TASK_COLUMNS: &str = "external_id, name, description, schedule_json, action_json, status, \
    schedule_type, action_type, created_at_ms, updated_at_ms, completed_at_ms, \
    last_run_ms, next_run_ms, run_count, last_error, \
    owner_session_id, owner_agent_id, max_retries, retry_delay_ms, retry_count";

impl SchedulerStore for SqliteSchedulerStore {
    fn insert_task(
        &self,
        task: &ScheduledTask,
        schedule_json: &str,
        action_json: &str,
    ) -> Result<(), SchedulerError> {
        let db = self.conn.lock();
        db.execute(
            "INSERT INTO tasks (external_id, name, description, schedule_json, action_json, status, \
                schedule_type, cron_expression, run_at_ms, action_type, \
                created_at_ms, updated_at_ms, next_run_ms, \
                owner_session_id, owner_agent_id, max_retries, retry_delay_ms, retry_count)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
            rusqlite::params![
                task.id,
                task.name,
                task.description,
                schedule_json,
                action_json,
                task.status.as_str(),
                task.schedule_type,
                task.schedule.cron_expression(),
                task.schedule.run_at_ms(),
                task.action_type,
                task.created_at_ms,
                task.updated_at_ms,
                task.next_run_ms,
                task.owner_session_id,
                task.owner_agent_id,
                task.max_retries,
                task.retry_delay_ms,
                task.retry_count,
            ],
        )
        .map_err(|e| SchedulerError::Database(e.to_string()))?;
        Ok(())
    }

    fn get_task(&self, task_id: &str) -> Result<ScheduledTask, SchedulerError> {
        let db = self.conn.lock();
        let sql = format!("SELECT {TASK_COLUMNS} FROM tasks WHERE external_id = ?1");
        let mut stmt = db.prepare(&sql).map_err(|e| SchedulerError::Database(e.to_string()))?;
        stmt.query_row([task_id], |row| Ok(row_to_task(row))).map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                SchedulerError::TaskNotFound { id: task_id.to_string() }
            }
            other => SchedulerError::Database(other.to_string()),
        })
    }

    fn list_tasks_filtered(
        &self,
        filter: &ListTasksFilter,
    ) -> Result<Vec<ScheduledTask>, SchedulerError> {
        let db = self.conn.lock();
        let mut sql = format!("SELECT {TASK_COLUMNS} FROM tasks");
        let mut conditions = Vec::new();
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(ref sid) = filter.session_id {
            conditions.push(format!("owner_session_id = ?{}", params.len() + 1));
            params.push(Box::new(sid.clone()));
        }
        if let Some(ref aid) = filter.agent_id {
            conditions.push(format!("owner_agent_id = ?{}", params.len() + 1));
            params.push(Box::new(aid.clone()));
        }
        if let Some(ref status) = filter.status {
            conditions.push(format!("status = ?{}", params.len() + 1));
            params.push(Box::new(status.clone()));
        }
        if let Some(ref st) = filter.schedule_type {
            conditions.push(format!("schedule_type = ?{}", params.len() + 1));
            params.push(Box::new(st.clone()));
        }
        if let Some(ref at) = filter.action_type {
            conditions.push(format!("action_type = ?{}", params.len() + 1));
            params.push(Box::new(at.clone()));
        }
        if !conditions.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&conditions.join(" AND "));
        }
        sql.push_str(" ORDER BY created_at_ms DESC");

        let mut stmt = db.prepare(&sql).map_err(|e| SchedulerError::Database(e.to_string()))?;

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        stmt.query_map(param_refs.as_slice(), |row| Ok(row_to_task(row)))
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .map_err(|e| SchedulerError::Database(e.to_string()))
    }

    fn list_tasks_for_workflow(
        &self,
        definition: &str,
    ) -> Result<Vec<ScheduledTask>, SchedulerError> {
        let db = self.conn.lock();
        let sql = format!(
            "SELECT {TASK_COLUMNS} FROM tasks \
             WHERE action_type = 'launch_workflow' \
               AND json_extract(action_json, '$.definition') = ?1"
        );
        let mut stmt = db.prepare(&sql).map_err(|e| SchedulerError::Database(e.to_string()))?;
        stmt.query_map([definition], |row| Ok(row_to_task(row)))
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .map_err(|e| SchedulerError::Database(e.to_string()))
    }

    fn cancel_task(&self, task_id: &str, now: u64) -> Result<usize, SchedulerError> {
        let db = self.conn.lock();
        let affected = db.execute(
            "UPDATE tasks SET status = 'cancelled', updated_at_ms = ?1, completed_at_ms = ?1 WHERE external_id = ?2 AND status IN ('pending', 'running')",
            rusqlite::params![now, task_id],
        )
        .map_err(|e| SchedulerError::Database(e.to_string()))?;
        Ok(affected)
    }

    fn task_exists(&self, task_id: &str) -> bool {
        let db = self.conn.lock();
        db.query_row("SELECT COUNT(*) FROM tasks WHERE external_id = ?1", [task_id], |r| {
            r.get::<_, i64>(0)
        })
        .map(|c| c > 0)
        .unwrap_or(false)
    }

    fn delete_task(&self, task_id: &str) -> Result<usize, SchedulerError> {
        let db = self.conn.lock();
        let affected = db
            .execute("DELETE FROM tasks WHERE external_id = ?1", [task_id])
            .map_err(|e| SchedulerError::Database(e.to_string()))?;
        Ok(affected)
    }

    fn update_task_name(&self, task_id: &str, name: &str, now: u64) -> Result<(), SchedulerError> {
        let db = self.conn.lock();
        let affected = db
            .execute(
                "UPDATE tasks SET name = ?1, updated_at_ms = ?2 WHERE external_id = ?3",
                rusqlite::params![name, now, task_id],
            )
            .map_err(|e| SchedulerError::Database(e.to_string()))?;
        if affected == 0 {
            return Err(SchedulerError::TaskNotFound { id: task_id.to_string() });
        }
        Ok(())
    }

    fn update_task_description(
        &self,
        task_id: &str,
        description: &str,
        now: u64,
    ) -> Result<(), SchedulerError> {
        let db = self.conn.lock();
        let affected = db
            .execute(
                "UPDATE tasks SET description = ?1, updated_at_ms = ?2 WHERE external_id = ?3",
                rusqlite::params![description, now, task_id],
            )
            .map_err(|e| SchedulerError::Database(e.to_string()))?;
        if affected == 0 {
            return Err(SchedulerError::TaskNotFound { id: task_id.to_string() });
        }
        Ok(())
    }

    fn update_task_schedule(
        &self,
        task_id: &str,
        schedule_json: &str,
        schedule_type: &str,
        cron_expression: Option<&str>,
        run_at_ms: Option<u64>,
        next_run_ms: Option<u64>,
        now: u64,
    ) -> Result<(), SchedulerError> {
        let db = self.conn.lock();
        let affected = db.execute(
            "UPDATE tasks SET schedule_json = ?1, schedule_type = ?2, cron_expression = ?3, run_at_ms = ?4, next_run_ms = ?5, updated_at_ms = ?6 WHERE external_id = ?7",
            rusqlite::params![schedule_json, schedule_type, cron_expression, run_at_ms, next_run_ms, now, task_id],
        )
        .map_err(|e| SchedulerError::Database(e.to_string()))?;
        if affected == 0 {
            return Err(SchedulerError::TaskNotFound { id: task_id.to_string() });
        }
        Ok(())
    }

    fn update_task_action(
        &self,
        task_id: &str,
        action_json: &str,
        action_type: &str,
        now: u64,
    ) -> Result<(), SchedulerError> {
        let db = self.conn.lock();
        let affected = db.execute(
            "UPDATE tasks SET action_json = ?1, action_type = ?2, updated_at_ms = ?3 WHERE external_id = ?4",
            rusqlite::params![action_json, action_type, now, task_id],
        )
        .map_err(|e| SchedulerError::Database(e.to_string()))?;
        if affected == 0 {
            return Err(SchedulerError::TaskNotFound { id: task_id.to_string() });
        }
        Ok(())
    }

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
    ) -> Result<(), SchedulerError> {
        let db = self.conn.lock();
        let mut sets = vec!["updated_at_ms = ?1".to_string()];
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(now)];

        if let Some(n) = name {
            params.push(Box::new(n.to_string()));
            sets.push(format!("name = ?{}", params.len()));
        }
        if let Some(d) = description {
            params.push(Box::new(d.to_string()));
            sets.push(format!("description = ?{}", params.len()));
        }
        if let Some(s) = schedule_json {
            params.push(Box::new(s.to_string()));
            sets.push(format!("schedule_json = ?{}", params.len()));
        }
        if let Some(st) = schedule_type {
            params.push(Box::new(st.to_string()));
            sets.push(format!("schedule_type = ?{}", params.len()));
        }
        if let Some(ce) = cron_expression {
            params.push(Box::new(ce.map(|s| s.to_string())));
            sets.push(format!("cron_expression = ?{}", params.len()));
        }
        if let Some(ra) = run_at_ms {
            params.push(Box::new(ra));
            sets.push(format!("run_at_ms = ?{}", params.len()));
        }
        if let Some(next) = next_run_ms {
            params.push(Box::new(next));
            sets.push(format!("next_run_ms = ?{}", params.len()));
        }
        if let Some(a) = action_json {
            params.push(Box::new(a.to_string()));
            sets.push(format!("action_json = ?{}", params.len()));
        }
        if let Some(at) = action_type {
            params.push(Box::new(at.to_string()));
            sets.push(format!("action_type = ?{}", params.len()));
        }
        if let Some(mr) = max_retries {
            params.push(Box::new(mr));
            sets.push(format!("max_retries = ?{}", params.len()));
        }
        if let Some(rd) = retry_delay_ms {
            params.push(Box::new(rd));
            sets.push(format!("retry_delay_ms = ?{}", params.len()));
        }

        params.push(Box::new(task_id.to_string()));
        let sql =
            format!("UPDATE tasks SET {} WHERE external_id = ?{}", sets.join(", "), params.len());

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        db.execute(&sql, param_refs.as_slice())
            .map_err(|e| SchedulerError::Database(e.to_string()))?;
        Ok(())
    }

    fn reset_task_to_pending(&self, task_id: &str, now: u64) -> Result<(), SchedulerError> {
        let db = self.conn.lock();
        let affected = db.execute(
            "UPDATE tasks SET status = 'pending', retry_count = 0, last_error = NULL, completed_at_ms = NULL, updated_at_ms = ?1 WHERE external_id = ?2",
            rusqlite::params![now, task_id],
        )
        .map_err(|e| SchedulerError::Database(e.to_string()))?;
        if affected == 0 {
            return Err(SchedulerError::TaskNotFound { id: task_id.to_string() });
        }
        Ok(())
    }

    fn list_task_runs(&self, task_id: &str, limit: usize) -> Result<Vec<TaskRun>, SchedulerError> {
        let db = self.conn.lock();
        let internal_id: i64 = db
            .query_row("SELECT id FROM tasks WHERE external_id = ?1", [task_id], |r| r.get(0))
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    SchedulerError::TaskNotFound { id: task_id.to_string() }
                }
                other => SchedulerError::Database(other.to_string()),
            })?;
        let mut stmt = db
            .prepare(
                "SELECT external_id, ?1, started_at_ms, completed_at_ms, status, error, result
                 FROM task_runs WHERE task_id = ?2 ORDER BY started_at_ms DESC LIMIT ?3",
            )
            .map_err(|e| SchedulerError::Database(e.to_string()))?;

        let runs = stmt
            .query_map(rusqlite::params![task_id, internal_id, limit], |row| {
                let status_str: String = row.get(4).unwrap_or_default();
                Ok(TaskRun {
                    id: row.get(0).unwrap_or_default(),
                    task_id: row.get(1).unwrap_or_default(),
                    started_at_ms: row.get(2).unwrap_or(0),
                    completed_at_ms: row.get(3).ok(),
                    status: TaskRunStatus::from_str_lossy(&status_str),
                    error: row
                        .get(5)
                        .ok()
                        .and_then(|v: String| if v.is_empty() { None } else { Some(v) }),
                    result: row.get::<_, String>(6).ok().and_then(|v| {
                        if v.is_empty() {
                            None
                        } else {
                            serde_json::from_str(&v).ok()
                        }
                    }),
                })
            })
            .map_err(|e| SchedulerError::Database(e.to_string()))?
            .filter_map(|r| r.ok())
            .collect();

        Ok(runs)
    }

    #[cfg(any(test, feature = "test-support"))]
    fn force_all_due(&self) {
        let db = self.conn.lock();
        let _ = db.execute("UPDATE tasks SET next_run_ms = 0 WHERE status = 'pending'", []);
    }

    fn get_due_task_ids(&self, now: u64) -> Vec<String> {
        let db = self.conn.lock();
        let mut stmt = match db.prepare(
            "SELECT external_id FROM tasks WHERE status = 'pending' AND (next_run_ms IS NULL OR next_run_ms <= ?1) LIMIT 200",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        stmt.query_map([now], |row| row.get::<_, String>(0))
            .map(|rows| rows.filter_map(|r| r.ok()).collect::<Vec<_>>())
            .unwrap_or_default()
    }

    fn mark_task_running(&self, task_id: &str, now: u64) -> bool {
        let db = self.conn.lock();
        let updated = db.execute(
            "UPDATE tasks SET status = 'running', updated_at_ms = ?1 WHERE external_id = ?2 AND status = 'pending'",
            rusqlite::params![now, task_id],
        ).unwrap_or(0);
        updated > 0
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
        let db = self.conn.lock();
        let affected = if let Some(retry_count) = new_retry_count {
            db.execute(
                "UPDATE tasks SET status = ?1, updated_at_ms = ?2, last_run_ms = ?3, next_run_ms = ?4, run_count = run_count + 1, last_error = ?5, retry_count = ?6, completed_at_ms = ?7 WHERE external_id = ?8 AND status = 'running'",
                rusqlite::params![status, updated_at_ms, last_run_ms, next_run_ms, last_error, retry_count, completed_at_ms, task_id],
            )
        } else if reset_retry_count {
            db.execute(
                "UPDATE tasks SET status = ?1, updated_at_ms = ?2, last_run_ms = ?3, next_run_ms = ?4, run_count = run_count + 1, last_error = ?5, retry_count = 0, completed_at_ms = ?6 WHERE external_id = ?7 AND status = 'running'",
                rusqlite::params![status, updated_at_ms, last_run_ms, next_run_ms, last_error, completed_at_ms, task_id],
            )
        } else {
            db.execute(
                "UPDATE tasks SET status = ?1, updated_at_ms = ?2, last_run_ms = ?3, next_run_ms = ?4, run_count = run_count + 1, last_error = ?5, completed_at_ms = ?6 WHERE external_id = ?7 AND status = 'running'",
                rusqlite::params![status, updated_at_ms, last_run_ms, next_run_ms, last_error, completed_at_ms, task_id],
            )
        };
        affected.map_err(|e| SchedulerError::Database(e.to_string()))
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
        let db = self.conn.lock();
        // Resolve internal integer ID from external_id
        let internal_id: i64 = db
            .query_row("SELECT id FROM tasks WHERE external_id = ?1", [task_id], |r| r.get(0))
            .map_err(|e| SchedulerError::Database(format!("resolve task internal id: {e}")))?;
        db.execute(
            "INSERT INTO task_runs (external_id, task_id, started_at_ms, completed_at_ms, status, error, result) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![run_id, internal_id, started_at_ms, completed_at_ms, status, error, result_json],
        ).map_err(|e| SchedulerError::Database(e.to_string()))?;
        Ok(())
    }

    fn prune_old_runs(&self, task_id: &str, keep: usize) {
        let db = self.conn.lock();
        // Resolve internal integer ID from external_id
        let internal_id: i64 =
            match db
                .query_row("SELECT id FROM tasks WHERE external_id = ?1", [task_id], |r| r.get(0))
            {
                Ok(id) => id,
                Err(_) => return,
            };
        let _ = db.execute(
            "DELETE FROM task_runs WHERE task_id = ?1 AND id NOT IN (SELECT id FROM task_runs WHERE task_id = ?1 ORDER BY started_at_ms DESC LIMIT ?2)",
            rusqlite::params![internal_id, keep],
        );
    }

    fn count_active_tasks(&self) -> usize {
        let db = self.conn.lock();
        db.query_row("SELECT COUNT(*) FROM tasks WHERE status IN ('pending', 'running')", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap_or(0) as usize
    }

    fn count_active_tasks_for_session(&self, session_id: &str) -> usize {
        let db = self.conn.lock();
        db.query_row(
            "SELECT COUNT(*) FROM tasks WHERE status IN ('pending', 'running') AND owner_session_id = ?1",
            [session_id],
            |r| r.get::<_, i64>(0),
        ).unwrap_or(0) as usize
    }
}
