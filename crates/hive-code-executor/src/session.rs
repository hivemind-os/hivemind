//! Session management for persistent code execution environments.
//!
//! Each conversation gets a [`Session`] that owns a [`CodeExecutor`] instance.
//! The [`SessionRegistry`] manages the lifecycle of sessions: creation, lookup,
//! idle reaping, and LRU eviction.

use crate::executor::{CodeExecutor, ExecutorConfig, ExecutorError};
use crate::wasm_executor::WasmExecutor;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use wasmtime::{Engine, Module};

/// Configuration for a code execution session.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    /// Executor-level configuration (timeouts, memory, etc.).
    pub executor: ExecutorConfig,
    /// Idle timeout: session is reaped after this duration of inactivity.
    pub idle_timeout: Duration,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            executor: ExecutorConfig::default(),
            idle_timeout: Duration::from_secs(600), // 10 minutes
        }
    }
}

/// Shared resources for WASM executor creation (engine + compiled module).
/// Created once and shared across all sessions for efficiency.
pub struct WasmRuntime {
    pub engine: Arc<Engine>,
    pub module: Arc<Module>,
    pub stdlib_dir: PathBuf,
}

/// A single code execution session tied to a conversation.
pub struct Session {
    pub id: String,
    executor: Arc<dyn CodeExecutor>,
    last_activity: Mutex<Instant>,
    idle_timeout: Duration,
}

impl Session {
    /// Create a new session with a WASM-sandboxed executor.
    pub async fn new_wasm(
        id: String,
        config: SessionConfig,
        runtime: &WasmRuntime,
    ) -> Result<Self, ExecutorError> {
        let executor = WasmExecutor::with_shared(
            config.executor,
            Arc::clone(&runtime.engine),
            Arc::clone(&runtime.module),
            runtime.stdlib_dir.clone(),
        );
        // Spawn the initial WASM instance
        executor.ensure_instance().await?;

        Ok(Self {
            id,
            executor: Arc::new(executor),
            last_activity: Mutex::new(Instant::now()),
            idle_timeout: config.idle_timeout,
        })
    }

    /// Create a new session with a subprocess-based executor (fallback).
    pub async fn new_subprocess(id: String, config: SessionConfig) -> Result<Self, ExecutorError> {
        let executor = crate::subprocess::SubprocessExecutor::new(config.executor).await?;
        Ok(Self {
            id,
            executor: Arc::new(executor),
            last_activity: Mutex::new(Instant::now()),
            idle_timeout: config.idle_timeout,
        })
    }

    /// Get a reference to the underlying code executor.
    pub fn executor(&self) -> &dyn CodeExecutor {
        self.executor.as_ref()
    }

    /// Get a cloned `Arc` of the underlying code executor.
    pub fn executor_arc(&self) -> Arc<dyn CodeExecutor> {
        Arc::clone(&self.executor)
    }

    /// Touch the session (update last activity timestamp).
    pub fn touch(&self) {
        *self.last_activity.lock() = Instant::now();
    }

    /// Check if the session has been idle longer than its timeout.
    pub fn is_idle(&self) -> bool {
        self.last_activity.lock().elapsed() > self.idle_timeout
    }

    /// Duration since last activity.
    pub fn idle_duration(&self) -> Duration {
        self.last_activity.lock().elapsed()
    }
}

/// Registry managing active code execution sessions.
///
/// Thread-safe: can be shared across async tasks via `Arc<SessionRegistry>`.
pub struct SessionRegistry {
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    default_config: SessionConfig,
    max_sessions: usize,
    /// Shared WASM runtime (None = fallback to subprocess).
    wasm_runtime: Option<Arc<WasmRuntime>>,
}

impl SessionRegistry {
    /// Create a registry that uses WASM-sandboxed executors.
    pub fn new_wasm(
        default_config: SessionConfig,
        max_sessions: usize,
        runtime: Arc<WasmRuntime>,
    ) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            default_config,
            max_sessions,
            wasm_runtime: Some(runtime),
        }
    }

    /// Create a registry that uses subprocess executors (fallback).
    pub fn new_subprocess(default_config: SessionConfig, max_sessions: usize) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            default_config,
            max_sessions,
            wasm_runtime: None,
        }
    }

    /// Create a registry that automatically selects the best backend.
    ///
    /// If `python_wasm_path` and `python_wasm_stdlib` point to existing files,
    /// uses the WASM-sandboxed executor. Otherwise falls back to subprocess.
    /// When both paths are `None`, checks `PYTHON_WASM_PATH` and
    /// `PYTHON_WASM_STDLIB` environment variables.
    pub fn new_auto(
        config: SessionConfig,
        max_sessions: usize,
        python_wasm_path: Option<&std::path::Path>,
        python_wasm_stdlib: Option<&std::path::Path>,
    ) -> Self {
        let wasm_path = python_wasm_path
            .map(|p| p.to_path_buf())
            .or_else(|| std::env::var("PYTHON_WASM_PATH").ok().map(PathBuf::from));
        let stdlib_path = python_wasm_stdlib
            .map(|p| p.to_path_buf())
            .or_else(|| std::env::var("PYTHON_WASM_STDLIB").ok().map(PathBuf::from));

        if let (Some(wasm_path), Some(stdlib_path)) = (wasm_path, stdlib_path) {
            if wasm_path.exists() && stdlib_path.exists() {
                match Self::try_create_wasm_registry(config.clone(), max_sessions, &wasm_path, &stdlib_path) {
                    Ok(registry) => {
                        tracing::info!("CodeAct: SessionRegistry using WASM backend");
                        return registry;
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "Failed to create WASM registry, falling back to subprocess");
                    }
                }
            }
        }

        tracing::info!("CodeAct: SessionRegistry using subprocess backend");
        Self::new_subprocess(config, max_sessions)
    }

    fn try_create_wasm_registry(
        config: SessionConfig,
        max_sessions: usize,
        wasm_path: &std::path::Path,
        stdlib_path: &std::path::Path,
    ) -> Result<Self, crate::executor::ExecutorError> {
        use wasmtime::{Config as WasmConfig, Engine as WasmEngine, Module as WasmModule};

        let mut engine_config = WasmConfig::new();
        engine_config.async_support(true);
        engine_config.epoch_interruption(true);

        let engine = WasmEngine::new(&engine_config).map_err(|e| {
            crate::executor::ExecutorError::NotReady(format!("Wasmtime engine: {e}"))
        })?;

        tracing::info!(path = %wasm_path.display(), "Compiling CPython WASI module for session registry");
        let module = WasmModule::from_file(&engine, wasm_path).map_err(|e| {
            crate::executor::ExecutorError::NotReady(format!("python.wasm compilation: {e}"))
        })?;

        let runtime = Arc::new(WasmRuntime {
            engine: Arc::new(engine),
            module: Arc::new(module),
            stdlib_dir: stdlib_path.to_path_buf(),
        });

        Ok(Self::new_wasm(config, max_sessions, runtime))
    }

    /// Get or create a session for the given conversation ID.
    ///
    /// `workspace` optionally overrides the executor's `working_directory` so
    /// that the Python process starts in the conversation's workspace rather
    /// than the daemon's cwd.
    pub async fn get_or_create(
        &self,
        session_id: &str,
        workspace: Option<&str>,
    ) -> Result<Arc<Session>, ExecutorError> {
        // Fast path: session already exists
        {
            let sessions = self.sessions.lock();
            if let Some(session) = sessions.get(session_id) {
                session.touch();
                return Ok(Arc::clone(session));
            }
        }

        // Opportunistic reap: clean up idle sessions on the creation path.
        // This avoids needing an external tokio::spawn timer, which can panic
        // if ChatService is constructed outside a Tokio runtime.
        self.reap_idle().await;

        // Slow path: create a new session
        self.evict_if_needed().await;

        // Merge workspace into config if provided
        let mut config = self.default_config.clone();
        if let Some(ws) = workspace {
            config.executor.working_directory = Some(ws.to_string());
        }

        let session = if let Some(ref runtime) = self.wasm_runtime {
            Session::new_wasm(
                session_id.to_string(),
                config,
                runtime,
            )
            .await?
        } else {
            Session::new_subprocess(
                session_id.to_string(),
                config,
            )
            .await?
        };
        let session = Arc::new(session);

        let mut sessions = self.sessions.lock();
        sessions.insert(session_id.to_string(), Arc::clone(&session));
        tracing::info!(
            session_id = session_id,
            active_sessions = sessions.len(),
            backend = if self.wasm_runtime.is_some() { "wasm" } else { "subprocess" },
            "CodeAct session created"
        );

        Ok(session)
    }

    /// Get an existing session (returns None if not found).
    pub fn get(&self, session_id: &str) -> Option<Arc<Session>> {
        let sessions = self.sessions.lock();
        sessions.get(session_id).map(|s| {
            s.touch();
            Arc::clone(s)
        })
    }

    /// Remove and shut down a specific session.
    pub async fn remove(&self, session_id: &str) -> Result<(), ExecutorError> {
        let session = {
            let mut sessions = self.sessions.lock();
            sessions.remove(session_id)
        };
        if let Some(session) = session {
            session.executor().shutdown().await?;
            tracing::info!(session_id = session_id, "CodeAct session removed");
        }
        Ok(())
    }

    /// Reset a session to a clean state (variables cleared, executor restarted).
    pub async fn reset(&self, session_id: &str) -> Result<(), ExecutorError> {
        let session = {
            let sessions = self.sessions.lock();
            sessions.get(session_id).cloned()
        };
        if let Some(session) = session {
            session.executor().reset().await?;
            session.touch();
            tracing::info!(session_id = session_id, "CodeAct session reset");
        }
        Ok(())
    }

    /// Reap idle sessions that have exceeded their timeout.
    pub async fn reap_idle(&self) {
        let idle_sessions: Vec<String> = {
            let sessions = self.sessions.lock();
            sessions
                .iter()
                .filter(|(_, s)| s.is_idle())
                .map(|(id, _)| id.clone())
                .collect()
        };

        for id in idle_sessions {
            if let Err(e) = self.remove(&id).await {
                tracing::warn!(session_id = %id, error = %e, "failed to reap idle session");
            } else {
                tracing::info!(session_id = %id, "reaped idle CodeAct session");
            }
        }
    }

    /// Evict the least recently used session if at capacity.
    async fn evict_if_needed(&self) {
        let to_evict = {
            let sessions = self.sessions.lock();
            if sessions.len() < self.max_sessions {
                return;
            }
            // Find LRU session
            sessions
                .iter()
                .max_by_key(|(_, s)| s.idle_duration())
                .map(|(id, _)| id.clone())
        };

        if let Some(id) = to_evict {
            tracing::info!(session_id = %id, "evicting LRU CodeAct session (at capacity)");
            let _ = self.remove(&id).await;
        }
    }

    /// Number of active sessions.
    pub fn active_count(&self) -> usize {
        self.sessions.lock().len()
    }

    /// Shut down all sessions.
    pub async fn shutdown_all(&self) {
        let ids: Vec<String> = {
            let sessions = self.sessions.lock();
            sessions.keys().cloned().collect()
        };
        for id in ids {
            let _ = self.remove(&id).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn session_config_defaults() {
        let config = SessionConfig::default();
        assert_eq!(config.idle_timeout, Duration::from_secs(600));
        assert_eq!(config.executor.execution_timeout_secs, 30);
    }

    #[tokio::test]
    async fn registry_tracks_count() {
        let registry = SessionRegistry::new_subprocess(SessionConfig::default(), 10);
        assert_eq!(registry.active_count(), 0);
    }
}
