use anyhow::{Context, Result};
use hive_contracts::{InferenceRuntimeKind, ModelTask};
use parking_lot::Mutex;
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;

// Re-export shared DTOs from hive-contracts
pub use hive_contracts::{InferenceParams, InstalledModel, ModelCapabilities, ModelStatus};

#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("model not found: {model_id}")]
    NotFound { model_id: String },
    #[error("model already exists: {model_id}")]
    AlreadyExists { model_id: String },
    #[error("database error: {0}")]
    Database(String),
}

// ---------------------------------------------------------------------------
// ModelRegistryStore trait
// ---------------------------------------------------------------------------

/// Abstract interface for a local model registry.
///
/// All methods are synchronous and the trait requires `Send + Sync` so
/// implementations can be shared across threads behind an `Arc`.
pub trait ModelRegistryStore: Send + Sync {
    fn insert(&self, model: &InstalledModel) -> Result<(), RegistryError>;
    fn get(&self, model_id: &str) -> Result<InstalledModel, RegistryError>;
    fn list(&self) -> Result<Vec<InstalledModel>, RegistryError>;
    fn list_by_runtime(
        &self,
        runtime: InferenceRuntimeKind,
    ) -> Result<Vec<InstalledModel>, RegistryError>;
    fn list_by_task(&self, task: ModelTask) -> Result<Vec<InstalledModel>, RegistryError>;
    fn update_status(&self, model_id: &str, status: ModelStatus) -> Result<(), RegistryError>;
    fn update_details(
        &self,
        model_id: &str,
        local_path: &Path,
        sha256: &str,
        size_bytes: u64,
    ) -> Result<(), RegistryError>;
    fn update_inference_params(
        &self,
        model_id: &str,
        params: &InferenceParams,
    ) -> Result<(), RegistryError>;
    fn remove(&self, model_id: &str) -> Result<(), RegistryError>;
    fn total_size_bytes(&self) -> Result<u64, RegistryError>;
}

// ---------------------------------------------------------------------------
// SqliteModelRegistry
// ---------------------------------------------------------------------------

pub struct SqliteModelRegistry {
    conn: Arc<Mutex<Connection>>,
}

impl Clone for SqliteModelRegistry {
    fn clone(&self) -> Self {
        Self { conn: Arc::clone(&self.conn) }
    }
}

/// Backward-compatible alias.
pub type LocalModelRegistry = SqliteModelRegistry;

impl SqliteModelRegistry {
    pub fn open(db_path: &Path) -> Result<Self> {
        let conn = Connection::open(db_path).with_context(|| {
            format!("failed to open local model registry at {}", db_path.display())
        })?;
        let registry = Self { conn: Arc::new(Mutex::new(conn)) };
        registry.init_schema()?;
        Ok(registry)
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()
            .context("failed to open in-memory local model registry")?;
        let registry = Self { conn: Arc::new(Mutex::new(conn)) };
        registry.init_schema()?;
        Ok(registry)
    }

    fn init_schema(&self) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS installed_models (
                id             TEXT PRIMARY KEY,
                hub_repo       TEXT NOT NULL,
                filename       TEXT NOT NULL,
                runtime        TEXT NOT NULL,
                capabilities   TEXT NOT NULL DEFAULT '{}',
                status         TEXT NOT NULL DEFAULT 'available',
                size_bytes     INTEGER NOT NULL DEFAULT 0,
                local_path     TEXT NOT NULL,
                sha256         TEXT,
                installed_at   TEXT NOT NULL DEFAULT (datetime('now')),
                inference_params TEXT NOT NULL DEFAULT '{}'
            );
            CREATE INDEX IF NOT EXISTS idx_models_runtime ON installed_models(runtime);
            CREATE INDEX IF NOT EXISTS idx_models_status ON installed_models(status);
            ",
        )
        .context("failed to initialize local model registry schema")?;

        // Migration: add inference_params column if it doesn't exist (for pre-existing DBs)
        conn.execute_batch(
            "ALTER TABLE installed_models ADD COLUMN inference_params TEXT NOT NULL DEFAULT '{}';",
        )
        .ok();

        Ok(())
    }
}

impl ModelRegistryStore for SqliteModelRegistry {
    fn insert(&self, model: &InstalledModel) -> Result<(), RegistryError> {
        let conn = self.conn.lock();
        let capabilities_json = serde_json::to_string(&model.capabilities)
            .map_err(|e| RegistryError::Database(e.to_string()))?;
        let runtime_str = runtime_to_str(model.runtime);
        let status_str = status_to_str(model.status);
        let local_path_str = model.local_path.to_string_lossy().to_string();
        let params_json = serde_json::to_string(&model.inference_params)
            .map_err(|e| RegistryError::Database(e.to_string()))?;

        conn.execute(
            "INSERT OR REPLACE INTO installed_models (id, hub_repo, filename, runtime, capabilities, status, size_bytes, local_path, sha256, installed_at, inference_params)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                model.id,
                model.hub_repo,
                model.filename,
                runtime_str,
                capabilities_json,
                status_str,
                model.size_bytes as i64,
                local_path_str,
                model.sha256,
                model.installed_at,
                params_json,
            ],
        ).map_err(|e| RegistryError::Database(e.to_string()))?;
        Ok(())
    }

    fn get(&self, model_id: &str) -> Result<InstalledModel, RegistryError> {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT id, hub_repo, filename, runtime, capabilities, status, size_bytes, local_path, sha256, installed_at, inference_params
             FROM installed_models WHERE id = ?1",
            params![model_id],
            |row| {
                Ok(row_to_model(row))
            },
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => RegistryError::NotFound { model_id: model_id.to_string() },
            other => RegistryError::Database(other.to_string()),
        })
    }

    fn list(&self) -> Result<Vec<InstalledModel>, RegistryError> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, hub_repo, filename, runtime, capabilities, status, size_bytes, local_path, sha256, installed_at, inference_params
             FROM installed_models ORDER BY installed_at DESC"
        ).map_err(|e| RegistryError::Database(e.to_string()))?;
        let models = stmt
            .query_map([], |row| Ok(row_to_model(row)))
            .map_err(|e| RegistryError::Database(e.to_string()))?
            .filter_map(|r| r.map_err(|e| tracing::warn!("skipping corrupt model row: {e}")).ok())
            .collect::<Vec<_>>();
        Ok(models)
    }

    fn list_by_runtime(
        &self,
        runtime: InferenceRuntimeKind,
    ) -> Result<Vec<InstalledModel>, RegistryError> {
        let conn = self.conn.lock();
        let runtime_str = runtime_to_str(runtime);
        let mut stmt = conn.prepare(
            "SELECT id, hub_repo, filename, runtime, capabilities, status, size_bytes, local_path, sha256, installed_at, inference_params
             FROM installed_models WHERE runtime = ?1 ORDER BY installed_at DESC"
        ).map_err(|e| RegistryError::Database(e.to_string()))?;
        let models = stmt
            .query_map(params![runtime_str], |row| Ok(row_to_model(row)))
            .map_err(|e| RegistryError::Database(e.to_string()))?
            .filter_map(|r| r.map_err(|e| tracing::warn!("skipping corrupt model row: {e}")).ok())
            .collect::<Vec<_>>();
        Ok(models)
    }

    fn list_by_task(&self, task: ModelTask) -> Result<Vec<InstalledModel>, RegistryError> {
        let task_str = serde_json::to_string(&task).unwrap_or_default();
        let task_key = task_str.trim_matches('"');
        let pattern = format!("%\"{task_key}\"%");
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, hub_repo, filename, runtime, capabilities, status, size_bytes, local_path, sha256, installed_at, inference_params
             FROM installed_models WHERE capabilities LIKE ?1 AND status != 'removed' ORDER BY installed_at DESC"
        ).map_err(|e| RegistryError::Database(e.to_string()))?;
        let models = stmt
            .query_map(params![pattern], |row| Ok(row_to_model(row)))
            .map_err(|e| RegistryError::Database(e.to_string()))?
            .filter_map(|r| r.map_err(|e| tracing::warn!("skipping corrupt model row: {e}")).ok())
            .collect::<Vec<_>>();
        Ok(models)
    }

    fn update_status(&self, model_id: &str, status: ModelStatus) -> Result<(), RegistryError> {
        let conn = self.conn.lock();
        let status_str = status_to_str(status);
        let updated = conn
            .execute(
                "UPDATE installed_models SET status = ?1 WHERE id = ?2",
                params![status_str, model_id],
            )
            .map_err(|e| RegistryError::Database(e.to_string()))?;
        if updated == 0 {
            return Err(RegistryError::NotFound { model_id: model_id.to_string() });
        }
        Ok(())
    }

    fn update_details(
        &self,
        model_id: &str,
        local_path: &Path,
        sha256: &str,
        size_bytes: u64,
    ) -> Result<(), RegistryError> {
        let conn = self.conn.lock();
        let updated = conn.execute(
            "UPDATE installed_models SET local_path = ?1, sha256 = ?2, size_bytes = ?3 WHERE id = ?4",
            params![local_path.to_string_lossy().as_ref(), sha256, size_bytes as i64, model_id],
        ).map_err(|e| RegistryError::Database(e.to_string()))?;
        if updated == 0 {
            return Err(RegistryError::NotFound { model_id: model_id.to_string() });
        }
        Ok(())
    }

    fn update_inference_params(
        &self,
        model_id: &str,
        params: &InferenceParams,
    ) -> Result<(), RegistryError> {
        let conn = self.conn.lock();
        let params_json =
            serde_json::to_string(params).map_err(|e| RegistryError::Database(e.to_string()))?;
        let updated = conn
            .execute(
                "UPDATE installed_models SET inference_params = ?1 WHERE id = ?2",
                params![params_json, model_id],
            )
            .map_err(|e| RegistryError::Database(e.to_string()))?;
        if updated == 0 {
            return Err(RegistryError::NotFound { model_id: model_id.to_string() });
        }
        Ok(())
    }

    fn remove(&self, model_id: &str) -> Result<(), RegistryError> {
        let conn = self.conn.lock();
        let deleted = conn
            .execute("DELETE FROM installed_models WHERE id = ?1", params![model_id])
            .map_err(|e| RegistryError::Database(e.to_string()))?;
        if deleted == 0 {
            return Err(RegistryError::NotFound { model_id: model_id.to_string() });
        }
        Ok(())
    }

    fn total_size_bytes(&self) -> Result<u64, RegistryError> {
        let conn = self.conn.lock();
        let size: i64 = conn.query_row(
            "SELECT COALESCE(SUM(size_bytes), 0) FROM installed_models WHERE status != 'removed'",
            [],
            |row| row.get(0),
        ).map_err(|e| RegistryError::Database(e.to_string()))?;
        Ok(size as u64)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn runtime_to_str(kind: InferenceRuntimeKind) -> &'static str {
    match kind {
        InferenceRuntimeKind::Candle => "candle",
        InferenceRuntimeKind::Onnx => "onnx",
        InferenceRuntimeKind::LlamaCpp => "llama-cpp",
    }
}

fn str_to_runtime(s: &str) -> InferenceRuntimeKind {
    match s {
        "candle" => InferenceRuntimeKind::Candle,
        "onnx" => InferenceRuntimeKind::Onnx,
        "llama-cpp" | "llama_cpp" => InferenceRuntimeKind::LlamaCpp,
        _ => InferenceRuntimeKind::LlamaCpp,
    }
}

fn status_to_str(status: ModelStatus) -> &'static str {
    match status {
        ModelStatus::Available => "available",
        ModelStatus::Downloading => "downloading",
        ModelStatus::Error => "error",
        ModelStatus::Removed => "removed",
    }
}

fn str_to_status(s: &str) -> ModelStatus {
    match s {
        "available" => ModelStatus::Available,
        "downloading" => ModelStatus::Downloading,
        "error" => ModelStatus::Error,
        "removed" => ModelStatus::Removed,
        _ => ModelStatus::Error,
    }
}

fn row_to_model(row: &rusqlite::Row<'_>) -> InstalledModel {
    let runtime_str: String = row.get(3).unwrap_or_default();
    let capabilities_json: String = row.get(4).unwrap_or_else(|_| "{}".to_string());
    let status_str: String = row.get(5).unwrap_or_default();
    let local_path_str: String = row.get(7).unwrap_or_default();
    let params_json: String = row.get(10).unwrap_or_else(|_| "{}".to_string());

    InstalledModel {
        id: row.get(0).unwrap_or_default(),
        hub_repo: row.get(1).unwrap_or_default(),
        filename: row.get(2).unwrap_or_default(),
        runtime: str_to_runtime(&runtime_str),
        capabilities: serde_json::from_str(&capabilities_json).unwrap_or_default(),
        status: str_to_status(&status_str),
        size_bytes: row.get::<_, i64>(6).unwrap_or(0) as u64,
        local_path: PathBuf::from(local_path_str),
        sha256: row.get(8).ok(),
        installed_at: row.get(9).unwrap_or_default(),
        inference_params: serde_json::from_str(&params_json).unwrap_or_default(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_model(id: &str) -> InstalledModel {
        InstalledModel {
            id: id.to_string(),
            hub_repo: "TheBloke/Llama-2-7B-GGUF".to_string(),
            filename: "llama-2-7b.Q4_K_M.gguf".to_string(),
            runtime: InferenceRuntimeKind::LlamaCpp,
            capabilities: ModelCapabilities {
                tasks: vec![ModelTask::Chat, ModelTask::TextGeneration],
                can_call_tools: false,
                has_reasoning: false,
                context_length: Some(4096),
                parameter_count: Some("7B".to_string()),
            },
            status: ModelStatus::Available,
            size_bytes: 4_000_000_000,
            local_path: PathBuf::from("/models/llama-2-7b.Q4_K_M.gguf"),
            sha256: Some("abc123".to_string()),
            installed_at: "2025-01-15T10:30:00Z".to_string(),
            inference_params: InferenceParams::default(),
        }
    }

    #[test]
    fn insert_and_get_model() {
        let registry = LocalModelRegistry::open_in_memory().unwrap();
        let model = sample_model("test-llama");
        registry.insert(&model).unwrap();

        let fetched = registry.get("test-llama").unwrap();
        assert_eq!(fetched.id, "test-llama");
        assert_eq!(fetched.hub_repo, "TheBloke/Llama-2-7B-GGUF");
        assert_eq!(fetched.capabilities.tasks.len(), 2);
        assert_eq!(fetched.capabilities.context_length, Some(4096));
    }

    #[test]
    fn list_models() {
        let registry = LocalModelRegistry::open_in_memory().unwrap();
        registry.insert(&sample_model("m1")).unwrap();
        registry.insert(&sample_model("m2")).unwrap();

        let models = registry.list().unwrap();
        assert_eq!(models.len(), 2);
    }

    #[test]
    fn list_by_runtime() {
        let registry = LocalModelRegistry::open_in_memory().unwrap();
        let mut onnx_model = sample_model("onnx-m1");
        onnx_model.runtime = InferenceRuntimeKind::Onnx;
        registry.insert(&sample_model("llama-m1")).unwrap();
        registry.insert(&onnx_model).unwrap();

        let llama_models = registry.list_by_runtime(InferenceRuntimeKind::LlamaCpp).unwrap();
        assert_eq!(llama_models.len(), 1);
        assert_eq!(llama_models[0].id, "llama-m1");

        let onnx_models = registry.list_by_runtime(InferenceRuntimeKind::Onnx).unwrap();
        assert_eq!(onnx_models.len(), 1);
    }

    #[test]
    fn list_by_task() {
        let registry = LocalModelRegistry::open_in_memory().unwrap();
        let mut embed_model = sample_model("embed-m");
        embed_model.capabilities.tasks = vec![ModelTask::Embedding];
        registry.insert(&sample_model("chat-m")).unwrap();
        registry.insert(&embed_model).unwrap();

        let chat_models = registry.list_by_task(ModelTask::Chat).unwrap();
        assert_eq!(chat_models.len(), 1);
        assert_eq!(chat_models[0].id, "chat-m");

        let embed_models = registry.list_by_task(ModelTask::Embedding).unwrap();
        assert_eq!(embed_models.len(), 1);
        assert_eq!(embed_models[0].id, "embed-m");
    }

    #[test]
    fn update_status() {
        let registry = LocalModelRegistry::open_in_memory().unwrap();
        registry.insert(&sample_model("test")).unwrap();

        registry.update_status("test", ModelStatus::Downloading).unwrap();
        let m = registry.get("test").unwrap();
        assert_eq!(m.status, ModelStatus::Downloading);
    }

    #[test]
    fn remove_model() {
        let registry = LocalModelRegistry::open_in_memory().unwrap();
        registry.insert(&sample_model("to-remove")).unwrap();

        registry.remove("to-remove").unwrap();
        let result = registry.get("to-remove");
        assert!(result.is_err());
    }

    #[test]
    fn total_size() {
        let registry = LocalModelRegistry::open_in_memory().unwrap();
        let mut m1 = sample_model("m1");
        m1.size_bytes = 1000;
        let mut m2 = sample_model("m2");
        m2.size_bytes = 2000;
        registry.insert(&m1).unwrap();
        registry.insert(&m2).unwrap();

        assert_eq!(registry.total_size_bytes().unwrap(), 3000);
    }

    #[test]
    fn insert_duplicate_replaces() {
        let registry = LocalModelRegistry::open_in_memory().unwrap();
        let m1 = sample_model("dup");
        registry.insert(&m1).unwrap();

        // Re-inserting the same id should succeed (INSERT OR REPLACE).
        let mut m2 = sample_model("dup");
        m2.size_bytes = 9999;
        registry.insert(&m2).unwrap();

        let fetched = registry.get("dup").unwrap();
        assert_eq!(fetched.size_bytes, 9999);

        // Should still be one row, not two.
        let all = registry.list().unwrap();
        assert_eq!(all.len(), 1);
    }

    #[test]
    fn get_nonexistent_returns_not_found() {
        let registry = LocalModelRegistry::open_in_memory().unwrap();
        let result = registry.get("no-such-model");
        assert!(matches!(result, Err(RegistryError::NotFound { .. })));
    }
}
