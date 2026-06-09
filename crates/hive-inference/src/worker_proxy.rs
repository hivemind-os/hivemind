use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::ipc::{IpcMethod, IpcPayload, IpcRequest, IpcResponse, IpcResult};
use crate::runtime::{
    InferenceError, InferenceOutput, InferenceRequest, InferenceRuntime, ModelLoadOptions,
    RuntimeInfo,
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
    /// Stores (model_id, model_path, gpu_options).
    loaded_model_paths: Mutex<Vec<(String, PathBuf, ModelLoadOptions)>>,
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

        let mut cmd = Command::new(&self.config.worker_binary);
        cmd.arg("--runtime")
            .arg(runtime_arg)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit()); // worker logs go to daemon's stderr

        // Inject CUDA runtime DLL path on Windows so the worker can find cudart/cublas.
        #[cfg(target_os = "windows")]
        {
            if let Ok(cuda_path) = std::env::var("CUDA_PATH") {
                let bin_x64 = PathBuf::from(&cuda_path).join("bin").join("x64");
                if bin_x64.exists() {
                    let current_path = std::env::var("PATH").unwrap_or_default();
                    if !current_path.contains(bin_x64.to_str().unwrap_or("")) {
                        let new_path = format!("{};{}", bin_x64.display(), current_path);
                        cmd.env("PATH", &new_path);
                        tracing::debug!(path = %bin_x64.display(), "injected CUDA bin\\x64 into worker PATH");
                    }
                }
            }
        }

        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        // Suppress the Windows "DLL not found" system error dialog. Without this,
        // a missing CUDA DLL (e.g. cublas64_12.dll) causes Windows to show a modal
        // dialog that blocks the process and the entire app. Instead we detect the
        // failure programmatically and report it to the user gracefully.
        #[cfg(target_os = "windows")]
        let _prev_error_mode = unsafe { suppress_dll_error_dialog() };

        let spawn_result = cmd.spawn();

        #[cfg(target_os = "windows")]
        unsafe {
            restore_error_mode(_prev_error_mode);
        }

        let mut child = spawn_result.map_err(|e| {
            InferenceError::WorkerCrashed(format!(
                "failed to spawn worker binary '{}': {e}",
                self.config.worker_binary.display()
            ))
        })?;

        // Give the process a moment to fail if it can't load required DLLs.
        // A missing DLL causes immediate exit (typically within milliseconds).
        std::thread::sleep(Duration::from_millis(100));
        if let Some(status) = child.try_wait().unwrap_or(None) {
            let code = status.code().unwrap_or(-1);
            // STATUS_DLL_NOT_FOUND = 0xC0000135 (3221225781 unsigned / -1073741515 signed)
            let is_dll_error = code == -1073741515 || code as u32 == 0xC0000135;
            let msg = if is_dll_error {
                format!(
                    "Runtime worker failed to start: required CUDA libraries not found. \
                     GPU acceleration requires the NVIDIA CUDA Toolkit to be installed.\n\n\
                     Download from: https://developer.nvidia.com/cuda-downloads\n\n\
                     The application will continue without GPU acceleration."
                )
            } else {
                format!(
                    "Runtime worker exited immediately (code {code:#x}). \
                     This may indicate missing libraries or a configuration issue."
                )
            };
            tracing::warn!(%code, binary = %self.config.worker_binary.display(), "{msg}");
            return Err(InferenceError::WorkerCrashed(msg));
        }

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
        let models: Vec<(String, PathBuf, ModelLoadOptions)> =
            self.loaded_model_paths.lock().clone();
        for (model_id, model_path, options) in &models {
            let req_id = self.next_id.fetch_add(1, Ordering::Relaxed);
            let request = IpcRequest {
                id: req_id,
                method: IpcMethod::ModelLoad {
                    model_id: model_id.clone(),
                    model_path: model_path.clone(),
                    gpu_layers: options.gpu_layers,
                    main_gpu: options.main_gpu,
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
        self.load_model_with_options(model_id, model_path, &ModelLoadOptions::default())
    }

    fn load_model_with_options(
        &self,
        model_id: &str,
        model_path: &Path,
        options: &ModelLoadOptions,
    ) -> Result<(), InferenceError> {
        let method = IpcMethod::ModelLoad {
            model_id: model_id.to_string(),
            model_path: model_path.to_path_buf(),
            gpu_layers: options.gpu_layers,
            main_gpu: options.main_gpu,
        };
        self.call(method)?;
        // Track the model so we can re-load it if the worker crashes.
        {
            let mut paths = self.loaded_model_paths.lock();
            if !paths.iter().any(|(id, _, _)| id == model_id) {
                paths.push((model_id.to_string(), model_path.to_path_buf(), options.clone()));
            }
        }
        Ok(())
    }

    fn unload_model(&self, model_id: &str) -> Result<(), InferenceError> {
        let method = IpcMethod::ModelUnload { model_id: model_id.to_string() };
        self.call(method)?;
        self.loaded_model_paths.lock().retain(|(id, _, _)| id != model_id);
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

// ---------------------------------------------------------------------------
// Windows: suppress system error dialogs for missing DLLs
// ---------------------------------------------------------------------------

/// Suppress the Windows "missing DLL" system error dialog before spawning
/// a child process. Returns the previous error mode so it can be restored.
#[cfg(target_os = "windows")]
unsafe fn suppress_dll_error_dialog() -> u32 {
    const SEM_FAILCRITICALERRORS: u32 = 0x0001;
    const SEM_NOGPFAULTERRORBOX: u32 = 0x0002;

    unsafe extern "system" {
        fn SetErrorMode(uMode: u32) -> u32;
    }
    unsafe { SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX) }
}

/// Restore the previous Windows error mode after spawning.
#[cfg(target_os = "windows")]
unsafe fn restore_error_mode(prev: u32) {
    unsafe extern "system" {
        fn SetErrorMode(uMode: u32) -> u32;
    }
    unsafe {
        SetErrorMode(prev);
    }
}
