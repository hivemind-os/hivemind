use crate::error::WorkflowError;
use crate::types::*;
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

// ---------------------------------------------------------------------------
// WorkflowPersistence trait
// ---------------------------------------------------------------------------

/// Abstraction over the persistence layer for workflow definitions and instances.
///
/// All methods are synchronous -- callers use `spawn_blocking` when calling from
/// async contexts.
pub trait WorkflowPersistence: Send + Sync {
    // -- Definition CRUD --

    /// Save a workflow definition (insert or replace).
    fn save_definition(
        &self,
        yaml_source: &str,
        def: &WorkflowDefinition,
    ) -> Result<(), WorkflowError>;

    /// Extended save that also persists bundled/archived metadata and an
    /// optional factory YAML hash for auto-update tracking.
    fn save_definition_ext(
        &self,
        yaml_source: &str,
        def: &WorkflowDefinition,
        factory_yaml_hash: Option<&str>,
    ) -> Result<(), WorkflowError>;

    /// Get a specific definition by name and version.
    fn get_definition(
        &self,
        name: &str,
        version: &str,
    ) -> Result<Option<(WorkflowDefinition, String)>, WorkflowError>;

    /// Get a specific definition by its immutable ID.
    fn get_definition_by_id(
        &self,
        id: &str,
    ) -> Result<Option<(WorkflowDefinition, String)>, WorkflowError>;

    /// Get the latest version of a definition by name.
    fn get_latest_definition(
        &self,
        name: &str,
    ) -> Result<Option<(WorkflowDefinition, String)>, WorkflowError>;

    /// List all definitions (summary only).
    fn list_definitions(&self) -> Result<Vec<WorkflowDefinitionSummary>, WorkflowError>;

    /// Delete a definition.
    fn delete_definition(&self, name: &str, version: &str) -> Result<bool, WorkflowError>;

    /// Get the factory YAML hash and stored YAML for a definition.
    fn get_definition_meta(
        &self,
        name: &str,
        version: &str,
    ) -> Result<Option<(String, Option<String>, bool)>, WorkflowError>;

    /// Set the `archived` flag on a workflow definition.
    fn set_archived(
        &self,
        name: &str,
        version: &str,
        archived: bool,
    ) -> Result<bool, WorkflowError>;

    /// Set the `triggers_paused` flag on a workflow definition.
    fn set_triggers_paused(
        &self,
        name: &str,
        version: &str,
        paused: bool,
    ) -> Result<bool, WorkflowError>;

    /// Check whether any version of the given workflow name is marked as bundled.
    fn is_bundled(&self, name: &str) -> Result<bool, WorkflowError>;

    // -- Instance CRUD --

    /// Create a new workflow instance from a definition snapshot.
    /// Returns the auto-assigned integer row-id.
    fn create_instance(&self, instance: &WorkflowInstance) -> Result<i64, WorkflowError>;

    /// Update an existing instance's mutable fields.
    fn update_instance(&self, instance: &WorkflowInstance) -> Result<(), WorkflowError>;

    /// Load a full workflow instance including step states.
    fn get_instance(&self, id: i64) -> Result<Option<WorkflowInstance>, WorkflowError>;

    /// List instances with optional filtering.
    fn list_instances(&self, filter: &InstanceFilter) -> Result<InstanceListResult, WorkflowError>;

    /// Delete an instance and its step states.
    fn delete_instance(&self, id: i64) -> Result<bool, WorkflowError>;

    /// Set the `archived` flag on a workflow instance.
    fn set_instance_archived(&self, id: i64, archived: bool) -> Result<bool, WorkflowError>;

    /// List steps waiting on feedback for instances owned by a given session.
    /// Returns tuples of (instance_id, step_id, definition_snapshot_json,
    /// interaction_prompt, interaction_choices_json, interaction_allow_freeform).
    #[allow(clippy::type_complexity)]
    fn list_waiting_feedback_for_session(
        &self,
        session_id: &str,
    ) -> Result<
        Vec<(i64, String, String, Option<String>, Option<String>, Option<bool>)>,
        WorkflowError,
    >;

    /// List ALL steps waiting on feedback across all instances.
    /// Returns tuples of (instance_id, step_id, definition_snapshot_json,
    /// interaction_prompt, interaction_choices_json, interaction_allow_freeform,
    /// parent_session_id).
    #[allow(clippy::type_complexity)]
    fn list_all_waiting_feedback(
        &self,
    ) -> Result<
        Vec<(i64, String, String, Option<String>, Option<String>, Option<bool>, String)>,
        WorkflowError,
    >;

    /// Return all child_agent_id values grouped by instance_id for active
    /// (running / waiting) workflow instances.
    fn list_child_agent_ids(
        &self,
    ) -> Result<std::collections::HashMap<i64, Vec<String>>, WorkflowError>;

    /// Set the child_agent_id on a specific step state row.
    fn set_child_agent_id(
        &self,
        instance_id: i64,
        step_id: &str,
        agent_id: &str,
    ) -> Result<(), WorkflowError>;

    // -- Trigger deduplication --

    /// Returns `true` if this (definition_name, external_id) pair was already recorded.
    fn is_trigger_seen(
        &self,
        definition_id: &str,
        external_id: &str,
    ) -> Result<bool, WorkflowError>;

    /// Record that a trigger was fired for this (definition_id, external_id).
    fn mark_trigger_seen(
        &self,
        definition_id: &str,
        external_id: &str,
    ) -> Result<(), WorkflowError>;

    /// Prune dedup entries older than `max_age_ms`.
    fn prune_trigger_dedup(&self, max_age_ms: u64) -> Result<usize, WorkflowError>;

    // -- Cron state --

    /// Get the last run time (ms since epoch) for a cron trigger.
    fn get_cron_last_run(
        &self,
        definition_id: &str,
        definition_version: &str,
        cron_expression: &str,
    ) -> Result<Option<u64>, WorkflowError>;

    /// Record the last run time for a cron trigger.
    fn set_cron_last_run(
        &self,
        definition_id: &str,
        definition_version: &str,
        cron_expression: &str,
        last_run_ms: u64,
    ) -> Result<(), WorkflowError>;

    /// Remove cron state for a definition (or specific version) when unregistering triggers.
    fn delete_cron_state(
        &self,
        definition_id: &str,
        definition_version: Option<&str>,
    ) -> Result<(), WorkflowError>;

    /// Get the replay cursor timestamp used by TriggerManager event replay.
    fn get_event_replay_cursor(&self) -> Result<Option<u64>, WorkflowError>;

    /// Advance/persist the replay cursor timestamp used by TriggerManager event replay.
    fn set_event_replay_cursor(&self, timestamp_ms: u64) -> Result<(), WorkflowError>;

    // -- Intercepted actions (shadow mode) --

    /// Store an intercepted action from a shadow-mode workflow run.
    /// Returns the auto-assigned row ID.
    fn save_intercepted_action(&self, action: &InterceptedAction) -> Result<i64, WorkflowError>;

    /// List intercepted actions for an instance with pagination.
    fn list_intercepted_actions(
        &self,
        instance_id: i64,
        limit: usize,
        offset: usize,
    ) -> Result<InterceptedActionPage, WorkflowError>;

    /// Compute a shadow summary by counting intercepted actions by kind.
    fn get_shadow_summary(&self, instance_id: i64) -> Result<ShadowSummary, WorkflowError>;

    // -- Definition run tracking --

    /// Record that a normal-mode run of a definition completed successfully.
    /// `definition_hash` is a SHA-256 of the definition JSON that was executed.
    fn record_successful_run(
        &self,
        name: &str,
        version: &str,
        definition_hash: &str,
        run_at_ms: u64,
    ) -> Result<(), WorkflowError>;

    // -- Maintenance --

    /// Prune completed/failed/killed instances older than `max_age_ms`.
    fn prune_completed_instances(&self, max_age_ms: u64) -> Result<usize, WorkflowError>;
}

/// Backward-compatible type alias.
pub type WorkflowStore = SqliteWorkflowStore;

/// Compute SHA-256 hex digest of a string.
pub fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn sql_push_param(
    where_clause: &mut String,
    params_vec: &mut Vec<Box<dyn rusqlite::types::ToSql>>,
    clause: &str,
    value: String,
) {
    params_vec.push(Box::new(value));
    where_clause.push_str(&format!("{} = ?{}", clause, params_vec.len()));
}

// ---------------------------------------------------------------------------
// Simple connection pool
// ---------------------------------------------------------------------------

struct ConnectionPool {
    path: PathBuf,
    pool: Mutex<Vec<Connection>>,
    max_size: usize,
}

impl ConnectionPool {
    fn new(path: PathBuf, max_size: usize) -> Self {
        Self { path, pool: Mutex::new(Vec::with_capacity(max_size)), max_size }
    }

    fn get(&self) -> Result<PooledConnection<'_>, WorkflowError> {
        let mut pool = self.pool.lock().unwrap();
        if let Some(conn) = pool.pop() {
            Ok(PooledConnection { conn: Some(conn), pool: self })
        } else {
            let conn = self.open_new()?;
            Ok(PooledConnection { conn: Some(conn), pool: self })
        }
    }

    fn return_conn(&self, conn: Connection) {
        let mut pool = self.pool.lock().unwrap();
        if pool.len() < self.max_size {
            pool.push(conn);
        }
        // else: drop the excess connection
    }

    fn open_new(&self) -> Result<Connection, WorkflowError> {
        let conn = Connection::open(&self.path)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;
             PRAGMA synchronous = NORMAL;",
        )?;
        // Register a deterministic scalar function that converts a dot-separated
        // version string (e.g. "1.10.3") into a single sortable i64 weight.
        // Supports up to 4 numeric segments, each 0–9999.
        conn.create_scalar_function(
            "version_weight",
            1,
            rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC
                | rusqlite::functions::FunctionFlags::SQLITE_UTF8,
            |ctx| {
                let version: String = ctx.get(0)?;
                let mut weight: i64 = 0;
                let multipliers = [1_000_000_000_000i64, 100_000_000, 10_000, 1];
                for (i, part) in version.split('.').take(4).enumerate() {
                    if let Ok(n) = part.parse::<i64>() {
                        weight += n * multipliers[i];
                    }
                }
                Ok(weight)
            },
        )?;
        Ok(conn)
    }
}

struct PooledConnection<'a> {
    conn: Option<Connection>,
    pool: &'a ConnectionPool,
}

impl<'a> std::ops::Deref for PooledConnection<'a> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        self.conn.as_ref().unwrap()
    }
}

impl<'a> std::ops::DerefMut for PooledConnection<'a> {
    fn deref_mut(&mut self) -> &mut Connection {
        self.conn.as_mut().unwrap()
    }
}

impl<'a> Drop for PooledConnection<'a> {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            self.pool.return_conn(conn);
        }
    }
}

// ---------------------------------------------------------------------------
// SqliteWorkflowStore
// ---------------------------------------------------------------------------

const POOL_SIZE: usize = 4;

/// SQLite-backed persistence for workflow definitions and instances.
///
/// Uses a connection-pool model: a small set of connections is reused across
/// operations. PRAGMAs are set once per connection. With WAL journal mode,
/// multiple readers can run concurrently.
pub struct SqliteWorkflowStore {
    pool: ConnectionPool,
}

impl SqliteWorkflowStore {
    /// Open (or create) a store at the given path.
    pub fn new(path: impl AsRef<Path>) -> Result<Self, WorkflowError> {
        let store = Self { pool: ConnectionPool::new(path.as_ref().to_path_buf(), POOL_SIZE) };
        {
            let conn = store.conn()?;
            Self::init_tables(&conn)?;
        }
        Ok(store)
    }

    /// Create a test store backed by a temp file.
    pub fn in_memory() -> Result<Self, WorkflowError> {
        let id = uuid::Uuid::new_v4().to_string().replace('-', "");
        let path = std::env::temp_dir().join(format!("hive_wf_test_{id}.db"));
        let store = Self { pool: ConnectionPool::new(path, POOL_SIZE) };
        {
            let conn = store.conn()?;
            Self::init_tables(&conn)?;
        }
        Ok(store)
    }

    /// Obtain a connection from the pool.
    fn conn(&self) -> Result<PooledConnection<'_>, WorkflowError> {
        self.pool.get()
    }

    fn init_tables(conn: &Connection) -> Result<(), WorkflowError> {
        conn.execute_batch(
            "
            -- Schema version tracking
            CREATE TABLE IF NOT EXISTS schema_version (
                version INTEGER NOT NULL
            );

            -- Workflow definitions: one row per workflow name (identity)
            CREATE TABLE IF NOT EXISTS workflow_definitions (
                id          INTEGER PRIMARY KEY,
                external_id TEXT    NOT NULL UNIQUE,
                name        TEXT    NOT NULL UNIQUE,
                bundled     INTEGER NOT NULL DEFAULT 0,
                created_at_ms INTEGER NOT NULL
            );

            -- Workflow definition versions: per-version data
            CREATE TABLE IF NOT EXISTS workflow_definition_versions (
                id               INTEGER PRIMARY KEY,
                definition_id    INTEGER NOT NULL REFERENCES workflow_definitions(id) ON DELETE CASCADE,
                version          TEXT    NOT NULL,
                description      TEXT,
                definition_yaml  TEXT    NOT NULL,
                definition_json  TEXT    NOT NULL,
                archived         INTEGER NOT NULL DEFAULT 0,
                triggers_paused  INTEGER NOT NULL DEFAULT 0,
                factory_yaml_hash TEXT,
                created_at_ms    INTEGER NOT NULL,
                updated_at_ms    INTEGER NOT NULL,
                UNIQUE (definition_id, version)
            );

            CREATE INDEX IF NOT EXISTS idx_def_versions_definition
                ON workflow_definition_versions(definition_id);

            -- Workflow instances
            CREATE TABLE IF NOT EXISTS workflow_instances (
                id                      INTEGER PRIMARY KEY,
                definition_name         TEXT    NOT NULL,
                definition_version      TEXT    NOT NULL,
                definition_snapshot     TEXT    NOT NULL,
                mode                    TEXT    NOT NULL DEFAULT 'background',
                status                  TEXT    NOT NULL DEFAULT 'pending'
                    CHECK (status IN ('pending','running','paused','waiting_on_input','waiting_on_event','completed','failed','killed')),
                variables               TEXT    NOT NULL DEFAULT '{}',
                parent_session_id       TEXT    NOT NULL,
                parent_agent_id         TEXT,
                trigger_step_id         TEXT,
                permissions             TEXT    NOT NULL DEFAULT '[]',
                workspace_path          TEXT,
                created_at_ms           INTEGER NOT NULL,
                updated_at_ms           INTEGER NOT NULL,
                completed_at_ms         INTEGER,
                output                  TEXT,
                error                   TEXT,
                resolved_result_message TEXT,
                active_loops            TEXT,
                goto_activated_steps    TEXT    NOT NULL DEFAULT '[]',
                goto_source_steps       TEXT    NOT NULL DEFAULT '[]',
                archived                INTEGER NOT NULL DEFAULT 0
            );

            CREATE INDEX IF NOT EXISTS idx_instances_parent
                ON workflow_instances(parent_session_id);
            CREATE INDEX IF NOT EXISTS idx_instances_definition
                ON workflow_instances(definition_name);
            CREATE INDEX IF NOT EXISTS idx_instances_mode
                ON workflow_instances(mode);

            -- Step states
            CREATE TABLE IF NOT EXISTS workflow_step_states (
                id                         INTEGER PRIMARY KEY,
                instance_id                INTEGER NOT NULL REFERENCES workflow_instances(id) ON DELETE CASCADE,
                step_id                    TEXT    NOT NULL,
                status                     TEXT    NOT NULL DEFAULT 'pending'
                    CHECK (status IN ('pending','running','completed','failed','skipped','waiting_on_input','waiting_on_event','waiting_for_delay','loop_waiting')),
                started_at_ms              INTEGER,
                completed_at_ms            INTEGER,
                outputs                    TEXT,
                error                      TEXT,
                retry_count                INTEGER NOT NULL DEFAULT 0,
                retry_delay_secs           INTEGER,
                resume_at_ms               INTEGER,
                child_workflow_id          INTEGER,
                child_agent_id             TEXT,
                interaction_request_id     TEXT,
                interaction_prompt         TEXT,
                interaction_choices        TEXT,
                interaction_allow_freeform INTEGER,
                UNIQUE (instance_id, step_id)
            );

            CREATE INDEX IF NOT EXISTS idx_step_states_instance_status
                ON workflow_step_states(instance_id, status);
            CREATE INDEX IF NOT EXISTS idx_step_states_child_agent
                ON workflow_step_states(child_agent_id)
                WHERE child_agent_id IS NOT NULL;

            -- Trigger deduplication (v2: keyed by immutable workflow definition_id)
            CREATE TABLE IF NOT EXISTS trigger_dedup_v2 (
                definition_id TEXT    NOT NULL,
                external_id   TEXT    NOT NULL,
                created_at_ms INTEGER NOT NULL,
                PRIMARY KEY (definition_id, external_id)
            );

            CREATE INDEX IF NOT EXISTS idx_trigger_dedup_v2_created
                ON trigger_dedup_v2(created_at_ms);

            -- Keep legacy table for backward compatibility/migration.
            CREATE TABLE IF NOT EXISTS trigger_dedup (
                definition_name TEXT    NOT NULL,
                external_id     TEXT    NOT NULL,
                created_at_ms   INTEGER NOT NULL,
                PRIMARY KEY (definition_name, external_id)
            );

            -- Cron trigger state (v2): keyed by definition_id + definition_version.
            CREATE TABLE IF NOT EXISTS cron_state_v2 (
                definition_id      TEXT NOT NULL,
                definition_version TEXT NOT NULL,
                cron_expression    TEXT NOT NULL,
                last_run_ms        INTEGER NOT NULL,
                PRIMARY KEY (definition_id, definition_version, cron_expression)
            );

            -- Keep legacy table for backward compatibility/migration.
            CREATE TABLE IF NOT EXISTS cron_state (
                definition_name TEXT NOT NULL,
                cron_expression TEXT NOT NULL,
                last_run_ms     INTEGER NOT NULL,
                PRIMARY KEY (definition_name, cron_expression)
            );

            -- Runtime service cursors/state.
            CREATE TABLE IF NOT EXISTS workflow_runtime_state (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            ",
        )?;

        // Backfill dedup v2 from legacy name-keyed rows.
        conn.execute(
            "INSERT OR IGNORE INTO trigger_dedup_v2 (definition_id, external_id, created_at_ms)
             SELECT d.external_id, t.external_id, t.created_at_ms
             FROM trigger_dedup t
             JOIN workflow_definitions d ON d.name = t.definition_name",
            [],
        )?;

        // Backfill cron v2 from legacy name-keyed rows for each known version.
        conn.execute(
            "INSERT OR IGNORE INTO cron_state_v2 (definition_id, definition_version, cron_expression, last_run_ms)
             SELECT d.external_id, v.version, c.cron_expression, c.last_run_ms
             FROM cron_state c
             JOIN workflow_definitions d ON d.name = c.definition_name
             JOIN workflow_definition_versions v ON v.definition_id = d.id",
            [],
        )?;

        // Seed schema version if not present.
        let count: i64 =
            conn.query_row("SELECT COUNT(*) FROM schema_version", [], |row| row.get(0))?;
        if count == 0 {
            conn.execute("INSERT INTO schema_version (version) VALUES (1)", [])?;
        }

        // Migration: add triggers_paused column if missing (existing DBs).
        let has_triggers_paused: bool = conn
            .prepare("SELECT 1 FROM pragma_table_info('workflow_definition_versions') WHERE name = 'triggers_paused'")?
            .exists([])?;
        if !has_triggers_paused {
            conn.execute_batch(
                "ALTER TABLE workflow_definition_versions ADD COLUMN triggers_paused INTEGER NOT NULL DEFAULT 0",
            )?;
        }

        // Migration: add archived column to workflow_definition_versions if missing.
        let has_def_archived: bool = conn
            .prepare(
                "SELECT 1 FROM pragma_table_info('workflow_definition_versions') WHERE name = 'archived'",
            )?
            .exists([])?;
        if !has_def_archived {
            conn.execute_batch(
                "ALTER TABLE workflow_definition_versions ADD COLUMN archived INTEGER NOT NULL DEFAULT 0",
            )?;
        }

        // Migration: add factory_yaml_hash column to workflow_definition_versions if missing.
        let has_factory_hash: bool = conn
            .prepare(
                "SELECT 1 FROM pragma_table_info('workflow_definition_versions') WHERE name = 'factory_yaml_hash'",
            )?
            .exists([])?;
        if !has_factory_hash {
            conn.execute_batch(
                "ALTER TABLE workflow_definition_versions ADD COLUMN factory_yaml_hash TEXT",
            )?;
        }

        // Migration: add archived column to workflow_instances if missing (existing DBs).
        let has_instance_archived: bool = conn
            .prepare(
                "SELECT 1 FROM pragma_table_info('workflow_instances') WHERE name = 'archived'",
            )?
            .exists([])?;
        if !has_instance_archived {
            conn.execute_batch(
                "ALTER TABLE workflow_instances ADD COLUMN archived INTEGER NOT NULL DEFAULT 0",
            )?;
        }

        // Create the archived index after ensuring the column exists.
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_instances_archived_status_created
                ON workflow_instances(archived, status, created_at_ms DESC)",
        )?;

        // Migration: add execution_mode column to workflow_instances if missing.
        let has_execution_mode: bool = conn
            .prepare(
                "SELECT 1 FROM pragma_table_info('workflow_instances') WHERE name = 'execution_mode'",
            )?
            .exists([])?;
        if !has_execution_mode {
            conn.execute_batch(
                "ALTER TABLE workflow_instances ADD COLUMN execution_mode TEXT NOT NULL DEFAULT 'normal'",
            )?;
        }

        // Shadow mode: intercepted actions table.
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS workflow_intercepted_actions (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                instance_id INTEGER NOT NULL REFERENCES workflow_instances(id) ON DELETE CASCADE,
                step_id     TEXT    NOT NULL,
                kind        TEXT    NOT NULL,
                timestamp_ms INTEGER NOT NULL,
                details     TEXT    NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_wia_instance
                ON workflow_intercepted_actions(instance_id);
            ",
        )?;

        // Migration: add definition run-tracking columns if missing.
        let has_last_run: bool = conn
            .prepare(
                "SELECT 1 FROM pragma_table_info('workflow_definition_versions') WHERE name = 'last_successful_run_at_ms'",
            )?
            .exists([])?;
        if !has_last_run {
            conn.execute_batch(
                "ALTER TABLE workflow_definition_versions ADD COLUMN last_successful_run_at_ms INTEGER;
                 ALTER TABLE workflow_definition_versions ADD COLUMN last_successful_definition_hash TEXT;",
            )?;
        }

        Ok(())
    }
}

impl WorkflowPersistence for SqliteWorkflowStore {
    // -----------------------------------------------------------------------
    // Definition CRUD
    // -----------------------------------------------------------------------

    fn save_definition(
        &self,
        yaml_source: &str,
        def: &WorkflowDefinition,
    ) -> Result<(), WorkflowError> {
        self.save_definition_ext(yaml_source, def, None)
    }

    fn save_definition_ext(
        &self,
        yaml_source: &str,
        def: &WorkflowDefinition,
        factory_yaml_hash: Option<&str>,
    ) -> Result<(), WorkflowError> {
        let conn = self.conn()?;
        let now = now_ms();

        // Upsert the definition identity row (one per workflow name).
        // On conflict (name already exists) only update `bundled`; preserve
        // the original external_id.
        conn.execute(
            "INSERT INTO workflow_definitions (external_id, name, bundled, created_at_ms)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(name) DO UPDATE SET bundled = excluded.bundled",
            params![def.id, def.name, def.bundled as i32, now],
        )?;

        let definition_id: i64 = conn.query_row(
            "SELECT id FROM workflow_definitions WHERE name = ?1",
            params![def.name],
            |row| row.get(0),
        )?;

        // Build JSON without bundled/archived (those live in columns only).
        let mut def_for_json = def.clone();
        def_for_json.bundled = false;
        def_for_json.archived = false;
        let json = serde_json::to_string(&def_for_json)?;

        conn.execute(
            "INSERT INTO workflow_definition_versions
             (definition_id, version, description, definition_yaml, definition_json,
              archived, factory_yaml_hash, created_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(definition_id, version) DO UPDATE SET
                description = excluded.description,
                definition_yaml = excluded.definition_yaml,
                definition_json = excluded.definition_json,
                archived = excluded.archived,
                factory_yaml_hash = excluded.factory_yaml_hash,
                updated_at_ms = excluded.updated_at_ms",
            params![
                definition_id,
                def.version,
                def.description,
                yaml_source,
                json,
                def.archived as i32,
                factory_yaml_hash,
                now,
                now,
            ],
        )?;
        Ok(())
    }

    fn get_definition(
        &self,
        name: &str,
        version: &str,
    ) -> Result<Option<(WorkflowDefinition, String)>, WorkflowError> {
        let conn = self.conn()?;
        let result: Option<(String, String, i32, i32, String, i32)> = conn
            .query_row(
                "SELECT v.definition_json, v.definition_yaml, d.bundled, v.archived, d.external_id, v.triggers_paused
                 FROM workflow_definition_versions v
                 JOIN workflow_definitions d ON d.id = v.definition_id
                 WHERE d.name = ?1 AND v.version = ?2",
                params![name, version],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
            )
            .optional()?;

        match result {
            Some((json, yaml, bundled, archived, ext_id, triggers_paused)) => {
                let mut def: WorkflowDefinition = serde_json::from_str(&json)?;
                def.bundled = bundled != 0;
                def.archived = archived != 0;
                def.triggers_paused = triggers_paused != 0;
                def.id = ext_id;
                Ok(Some((def, yaml)))
            }
            None => Ok(None),
        }
    }

    fn get_definition_by_id(
        &self,
        id: &str,
    ) -> Result<Option<(WorkflowDefinition, String)>, WorkflowError> {
        let conn = self.conn()?;
        // Fetch all versions for this definition, pick the latest.
        let mut stmt = conn.prepare(
            "SELECT v.version, v.definition_json, v.definition_yaml, d.bundled, v.archived, d.external_id, v.triggers_paused
             FROM workflow_definition_versions v
             JOIN workflow_definitions d ON d.id = v.definition_id
             WHERE d.external_id = ?1",
        )?;
        let rows = stmt.query_map(params![id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i32>(3)?,
                row.get::<_, i32>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, i32>(6)?,
            ))
        })?;

        let mut candidates: Vec<(String, String, String, i32, i32, String, i32)> = Vec::new();
        for row in rows {
            candidates.push(row?);
        }
        if candidates.is_empty() {
            return Ok(None);
        }

        candidates.sort_by(|a, b| cmp_version_strings(&a.0, &b.0));
        let (_version, json, yaml, bundled, archived, ext_id, triggers_paused) =
            candidates.pop().unwrap();
        let mut def: WorkflowDefinition = serde_json::from_str(&json)?;
        def.bundled = bundled != 0;
        def.archived = archived != 0;
        def.triggers_paused = triggers_paused != 0;
        def.id = ext_id;
        Ok(Some((def, yaml)))
    }

    fn get_latest_definition(
        &self,
        name: &str,
    ) -> Result<Option<(WorkflowDefinition, String)>, WorkflowError> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT v.version, v.definition_json, v.definition_yaml, d.bundled, v.archived, d.external_id, v.triggers_paused
             FROM workflow_definition_versions v
             JOIN workflow_definitions d ON d.id = v.definition_id
             WHERE d.name = ?1",
        )?;
        let rows = stmt.query_map(params![name], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i32>(3)?,
                row.get::<_, i32>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, i32>(6)?,
            ))
        })?;

        let mut candidates: Vec<(String, String, String, i32, i32, String, i32)> = Vec::new();
        for row in rows {
            candidates.push(row?);
        }
        if candidates.is_empty() {
            return Ok(None);
        }

        candidates.sort_by(|a, b| cmp_version_strings(&a.0, &b.0));
        let (_version, json, yaml, bundled, archived, ext_id, triggers_paused) =
            candidates.pop().unwrap();
        let mut def: WorkflowDefinition = serde_json::from_str(&json)?;
        def.bundled = bundled != 0;
        def.archived = archived != 0;
        def.triggers_paused = triggers_paused != 0;
        def.id = ext_id;
        Ok(Some((def, yaml)))
    }

    fn list_definitions(&self) -> Result<Vec<WorkflowDefinitionSummary>, WorkflowError> {
        let conn = self.conn()?;
        // Only return the latest version per definition.  We pick the "latest"
        // by doing a semantic numeric comparison on up to three dot-separated
        // segments of the version string (major.minor.patch), falling back to
        // the rowid as a final tie-breaker.
        let mut stmt = conn.prepare(
            "SELECT d.external_id, d.name, v.version, v.description, v.definition_json,
                    v.created_at_ms, v.updated_at_ms, d.bundled, v.archived, v.triggers_paused,
                    v.last_successful_run_at_ms, v.last_successful_definition_hash
             FROM workflow_definition_versions v
             JOIN workflow_definitions d ON d.id = v.definition_id
             WHERE v.id = (
                 SELECT v2.id FROM workflow_definition_versions v2
                 WHERE v2.definition_id = v.definition_id
                 ORDER BY version_weight(v2.version) DESC, v2.id DESC
                 LIMIT 1
             )
             ORDER BY d.name",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, u64>(5)?,
                row.get::<_, u64>(6)?,
                row.get::<_, i32>(7)?,
                row.get::<_, i32>(8)?,
                row.get::<_, i32>(9)?,
                row.get::<_, Option<u64>>(10)?,
                row.get::<_, Option<String>>(11)?,
            ))
        })?;

        let mut defs = Vec::new();
        for row in rows {
            let (
                ext_id,
                name,
                version,
                description,
                json_str,
                created_at_ms,
                updated_at_ms,
                bundled,
                archived,
                triggers_paused,
                last_successful_run_at_ms,
                last_successful_definition_hash,
            ) = row?;
            let def: WorkflowDefinition = match serde_json::from_str(&json_str) {
                Ok(d) => d,
                Err(e) => {
                    tracing::warn!(
                        name = %name,
                        version = %version,
                        error = %e,
                        "skipping workflow definition with invalid JSON"
                    );
                    continue;
                }
            };
            // Compute is_untested: true if never run or definition changed since
            // last successful run.
            let current_hash = sha256_hex(&json_str);
            let is_untested = match &last_successful_definition_hash {
                Some(h) => h != &current_hash,
                None => true,
            };
            let trigger_types: Vec<String> = def
                .trigger_defs()
                .map(|t| match &t.trigger_type {
                    TriggerType::Manual { .. } => "manual".to_string(),
                    TriggerType::IncomingMessage { .. } => "incoming_message".to_string(),
                    TriggerType::EventPattern { .. } => "event_pattern".to_string(),
                    TriggerType::McpNotification { .. } => "mcp_notification".to_string(),
                    TriggerType::Schedule { .. } => "schedule".to_string(),
                })
                .collect();
            defs.push(WorkflowDefinitionSummary {
                id: ext_id,
                name,
                version,
                description,
                mode: def.mode,
                trigger_types,
                step_count: def.steps.len(),
                created_at_ms,
                updated_at_ms,
                bundled: bundled != 0,
                archived: archived != 0,
                triggers_paused: triggers_paused != 0,
                last_successful_run_at_ms,
                is_untested,
            });
        }
        Ok(defs)
    }

    fn delete_definition(&self, name: &str, version: &str) -> Result<bool, WorkflowError> {
        let conn = self.conn()?;

        // Look up the definition_id.
        let def_row: Option<i64> = conn
            .query_row(
                "SELECT id FROM workflow_definitions WHERE name = ?1",
                params![name],
                |row| row.get(0),
            )
            .optional()?;

        let Some(definition_id) = def_row else {
            return Ok(false);
        };

        let rows = conn.execute(
            "DELETE FROM workflow_definition_versions WHERE definition_id = ?1 AND version = ?2",
            params![definition_id, version],
        )?;

        if rows == 0 {
            return Ok(false);
        }

        // If no versions remain, delete the parent definition row.
        let remaining: i64 = conn.query_row(
            "SELECT COUNT(*) FROM workflow_definition_versions WHERE definition_id = ?1",
            params![definition_id],
            |row| row.get(0),
        )?;
        if remaining == 0 {
            conn.execute("DELETE FROM workflow_definitions WHERE id = ?1", params![definition_id])?;
        }

        Ok(true)
    }

    fn get_definition_meta(
        &self,
        name: &str,
        version: &str,
    ) -> Result<Option<(String, Option<String>, bool)>, WorkflowError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT v.definition_yaml, v.factory_yaml_hash, v.archived
             FROM workflow_definition_versions v
             JOIN workflow_definitions d ON d.id = v.definition_id
             WHERE d.name = ?1 AND v.version = ?2",
            params![name, version],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, i32>(2)? != 0,
                ))
            },
        )
        .optional()
        .map_err(Into::into)
    }

    fn set_archived(
        &self,
        name: &str,
        version: &str,
        archived: bool,
    ) -> Result<bool, WorkflowError> {
        let conn = self.conn()?;
        let rows = conn.execute(
            "UPDATE workflow_definition_versions SET archived = ?3
             WHERE definition_id = (SELECT id FROM workflow_definitions WHERE name = ?1)
               AND version = ?2",
            params![name, version, archived as i32],
        )?;
        Ok(rows > 0)
    }

    fn set_triggers_paused(
        &self,
        name: &str,
        version: &str,
        paused: bool,
    ) -> Result<bool, WorkflowError> {
        let conn = self.conn()?;
        let rows = conn.execute(
            "UPDATE workflow_definition_versions SET triggers_paused = ?3
             WHERE definition_id = (SELECT id FROM workflow_definitions WHERE name = ?1)
               AND version = ?2",
            params![name, version, paused as i32],
        )?;
        Ok(rows > 0)
    }

    fn is_bundled(&self, name: &str) -> Result<bool, WorkflowError> {
        let conn = self.conn()?;
        let count: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM workflow_definitions WHERE name = ?1 AND bundled = 1",
                params![name],
                |row| row.get(0),
            )
            .unwrap_or(0);
        Ok(count > 0)
    }

    // -----------------------------------------------------------------------
    // Instance CRUD
    // -----------------------------------------------------------------------

    fn create_instance(&self, instance: &WorkflowInstance) -> Result<i64, WorkflowError> {
        let mut conn = self.conn()?;
        let tx = conn.transaction().map_err(|e| WorkflowError::Store(e.to_string()))?;

        let def_snapshot = serde_json::to_string(&instance.definition)?;
        let variables = serde_json::to_string(&instance.variables)?;
        let permissions = serde_json::to_string(&instance.permissions)?;
        let active_loops = if instance.active_loops.is_empty() {
            None
        } else {
            Some(serde_json::to_string(&instance.active_loops)?)
        };
        let status =
            serde_json::to_value(instance.status)?.as_str().unwrap_or("pending").to_string();
        let mode = serde_json::to_value(instance.definition.mode)?
            .as_str()
            .unwrap_or("background")
            .to_string();
        let goto_activated_steps = serde_json::to_string(&instance.goto_activated_steps)
            .unwrap_or_else(|_| "[]".to_string());
        let goto_source_steps =
            serde_json::to_string(&instance.goto_source_steps).unwrap_or_else(|_| "[]".to_string());
        let execution_mode = serde_json::to_value(instance.execution_mode)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_else(|| "normal".to_string());

        tx.execute(
            "INSERT INTO workflow_instances
             (definition_name, definition_version, definition_snapshot,
              mode, status, variables, parent_session_id, parent_agent_id, trigger_step_id,
              permissions, workspace_path, created_at_ms, updated_at_ms,
              completed_at_ms, output, error, resolved_result_message, active_loops,
              goto_activated_steps, goto_source_steps, execution_mode)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21)",
            params![
                instance.definition.name,
                instance.definition.version,
                def_snapshot,
                mode,
                status,
                variables,
                instance.parent_session_id,
                instance.parent_agent_id,
                instance.trigger_step_id,
                permissions,
                instance.workspace_path,
                instance.created_at_ms,
                instance.updated_at_ms,
                instance.completed_at_ms,
                instance.output.as_ref().map(|v| serde_json::to_string(v).unwrap_or_default()),
                instance.error,
                instance.resolved_result_message,
                active_loops,
                goto_activated_steps,
                goto_source_steps,
                execution_mode,
            ],
        )?;

        let rowid = tx.last_insert_rowid();

        for (step_id, state) in &instance.step_states {
            save_step_state_on(&tx, rowid, step_id, state)?;
        }

        tx.commit().map_err(|e| WorkflowError::Store(e.to_string()))?;
        Ok(rowid)
    }

    fn update_instance(&self, instance: &WorkflowInstance) -> Result<(), WorkflowError> {
        let mut conn = self.conn()?;
        let tx = conn.transaction().map_err(|e| WorkflowError::Store(e.to_string()))?;

        let variables = serde_json::to_string(&instance.variables)?;
        let permissions = serde_json::to_string(&instance.permissions)?;
        let active_loops = if instance.active_loops.is_empty() {
            None
        } else {
            Some(serde_json::to_string(&instance.active_loops)?)
        };
        let status =
            serde_json::to_value(instance.status)?.as_str().unwrap_or("pending").to_string();
        let output_str =
            instance.output.as_ref().map(|v| serde_json::to_string(v).unwrap_or_default());
        let goto_activated_steps = serde_json::to_string(&instance.goto_activated_steps)
            .unwrap_or_else(|_| "[]".to_string());
        let goto_source_steps =
            serde_json::to_string(&instance.goto_source_steps).unwrap_or_else(|_| "[]".to_string());

        tx.execute(
            "UPDATE workflow_instances SET
                status = ?1, variables = ?2, permissions = ?3,
                updated_at_ms = ?4, completed_at_ms = ?5, output = ?6, error = ?7,
                resolved_result_message = ?8, active_loops = ?9,
                goto_activated_steps = ?10, goto_source_steps = ?11
             WHERE id = ?12",
            params![
                status,
                variables,
                permissions,
                instance.updated_at_ms,
                instance.completed_at_ms,
                output_str,
                instance.error,
                instance.resolved_result_message,
                active_loops,
                goto_activated_steps,
                goto_source_steps,
                instance.id,
            ],
        )?;

        for (step_id, state) in &instance.step_states {
            save_step_state_on(&tx, instance.id, step_id, state)?;
        }

        tx.commit().map_err(|e| WorkflowError::Store(e.to_string()))?;
        Ok(())
    }

    fn get_instance(&self, id: i64) -> Result<Option<WorkflowInstance>, WorkflowError> {
        let conn = self.conn()?;

        let row = conn
            .query_row(
                "SELECT id, definition_snapshot, status, variables, parent_session_id,
                        parent_agent_id, permissions, created_at_ms, updated_at_ms,
                        completed_at_ms, output, error, workspace_path, resolved_result_message,
                        active_loops, goto_activated_steps, goto_source_steps, trigger_step_id,
                        execution_mode
                 FROM workflow_instances WHERE id = ?1",
                params![id],
                |row| {
                    let rowid: i64 = row.get(0)?;
                    let def_snapshot: String = row.get(1)?;
                    let status_str: String = row.get(2)?;
                    let variables_str: String = row.get(3)?;
                    let parent_session_id: String = row.get(4)?;
                    let parent_agent_id: Option<String> = row.get(5)?;
                    let permissions_str: String = row.get(6)?;
                    let created_at_ms: u64 = row.get(7)?;
                    let updated_at_ms: u64 = row.get(8)?;
                    let completed_at_ms: Option<u64> = row.get(9)?;
                    let output_str: Option<String> = row.get(10)?;
                    let error: Option<String> = row.get(11)?;
                    let workspace_path: Option<String> = row.get(12)?;
                    let resolved_result_message: Option<String> = row.get(13)?;
                    let active_loops_str: Option<String> = row.get(14)?;
                    let goto_activated_steps_str: Option<String> = row.get(15)?;
                    let goto_source_steps_str: Option<String> = row.get(16)?;
                    let trigger_step_id: Option<String> = row.get(17)?;
                    let execution_mode_str: String = row.get::<_, Option<String>>(18)?.unwrap_or_else(|| "normal".to_string());
                    Ok((
                        rowid,
                        def_snapshot,
                        status_str,
                        variables_str,
                        parent_session_id,
                        parent_agent_id,
                        permissions_str,
                        created_at_ms,
                        updated_at_ms,
                        completed_at_ms,
                        output_str,
                        error,
                        workspace_path,
                        resolved_result_message,
                        active_loops_str,
                        goto_activated_steps_str,
                        goto_source_steps_str,
                        trigger_step_id,
                        execution_mode_str,
                    ))
                },
            )
            .optional()?;

        let Some((
            rowid,
            def_snapshot,
            status_str,
            variables_str,
            parent_session_id,
            parent_agent_id,
            permissions_str,
            created_at_ms,
            updated_at_ms,
            completed_at_ms,
            output_str,
            error,
            workspace_path,
            resolved_result_message,
            active_loops_str,
            goto_activated_steps_str,
            goto_source_steps_str,
            trigger_step_id,
            execution_mode_str,
        )) = row
        else {
            return Ok(None);
        };

        let definition: WorkflowDefinition = serde_json::from_str(&def_snapshot)?;
        let status: WorkflowStatus = serde_json::from_value(serde_json::Value::String(status_str))
            .unwrap_or_else(|_| {
                tracing::warn!(
                    instance_id = rowid,
                    "Unknown workflow status in DB, defaulting to Pending"
                );
                WorkflowStatus::Pending
            });
        let variables: serde_json::Value = serde_json::from_str(&variables_str)?;
        let permissions: Vec<PermissionEntry> = serde_json::from_str(&permissions_str)?;
        let output: Option<serde_json::Value> =
            output_str.and_then(|s: String| serde_json::from_str(&s).inspect_err(|e| {
                tracing::warn!(instance_id = rowid, error = %e, "Failed to parse workflow output JSON");
            }).ok());
        let active_loops: HashMap<String, LoopState> = active_loops_str
            .and_then(|s| serde_json::from_str(&s).inspect_err(|e| {
                tracing::warn!(instance_id = rowid, error = %e, "Failed to parse active_loops JSON, using empty default");
            }).ok())
            .unwrap_or_default();
        let goto_activated_steps = goto_activated_steps_str
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let goto_source_steps =
            goto_source_steps_str.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let execution_mode: ExecutionMode =
            serde_json::from_value(serde_json::Value::String(execution_mode_str))
                .unwrap_or_default();

        let step_states = load_step_states_on(&conn, rowid)?;

        Ok(Some(WorkflowInstance {
            id: rowid,
            definition,
            status,
            variables,
            step_states,
            parent_session_id,
            parent_agent_id,
            trigger_step_id,
            permissions,
            workspace_path,
            created_at_ms,
            updated_at_ms,
            completed_at_ms,
            output,
            error,
            resolved_result_message,
            goto_activated_steps,
            goto_source_steps,
            active_loops,
            execution_mode,
            shadow_overrides: HashMap::new(),
        }))
    }

    fn list_instances(&self, filter: &InstanceFilter) -> Result<InstanceListResult, WorkflowError> {
        let conn = self.conn()?;
        let mut where_clause = String::from(" WHERE 1=1");
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if !filter.statuses.is_empty() {
            let placeholders: Vec<String> = filter
                .statuses
                .iter()
                .map(|status| {
                    let status_str = serde_json::to_value(status)
                        .ok()
                        .and_then(|v| v.as_str().map(String::from))
                        .unwrap_or_default();
                    params_vec.push(Box::new(status_str));
                    format!("?{}", params_vec.len())
                })
                .collect();
            where_clause.push_str(&format!(" AND i.status IN ({})", placeholders.join(", ")));
        }
        if !filter.definition_names.is_empty() {
            let placeholders: Vec<String> = filter
                .definition_names
                .iter()
                .map(|name| {
                    params_vec.push(Box::new(name.clone()));
                    format!("?{}", params_vec.len())
                })
                .collect();
            where_clause
                .push_str(&format!(" AND i.definition_name IN ({})", placeholders.join(", ")));
        }
        if let Some(ref sid) = filter.parent_session_id {
            sql_push_param(
                &mut where_clause,
                &mut params_vec,
                " AND i.parent_session_id",
                sid.clone(),
            );
        }
        if let Some(ref aid) = filter.parent_agent_id {
            sql_push_param(
                &mut where_clause,
                &mut params_vec,
                " AND i.parent_agent_id",
                aid.clone(),
            );
        }
        if let Some(ref did) = filter.definition_id {
            params_vec.push(Box::new(did.clone()));
            where_clause.push_str(&format!(
                " AND i.definition_name IN (SELECT name FROM workflow_definitions WHERE external_id = ?{})",
                params_vec.len()
            ));
        }
        if let Some(ref mode) = filter.mode {
            let mode_str = serde_json::to_value(mode)
                .ok()
                .and_then(|v| v.as_str().map(String::from))
                .unwrap_or_default();
            params_vec.push(Box::new(mode_str));
            where_clause.push_str(&format!(" AND i.mode = ?{}", params_vec.len()));
        }
        if !filter.include_archived {
            where_clause.push_str(" AND i.archived = 0");
        }

        // Count query
        let count_sql = format!("SELECT COUNT(*) FROM workflow_instances i{where_clause}");
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();
        let total: usize = conn.query_row(&count_sql, param_refs.as_slice(), |row| row.get(0))?;

        // Data query with LEFT JOIN for step state aggregates (replaces 5 correlated subqueries).
        let mut sql = format!(
            "SELECT i.id, i.definition_name, i.status, i.parent_session_id, i.parent_agent_id,
                    i.created_at_ms, i.updated_at_ms, i.completed_at_ms,
                    COALESCE(ss.step_count, 0),
                    COALESCE(ss.steps_completed, 0),
                    COALESCE(ss.steps_failed, 0),
                    COALESCE(ss.steps_running, 0),
                    COALESCE(ss.pending_interactions, 0),
                    i.definition_version,
                    i.trigger_step_id,
                    i.archived,
                    i.execution_mode
             FROM workflow_instances i
             LEFT JOIN (
                 SELECT instance_id,
                        COUNT(*) as step_count,
                        SUM(CASE WHEN status = 'completed' THEN 1 ELSE 0 END) as steps_completed,
                        SUM(CASE WHEN status = 'failed' THEN 1 ELSE 0 END) as steps_failed,
                        SUM(CASE WHEN status = 'running' THEN 1 ELSE 0 END) as steps_running,
                        SUM(CASE WHEN status = 'waiting_on_input' THEN 1 ELSE 0 END) as pending_interactions
                 FROM workflow_step_states
                 GROUP BY instance_id
             ) ss ON ss.instance_id = i.id{where_clause} ORDER BY i.created_at_ms DESC"
        );

        if let Some(limit) = filter.limit {
            params_vec.push(Box::new(limit as i64));
            sql.push_str(&format!(" LIMIT ?{}", params_vec.len()));
            if let Some(offset) = filter.offset {
                params_vec.push(Box::new(offset as i64));
                sql.push_str(&format!(" OFFSET ?{}", params_vec.len()));
            }
        }

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(param_refs.as_slice(), |row| {
            Ok(WorkflowInstanceSummary {
                id: row.get(0)?,
                definition_name: row.get(1)?,
                status: serde_json::from_value(serde_json::Value::String(row.get::<_, String>(2)?))
                    .unwrap_or(WorkflowStatus::Pending),
                parent_session_id: row.get(3)?,
                parent_agent_id: row.get(4)?,
                created_at_ms: row.get(5)?,
                updated_at_ms: row.get(6)?,
                completed_at_ms: row.get(7)?,
                step_count: row.get::<_, usize>(8)?,
                steps_completed: row.get::<_, usize>(9)?,
                steps_failed: row.get::<_, usize>(10)?,
                steps_running: row.get::<_, usize>(11)?,
                has_pending_interaction: row.get::<_, usize>(12)? > 0,
                definition_version: row.get(13)?,
                trigger_step_id: row.get(14)?,
                pending_agent_approvals: 0,
                pending_agent_questions: 0,
                child_agent_ids: Vec::new(),
                archived: row.get::<_, i32>(15)? != 0,
                execution_mode: row.get::<_, Option<String>>(16)?
                    .and_then(|s| serde_json::from_value(serde_json::Value::String(s)).ok())
                    .unwrap_or_default(),
            })
        })?;

        let mut items = Vec::new();
        for row in rows {
            items.push(row?);
        }
        Ok(InstanceListResult { items, total })
    }

    fn delete_instance(&self, id: i64) -> Result<bool, WorkflowError> {
        let conn = self.conn()?;
        let rows = conn.execute("DELETE FROM workflow_instances WHERE id = ?1", params![id])?;
        Ok(rows > 0)
    }

    fn set_instance_archived(&self, id: i64, archived: bool) -> Result<bool, WorkflowError> {
        let conn = self.conn()?;
        let rows = conn.execute(
            "UPDATE workflow_instances SET archived = ?2 WHERE id = ?1",
            params![id, archived as i32],
        )?;
        Ok(rows > 0)
    }

    fn list_waiting_feedback_for_session(
        &self,
        session_id: &str,
    ) -> Result<
        Vec<(i64, String, String, Option<String>, Option<String>, Option<bool>)>,
        WorkflowError,
    > {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT i.id, s.step_id, i.definition_snapshot,
                    s.interaction_prompt, s.interaction_choices, s.interaction_allow_freeform
             FROM workflow_step_states s
             JOIN workflow_instances i ON i.id = s.instance_id
             WHERE s.status = 'waiting_on_input'
               AND i.parent_session_id = ?1
               AND i.status IN ('running', 'waiting_on_input')",
        )?;
        let rows = stmt.query_map(params![session_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<bool>>(5)?,
            ))
        })?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    fn list_all_waiting_feedback(
        &self,
    ) -> Result<
        Vec<(i64, String, String, Option<String>, Option<String>, Option<bool>, String)>,
        WorkflowError,
    > {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT i.id, s.step_id, i.definition_snapshot,
                    s.interaction_prompt, s.interaction_choices, s.interaction_allow_freeform,
                    i.parent_session_id
             FROM workflow_step_states s
             JOIN workflow_instances i ON i.id = s.instance_id
             WHERE s.status = 'waiting_on_input'
               AND i.status IN ('running', 'waiting_on_input')",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<bool>>(5)?,
                row.get::<_, String>(6)?,
            ))
        })?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    fn list_child_agent_ids(
        &self,
    ) -> Result<std::collections::HashMap<i64, Vec<String>>, WorkflowError> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT i.id, s.child_agent_id
             FROM workflow_step_states s
             JOIN workflow_instances i ON i.id = s.instance_id
             WHERE s.child_agent_id IS NOT NULL
               AND i.status IN ('running', 'waiting_on_input', 'waiting_on_event')",
        )?;
        let rows =
            stmt.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)))?;
        let mut map: std::collections::HashMap<i64, Vec<String>> = std::collections::HashMap::new();
        for row in rows {
            let (instance_id, agent_id) = row?;
            map.entry(instance_id).or_default().push(agent_id);
        }
        Ok(map)
    }

    fn set_child_agent_id(
        &self,
        instance_id: i64,
        step_id: &str,
        agent_id: &str,
    ) -> Result<(), WorkflowError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE workflow_step_states SET child_agent_id = ?1
             WHERE instance_id = ?2
               AND step_id = ?3",
            params![agent_id, instance_id, step_id],
        )?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Trigger deduplication
    // -----------------------------------------------------------------------

    fn is_trigger_seen(
        &self,
        definition_id: &str,
        external_id: &str,
    ) -> Result<bool, WorkflowError> {
        let conn = self.conn()?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM trigger_dedup_v2 WHERE definition_id = ?1 AND external_id = ?2",
            params![definition_id, external_id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    fn mark_trigger_seen(
        &self,
        definition_id: &str,
        external_id: &str,
    ) -> Result<(), WorkflowError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT OR IGNORE INTO trigger_dedup_v2 (definition_id, external_id, created_at_ms)
             VALUES (?1, ?2, ?3)",
            params![definition_id, external_id, now_ms()],
        )?;
        Ok(())
    }

    fn prune_trigger_dedup(&self, max_age_ms: u64) -> Result<usize, WorkflowError> {
        let conn = self.conn()?;
        let cutoff = now_ms().saturating_sub(max_age_ms);
        let deleted_v2 = conn
            .execute("DELETE FROM trigger_dedup_v2 WHERE created_at_ms <= ?1", params![cutoff])?;
        // Best-effort cleanup of legacy rows retained for compatibility.
        let _ =
            conn.execute("DELETE FROM trigger_dedup WHERE created_at_ms <= ?1", params![cutoff]);
        Ok(deleted_v2)
    }

    // -----------------------------------------------------------------------
    // Cron state
    // -----------------------------------------------------------------------

    fn get_cron_last_run(
        &self,
        definition_id: &str,
        definition_version: &str,
        cron_expression: &str,
    ) -> Result<Option<u64>, WorkflowError> {
        let conn = self.conn()?;
        let result: Option<u64> = conn
            .query_row(
                "SELECT last_run_ms FROM cron_state_v2
                 WHERE definition_id = ?1 AND definition_version = ?2 AND cron_expression = ?3",
                params![definition_id, definition_version, cron_expression],
                |row| row.get(0),
            )
            .optional()?;
        Ok(result)
    }

    fn set_cron_last_run(
        &self,
        definition_id: &str,
        definition_version: &str,
        cron_expression: &str,
        last_run_ms: u64,
    ) -> Result<(), WorkflowError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO cron_state_v2 (definition_id, definition_version, cron_expression, last_run_ms)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(definition_id, definition_version, cron_expression)
             DO UPDATE SET last_run_ms = excluded.last_run_ms",
            params![definition_id, definition_version, cron_expression, last_run_ms],
        )?;
        Ok(())
    }

    fn delete_cron_state(
        &self,
        definition_id: &str,
        definition_version: Option<&str>,
    ) -> Result<(), WorkflowError> {
        let conn = self.conn()?;
        if let Some(version) = definition_version {
            conn.execute(
                "DELETE FROM cron_state_v2 WHERE definition_id = ?1 AND definition_version = ?2",
                params![definition_id, version],
            )?;
        } else {
            conn.execute(
                "DELETE FROM cron_state_v2 WHERE definition_id = ?1",
                params![definition_id],
            )?;
        }
        Ok(())
    }

    fn get_event_replay_cursor(&self) -> Result<Option<u64>, WorkflowError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT value FROM workflow_runtime_state WHERE key = 'trigger_event_replay_cursor_ms'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map(|opt| opt.and_then(|v| v.parse::<u64>().ok()))
        .map_err(Into::into)
    }

    fn set_event_replay_cursor(&self, timestamp_ms: u64) -> Result<(), WorkflowError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO workflow_runtime_state (key, value)
             VALUES ('trigger_event_replay_cursor_ms', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![timestamp_ms.to_string()],
        )?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Intercepted actions (shadow mode)
    // -----------------------------------------------------------------------

    fn save_intercepted_action(&self, action: &InterceptedAction) -> Result<i64, WorkflowError> {
        let conn = self.conn()?;
        let details_str = serde_json::to_string(&action.details)?;
        conn.execute(
            "INSERT INTO workflow_intercepted_actions (instance_id, step_id, kind, timestamp_ms, details)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                action.instance_id,
                action.step_id,
                action.kind,
                action.timestamp_ms,
                details_str,
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    fn list_intercepted_actions(
        &self,
        instance_id: i64,
        limit: usize,
        offset: usize,
    ) -> Result<InterceptedActionPage, WorkflowError> {
        let conn = self.conn()?;
        let total: usize = conn.query_row(
            "SELECT COUNT(*) FROM workflow_intercepted_actions WHERE instance_id = ?1",
            params![instance_id],
            |row| row.get(0),
        )?;

        let mut stmt = conn.prepare(
            "SELECT id, instance_id, step_id, kind, timestamp_ms, details
             FROM workflow_intercepted_actions
             WHERE instance_id = ?1
             ORDER BY id ASC
             LIMIT ?2 OFFSET ?3",
        )?;
        let rows = stmt.query_map(params![instance_id, limit as i64, offset as i64], |row| {
            let details_str: String = row.get(5)?;
            let details: serde_json::Value =
                serde_json::from_str(&details_str).unwrap_or(serde_json::Value::Null);
            Ok(InterceptedAction {
                id: row.get(0)?,
                instance_id: row.get(1)?,
                step_id: row.get(2)?,
                kind: row.get(3)?,
                timestamp_ms: row.get(4)?,
                details,
            })
        })?;

        let mut items = Vec::new();
        for row in rows {
            items.push(row?);
        }
        Ok(InterceptedActionPage { items, total })
    }

    fn get_shadow_summary(&self, instance_id: i64) -> Result<ShadowSummary, WorkflowError> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT kind, COUNT(*) FROM workflow_intercepted_actions
             WHERE instance_id = ?1
             GROUP BY kind",
        )?;
        let rows = stmt.query_map(params![instance_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?))
        })?;

        let mut summary = ShadowSummary::default();
        for row in rows {
            let (kind, count) = row?;
            summary.total_intercepted += count;
            match kind.as_str() {
                "tool_call" => summary.tool_calls_intercepted += count,
                "agent_invocation" => summary.agent_invocations_intercepted += count,
                "workflow_launch" => summary.workflow_launches_intercepted += count,
                "scheduled_task" => summary.scheduled_tasks_intercepted += count,
                "agent_signal" => summary.agent_signals_intercepted += count,
                _ => {} // Unknown kinds still count toward total
            }
        }
        Ok(summary)
    }

    fn record_successful_run(
        &self,
        name: &str,
        version: &str,
        definition_hash: &str,
        run_at_ms: u64,
    ) -> Result<(), WorkflowError> {
        let conn = self.conn()?;
        // Only overwrite if this run is newer than the stored one (prevents
        // stale completions from regressing the status).
        conn.execute(
            "UPDATE workflow_definition_versions
             SET last_successful_run_at_ms = ?1,
                 last_successful_definition_hash = ?2
             WHERE definition_id = (SELECT id FROM workflow_definitions WHERE name = ?3)
               AND version = ?4
               AND (last_successful_run_at_ms IS NULL OR last_successful_run_at_ms < ?1)",
            params![run_at_ms, definition_hash, name, version],
        )?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Maintenance
    // -----------------------------------------------------------------------

    fn prune_completed_instances(&self, max_age_ms: u64) -> Result<usize, WorkflowError> {
        let conn = self.conn()?;
        let cutoff = now_ms().saturating_sub(max_age_ms);
        let deleted = conn.execute(
            "DELETE FROM workflow_instances
             WHERE status IN ('completed', 'failed', 'killed')
               AND completed_at_ms IS NOT NULL
               AND completed_at_ms <= ?1",
            params![cutoff],
        )?;
        Ok(deleted)
    }
}

fn save_step_state_on(
    conn: &Connection,
    instance_rowid: i64,
    step_id: &str,
    state: &StepState,
) -> Result<(), WorkflowError> {
    let status = serde_json::to_value(state.status)?.as_str().unwrap_or("pending").to_string();
    let outputs_str = state.outputs.as_ref().map(|v| serde_json::to_string(v).unwrap_or_default());

    let choices_str =
        state.interaction_choices.as_ref().map(|v| serde_json::to_string(v).unwrap_or_default());

    conn.execute(
        "INSERT INTO workflow_step_states
         (instance_id, step_id, status, started_at_ms, completed_at_ms, outputs, error,
          retry_count, child_workflow_id, child_agent_id, interaction_request_id,
          interaction_prompt, interaction_choices, interaction_allow_freeform, retry_delay_secs,
          resume_at_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
         ON CONFLICT(instance_id, step_id) DO UPDATE SET
            status = excluded.status,
            started_at_ms = excluded.started_at_ms,
            completed_at_ms = excluded.completed_at_ms,
            outputs = excluded.outputs,
            error = excluded.error,
            retry_count = excluded.retry_count,
            child_workflow_id = excluded.child_workflow_id,
            child_agent_id = excluded.child_agent_id,
            interaction_request_id = excluded.interaction_request_id,
            interaction_prompt = excluded.interaction_prompt,
            interaction_choices = excluded.interaction_choices,
            interaction_allow_freeform = excluded.interaction_allow_freeform,
            retry_delay_secs = excluded.retry_delay_secs,
            resume_at_ms = excluded.resume_at_ms",
        params![
            instance_rowid,
            step_id,
            status,
            state.started_at_ms,
            state.completed_at_ms,
            outputs_str,
            state.error,
            state.retry_count,
            state.child_workflow_id,
            state.child_agent_id,
            state.interaction_request_id,
            state.interaction_prompt,
            choices_str,
            state.interaction_allow_freeform,
            state.retry_delay_secs.map(|v| v as i64),
            state.resume_at_ms.map(|v| v as i64),
        ],
    )?;
    Ok(())
}

fn load_step_states_on(
    conn: &Connection,
    instance_rowid: i64,
) -> Result<std::collections::HashMap<String, StepState>, WorkflowError> {
    let mut stmt = conn.prepare(
        "SELECT step_id, status, started_at_ms, completed_at_ms, outputs, error,
                retry_count, child_workflow_id, child_agent_id, interaction_request_id,
                interaction_prompt, interaction_choices, interaction_allow_freeform,
                retry_delay_secs, resume_at_ms
         FROM workflow_step_states WHERE instance_id = ?1",
    )?;
    let rows = stmt.query_map(params![instance_rowid], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<u64>>(2)?,
            row.get::<_, Option<u64>>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<String>>(5)?,
            row.get::<_, u32>(6)?,
            row.get::<_, Option<i64>>(7)?,
            row.get::<_, Option<String>>(8)?,
            row.get::<_, Option<String>>(9)?,
            row.get::<_, Option<String>>(10)?,
            row.get::<_, Option<String>>(11)?,
            row.get::<_, Option<bool>>(12)?,
            row.get::<_, Option<i64>>(13)?,
            row.get::<_, Option<i64>>(14)?,
        ))
    })?;

    let mut map = std::collections::HashMap::new();
    for row in rows {
        let (
            step_id,
            status_str,
            started_at_ms,
            completed_at_ms,
            outputs_str,
            error,
            retry_count,
            child_workflow_id,
            child_agent_id,
            interaction_request_id,
            interaction_prompt,
            interaction_choices_str,
            interaction_allow_freeform,
            retry_delay_secs_raw,
            resume_at_ms_raw,
        ) = row?;

        let status: StepStatus = serde_json::from_value(serde_json::Value::String(
            status_str.clone(),
        ))
        .unwrap_or_else(|_| {
            tracing::warn!(step_id = %step_id, "Unknown step status in DB, defaulting to Pending");
            StepStatus::Pending
        });
        let outputs: Option<serde_json::Value> =
            outputs_str.and_then(|s| serde_json::from_str(&s).ok());
        let interaction_choices: Option<Vec<String>> =
            interaction_choices_str.and_then(|s| serde_json::from_str(&s).ok());

        map.insert(
            step_id.clone(),
            StepState {
                step_id,
                status,
                started_at_ms,
                completed_at_ms,
                outputs,
                error,
                retry_count,
                retry_delay_secs: retry_delay_secs_raw.map(|v| v as u64),
                child_workflow_id,
                child_agent_id,
                interaction_request_id,
                interaction_prompt,
                interaction_choices,
                interaction_allow_freeform,
                resume_at_ms: resume_at_ms_raw.map(|v| v as u64),
            },
        );
    }
    Ok(map)
}

impl Drop for SqliteWorkflowStore {
    fn drop(&mut self) {
        // Clean up test temp files.
        if let Some(name) = self.pool.path.file_name().and_then(|n| n.to_str()) {
            if name.contains("hive_wf_test_") {
                // Drop all pooled connections first so the file is unlocked.
                let mut pool = self.pool.pool.lock().unwrap();
                pool.clear();
                drop(pool);

                let path = &self.pool.path;
                let _ = std::fs::remove_file(path);
                let mut wal = path.as_os_str().to_owned();
                wal.push("-wal");
                let _ = std::fs::remove_file(&wal);
                let mut shm = path.as_os_str().to_owned();
                shm.push("-shm");
                let _ = std::fs::remove_file(&shm);
            }
        }
    }
}

/// Compare two version strings semantically.
fn cmp_version_strings(a: &str, b: &str) -> std::cmp::Ordering {
    let a_parts: Vec<&str> = a.split('.').collect();
    let b_parts: Vec<&str> = b.split('.').collect();
    let len = a_parts.len().max(b_parts.len());
    for i in 0..len {
        let ap = a_parts.get(i).copied().unwrap_or("");
        let bp = b_parts.get(i).copied().unwrap_or("");
        let ord = match (ap.parse::<u64>(), bp.parse::<u64>()) {
            (Ok(an), Ok(bn)) => an.cmp(&bn),
            _ => ap.cmp(bp),
        };
        if ord != std::cmp::Ordering::Equal {
            return ord;
        }
    }
    std::cmp::Ordering::Equal
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    // -----------------------------------------------------------------------
    // InMemoryWorkflowStore -- test double
    // -----------------------------------------------------------------------

    struct InMemoryWorkflowInner {
        definitions: HashMap<(String, String), (WorkflowDefinition, String)>,
        factory_hashes: HashMap<(String, String), Option<String>>,
        instances: HashMap<i64, WorkflowInstance>,
        trigger_dedup: HashMap<(String, String), u64>,
        cron_state: HashMap<(String, String, String), u64>,
        event_replay_cursor: Option<u64>,
        def_insert_order: HashMap<(String, String), u64>,
        next_order: u64,
        next_instance_id: i64,
        intercepted_actions: Vec<InterceptedAction>,
        next_action_id: i64,
        /// (name, version) → (run_at_ms, definition_hash)
        successful_runs: HashMap<(String, String), (u64, String)>,
    }

    pub(crate) struct InMemoryWorkflowStore {
        inner: Mutex<InMemoryWorkflowInner>,
    }

    impl InMemoryWorkflowStore {
        pub fn new() -> Self {
            Self {
                inner: Mutex::new(InMemoryWorkflowInner {
                    definitions: HashMap::new(),
                    factory_hashes: HashMap::new(),
                    instances: HashMap::new(),
                    trigger_dedup: HashMap::new(),
                    cron_state: HashMap::new(),
                    event_replay_cursor: None,
                    def_insert_order: HashMap::new(),
                    next_order: 0,
                    next_instance_id: 1,
                    intercepted_actions: Vec::new(),
                    next_action_id: 1,
                    successful_runs: HashMap::new(),
                }),
            }
        }
    }

    fn trigger_type_name(t: &TriggerType) -> String {
        match t {
            TriggerType::Manual { .. } => "manual".to_string(),
            TriggerType::IncomingMessage { .. } => "incoming_message".to_string(),
            TriggerType::EventPattern { .. } => "event_pattern".to_string(),
            TriggerType::McpNotification { .. } => "mcp_notification".to_string(),
            TriggerType::Schedule { .. } => "schedule".to_string(),
        }
    }

    fn instance_to_summary(inst: &WorkflowInstance) -> WorkflowInstanceSummary {
        let step_count = inst.step_states.len();
        let steps_completed =
            inst.step_states.values().filter(|s| s.status == StepStatus::Completed).count();
        let steps_failed =
            inst.step_states.values().filter(|s| s.status == StepStatus::Failed).count();
        let steps_running =
            inst.step_states.values().filter(|s| s.status == StepStatus::Running).count();
        let has_pending_interaction =
            inst.step_states.values().any(|s| s.status == StepStatus::WaitingOnInput);
        let child_agent_ids: Vec<String> =
            inst.step_states.values().filter_map(|s| s.child_agent_id.clone()).collect();
        WorkflowInstanceSummary {
            id: inst.id,
            definition_name: inst.definition.name.clone(),
            definition_version: inst.definition.version.clone(),
            status: inst.status.clone(),
            parent_session_id: inst.parent_session_id.clone(),
            parent_agent_id: inst.parent_agent_id.clone(),
            trigger_step_id: inst.trigger_step_id.clone(),
            created_at_ms: inst.created_at_ms,
            updated_at_ms: inst.updated_at_ms,
            completed_at_ms: inst.completed_at_ms,
            step_count,
            steps_completed,
            steps_failed,
            steps_running,
            has_pending_interaction,
            pending_agent_approvals: 0,
            pending_agent_questions: 0,
            child_agent_ids,
            archived: false,
            execution_mode: inst.execution_mode,
        }
    }

    impl WorkflowPersistence for InMemoryWorkflowStore {
        fn save_definition(
            &self,
            yaml_source: &str,
            def: &WorkflowDefinition,
        ) -> Result<(), WorkflowError> {
            self.save_definition_ext(yaml_source, def, None)
        }

        fn save_definition_ext(
            &self,
            yaml_source: &str,
            def: &WorkflowDefinition,
            factory_yaml_hash: Option<&str>,
        ) -> Result<(), WorkflowError> {
            let mut inner = self.inner.lock().unwrap();
            let key = (def.name.clone(), def.version.clone());
            inner.definitions.insert(key.clone(), (def.clone(), yaml_source.to_string()));
            inner.factory_hashes.insert(key.clone(), factory_yaml_hash.map(|s| s.to_string()));
            let order = inner.next_order;
            inner.next_order += 1;
            inner.def_insert_order.insert(key, order);
            Ok(())
        }

        fn get_definition(
            &self,
            name: &str,
            version: &str,
        ) -> Result<Option<(WorkflowDefinition, String)>, WorkflowError> {
            let inner = self.inner.lock().unwrap();
            Ok(inner.definitions.get(&(name.to_string(), version.to_string())).cloned())
        }

        fn get_definition_by_id(
            &self,
            id: &str,
        ) -> Result<Option<(WorkflowDefinition, String)>, WorkflowError> {
            let inner = self.inner.lock().unwrap();
            Ok(inner.definitions.values().find(|(d, _)| d.id == id).cloned())
        }

        fn get_latest_definition(
            &self,
            name: &str,
        ) -> Result<Option<(WorkflowDefinition, String)>, WorkflowError> {
            let inner = self.inner.lock().unwrap();
            let mut best: Option<(&(String, String), u64)> = None;
            for (key, order) in &inner.def_insert_order {
                if key.0 == name {
                    if best.is_none() || *order > best.unwrap().1 {
                        best = Some((key, *order));
                    }
                }
            }
            match best {
                Some((key, _)) => Ok(inner.definitions.get(key).cloned()),
                None => Ok(None),
            }
        }

        fn list_definitions(&self) -> Result<Vec<WorkflowDefinitionSummary>, WorkflowError> {
            let inner = self.inner.lock().unwrap();
            // Build summaries grouped by definition name, keeping only the latest version.
            let mut latest_by_name: std::collections::HashMap<String, WorkflowDefinitionSummary> =
                std::collections::HashMap::new();
            for ((name, _version), (def, _yaml)) in &inner.definitions {
                let trigger_types: Vec<String> =
                    def.trigger_defs().map(|t| trigger_type_name(&t.trigger_type)).collect();
                let json_str = serde_json::to_string(def).unwrap_or_default();
                let current_hash = sha256_hex(&json_str);
                let run_meta = inner.successful_runs.get(&(def.name.clone(), def.version.clone()));
                let last_successful_run_at_ms = run_meta.map(|(ts, _)| *ts);
                let is_untested = match run_meta {
                    Some((_, h)) => h != &current_hash,
                    None => true,
                };
                let summary = WorkflowDefinitionSummary {
                    id: def.id.clone(),
                    name: def.name.clone(),
                    version: def.version.clone(),
                    description: def.description.clone(),
                    mode: def.mode,
                    trigger_types,
                    step_count: def.steps.len(),
                    created_at_ms: 0,
                    updated_at_ms: 0,
                    bundled: def.bundled,
                    archived: def.archived,
                    triggers_paused: def.triggers_paused,
                    last_successful_run_at_ms,
                    is_untested,
                };
                match latest_by_name.get(name) {
                    Some(existing)
                        if cmp_version_strings(&existing.version, &def.version)
                            != std::cmp::Ordering::Less => {}
                    _ => {
                        latest_by_name.insert(def.name.clone(), summary);
                    }
                }
            }
            let mut defs: Vec<WorkflowDefinitionSummary> = latest_by_name.into_values().collect();
            defs.sort_by(|a, b| a.name.cmp(&b.name));
            Ok(defs)
        }

        fn delete_definition(&self, name: &str, version: &str) -> Result<bool, WorkflowError> {
            let mut inner = self.inner.lock().unwrap();
            let key = (name.to_string(), version.to_string());
            inner.factory_hashes.remove(&key);
            inner.def_insert_order.remove(&key);
            Ok(inner.definitions.remove(&key).is_some())
        }

        fn get_definition_meta(
            &self,
            name: &str,
            version: &str,
        ) -> Result<Option<(String, Option<String>, bool)>, WorkflowError> {
            let inner = self.inner.lock().unwrap();
            let key = (name.to_string(), version.to_string());
            match inner.definitions.get(&key) {
                Some((def, yaml)) => {
                    let factory_hash = inner.factory_hashes.get(&key).cloned().flatten();
                    Ok(Some((yaml.clone(), factory_hash, def.bundled)))
                }
                None => Ok(None),
            }
        }

        fn set_archived(
            &self,
            name: &str,
            version: &str,
            archived: bool,
        ) -> Result<bool, WorkflowError> {
            let mut inner = self.inner.lock().unwrap();
            let key = (name.to_string(), version.to_string());
            match inner.definitions.get_mut(&key) {
                Some((def, _)) => {
                    def.archived = archived;
                    Ok(true)
                }
                None => Ok(false),
            }
        }

        fn set_triggers_paused(
            &self,
            name: &str,
            version: &str,
            paused: bool,
        ) -> Result<bool, WorkflowError> {
            let mut inner = self.inner.lock().unwrap();
            let key = (name.to_string(), version.to_string());
            match inner.definitions.get_mut(&key) {
                Some((def, _)) => {
                    def.triggers_paused = paused;
                    Ok(true)
                }
                None => Ok(false),
            }
        }

        fn is_bundled(&self, name: &str) -> Result<bool, WorkflowError> {
            let inner = self.inner.lock().unwrap();
            Ok(inner.definitions.iter().any(|((n, _), (def, _))| n == name && def.bundled))
        }

        fn create_instance(&self, instance: &WorkflowInstance) -> Result<i64, WorkflowError> {
            let mut inner = self.inner.lock().unwrap();
            let id = inner.next_instance_id;
            inner.next_instance_id += 1;
            let mut inst = instance.clone();
            inst.id = id;
            inner.instances.insert(id, inst);
            Ok(id)
        }

        fn update_instance(&self, instance: &WorkflowInstance) -> Result<(), WorkflowError> {
            let mut inner = self.inner.lock().unwrap();
            inner.instances.insert(instance.id, instance.clone());
            Ok(())
        }

        fn get_instance(&self, id: i64) -> Result<Option<WorkflowInstance>, WorkflowError> {
            let inner = self.inner.lock().unwrap();
            Ok(inner.instances.get(&id).cloned())
        }

        fn list_instances(
            &self,
            filter: &InstanceFilter,
        ) -> Result<InstanceListResult, WorkflowError> {
            let inner = self.inner.lock().unwrap();
            let mut items: Vec<&WorkflowInstance> = inner.instances.values().collect();
            if !filter.statuses.is_empty() {
                items.retain(|i| filter.statuses.iter().any(|s| *s == i.status));
            }
            if !filter.definition_names.is_empty() {
                items.retain(|i| filter.definition_names.contains(&i.definition.name));
            }
            if let Some(ref def_id) = filter.definition_id {
                items.retain(|i| i.definition.id == *def_id);
            }
            if let Some(ref sid) = filter.parent_session_id {
                items.retain(|i| i.parent_session_id == *sid);
            }
            if let Some(ref aid) = filter.parent_agent_id {
                items.retain(|i| i.parent_agent_id.as_deref() == Some(aid.as_str()));
            }
            if let Some(ref mode) = filter.mode {
                items.retain(|i| i.definition.mode == *mode);
            }
            let total = items.len();
            items.sort_by(|a, b| b.created_at_ms.cmp(&a.created_at_ms));
            let offset = filter.offset.unwrap_or(0);
            let limit = filter.limit.unwrap_or(usize::MAX);
            let page: Vec<WorkflowInstanceSummary> =
                items.into_iter().skip(offset).take(limit).map(instance_to_summary).collect();
            Ok(InstanceListResult { items: page, total })
        }

        fn delete_instance(&self, id: i64) -> Result<bool, WorkflowError> {
            let mut inner = self.inner.lock().unwrap();
            Ok(inner.instances.remove(&id).is_some())
        }

        fn set_instance_archived(&self, _id: i64, _archived: bool) -> Result<bool, WorkflowError> {
            // In-memory store does not track archived flag; always return true.
            Ok(true)
        }

        fn list_waiting_feedback_for_session(
            &self,
            session_id: &str,
        ) -> Result<
            Vec<(i64, String, String, Option<String>, Option<String>, Option<bool>)>,
            WorkflowError,
        > {
            let inner = self.inner.lock().unwrap();
            let mut result = Vec::new();
            for inst in inner.instances.values() {
                if inst.parent_session_id != session_id {
                    continue;
                }
                for ss in inst.step_states.values() {
                    if ss.status == StepStatus::WaitingOnInput {
                        let def_json = serde_json::to_string(&inst.definition).unwrap_or_default();
                        let choices_json = ss
                            .interaction_choices
                            .as_ref()
                            .map(|c| serde_json::to_string(c).unwrap_or_default());
                        result.push((
                            inst.id,
                            ss.step_id.clone(),
                            def_json,
                            ss.interaction_prompt.clone(),
                            choices_json,
                            ss.interaction_allow_freeform,
                        ));
                    }
                }
            }
            Ok(result)
        }

        fn list_all_waiting_feedback(
            &self,
        ) -> Result<
            Vec<(i64, String, String, Option<String>, Option<String>, Option<bool>, String)>,
            WorkflowError,
        > {
            let inner = self.inner.lock().unwrap();
            let mut result = Vec::new();
            for inst in inner.instances.values() {
                for ss in inst.step_states.values() {
                    if ss.status == StepStatus::WaitingOnInput {
                        let def_json = serde_json::to_string(&inst.definition).unwrap_or_default();
                        let choices_json = ss
                            .interaction_choices
                            .as_ref()
                            .map(|c| serde_json::to_string(c).unwrap_or_default());
                        result.push((
                            inst.id,
                            ss.step_id.clone(),
                            def_json,
                            ss.interaction_prompt.clone(),
                            choices_json,
                            ss.interaction_allow_freeform,
                            inst.parent_session_id.clone(),
                        ));
                    }
                }
            }
            Ok(result)
        }

        fn list_child_agent_ids(
            &self,
        ) -> Result<std::collections::HashMap<i64, Vec<String>>, WorkflowError> {
            let inner = self.inner.lock().unwrap();
            let mut map: std::collections::HashMap<i64, Vec<String>> =
                std::collections::HashMap::new();
            for inst in inner.instances.values() {
                match inst.status {
                    WorkflowStatus::Running
                    | WorkflowStatus::WaitingOnInput
                    | WorkflowStatus::WaitingOnEvent => {}
                    _ => continue,
                }
                for ss in inst.step_states.values() {
                    if let Some(ref aid) = ss.child_agent_id {
                        map.entry(inst.id).or_default().push(aid.clone());
                    }
                }
            }
            Ok(map)
        }

        fn set_child_agent_id(
            &self,
            instance_id: i64,
            step_id: &str,
            agent_id: &str,
        ) -> Result<(), WorkflowError> {
            let mut inner = self.inner.lock().unwrap();
            if let Some(inst) = inner.instances.get_mut(&instance_id) {
                if let Some(ss) = inst.step_states.get_mut(step_id) {
                    ss.child_agent_id = Some(agent_id.to_string());
                }
            }
            Ok(())
        }

        fn is_trigger_seen(
            &self,
            definition_id: &str,
            external_id: &str,
        ) -> Result<bool, WorkflowError> {
            let inner = self.inner.lock().unwrap();
            Ok(inner
                .trigger_dedup
                .contains_key(&(definition_id.to_string(), external_id.to_string())))
        }

        fn mark_trigger_seen(
            &self,
            definition_id: &str,
            external_id: &str,
        ) -> Result<(), WorkflowError> {
            let mut inner = self.inner.lock().unwrap();
            inner
                .trigger_dedup
                .insert((definition_id.to_string(), external_id.to_string()), now_ms());
            Ok(())
        }

        fn prune_trigger_dedup(&self, max_age_ms: u64) -> Result<usize, WorkflowError> {
            let mut inner = self.inner.lock().unwrap();
            let now = now_ms();
            let before = inner.trigger_dedup.len();
            inner.trigger_dedup.retain(|_, ts| now - *ts < max_age_ms);
            Ok(before - inner.trigger_dedup.len())
        }

        fn get_cron_last_run(
            &self,
            definition_id: &str,
            definition_version: &str,
            cron_expression: &str,
        ) -> Result<Option<u64>, WorkflowError> {
            let inner = self.inner.lock().unwrap();
            Ok(inner
                .cron_state
                .get(&(
                    definition_id.to_string(),
                    definition_version.to_string(),
                    cron_expression.to_string(),
                ))
                .copied())
        }

        fn set_cron_last_run(
            &self,
            definition_id: &str,
            definition_version: &str,
            cron_expression: &str,
            last_run_ms: u64,
        ) -> Result<(), WorkflowError> {
            let mut inner = self.inner.lock().unwrap();
            inner.cron_state.insert(
                (
                    definition_id.to_string(),
                    definition_version.to_string(),
                    cron_expression.to_string(),
                ),
                last_run_ms,
            );
            Ok(())
        }

        fn delete_cron_state(
            &self,
            definition_id: &str,
            definition_version: Option<&str>,
        ) -> Result<(), WorkflowError> {
            let mut inner = self.inner.lock().unwrap();
            if let Some(version) = definition_version {
                inner.cron_state.retain(|(id, ver, _), _| !(id == definition_id && ver == version));
            } else {
                inner.cron_state.retain(|(id, _, _), _| id != definition_id);
            }
            Ok(())
        }

        fn get_event_replay_cursor(&self) -> Result<Option<u64>, WorkflowError> {
            let inner = self.inner.lock().unwrap();
            Ok(inner.event_replay_cursor)
        }

        fn set_event_replay_cursor(&self, timestamp_ms: u64) -> Result<(), WorkflowError> {
            let mut inner = self.inner.lock().unwrap();
            inner.event_replay_cursor = Some(timestamp_ms);
            Ok(())
        }

        fn prune_completed_instances(&self, max_age_ms: u64) -> Result<usize, WorkflowError> {
            let mut inner = self.inner.lock().unwrap();
            let cutoff = now_ms().saturating_sub(max_age_ms);
            let before = inner.instances.len();
            let removed_ids: Vec<i64> = inner
                .instances
                .iter()
                .filter(|(_, inst)| {
                    matches!(
                        inst.status,
                        WorkflowStatus::Completed | WorkflowStatus::Failed | WorkflowStatus::Killed
                    ) && inst.completed_at_ms.map_or(false, |t| t < cutoff)
                })
                .map(|(&id, _)| id)
                .collect();
            for id in &removed_ids {
                inner.instances.remove(id);
                inner.intercepted_actions.retain(|a| a.instance_id != *id);
            }
            Ok(before - inner.instances.len())
        }

        fn save_intercepted_action(&self, action: &InterceptedAction) -> Result<i64, WorkflowError> {
            let mut inner = self.inner.lock().unwrap();
            let id = inner.next_action_id;
            inner.next_action_id += 1;
            let mut stored = action.clone();
            stored.id = id;
            inner.intercepted_actions.push(stored);
            Ok(id)
        }

        fn list_intercepted_actions(
            &self,
            instance_id: i64,
            limit: usize,
            offset: usize,
        ) -> Result<InterceptedActionPage, WorkflowError> {
            let inner = self.inner.lock().unwrap();
            let matching: Vec<&InterceptedAction> = inner
                .intercepted_actions
                .iter()
                .filter(|a| a.instance_id == instance_id)
                .collect();
            let total = matching.len();
            let items: Vec<InterceptedAction> =
                matching.into_iter().skip(offset).take(limit).cloned().collect();
            Ok(InterceptedActionPage { items, total })
        }

        fn get_shadow_summary(&self, instance_id: i64) -> Result<ShadowSummary, WorkflowError> {
            let inner = self.inner.lock().unwrap();
            let mut summary = ShadowSummary::default();
            for action in &inner.intercepted_actions {
                if action.instance_id != instance_id {
                    continue;
                }
                summary.total_intercepted += 1;
                match action.kind.as_str() {
                    "tool_call" => summary.tool_calls_intercepted += 1,
                    "agent_invocation" => summary.agent_invocations_intercepted += 1,
                    "workflow_launch" => summary.workflow_launches_intercepted += 1,
                    "scheduled_task" => summary.scheduled_tasks_intercepted += 1,
                    "agent_signal" => summary.agent_signals_intercepted += 1,
                    _ => {}
                }
            }
            Ok(summary)
        }

        fn record_successful_run(
            &self,
            name: &str,
            version: &str,
            definition_hash: &str,
            run_at_ms: u64,
        ) -> Result<(), WorkflowError> {
            let mut inner = self.inner.lock().unwrap();
            let key = (name.to_string(), version.to_string());
            // Only update if this run is newer.
            match inner.successful_runs.get(&key) {
                Some((existing_ts, _)) if *existing_ts >= run_at_ms => {}
                _ => {
                    inner.successful_runs.insert(key, (run_at_ms, definition_hash.to_string()));
                }
            }
            Ok(())
        }
    }

    // -----------------------------------------------------------------------
    // Test helpers
    // -----------------------------------------------------------------------

    fn sample_definition() -> WorkflowDefinition {
        WorkflowDefinition {
            id: generate_workflow_id(),
            name: "test-workflow".into(),
            version: "1.0".into(),
            description: Some("A test workflow".into()),
            variables: serde_json::json!({"type": "object", "properties": {"result": {"type": "string"}}}),
            steps: vec![
                StepDef {
                    id: "start".into(),
                    step_type: StepType::Trigger {
                        trigger: TriggerDef {
                            trigger_type: TriggerType::Manual {
                                inputs: vec![],
                                input_schema: None,
                            },
                        },
                    },
                    outputs: HashMap::new(),
                    on_error: None,
                    next: vec!["end".into()],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                },
                StepDef {
                    id: "end".into(),
                    step_type: StepType::ControlFlow { control: ControlFlowDef::EndWorkflow },
                    outputs: HashMap::new(),
                    on_error: None,
                    next: vec![],
                    timeout_secs: None,
                    designer_x: None,
                    designer_y: None,
                },
            ],
            output: None,
            requested_tools: vec![],
            permissions: vec![],
            attachments: vec![],
            tests: vec![],
            mode: WorkflowMode::default(),
            result_message: None,
            bundled: false,
            archived: false,
            triggers_paused: false,
        }
    }

    fn sample_instance(def: &WorkflowDefinition) -> WorkflowInstance {
        let mut step_states = HashMap::new();
        for step in &def.steps {
            step_states.insert(
                step.id.clone(),
                StepState {
                    step_id: step.id.clone(),
                    status: StepStatus::Pending,
                    started_at_ms: None,
                    completed_at_ms: None,
                    outputs: None,
                    error: None,
                    retry_count: 0,
                    retry_delay_secs: None,
                    child_workflow_id: None,
                    child_agent_id: None,
                    interaction_request_id: None,
                    interaction_prompt: None,
                    interaction_choices: None,
                    interaction_allow_freeform: None,
                    resume_at_ms: None,
                },
            );
        }

        WorkflowInstance {
            id: 0,
            definition: def.clone(),
            status: WorkflowStatus::Running,
            variables: serde_json::json!({"result": ""}),
            step_states,
            parent_session_id: "session-123".into(),
            parent_agent_id: None,
            trigger_step_id: None,
            permissions: vec![],
            created_at_ms: 1000,
            updated_at_ms: 1000,
            completed_at_ms: None,
            output: None,
            error: None,
            workspace_path: None,
            resolved_result_message: None,
            goto_activated_steps: std::collections::HashSet::new(),
            goto_source_steps: std::collections::HashSet::new(),
            active_loops: std::collections::HashMap::new(),
            execution_mode: ExecutionMode::default(),
            shadow_overrides: std::collections::HashMap::new(),
        }
    }

    // -----------------------------------------------------------------------
    // Tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_in_memory_workflow_store_roundtrip() {
        let store = InMemoryWorkflowStore::new();
        let def = sample_definition();
        let yaml = serde_yaml::to_string(&def).unwrap();

        store.save_definition(&yaml, &def).unwrap();

        let (loaded, loaded_yaml) =
            store.get_definition("test-workflow", "1.0").unwrap().expect("definition should exist");
        assert_eq!(loaded.name, "test-workflow");
        assert_eq!(loaded.steps.len(), 2);
        assert_eq!(loaded_yaml, yaml);

        let list = store.list_definitions().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "test-workflow");

        let inst = sample_instance(&def);
        let inst_id = store.create_instance(&inst).unwrap();

        let loaded_inst = store.get_instance(inst_id).unwrap().expect("instance should exist");
        assert_eq!(loaded_inst.id, inst_id);
        assert_eq!(loaded_inst.parent_session_id, "session-123");

        let result = store.list_instances(&InstanceFilter::default()).unwrap();
        assert_eq!(result.total, 1);

        let deleted = store.delete_instance(inst_id).unwrap();
        assert!(deleted);
        assert!(store.get_instance(inst_id).unwrap().is_none());
        let deleted_again = store.delete_instance(inst_id).unwrap();
        assert!(!deleted_again);
    }

    #[test]
    fn test_definition_crud() {
        let store = WorkflowStore::in_memory().unwrap();
        let def = sample_definition();
        let yaml = serde_yaml::to_string(&def).unwrap();

        // Save
        store.save_definition(&yaml, &def).unwrap();

        // Get
        let (loaded, loaded_yaml) = store.get_definition("test-workflow", "1.0").unwrap().unwrap();
        assert_eq!(loaded.name, "test-workflow");
        assert_eq!(loaded.steps.len(), 2);
        assert!(!loaded_yaml.is_empty());

        // List
        let list = store.list_definitions().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "test-workflow");
        assert_eq!(list[0].step_count, 2);

        // Delete
        assert!(store.delete_definition("test-workflow", "1.0").unwrap());
        assert!(store.get_definition("test-workflow", "1.0").unwrap().is_none());
    }

    #[test]
    fn test_instance_crud() {
        let store = WorkflowStore::in_memory().unwrap();
        let def = sample_definition();
        let instance = sample_instance(&def);

        // Create
        let inst_id = store.create_instance(&instance).unwrap();

        // Get
        let loaded = store.get_instance(inst_id).unwrap().unwrap();
        assert_eq!(loaded.id, inst_id);
        assert_eq!(loaded.status, WorkflowStatus::Running);
        assert_eq!(loaded.step_states.len(), 2);
        assert_eq!(loaded.parent_session_id, "session-123");

        // Update
        let mut updated = loaded;
        updated.status = WorkflowStatus::Completed;
        updated.completed_at_ms = Some(2000);
        updated.updated_at_ms = 2000;
        updated.step_states.get_mut("start").unwrap().status = StepStatus::Completed;
        store.update_instance(&updated).unwrap();

        let reloaded = store.get_instance(inst_id).unwrap().unwrap();
        assert_eq!(reloaded.status, WorkflowStatus::Completed);
        assert_eq!(reloaded.completed_at_ms, Some(2000));
        assert_eq!(reloaded.step_states["start"].status, StepStatus::Completed);

        // List
        let list = store.list_instances(&InstanceFilter::default()).unwrap();
        assert_eq!(list.items.len(), 1);
        assert_eq!(list.items[0].id, inst_id);

        // List with filter
        let filtered = store
            .list_instances(&InstanceFilter {
                statuses: vec![WorkflowStatus::Completed],
                ..Default::default()
            })
            .unwrap();
        assert_eq!(filtered.items.len(), 1);

        let empty = store
            .list_instances(&InstanceFilter {
                statuses: vec![WorkflowStatus::Running],
                ..Default::default()
            })
            .unwrap();
        assert_eq!(empty.items.len(), 0);

        // Delete
        assert!(store.delete_instance(inst_id).unwrap());
        assert!(store.get_instance(inst_id).unwrap().is_none());
    }

    #[test]
    fn test_get_latest_definition() {
        let store = WorkflowStore::in_memory().unwrap();
        let mut def = sample_definition();
        let yaml1 = serde_yaml::to_string(&def).unwrap();
        store.save_definition(&yaml1, &def).unwrap();

        def.version = "2.0".into();
        def.description = Some("Updated".into());
        let yaml2 = serde_yaml::to_string(&def).unwrap();
        store.save_definition(&yaml2, &def).unwrap();

        let (latest, _) = store.get_latest_definition("test-workflow").unwrap().unwrap();
        assert_eq!(latest.version, "2.0");
        assert_eq!(latest.description, Some("Updated".into()));
    }

    #[test]
    fn test_definition_id_preserved_across_versions() {
        let store = WorkflowStore::in_memory().unwrap();
        let mut def = sample_definition();
        let yaml1 = serde_yaml::to_string(&def).unwrap();
        store.save_definition(&yaml1, &def).unwrap();

        let (loaded1, _) = store.get_definition("test-workflow", "1.0").unwrap().unwrap();
        let original_id = loaded1.id.clone();

        // Save a new version with a different id on the input struct.
        def.version = "2.0".into();
        def.id = generate_workflow_id(); // new id
        let yaml2 = serde_yaml::to_string(&def).unwrap();
        store.save_definition(&yaml2, &def).unwrap();

        // Both versions should share the same external_id (the one set first).
        let (loaded2, _) = store.get_definition("test-workflow", "2.0").unwrap().unwrap();
        assert_eq!(loaded2.id, original_id);

        // get_definition_by_id should return the latest version.
        let (by_id, _) = store.get_definition_by_id(&original_id).unwrap().unwrap();
        assert_eq!(by_id.version, "2.0");
    }

    #[test]
    fn test_definition_delete_last_version_removes_parent() {
        let store = WorkflowStore::in_memory().unwrap();
        let mut def = sample_definition();
        let yaml1 = serde_yaml::to_string(&def).unwrap();
        store.save_definition(&yaml1, &def).unwrap();

        def.version = "2.0".into();
        let yaml2 = serde_yaml::to_string(&def).unwrap();
        store.save_definition(&yaml2, &def).unwrap();

        // Delete version 1.0 -- parent remains.
        assert!(store.delete_definition("test-workflow", "1.0").unwrap());
        assert!(store.get_latest_definition("test-workflow").unwrap().is_some());

        // Delete version 2.0 -- parent is also removed.
        assert!(store.delete_definition("test-workflow", "2.0").unwrap());
        assert!(store.get_latest_definition("test-workflow").unwrap().is_none());
        assert!(store.is_bundled("test-workflow").unwrap() == false);
    }

    #[test]
    fn test_prune_completed_instances() {
        let store = WorkflowStore::in_memory().unwrap();
        let def = sample_definition();
        let mut inst = sample_instance(&def);
        inst.status = WorkflowStatus::Completed;
        inst.completed_at_ms = Some(1000);
        let inst_id = store.create_instance(&inst).unwrap();

        // Pruning with a cutoff that includes the instance.
        let pruned = store.prune_completed_instances(500).unwrap();
        assert_eq!(pruned, 1);
        assert!(store.get_instance(inst_id).unwrap().is_none());
    }

    #[test]
    fn test_list_instances_mode_filter() {
        let store = WorkflowStore::in_memory().unwrap();
        let def = sample_definition(); // default mode is Background
        let inst = sample_instance(&def);
        store.create_instance(&inst).unwrap();

        let bg = store
            .list_instances(&InstanceFilter {
                mode: Some(WorkflowMode::Background),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(bg.items.len(), 1);

        let chat = store
            .list_instances(&InstanceFilter {
                mode: Some(WorkflowMode::Chat),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(chat.items.len(), 0);
    }

    #[test]
    fn test_cron_state_scoped_by_definition_id_and_version() {
        let store = WorkflowStore::in_memory().unwrap();

        store.set_cron_last_run("def-1", "1.0", "*/5 * * * * *", 100).unwrap();
        store.set_cron_last_run("def-1", "2.0", "*/5 * * * * *", 200).unwrap();
        store.set_cron_last_run("def-2", "1.0", "*/5 * * * * *", 300).unwrap();

        assert_eq!(store.get_cron_last_run("def-1", "1.0", "*/5 * * * * *").unwrap(), Some(100));
        assert_eq!(store.get_cron_last_run("def-1", "2.0", "*/5 * * * * *").unwrap(), Some(200));
        assert_eq!(store.get_cron_last_run("def-2", "1.0", "*/5 * * * * *").unwrap(), Some(300));

        store.delete_cron_state("def-1", Some("1.0")).unwrap();
        assert_eq!(store.get_cron_last_run("def-1", "1.0", "*/5 * * * * *").unwrap(), None);
        assert_eq!(store.get_cron_last_run("def-1", "2.0", "*/5 * * * * *").unwrap(), Some(200));

        store.delete_cron_state("def-1", None).unwrap();
        assert_eq!(store.get_cron_last_run("def-1", "2.0", "*/5 * * * * *").unwrap(), None);
        assert_eq!(store.get_cron_last_run("def-2", "1.0", "*/5 * * * * *").unwrap(), Some(300));
    }

    #[test]
    fn test_event_replay_cursor_roundtrip() {
        let store = WorkflowStore::in_memory().unwrap();

        assert_eq!(store.get_event_replay_cursor().unwrap(), None);

        store.set_event_replay_cursor(1234).unwrap();
        assert_eq!(store.get_event_replay_cursor().unwrap(), Some(1234));

        store.set_event_replay_cursor(5678).unwrap();
        assert_eq!(store.get_event_replay_cursor().unwrap(), Some(5678));
    }

    // -----------------------------------------------------------------------
    // list_definitions: latest-version-only tests
    // -----------------------------------------------------------------------

    /// Helper: create a definition with a given name and version.
    fn def_with_name_version(name: &str, version: &str) -> WorkflowDefinition {
        let mut def = sample_definition();
        def.name = name.into();
        def.version = version.into();
        def.id = generate_workflow_id();
        def
    }

    #[test]
    fn test_list_definitions_returns_latest_version_only() {
        let store = WorkflowStore::in_memory().unwrap();

        let def_v1 = def_with_name_version("wf-a", "1.0");
        store.save_definition(&serde_yaml::to_string(&def_v1).unwrap(), &def_v1).unwrap();

        let mut def_v2 = def_with_name_version("wf-a", "2.0");
        def_v2.id = def_v1.id.clone(); // same workflow identity
        store.save_definition(&serde_yaml::to_string(&def_v2).unwrap(), &def_v2).unwrap();

        let list = store.list_definitions().unwrap();
        assert_eq!(list.len(), 1, "should return only one entry per workflow");
        assert_eq!(list[0].version, "2.0");
    }

    #[test]
    fn test_list_definitions_multiple_workflows_latest_only() {
        let store = WorkflowStore::in_memory().unwrap();

        // Workflow A: versions 1.0 and 2.0
        let a_v1 = def_with_name_version("wf-a", "1.0");
        store.save_definition(&serde_yaml::to_string(&a_v1).unwrap(), &a_v1).unwrap();
        let mut a_v2 = def_with_name_version("wf-a", "2.0");
        a_v2.id = a_v1.id.clone();
        store.save_definition(&serde_yaml::to_string(&a_v2).unwrap(), &a_v2).unwrap();

        // Workflow B: versions 1.0 and 3.0
        let b_v1 = def_with_name_version("wf-b", "1.0");
        store.save_definition(&serde_yaml::to_string(&b_v1).unwrap(), &b_v1).unwrap();
        let mut b_v3 = def_with_name_version("wf-b", "3.0");
        b_v3.id = b_v1.id.clone();
        store.save_definition(&serde_yaml::to_string(&b_v3).unwrap(), &b_v3).unwrap();

        let list = store.list_definitions().unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "wf-a");
        assert_eq!(list[0].version, "2.0");
        assert_eq!(list[1].name, "wf-b");
        assert_eq!(list[1].version, "3.0");
    }

    #[test]
    fn test_list_definitions_semver_ordering() {
        let store = WorkflowStore::in_memory().unwrap();

        // Save versions out of order: "1.0", "10.0", "2.0"
        // Lexicographic would pick "2.0"; correct answer is "10.0".
        let v1 = def_with_name_version("wf-semver", "1.0");
        store.save_definition(&serde_yaml::to_string(&v1).unwrap(), &v1).unwrap();
        let mut v10 = def_with_name_version("wf-semver", "10.0");
        v10.id = v1.id.clone();
        store.save_definition(&serde_yaml::to_string(&v10).unwrap(), &v10).unwrap();
        let mut v2 = def_with_name_version("wf-semver", "2.0");
        v2.id = v1.id.clone();
        store.save_definition(&serde_yaml::to_string(&v2).unwrap(), &v2).unwrap();

        let list = store.list_definitions().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].version, "10.0", "should pick 10.0, not 2.0 (lexicographic)");
    }

    #[test]
    fn test_list_definitions_multi_digit_minor() {
        let store = WorkflowStore::in_memory().unwrap();

        // "1.9" < "1.10" < "1.100" numerically
        let v9 = def_with_name_version("wf-minor", "1.9");
        store.save_definition(&serde_yaml::to_string(&v9).unwrap(), &v9).unwrap();
        let mut v10 = def_with_name_version("wf-minor", "1.10");
        v10.id = v9.id.clone();
        store.save_definition(&serde_yaml::to_string(&v10).unwrap(), &v10).unwrap();
        let mut v100 = def_with_name_version("wf-minor", "1.100");
        v100.id = v9.id.clone();
        store.save_definition(&serde_yaml::to_string(&v100).unwrap(), &v100).unwrap();

        let list = store.list_definitions().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].version, "1.100", "should pick 1.100 (numeric, not lexicographic)");
    }

    #[test]
    fn test_list_definitions_three_part_versions() {
        let store = WorkflowStore::in_memory().unwrap();

        let v100 = def_with_name_version("wf-3part", "1.0.0");
        store.save_definition(&serde_yaml::to_string(&v100).unwrap(), &v100).unwrap();
        let mut v110 = def_with_name_version("wf-3part", "1.1.0");
        v110.id = v100.id.clone();
        store.save_definition(&serde_yaml::to_string(&v110).unwrap(), &v110).unwrap();
        let mut v101 = def_with_name_version("wf-3part", "1.0.1");
        v101.id = v100.id.clone();
        store.save_definition(&serde_yaml::to_string(&v101).unwrap(), &v101).unwrap();

        let list = store.list_definitions().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].version, "1.1.0", "1.1.0 > 1.0.1 > 1.0.0");
    }

    #[test]
    fn test_list_definitions_single_version() {
        let store = WorkflowStore::in_memory().unwrap();

        let def = def_with_name_version("wf-single", "1.0");
        store.save_definition(&serde_yaml::to_string(&def).unwrap(), &def).unwrap();

        let list = store.list_definitions().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "wf-single");
        assert_eq!(list[0].version, "1.0");
    }

    #[test]
    fn test_list_definitions_archived_latest() {
        let store = WorkflowStore::in_memory().unwrap();

        let v1 = def_with_name_version("wf-arch", "1.0");
        store.save_definition(&serde_yaml::to_string(&v1).unwrap(), &v1).unwrap();

        let mut v2 = def_with_name_version("wf-arch", "2.0");
        v2.id = v1.id.clone();
        store.save_definition(&serde_yaml::to_string(&v2).unwrap(), &v2).unwrap();
        store.set_archived("wf-arch", "2.0", true).unwrap();

        let list = store.list_definitions().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].version, "2.0");
        assert!(list[0].archived, "latest version should report archived=true");
    }
}
