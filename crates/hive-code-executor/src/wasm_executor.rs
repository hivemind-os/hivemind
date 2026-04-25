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
const SENTINEL_START: &str = "__HIVEMIND_EXEC_START__";
const SENTINEL_END: &str = "__HIVEMIND_EXEC_END__";
const SENTINEL_ERROR: &str = "__HIVEMIND_EXEC_ERROR__";

/// Python REPL wrapper that runs inside the WASM sandbox.
/// Same protocol as the subprocess executor but running inside CPython-WASI.
const PYTHON_WRAPPER: &str = r#"
import sys, io, traceback

_original_stdout = sys.stdout
_original_stdin = sys.stdin

def _hivemind_exec_loop():
    while True:
        line = _original_stdin.readline()
        if not line:
            break
        line = line.strip()
        if line != "__HIVEMIND_EXEC_START__":
            continue

        code_lines = []
        while True:
            line = _original_stdin.readline()
            if not line:
                return
            if line.strip() == "__HIVEMIND_EXEC_END__":
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

        _original_stdout.write("__HIVEMIND_EXEC_START__\n")
        if is_error:
            _original_stdout.write("__HIVEMIND_EXEC_ERROR__\n")
        _original_stdout.write(stdout_val)
        if stderr_val:
            _original_stdout.write("\n__HIVEMIND_STDERR__\n")
            _original_stdout.write(stderr_val)
        _original_stdout.write("\n__HIVEMIND_EXEC_END__\n")
        _original_stdout.flush()

_hivemind_exec_loop()
"#;

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

        let executor = Self {
            instance: Mutex::new(None),
            config,
            engine,
            module,
            stdlib_dir: stdlib_dir.to_path_buf(),
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
        Self {
            instance: Mutex::new(None),
            config,
            engine,
            module,
            stdlib_dir,
        }
    }

    /// Ensure the WASM instance is running, spawning if needed.
    pub async fn ensure_instance(&self) -> Result<(), ExecutorError> {
        let guard = self.instance.lock().await;
        if guard.is_some() {
            return Ok(());
        }
        drop(guard);
        self.spawn_instance().await
    }

    /// Spawn a fresh WASM Python instance.
    async fn spawn_instance(&self) -> Result<(), ExecutorError> {
        let engine = Arc::clone(&self.engine);
        let module = Arc::clone(&self.module);
        let stdlib_dir = self.stdlib_dir.clone();
        let memory_limit = (self.config.memory_limit_mb as usize) * 1024 * 1024;

        // Create bidirectional channels for stdin/stdout.
        // Host writes to host_stdin_writer → WASM reads from wasm_stdin_reader
        // WASM writes to wasm_stdout_writer → Host reads from host_stdout_reader
        let (host_stdin_writer, wasm_stdin_reader) = tokio::io::duplex(65536);
        let (wasm_stdout_writer, host_stdout_reader) = tokio::io::duplex(65536);

        let engine_for_task = Arc::clone(&engine);

        // Spawn the WASM execution on a tokio task
        let wasm_task = tokio::task::spawn(async move {
            if let Err(e) = run_wasm_instance(
                &engine_for_task,
                &module,
                &stdlib_dir,
                memory_limit,
                wasm_stdin_reader,
                wasm_stdout_writer,
            )
            .await
            {
                tracing::error!(error = %e, "WASM Python instance exited with error");
            } else {
                tracing::debug!("WASM Python instance exited normally");
            }
        });

        let instance = WasmInstance {
            stdin_writer: host_stdin_writer,
            stdout_reader: BufReader::new(host_stdout_reader),
            wasm_task,
            engine: Arc::clone(&engine),
        };

        let mut guard = self.instance.lock().await;
        *guard = Some(instance);

        // Give the WASM instance a moment to initialize
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

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
        let payload = format!("{SENTINEL_START}\n{code}\n{SENTINEL_END}\n");
        inst.stdin_writer
            .write_all(payload.as_bytes())
            .await
            .map_err(|e| ExecutorError::ExecutionFailed(format!("failed to write to WASM stdin: {e}")))?;
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
        let result = self.read_execution_output(inst, options, max_output).await;

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

            if trimmed == SENTINEL_START {
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
            if trimmed == SENTINEL_END {
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
    memory_limit: usize,
    wasm_stdin: DuplexStream,
    wasm_stdout: DuplexStream,
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
    wasi_builder.inherit_stderr(); // stderr goes to host for debugging

    // Preopen Python stdlib (read-only)
    wasi_builder
        .preopened_dir(stdlib_dir, "/usr/lib/python3", DirPerms::READ, FilePerms::READ)
        .map_err(|e| {
            ExecutorError::NotReady(format!("failed to preopen stdlib dir: {e}"))
        })?;

    // Pass args to CPython: python.wasm -c <wrapper_script>
    wasi_builder.args(&["python.wasm", "-c", PYTHON_WRAPPER]);

    // Set environment variables for reproducible behavior
    wasi_builder.env("PYTHONDONTWRITEBYTECODE", "1");
    wasi_builder.env("PYTHONNOUSERSITE", "1");
    wasi_builder.env("PYTHONPATH", "/usr/lib/python3");

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
    async fn execute(
        &self,
        code: &str,
        language: Language,
    ) -> Result<ExecutionResult, ExecutorError> {
        self.execute_with_tools(code, language, &ExecutionOptions::default())
            .await
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
        tracing::debug!("WASM executor shut down");
        Ok(())
    }

    async fn is_alive(&self) -> bool {
        let guard = self.instance.lock().await;
        guard.is_some()
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
