use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::ipc::{IpcMethod, IpcPayload, IpcRequest, IpcResponse, IpcResult};
use crate::runtime::{
    InferenceError, InferenceOutput, InferenceRequest, InferenceRuntime, RuntimeInfo,
};
use hive_core::InferenceRuntimeKind;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for a [`RuntimeWorkerProxy`].
#[derive(Debug, Clone)]
pub struct WorkerConfig {
    /// Path to the `hive-runtime-worker` binary.
    pub worker_binary: PathBuf,
    /// Which runtime kind this worker should host.
    pub runtime_kind: InferenceRuntimeKind,
    /// Maximum time to wait for a single request (default: 15 minutes).
    pub request_timeout: Duration,
    /// Maximum number of automatic restarts before giving up.
    pub max_restarts: u32,
    /// Base delay for exponential backoff between restarts.
    pub restart_backoff_base: Duration,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            worker_binary: PathBuf::from("hive-runtime-worker"),
            runtime_kind: InferenceRuntimeKind::LlamaCpp,
            request_timeout: Duration::from_secs(900),
            max_restarts: 5,
            restart_backoff_base: Duration::from_millis(500),
        }
    }
}

// ---------------------------------------------------------------------------
// Worker State
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerState {
    /// Worker is not running.
    Stopped,
    /// Worker process is alive and ready.
    Ready,
    /// Worker process crashed.
    Crashed,
}

// ---------------------------------------------------------------------------
// Worker Handle (the actual child process I/O — fully synchronous)
// ---------------------------------------------------------------------------

struct WorkerHandle {
    child: Child,
    stdin: std::process::ChildStdin,
    reader: BufReader<std::process::ChildStdout>,
}

// ---------------------------------------------------------------------------
// RuntimeWorkerProxy
// ---------------------------------------------------------------------------

/// An [`InferenceRuntime`] implementation that delegates all calls to an
/// isolated child process over newline-delimited JSON on stdio.
///
/// All I/O is synchronous (blocking). Concurrent callers are serialized
/// via a mutex — the worker processes one request at a time.
pub struct RuntimeWorkerProxy {
    config: WorkerConfig,
    /// Serializes access to the worker process.
    handle: Mutex<Option<WorkerHandle>>,
    /// Monotonically increasing request id.
    next_id: AtomicU64,
    /// Current state.
    state: Mutex<WorkerState>,
    /// Number of consecutive crashes without a successful request.
    crash_count: Mutex<u32>,
    /// Cached info from the worker (populated on first connect).
    cached_kind: Mutex<Option<InferenceRuntimeKind>>,
    /// Models that were loaded into the worker. On crash recovery the new
    /// worker must be told to re-load these so they remain available.
    loaded_model_paths: Mutex<Vec<(String, PathBuf)>>,
}

impl RuntimeWorkerProxy {
    pub fn new(config: WorkerConfig) -> Self {
        let kind = config.runtime_kind;
        Self {
            config,
            handle: Mutex::new(None),
            next_id: AtomicU64::new(1),
            state: Mutex::new(WorkerState::Stopped),
            crash_count: Mutex::new(0),
            cached_kind: Mutex::new(Some(kind)),
            loaded_model_paths: Mutex::new(Vec::new()),
        }
    }

    /// Returns the current worker state.
    pub fn state(&self) -> WorkerState {
        *self.state.lock()
    }

    /// Spawns the worker process if it isn't already running.
    fn ensure_started(&self, handle: &mut Option<WorkerHandle>) -> Result<(), InferenceError> {
        if handle.is_some() {
            return Ok(());
        }

        let runtime_arg = match self.config.runtime_kind {
            InferenceRuntimeKind::Candle => "candle",
            InferenceRuntimeKind::Onnx => "onnx",
            InferenceRuntimeKind::LlamaCpp => "llama-cpp",
        };

        tracing::info!(
            binary = %self.config.worker_binary.display(),
            runtime = runtime_arg,
            "spawning runtime worker"
        );

        let mut child = Command::new(&self.config.worker_binary)
            .arg("--runtime")
            .arg(runtime_arg)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit()) // worker logs go to daemon's stderr
            .spawn()
            .map_err(|e| {
                InferenceError::WorkerCrashed(format!(
                    "failed to spawn worker binary '{}': {e}",
                    self.config.worker_binary.display()
                ))
            })?;

        let stdin = child.stdin.take().ok_or_else(|| {
            InferenceError::WorkerCrashed("failed to capture worker stdin".into())
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            InferenceError::WorkerCrashed("failed to capture worker stdout".into())
        })?;

        *handle = Some(WorkerHandle { child, stdin, reader: BufReader::new(stdout) });
        *self.state.lock() = WorkerState::Ready;
        *self.crash_count.lock() = 0;

        // Re-load any models that were loaded before the worker crashed.
        let models: Vec<(String, PathBuf)> = self.loaded_model_paths.lock().clone();
        for (model_id, model_path) in &models {
            let req_id = self.next_id.fetch_add(1, Ordering::Relaxed);
            let request = IpcRequest {
                id: req_id,
                method: IpcMethod::ModelLoad {
                    model_id: model_id.clone(),
                    model_path: model_path.clone(),
                },
            };
            match self.send_and_receive(handle, &request) {
                Ok(resp) if matches!(resp.payload, IpcPayload::Result(_)) => {
                    tracing::info!(
                        model_id = %model_id,
                        "re-loaded model into restarted worker"
                    );
                }
                Ok(resp) => {
                    return Err(InferenceError::LoadFailed(format!(
                        "failed to re-load model `{model_id}` into restarted worker: {:?}",
                        resp.payload
                    )));
                }
                Err(e) => {
                    return Err(InferenceError::LoadFailed(format!(
                        "failed to re-load model `{model_id}` into restarted worker: {e}"
                    )));
                }
            }
        }

        Ok(())
    }

    /// Sends a request and reads the response. If the worker crashes,
    /// marks it as crashed and attempts a restart (with backoff).
    fn call(&self, method: IpcMethod) -> Result<IpcResult, InferenceError> {
        let mut handle_guard = self.handle.lock();

        // Try up to max_restarts + 1 times (initial attempt + restarts).
        let max_attempts = self.config.max_restarts + 1;

        for attempt in 0..max_attempts {
            if attempt > 0 {
                // Exponential backoff before restart.
                let delay = self.config.restart_backoff_base * 2u32.saturating_pow(attempt - 1);
                tracing::warn!(
                    attempt,
                    delay_ms = delay.as_millis(),
                    "restarting worker after crash"
                );
                std::thread::sleep(delay);
                // Clear the dead handle so ensure_started spawns a new one.
                *handle_guard = None;
            }

            self.ensure_started(&mut handle_guard)?;

            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            let request = IpcRequest { id, method: method.clone() };

            match self.send_and_receive(&mut handle_guard, &request) {
                Ok(response) => {
                    if response.id != id {
                        return Err(InferenceError::Other(format!(
                            "response id mismatch: expected {id}, got {}",
                            response.id
                        )));
                    }
                    // Reset crash count on successful operation.
                    *self.crash_count.lock() = 0;
                    return match response.payload {
                        IpcPayload::Result(result) => Ok(result),
                        IpcPayload::Error(err) => Err(ipc_error_to_inference_error(&err)),
                    };
                }
                Err(e) => {
                    // Worker probably crashed. Mark it and try restarting.
                    tracing::error!(error = %e, attempt, "worker communication failed");
                    *self.state.lock() = WorkerState::Crashed;
                    let count = {
                        let mut c = self.crash_count.lock();
                        *c += 1;
                        *c
                    };
                    // Kill the child if it's still somehow alive.
                    if let Some(ref mut h) = *handle_guard {
                        let _ = h.child.kill();
                    }
                    *handle_guard = None;

                    if count >= self.config.max_restarts {
                        return Err(InferenceError::WorkerCrashed(format!(
                            "worker crashed {count} times, giving up: {e}"
                        )));
                    }
                }
            }
        }

        Err(InferenceError::WorkerCrashed("exhausted all restart attempts".into()))
    }

    fn send_and_receive(
        &self,
        handle: &mut Option<WorkerHandle>,
        request: &IpcRequest,
    ) -> Result<IpcResponse, InferenceError> {
        let h = handle
            .as_mut()
            .ok_or_else(|| InferenceError::WorkerCrashed("worker handle not available".into()))?;

        let mut json = serde_json::to_string(request)
            .map_err(|e| InferenceError::Other(format!("failed to serialize request: {e}")))?;
        json.push('\n');

        let timeout = self.config.request_timeout;
        let child_pid = h.child.id();

        // Spawn a watchdog thread that kills the worker if the request
        // exceeds the configured timeout.
        let (done_tx, done_rx) = std::sync::mpsc::sync_channel::<()>(1);
        let watchdog = std::thread::spawn(move || {
            if done_rx.recv_timeout(timeout).is_err() {
                tracing::error!(
                    pid = child_pid,
                    timeout_secs = timeout.as_secs(),
                    "worker request timed out, killing child process"
                );
                kill_process_by_pid(child_pid);
            }
        });

        let start = Instant::now();

        // Send request (blocking write).
        let write_result = h.stdin.write_all(json.as_bytes()).and_then(|()| h.stdin.flush());
        if let Err(e) = write_result {
            let _ = done_tx.send(());
            let _ = watchdog.join();
            return Err(InferenceError::WorkerCrashed(format!("write to worker failed: {e}")));
        }

        // Read response (blocking read).
        let mut response_line = String::new();
        let read_result = h.reader.read_line(&mut response_line);

        // Cancel the watchdog.
        let _ = done_tx.send(());
        let _ = watchdog.join();

        let bytes_read = read_result.map_err(|e| {
            if start.elapsed() >= timeout {
                InferenceError::Timeout { seconds: timeout.as_secs() }
            } else {
                InferenceError::WorkerCrashed(format!("read from worker failed: {e}"))
            }
        })?;

        if bytes_read == 0 {
            return if start.elapsed() >= timeout {
                Err(InferenceError::Timeout { seconds: timeout.as_secs() })
            } else {
                Err(InferenceError::WorkerCrashed("worker closed stdout (process exited)".into()))
            };
        }

        let response: IpcResponse = serde_json::from_str(response_line.trim()).map_err(|e| {
            InferenceError::Other(format!(
                "failed to parse worker response: {e}\nraw: {response_line}"
            ))
        })?;
        Ok(response)
    }

    /// Shuts down the worker process gracefully.
    pub fn shutdown(&self) {
        let mut handle = self.handle.lock();
        if let Some(ref mut h) = *handle {
            let _ = h.child.kill();
            let _ = h.child.wait();
        }
        *handle = None;
        *self.state.lock() = WorkerState::Stopped;
    }
}

/// Kills a process by PID using platform-specific commands.
fn kill_process_by_pid(pid: u32) {
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(not(windows))]
    {
        let _ = Command::new("kill")
            .args(["-9", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

fn ipc_error_to_inference_error(err: &crate::ipc::IpcError) -> InferenceError {
    match err.code.as_str() {
        "model_not_loaded" => InferenceError::ModelNotLoaded { model_id: err.message.clone() },
        "load_failed" => InferenceError::LoadFailed(err.message.clone()),
        "inference_failed" => InferenceError::InferenceFailed(err.message.clone()),
        "model_file_not_found" => InferenceError::ModelFileNotFound(err.message.clone()),
        "worker_crashed" | "panic" => InferenceError::WorkerCrashed(err.message.clone()),
        "timeout" => {
            let seconds = err
                .message
                .split_whitespace()
                .find_map(|part| part.strip_suffix('s'))
                .and_then(|part| part.parse::<u64>().ok())
                .unwrap_or(0);
            InferenceError::Timeout { seconds }
        }
        _ => InferenceError::Other(format!("{}: {}", err.code, err.message)),
    }
}

// ---------------------------------------------------------------------------
// InferenceRuntime implementation
// ---------------------------------------------------------------------------

impl InferenceRuntime for RuntimeWorkerProxy {
    fn kind(&self) -> InferenceRuntimeKind {
        self.cached_kind.lock().unwrap_or(self.config.runtime_kind)
    }

    fn is_available(&self) -> bool {
        // Optimistic: the runtime is available if we can spawn the worker.
        // Actual availability is checked on first use.
        true
    }

    fn info(&self) -> RuntimeInfo {
        RuntimeInfo {
            kind: self.kind(),
            version: "worker-proxy".to_string(),
            supports_gpu: false,
            loaded_model: None,
            memory_used_bytes: 0,
        }
    }

    fn load_model(&self, model_id: &str, model_path: &Path) -> Result<(), InferenceError> {
        let method = IpcMethod::ModelLoad {
            model_id: model_id.to_string(),
            model_path: model_path.to_path_buf(),
        };
        self.call(method)?;
        // Track the model so we can re-load it if the worker crashes.
        {
            let mut paths = self.loaded_model_paths.lock();
            if !paths.iter().any(|(id, _)| id == model_id) {
                paths.push((model_id.to_string(), model_path.to_path_buf()));
            }
        }
        Ok(())
    }

    fn unload_model(&self, model_id: &str) -> Result<(), InferenceError> {
        let method = IpcMethod::ModelUnload { model_id: model_id.to_string() };
        self.call(method)?;
        self.loaded_model_paths.lock().retain(|(id, _)| id != model_id);
        Ok(())
    }

    fn is_model_loaded(&self, model_id: &str) -> bool {
        let method = IpcMethod::ModelIsLoaded { model_id: model_id.to_string() };
        match self.call(method) {
            Ok(IpcResult::Bool(b)) => b,
            _ => false,
        }
    }

    fn infer(
        &self,
        model_id: &str,
        request: &InferenceRequest,
    ) -> Result<InferenceOutput, InferenceError> {
        let method = IpcMethod::ModelInfer {
            model_id: model_id.to_string(),
            request: request.clone(),
            attachments: vec![],
        };
        match self.call(method)? {
            IpcResult::InferenceOutput(output) => Ok(output),
            other => Err(InferenceError::Other(format!("unexpected result type: {other:?}"))),
        }
    }

    fn embed(&self, model_id: &str, text: &str) -> Result<Vec<f32>, InferenceError> {
        let method =
            IpcMethod::ModelEmbed { model_id: model_id.to_string(), text: text.to_string() };
        match self.call(method)? {
            IpcResult::Embeddings(v) => Ok(v),
            other => Err(InferenceError::Other(format!("unexpected result type: {other:?}"))),
        }
    }

    fn supported_formats(&self) -> Vec<String> {
        match self.call(IpcMethod::RuntimeFormats) {
            Ok(IpcResult::Formats(f)) => f,
            _ => vec![],
        }
    }
}

// SAFETY: RuntimeWorkerProxy communicates with an external process via IPC
// (stdin/stdout). All mutable state is protected by parking_lot::Mutex.
unsafe impl Send for RuntimeWorkerProxy {}
unsafe impl Sync for RuntimeWorkerProxy {}
