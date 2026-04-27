//! WASM-sandboxed Python executor using Wasmtime + CPython WASI.
//!
//! Runs a CPython WASI module inside Wasmtime with true sandboxing:
//! - Memory limits via Wasmtime StoreLimits
//! - Filesystem access only to preopened dirs (stdlib, scratch)
//! - No network access (WASI sockets not enabled)
//! - No subprocess spawning possible from WASM
//! - CPU timeout via Wasmtime epoch interruption

use crate::executor::{CodeExecutor, ExecutionResult, ExecutorConfig, ExecutorError, Language};
use crate::tool_bridge::{self, ExecutionOptions, ToolCallResponse};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};
use tokio::sync::Mutex;
use wasmtime::{Config, Engine, Linker, Module, Store, StoreLimits, StoreLimitsBuilder, UpdateDeadline};
use wasmtime_wasi::preview1::{self, WasiP1Ctx};
use wasmtime_wasi::{DirPerms, FilePerms, WasiCtxBuilder};

/// Sentinel markers for the REPL protocol (same as subprocess).
/// Augmented with a per-session nonce to prevent accidental collision.
const SENTINEL_ERROR: &str = "__HIVEMIND_EXEC_ERROR__";

/// Generate a random 16-char hex nonce for sentinel uniqueness.
fn generate_nonce() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{:016x}", seed)
}

fn sentinel_start(nonce: &str) -> String {
    format!("__HIVEMIND_EXEC_START_{nonce}__")
}

fn sentinel_end(nonce: &str) -> String {
    format!("__HIVEMIND_EXEC_END_{nonce}__")
}

/// Generate the Python REPL wrapper with per-session nonce sentinels.
fn generate_python_wrapper(nonce: &str) -> String {
    let start = sentinel_start(nonce);
    let end = sentinel_end(nonce);
    format!(
        r#"
import sys, io, traceback, os

_original_stdout = sys.stdout
_original_stdin = sys.stdin

# Set working directory to the preopened workspace if available
try:
    os.chdir("/workspace")
except Exception:
    pass

def _hivemind_exec_loop():
    while True:
        line = _original_stdin.readline()
        if not line:
            break
        line = line.strip()
        if line != "{start}":
            continue

        code_lines = []
        while True:
            line = _original_stdin.readline()
            if not line:
                return
            if line.strip() == "{end}":
                break
            code_lines.append(line)

        code = "".join(code_lines)

        old_stdout = sys.stdout
        old_stderr = sys.stderr
        captured_stdout = io.StringIO()
        captured_stderr = io.StringIO()
        sys.stdout = captured_stdout
        sys.stderr = captured_stderr
        is_error = False

        try:
            exec(compile(code, "<codeact>", "exec"), globals())
        except Exception:
            is_error = True
            traceback.print_exc(file=captured_stderr)
        finally:
            sys.stdout = old_stdout
            sys.stderr = old_stderr

        stdout_val = captured_stdout.getvalue()
        stderr_val = captured_stderr.getvalue()

        _original_stdout.write("{start}\n")
        if is_error:
            _original_stdout.write("__HIVEMIND_EXEC_ERROR__\n")
        _original_stdout.write(stdout_val)
        if stderr_val:
            _original_stdout.write("\n__HIVEMIND_STDERR__\n")
            _original_stdout.write(stderr_val)
        _original_stdout.write("\n{end}\n")
        _original_stdout.flush()

_hivemind_exec_loop()
"#,
        start = start,
        end = end,
    )
}

/// State for a running WASM Python instance.
struct WasmInstance {
    /// Write code to this to send to the WASM Python's stdin.
    stdin_writer: DuplexStream,
    /// Read output from the WASM Python's stdout.
    stdout_reader: BufReader<DuplexStream>,
    /// Handle to the WASM execution task.
    wasm_task: tokio::task::JoinHandle<()>,
    /// Engine handle for epoch advancement.
    engine: Arc<Engine>,
    /// Captured stderr from CPython (for crash diagnostics).
    stderr_buf: Arc<tokio::sync::Mutex<String>>,
}

/// A code executor backed by CPython running inside a Wasmtime WASM sandbox.
pub struct WasmExecutor {
    instance: Mutex<Option<WasmInstance>>,
    config: ExecutorConfig,
    /// Shared Wasmtime engine (pre-configured with epoch interruption).
    engine: Arc<Engine>,
    /// Pre-compiled CPython WASI module.
    module: Arc<Module>,
    /// Path to the extracted Python stdlib directory.
    stdlib_dir: PathBuf,
    /// Per-session nonce for sentinel uniqueness.
    nonce: String,
    /// Per-session temp directory (cleaned up on shutdown).
    tmp_dir: PathBuf,
}

impl WasmExecutor {
    /// Create a new WASM executor.
    ///
    /// `python_wasm_path` - path to the CPython WASI binary (python.wasm)
    /// `stdlib_dir` - path to the Python standard library directory
    pub async fn new(
        config: ExecutorConfig,
        python_wasm_path: &Path,
        stdlib_dir: &Path,
    ) -> Result<Self, ExecutorError> {
        // Configure engine with epoch interruption for CPU timeouts
        let mut engine_config = Config::new();
        engine_config.async_support(true);
        engine_config.epoch_interruption(true);

        let engine = Engine::new(&engine_config).map_err(|e| {
            ExecutorError::NotReady(format!("failed to create Wasmtime engine: {e}"))
        })?;
        let engine = Arc::new(engine);

        // Compile the CPython WASI module
        tracing::info!(path = %python_wasm_path.display(), "compiling CPython WASI module");
        let module = Module::from_file(&engine, python_wasm_path).map_err(|e| {
            ExecutorError::NotReady(format!("failed to compile python.wasm: {e}"))
        })?;
        let module = Arc::new(module);
        tracing::info!("CPython WASI module compiled successfully");

        let nonce = generate_nonce();
        let tmp_dir = std::env::temp_dir().join(format!("hivemind-wasm-{}", &nonce));

        let executor = Self {
            instance: Mutex::new(None),
            config,
            engine,
            module,
            stdlib_dir: stdlib_dir.to_path_buf(),
            nonce,
            tmp_dir,
        };

        // Spawn the initial WASM instance
        executor.spawn_instance().await?;

        Ok(executor)
    }

    /// Create a new WASM executor from a pre-compiled engine and module.
    /// Use this to share engine/module across sessions for efficiency.
    pub fn with_shared(
        config: ExecutorConfig,
        engine: Arc<Engine>,
        module: Arc<Module>,
        stdlib_dir: PathBuf,
    ) -> Self {
        let nonce = generate_nonce();
        let tmp_dir = std::env::temp_dir().join(format!("hivemind-wasm-{}", &nonce));
        Self {
            instance: Mutex::new(None),
            config,
            engine,
            module,
            stdlib_dir,
            nonce,
            tmp_dir,
        }
    }

    /// Ensure the WASM instance is running, spawning if needed.
    /// If a previous instance crashed, it's cleaned up and a new one spawned.
    pub async fn ensure_instance(&self) -> Result<(), ExecutorError> {
        let mut guard = self.instance.lock().await;
        if let Some(ref inst) = *guard {
            if !inst.wasm_task.is_finished() {
                return Ok(());
            }
            // Previous instance is dead — clean up and respawn
            tracing::warn!("WASM Python instance died — respawning");
            let _ = guard.take();
        }
        drop(guard);
        self.spawn_instance().await
    }

    /// Spawn a fresh WASM Python instance.
    ///
    /// After spawning, this waits for CPython to actually initialize successfully
    /// (by checking the task hasn't exited and stdout is responsive). If CPython
    /// crashes during Py_Initialize() (e.g., missing python312.zip), this returns
    /// an error with the captured stderr instead of a misleading "broken pipe" later.
    async fn spawn_instance(&self) -> Result<(), ExecutorError> {
        let engine = Arc::clone(&self.engine);
        let module = Arc::clone(&self.module);
        let stdlib_dir = self.stdlib_dir.clone();
        let memory_limit = (self.config.memory_limit_mb as usize) * 1024 * 1024;
        let nonce = self.nonce.clone();
        let workspace_dir = self.config.working_directory.clone();
        let tmp_dir = self.tmp_dir.clone();

        // Validate that the critical python312.zip exists before spawning.
        // This catches the most common failure mode early with a clear message.
        let stdlib_parent = stdlib_dir.parent().unwrap_or(&stdlib_dir);
        if let Some(stdlib_name) = stdlib_dir.file_name().and_then(|n| n.to_str()) {
            let zip_name = format!("{}.zip", stdlib_name.replace('.', ""));
            let zip_path = stdlib_parent.join(&zip_name);
            if !zip_path.exists() {
                return Err(ExecutorError::NotReady(format!(
                    "CPython WASI runtime is incomplete: '{}' is missing. \
                     CPython requires this archive for the 'encodings' module during initialization. \
                     Reinstall the application or ensure the WASM runtime is fully extracted.",
                    zip_path.display()
                )));
            }
        }

        // Create bidirectional channels for stdin/stdout.
        // Host writes to host_stdin_writer → WASM reads from wasm_stdin_reader
        // WASM writes to wasm_stdout_writer → Host reads from host_stdout_reader
        let (host_stdin_writer, wasm_stdin_reader) = tokio::io::duplex(65536);
        let (wasm_stdout_writer, host_stdout_reader) = tokio::io::duplex(65536);

        // Capture stderr into a shared buffer instead of inheriting.
        // If CPython crashes during init, we can report what went wrong.
        let (wasm_stderr_writer, wasm_stderr_reader) = tokio::io::duplex(8192);
        let stderr_buf: Arc<tokio::sync::Mutex<String>> = Arc::new(tokio::sync::Mutex::new(String::new()));
        let stderr_buf_for_task = Arc::clone(&stderr_buf);

        // Spawn a task to drain stderr into the buffer
        let stderr_drain = tokio::spawn(async move {
            let mut reader = BufReader::new(wasm_stderr_reader);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) => break, // EOF
                    Ok(_) => {
                        tracing::debug!(target: "cpython_stderr", "{}", line.trim_end());
                        let mut buf = stderr_buf_for_task.lock().await;
                        // Cap buffer at 8KB to prevent unbounded growth
                        if buf.len() < 8192 {
                            buf.push_str(&line);
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        let engine_for_task = Arc::clone(&engine);

        // Spawn the WASM execution on a tokio task.
        // tokio::task::spawn catches panics — they become JoinError::Panic
        // rather than crashing the daemon.
        let wasm_task = tokio::task::spawn(async move {
            if let Err(e) = run_wasm_instance(
                &engine_for_task,
                &module,
                &stdlib_dir,
                workspace_dir.as_deref(),
                memory_limit,
                wasm_stdin_reader,
                wasm_stdout_writer,
                wasm_stderr_writer,
                &nonce,
                &tmp_dir,
            )
            .await
            {
                tracing::error!(error = %e, "WASM Python instance exited with error");
            } else {
                tracing::debug!("WASM Python instance exited normally");
            }
            // Drop stderr drain when wasm task ends
            stderr_drain.abort();
        });

        let instance = WasmInstance {
            stdin_writer: host_stdin_writer,
            stdout_reader: BufReader::new(host_stdout_reader),
            wasm_task,
            engine: Arc::clone(&engine),
            stderr_buf: Arc::clone(&stderr_buf),
        };

        let mut guard = self.instance.lock().await;
        *guard = Some(instance);

        // Boot readiness check: wait for CPython to initialize, but verify
        // it hasn't crashed. If the task exits within the boot window, CPython
        // failed during Py_Initialize() — report stderr instead of "broken pipe".
        let boot_timeout = std::time::Duration::from_millis(500);
        let check_interval = std::time::Duration::from_millis(50);
        let boot_start = Instant::now();

        while boot_start.elapsed() < boot_timeout {
            tokio::time::sleep(check_interval).await;
            if let Some(ref inst) = *guard {
                if inst.wasm_task.is_finished() {
                    // CPython died during initialization
                    let stderr_content = inst.stderr_buf.lock().await.clone();
                    let _ = guard.take(); // Clean up the dead instance
                    let detail = if stderr_content.is_empty() {
                        "CPython WASI process exited immediately during initialization. \
                         This usually means the stdlib zip archive is missing or inaccessible."
                            .to_string()
                    } else {
                        format!(
                            "CPython WASI process crashed during initialization:\n{}",
                            stderr_content.trim()
                        )
                    };
                    return Err(ExecutorError::NotReady(detail));
                }
            }
        }

        Ok(())
    }

    /// Execute code with optional tool call support.
    pub async fn execute_with_tools(
        &self,
        code: &str,
        _language: Language,
        options: &ExecutionOptions<'_>,
    ) -> Result<ExecutionResult, ExecutorError> {
        self.ensure_instance().await?;

        let start = Instant::now();
        let mut guard = self.instance.lock().await;
        let inst = guard
            .as_mut()
            .ok_or_else(|| ExecutorError::NotReady("WASM instance not running".into()))?;

        // Send code to the WASM Python REPL
        let s_start = sentinel_start(&self.nonce);
        let s_end = sentinel_end(&self.nonce);
        let payload = format!("{s_start}\n{code}\n{s_end}\n");
        if let Err(e) = inst.stdin_writer.write_all(payload.as_bytes()).await {
            // If we get broken pipe, CPython has crashed. Include stderr for diagnostics.
            let stderr_content = inst.stderr_buf.lock().await.clone();
            let detail = if stderr_content.is_empty() {
                format!(
                    "CPython WASM process is not running (write failed: {e}). \
                     The process may have crashed during initialization — check that \
                     the WASM Python runtime is fully installed."
                )
            } else {
                format!(
                    "CPython WASM process crashed (write failed: {e}):\n{}",
                    stderr_content.trim()
                )
            };
            return Err(ExecutorError::ExecutionFailed(detail));
        }
        inst.stdin_writer
            .flush()
            .await
            .map_err(|e| ExecutorError::ExecutionFailed(format!("failed to flush WASM stdin: {e}")))?;

        // Start epoch ticker for CPU timeout
        let timeout_secs = self.config.execution_timeout_secs;
        let engine_for_timeout = Arc::clone(&inst.engine);
        let epoch_handle = tokio::spawn(async move {
            let tick_interval = std::time::Duration::from_millis(100);
            let deadline = std::time::Duration::from_secs(timeout_secs);
            let start = Instant::now();
            loop {
                tokio::time::sleep(tick_interval).await;
                engine_for_timeout.increment_epoch();
                if start.elapsed() > deadline {
                    break;
                }
            }
        });

        let max_output = self.config.max_output_bytes;

        // Read output until end sentinel, handling tool calls mid-stream
        let result = self.read_execution_output(inst, options, max_output, &s_start, &s_end).await;

        epoch_handle.abort();

        match result {
            Ok(mut exec_result) => {
                exec_result.duration_ms = start.elapsed().as_millis() as u64;
                Ok(exec_result)
            }
            Err(ExecutorError::Timeout { .. }) => {
                // Instance is poisoned after timeout — kill it
                tracing::warn!("WASM execution timed out, destroying instance");
                if let Some(inst) = guard.take() {
                    inst.wasm_task.abort();
                }
                Err(ExecutorError::Timeout { timeout_secs })
            }
            Err(e) => {
                // Instance may be poisoned — kill it to be safe
                if let Some(inst) = guard.take() {
                    inst.wasm_task.abort();
                }
                Err(e)
            }
        }
    }

    /// Read execution output from the WASM instance, handling tool calls.
    async fn read_execution_output(
        &self,
        inst: &mut WasmInstance,
        options: &ExecutionOptions<'_>,
        max_output: usize,
        s_start: &str,
        s_end: &str,
    ) -> Result<ExecutionResult, ExecutorError> {
        let mut stdout = String::new();
        let mut stderr = String::new();
        let mut is_error = false;
        let mut started = false;
        let mut in_stderr = false;
        let mut total_bytes = 0usize;

        loop {
            let mut line = String::new();
            let read_result = tokio::time::timeout(
                std::time::Duration::from_secs(self.config.execution_timeout_secs + 5),
                inst.stdout_reader.read_line(&mut line),
            )
            .await;

            let n = match read_result {
                Ok(Ok(n)) => n,
                Ok(Err(e)) => {
                    return Err(ExecutorError::ExecutionFailed(format!(
                        "failed to read WASM stdout: {e}"
                    )));
                }
                Err(_) => {
                    return Err(ExecutorError::Timeout {
                        timeout_secs: self.config.execution_timeout_secs,
                    });
                }
            };

            if n == 0 {
                return Err(ExecutorError::ExecutionFailed(
                    "WASM Python instance exited unexpectedly".into(),
                ));
            }

            let trimmed = line.trim();

            // Check for tool call frame
            if let Some(request) = tool_bridge::parse_tool_call_line(trimmed) {
                let response = if let Some(handler) = options.tool_call_handler {
                    handler.handle_tool_call(request).await
                } else {
                    ToolCallResponse {
                        request_id: request.request_id,
                        result: None,
                        error: Some("no tool handler configured".into()),
                        truncated: false,
                    }
                };
                let resp_json = tool_bridge::serialize_tool_response(&response);
                inst.stdin_writer
                    .write_all(format!("{resp_json}\n").as_bytes())
                    .await
                    .map_err(|e| {
                        ExecutorError::ExecutionFailed(format!(
                            "failed to write tool response: {e}"
                        ))
                    })?;
                inst.stdin_writer.flush().await.map_err(|e| {
                    ExecutorError::ExecutionFailed(format!("failed to flush tool response: {e}"))
                })?;
                continue;
            }

            if trimmed == s_start {
                started = true;
                continue;
            }
            if !started {
                continue;
            }
            if trimmed == SENTINEL_ERROR {
                is_error = true;
                continue;
            }
            if trimmed == "__HIVEMIND_STDERR__" {
                in_stderr = true;
                continue;
            }
            if trimmed == s_end {
                break;
            }

            total_bytes += line.len();
            if total_bytes > max_output {
                return Err(ExecutorError::OutputTooLarge {
                    max_bytes: max_output,
                });
            }

            if in_stderr {
                stderr.push_str(&line);
            } else {
                stdout.push_str(&line);
            }
        }

        Ok(ExecutionResult {
            stdout: stdout.trim_end().to_string(),
            stderr: stderr.trim_end().to_string(),
            is_error,
            duration_ms: 0, // filled in by caller
        })
    }
}

/// Wrapper state for the WASM store, holding both WASI context and resource limits.
struct WasmState {
    wasi: WasiP1Ctx,
    limits: StoreLimits,
}

/// Run the CPython WASI module inside Wasmtime.
///
/// This function blocks the async task until the Python REPL exits.
async fn run_wasm_instance(
    engine: &Engine,
    module: &Module,
    stdlib_dir: &Path,
    workspace_dir: Option<&str>,
    memory_limit: usize,
    wasm_stdin: DuplexStream,
    wasm_stdout: DuplexStream,
    wasm_stderr: DuplexStream,
    nonce: &str,
    tmp_dir: &Path,
) -> Result<(), ExecutorError> {
    use wasmtime_wasi::pipe::{AsyncReadStream, AsyncWriteStream};
    use wasmtime_wasi::{AsyncStdinStream, AsyncStdoutStream};

    // Build WASI context with sandboxed permissions
    let mut wasi_builder = WasiCtxBuilder::new();

    // Wire stdin/stdout through our duplex channels
    let stdin_stream = AsyncStdinStream::new(AsyncReadStream::new(wasm_stdin));
    let stdout_stream = AsyncStdoutStream::new(AsyncWriteStream::new(65536, wasm_stdout));
    wasi_builder.stdin(stdin_stream);
    wasi_builder.stdout(stdout_stream);

    // Wire stderr through a captured stream (not inherited) so we can
    // report CPython crash diagnostics back to the user/caller.
    let stderr_stream = AsyncStdoutStream::new(AsyncWriteStream::new(8192, wasm_stderr));
    wasi_builder.stderr(stderr_stream);

    // Preopen Python stdlib (read-only).
    //
    // CPython's compiled-in prefix is /usr/local.  During very early init it
    // searches {prefix}/lib/python312.zip and {prefix}/lib/python3.12/ for
    // critical bootstrap modules like `encodings`.  The vmware-labs WASI
    // tarball ships the stdlib as:
    //   lib/python312.zip       – bulk of the stdlib (including encodings)
    //   lib/python3.12/os.py    – os module (loaded before zipimport)
    //   lib/python3.12/lib-dynload/  – C extension stubs
    //
    // We mount the *parent* directory (containing both the zip and the
    // version-specific subdirectory) at /usr/local/lib/ so CPython's
    // standard path resolution finds everything it needs.
    let stdlib_parent = stdlib_dir
        .parent()
        .unwrap_or(stdlib_dir);
    wasi_builder
        .preopened_dir(stdlib_parent, "/usr/local/lib", DirPerms::READ, FilePerms::READ)
        .map_err(|e| {
            ExecutorError::NotReady(format!("failed to preopen stdlib dir: {e}"))
        })?;

    // Preopen workspace directory (read-write) — this is the only writable
    // directory available to user code. All paths outside are inaccessible.
    if let Some(ws) = workspace_dir {
        let ws_path = Path::new(ws);
        if ws_path.exists() {
            wasi_builder
                .preopened_dir(ws_path, "/workspace", DirPerms::all(), FilePerms::all())
                .map_err(|e| {
                    ExecutorError::NotReady(format!("failed to preopen workspace dir: {e}"))
                })?;
        }
    }

    // Preopen a scratch/tmp directory for Python's tempfile module
    std::fs::create_dir_all(tmp_dir).map_err(|e| {
        ExecutorError::NotReady(format!("failed to create tmp dir: {e}"))
    })?;
    wasi_builder
        .preopened_dir(tmp_dir, "/tmp", DirPerms::all(), FilePerms::all())
        .map_err(|e| {
            ExecutorError::NotReady(format!("failed to preopen tmp dir: {e}"))
        })?;

    // Pass args to CPython: python.wasm -c <wrapper_script>
    let wrapper = generate_python_wrapper(nonce);
    wasi_builder.args(&["python.wasm", "-c", &wrapper]);

    // Set environment variables for reproducible behavior.
    // PYTHONHOME=/usr/local tells CPython where its prefix is, matching the
    // preopened /usr/local/lib/ mount above.
    wasi_builder.env("PYTHONHOME", "/usr/local");
    wasi_builder.env("PYTHONDONTWRITEBYTECODE", "1");
    wasi_builder.env("PYTHONNOUSERSITE", "1");
    wasi_builder.env("PYTHONPATH", "/usr/local/lib/python3.12");
    wasi_builder.env("TMPDIR", "/tmp");
    if workspace_dir.is_some() {
        wasi_builder.env("HOME", "/workspace");
    }

    let wasi_ctx = wasi_builder.build_p1();

    // Configure store with memory limits via wrapper state
    let limits = StoreLimitsBuilder::new()
        .memory_size(memory_limit)
        .build();

    let state = WasmState {
        wasi: wasi_ctx,
        limits,
    };

    let mut store = Store::new(engine, state);
    store.limiter(|s| &mut s.limits);

    // Configure epoch deadline: 1 epoch tick = ~100ms, set generous initial deadline
    store.epoch_deadline_callback(|_store| Ok(UpdateDeadline::Continue(100)));

    // Link WASI Preview 1 functions
    let mut linker: Linker<WasmState> = Linker::new(engine);
    preview1::add_to_linker_async(&mut linker, |s| &mut s.wasi).map_err(|e| {
        ExecutorError::NotReady(format!("failed to link WASI functions: {e}"))
    })?;

    // Instantiate
    let instance = linker
        .instantiate_async(&mut store, module)
        .await
        .map_err(|e| {
            ExecutorError::ExecutionFailed(format!("failed to instantiate WASM module: {e}"))
        })?;

    // Call _start (this runs the Python REPL loop until stdin closes)
    let start_func = instance
        .get_typed_func::<(), ()>(&mut store, "_start")
        .map_err(|e| {
            ExecutorError::ExecutionFailed(format!("_start function not found: {e}"))
        })?;

    match start_func.call_async(&mut store, ()).await {
        Ok(()) => Ok(()),
        Err(e) => {
            // Check if this was an epoch interruption (timeout)
            let msg = e.to_string();
            if msg.contains("epoch") || msg.contains("interrupt") {
                Err(ExecutorError::Timeout {
                    timeout_secs: 0, // will be overridden by caller
                })
            } else {
                // _start exiting normally (e.g., stdin closed) is OK
                tracing::debug!(error = %e, "WASM _start exited");
                Ok(())
            }
        }
    }
}

#[async_trait::async_trait]
impl CodeExecutor for WasmExecutor {
    async fn execute_with_tools(
        &self,
        code: &str,
        language: Language,
        options: &ExecutionOptions<'_>,
    ) -> Result<ExecutionResult, ExecutorError> {
        // Delegate to the concrete method
        WasmExecutor::execute_with_tools(self, code, language, options).await
    }

    async fn reset(&self) -> Result<(), ExecutorError> {
        let mut guard = self.instance.lock().await;
        // Kill existing instance
        if let Some(inst) = guard.take() {
            inst.wasm_task.abort();
        }
        drop(guard);
        // Spawn fresh instance
        self.spawn_instance().await?;
        tracing::debug!("WASM executor reset — fresh Python instance");
        Ok(())
    }

    async fn shutdown(&self) -> Result<(), ExecutorError> {
        let mut guard = self.instance.lock().await;
        if let Some(inst) = guard.take() {
            inst.wasm_task.abort();
        }
        // Best-effort cleanup of per-session temp directory
        let _ = std::fs::remove_dir_all(&self.tmp_dir);
        tracing::debug!("WASM executor shut down");
        Ok(())
    }

    async fn is_alive(&self) -> bool {
        let guard = self.instance.lock().await;
        match &*guard {
            Some(inst) => !inst.wasm_task.is_finished(),
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tests require a CPython WASI binary. Set PYTHON_WASM_PATH and PYTHON_WASM_STDLIB
    // environment variables to point to the binary and stdlib directory.
    //
    // Example:
    //   PYTHON_WASM_PATH=/path/to/python.wasm
    //   PYTHON_WASM_STDLIB=/path/to/lib/python3.13
    //
    // Download from: https://github.com/vmware-labs/webassembly-language-runtimes/releases

    fn wasm_paths() -> Option<(PathBuf, PathBuf)> {
        let wasm_path = std::env::var("PYTHON_WASM_PATH").ok()?;
        let stdlib_path = std::env::var("PYTHON_WASM_STDLIB").ok()?;
        let wasm = PathBuf::from(wasm_path);
        let stdlib = PathBuf::from(stdlib_path);
        if wasm.exists() && stdlib.exists() {
            Some((wasm, stdlib))
        } else {
            None
        }
    }

    async fn make_executor() -> Option<WasmExecutor> {
        let (wasm_path, stdlib_path) = wasm_paths()?;
        let config = ExecutorConfig {
            execution_timeout_secs: 30,
            max_output_bytes: 1_000_000,
            memory_limit_mb: 256,
            working_directory: None,
            allow_network: false,
        };
        match WasmExecutor::new(config, &wasm_path, &stdlib_path).await {
            Ok(exec) => Some(exec),
            Err(e) => {
                eprintln!("Failed to create WASM executor: {e}");
                None
            }
        }
    }

    #[tokio::test]
    async fn basic_execution() {
        let exec = match make_executor().await {
            Some(e) => e,
            None => {
                eprintln!("Skipping: PYTHON_WASM_PATH/PYTHON_WASM_STDLIB not set");
                return;
            }
        };

        let result = exec
            .execute("print('hello from wasm')", Language::Python)
            .await
            .unwrap();
        assert!(!result.is_error, "Execution error: {:?}", result);
        assert_eq!(result.stdout.trim(), "hello from wasm");
    }

    #[tokio::test]
    async fn state_persists_across_calls() {
        let exec = match make_executor().await {
            Some(e) => e,
            None => {
                eprintln!("Skipping: PYTHON_WASM_PATH/PYTHON_WASM_STDLIB not set");
                return;
            }
        };

        let r1 = exec.execute("x = 42", Language::Python).await.unwrap();
        assert!(!r1.is_error);

        let r2 = exec.execute("print(x)", Language::Python).await.unwrap();
        assert!(!r2.is_error);
        assert_eq!(r2.stdout.trim(), "42");
    }

    #[tokio::test]
    async fn error_handling() {
        let exec = match make_executor().await {
            Some(e) => e,
            None => {
                eprintln!("Skipping: PYTHON_WASM_PATH/PYTHON_WASM_STDLIB not set");
                return;
            }
        };

        let result = exec
            .execute("raise ValueError('oops')", Language::Python)
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.stderr.contains("ValueError"));
        assert!(result.stderr.contains("oops"));
    }

    #[tokio::test]
    async fn reset_clears_state() {
        let exec = match make_executor().await {
            Some(e) => e,
            None => {
                eprintln!("Skipping: PYTHON_WASM_PATH/PYTHON_WASM_STDLIB not set");
                return;
            }
        };

        exec.execute("y = 99", Language::Python).await.unwrap();
        exec.reset().await.unwrap();

        let result = exec.execute("print(y)", Language::Python).await.unwrap();
        assert!(result.is_error);
        assert!(result.stderr.contains("NameError"));
    }

    #[tokio::test]
    async fn multiline_code() {
        let exec = match make_executor().await {
            Some(e) => e,
            None => {
                eprintln!("Skipping: PYTHON_WASM_PATH/PYTHON_WASM_STDLIB not set");
                return;
            }
        };

        let code = "
for i in range(3):
    print(f'item {i}')
";
        let result = exec.execute(code, Language::Python).await.unwrap();
        assert!(!result.is_error, "Execution error: {:?}", result);
        assert!(result.stdout.contains("item 0"));
        assert!(result.stdout.contains("item 1"));
        assert!(result.stdout.contains("item 2"));
    }

    #[tokio::test]
    async fn stdlib_imports() {
        let exec = match make_executor().await {
            Some(e) => e,
            None => {
                eprintln!("Skipping: PYTHON_WASM_PATH/PYTHON_WASM_STDLIB not set");
                return;
            }
        };

        // Test imports that stress startup paths
        let code = r#"
import json, pathlib, os
data = json.dumps({"key": "value"})
print(data)
"#;
        let result = exec.execute(code, Language::Python).await.unwrap();
        assert!(!result.is_error, "Import error: {:?}", result);
        assert!(result.stdout.contains("key"));
    }

    #[tokio::test]
    async fn tool_call_bridge() {
        use crate::tool_bridge::{
            generate_bridge_code, BridgedToolInfo, CodeActToolMode, ToolCallHandler,
            ToolCallRequest, ToolCallResponse,
        };
        use serde_json::json;

        struct MockHandler;

        #[async_trait::async_trait]
        impl ToolCallHandler for MockHandler {
            async fn handle_tool_call(&self, req: ToolCallRequest) -> ToolCallResponse {
                if req.tool_id == "test.greet" {
                    let name = req.args["name"].as_str().unwrap_or("world");
                    ToolCallResponse {
                        request_id: req.request_id,
                        result: Some(json!(format!("Hello, {name}!"))),
                        error: None,
                        truncated: false,
                    }
                } else {
                    ToolCallResponse {
                        request_id: req.request_id,
                        result: None,
                        error: Some(format!("unknown tool: {}", req.tool_id)),
                        truncated: false,
                    }
                }
            }
        }

        let exec = match make_executor().await {
            Some(e) => e,
            None => {
                eprintln!("Skipping: PYTHON_WASM_PATH/PYTHON_WASM_STDLIB not set");
                return;
            }
        };

        let tools = vec![BridgedToolInfo {
            tool_id: "test.greet".into(),
            description: "Greet someone".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "Name to greet"}
                },
                "required": ["name"]
            }),
            mode: CodeActToolMode::Bridged,
        }];
        let bridge_code = generate_bridge_code(&tools);

        let handler = MockHandler;
        let options = ExecutionOptions {
            tool_call_handler: Some(&handler),
        };

        let r1 = exec
            .execute_with_tools(&bridge_code, Language::Python, &options)
            .await
            .unwrap();
        assert!(!r1.is_error, "Bridge injection failed: {:?}", r1);

        let r2 = exec
            .execute_with_tools(
                "result = test_greet(name='Alice')\nprint(result)",
                Language::Python,
                &options,
            )
            .await
            .unwrap();
        assert!(!r2.is_error, "Tool call failed: {:?}", r2);
        assert!(
            r2.stdout.contains("Hello, Alice!"),
            "Expected greeting in output, got: {}",
            r2.stdout
        );
    }
}
