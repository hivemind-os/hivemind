//! Subprocess-based Python executor (MVP implementation).
//!
//! Spawns a persistent Python subprocess in REPL mode and communicates
//! via stdin/stdout pipes with sentinel markers for output delineation.
//!
//! This is the initial implementation. The target architecture uses
//! Wasmtime + Pyodide for stronger WASM-level sandboxing. The subprocess
//! executor serves as the development/testing backend and as a fallback
//! when the WASM runtime is not available.

use crate::executor::{CodeExecutor, ExecutionResult, ExecutorConfig, ExecutorError, Language};
use crate::tool_bridge::{self, ExecutionOptions, ToolCallResponse};
use std::process::Stdio;
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

/// Sentinel markers used to delineate code execution boundaries in the
/// subprocess output stream. Augmented with a per-session nonce to prevent
/// accidental collision with user-printed text.
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

/// Build sentinel strings from a nonce.
fn sentinel_start(nonce: &str) -> String {
    format!("__HIVEMIND_EXEC_START_{nonce}__")
}

fn sentinel_end(nonce: &str) -> String {
    format!("__HIVEMIND_EXEC_END_{nonce}__")
}

/// Generate the Python wrapper script with per-session nonce sentinels.
fn generate_python_wrapper(nonce: &str) -> String {
    let start = sentinel_start(nonce);
    let end = sentinel_end(nonce);
    format!(
        r#"
import sys, io, traceback

# Store original I/O handles BEFORE any redirection.
# The tool bridge function uses these to communicate with the host.
_original_stdout = sys.stdout
_original_stdin = sys.stdin

def _hivemind_exec_loop():
    while True:
        # Read until we get the start sentinel
        line = _original_stdin.readline()
        if not line:
            break
        line = line.strip()
        if line != "{start}":
            continue

        # Collect code lines until end sentinel
        code_lines = []
        while True:
            line = _original_stdin.readline()
            if not line:
                return
            if line.strip() == "{end}":
                break
            code_lines.append(line)

        code = "".join(code_lines)

        # Execute the code with stdout/stderr capture
        old_stdout = sys.stdout
        old_stderr = sys.stderr
        captured_stdout = io.StringIO()
        captured_stderr = io.StringIO()
        sys.stdout = captured_stdout
        sys.stderr = captured_stderr
        is_error = False

        try:
            # Use exec for statements, eval won't work for multi-line code
            exec(compile(code, "<codeact>", "exec"), globals())
        except Exception:
            is_error = True
            traceback.print_exc(file=captured_stderr)
        finally:
            sys.stdout = old_stdout
            sys.stderr = old_stderr

        stdout_val = captured_stdout.getvalue()
        stderr_val = captured_stderr.getvalue()

        # Write results with sentinels to the ORIGINAL stdout
        # (not sys.stdout, which user code might have replaced)
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

/// A code executor backed by a persistent Python subprocess.
pub struct SubprocessExecutor {
    child: Mutex<Option<ChildProcess>>,
    config: ExecutorConfig,
    /// Per-session nonce for sentinel uniqueness.
    nonce: String,
}

struct ChildProcess {
    child: Child,
    stdin: tokio::process::ChildStdin,
    stdout_reader: BufReader<tokio::process::ChildStdout>,
}

impl SubprocessExecutor {
    /// Create a new subprocess executor, spawning the Python process.
    pub async fn new(config: ExecutorConfig) -> Result<Self, ExecutorError> {
        let nonce = generate_nonce();
        let child = Self::spawn_python(&config, &nonce).await?;
        Ok(Self {
            child: Mutex::new(Some(child)),
            config,
            nonce,
        })
    }

    /// Execute code with optional tool call support.
    ///
    /// When `options.tool_call_handler` is set, the Python code can call
    /// bridged tool functions. Tool calls are intercepted in the output
    /// stream and dispatched to the handler. Results are written back
    /// to the subprocess's stdin.
    pub async fn execute_with_tools(
        &self,
        code: &str,
        _language: Language,
        options: &ExecutionOptions<'_>,
    ) -> Result<ExecutionResult, ExecutorError> {
        let start = Instant::now();
        let mut guard = self.child.lock().await;
        let proc = guard
            .as_mut()
            .ok_or_else(|| ExecutorError::NotReady("executor has been shut down".into()))?;

        // Send code to the subprocess
        let s_start = sentinel_start(&self.nonce);
        let s_end = sentinel_end(&self.nonce);
        let payload = format!("{s_start}\n{code}\n{s_end}\n");
        proc.stdin
            .write_all(payload.as_bytes())
            .await
            .map_err(|e| ExecutorError::ExecutionFailed(format!("failed to write to stdin: {e}")))?;
        proc.stdin
            .flush()
            .await
            .map_err(|e| ExecutorError::ExecutionFailed(format!("failed to flush stdin: {e}")))?;

        // Read output until we get the end sentinel.
        // During execution, the Python bridge may emit tool call frames on stdout.
        // We intercept those and dispatch them to the handler.
        let timeout = self.config.execution_timeout();
        let max_output = self.config.max_output_bytes;

        let result = tokio::time::timeout(timeout, async {
            let mut stdout = String::new();
            let mut stderr = String::new();
            let mut is_error = false;
            let mut started = false;
            let mut in_stderr = false;
            let mut total_bytes = 0usize;

            loop {
                let mut line = String::new();
                let n = proc
                    .stdout_reader
                    .read_line(&mut line)
                    .await
                    .map_err(|e| {
                        ExecutorError::ExecutionFailed(format!("failed to read stdout: {e}"))
                    })?;
                if n == 0 {
                    return Err(ExecutorError::ExecutionFailed(
                        "Python process exited unexpectedly".into(),
                    ));
                }

                let trimmed = line.trim();

                // Check for tool call frame (can appear before or after EXEC_START)
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
                    proc.stdin
                        .write_all(format!("{resp_json}\n").as_bytes())
                        .await
                        .map_err(|e| {
                            ExecutorError::ExecutionFailed(format!(
                                "failed to write tool response: {e}"
                            ))
                        })?;
                    proc.stdin.flush().await.map_err(|e| {
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
                duration_ms: start.elapsed().as_millis() as u64,
            })
        })
        .await;

        match result {
            Ok(inner) => {
                // If the process exited unexpectedly (EOF on stdout), clear
                // the guard so is_alive() returns false and recovery kicks in.
                if let Err(ExecutorError::ExecutionFailed(ref msg)) = inner {
                    if msg.contains("exited unexpectedly") || msg.contains("failed to read stdout") {
                        *guard = None;
                    }
                }
                inner
            }
            Err(_) => {
                // Timeout — kill the subprocess and mark as error
                if let Some(ref mut proc) = guard.as_mut() {
                    let _ = proc.child.kill().await;
                }
                *guard = None;
                Err(ExecutorError::Timeout {
                    timeout_secs: self.config.execution_timeout_secs,
                })
            }
        }
    }

    async fn spawn_python(config: &ExecutorConfig, nonce: &str) -> Result<ChildProcess, ExecutorError> {
        let wrapper = generate_python_wrapper(nonce);
        // On Windows, `python3` often resolves to a Microsoft Store alias that
        // spawns successfully but exits immediately. Try `python` first on Windows.
        #[cfg(target_os = "windows")]
        let candidates = ["python", "python3"];
        #[cfg(not(target_os = "windows"))]
        let candidates = ["python3", "python"];

        let mut last_err = None;
        let mut child: Option<Child> = None;

        for name in &candidates {
            let mut cmd = Command::new(name);
            cmd.arg("-u") // unbuffered
                .arg("-c")
                .arg(&wrapper)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                // Redirect stderr to null — we capture Python stderr via StringIO
                // in the wrapper. Piping without reading causes deadlocks when
                // native code (e.g. SSL/urllib) writes to the C stderr fd.
                .stderr(Stdio::null());
            if let Some(ref cwd) = config.working_directory {
                cmd.current_dir(cwd);
            }
            match cmd.spawn() {
                Ok(c) => {
                    child = Some(c);
                    break;
                }
                Err(e) => {
                    last_err = Some(e);
                }
            }
        }

        let mut child = child.ok_or_else(|| {
            ExecutorError::NotReady(format!(
                "failed to spawn Python (tried {:?}): {}",
                candidates,
                last_err.map_or("unknown".to_string(), |e| e.to_string())
            ))
        })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| ExecutorError::NotReady("no stdin on Python process".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ExecutorError::NotReady("no stdout on Python process".into()))?;
        let stdout_reader = BufReader::new(stdout);

        Ok(ChildProcess {
            child,
            stdin,
            stdout_reader,
        })
    }
}

#[async_trait::async_trait]
impl CodeExecutor for SubprocessExecutor {
    async fn execute_with_tools(
        &self,
        code: &str,
        language: Language,
        options: &ExecutionOptions<'_>,
    ) -> Result<ExecutionResult, ExecutorError> {
        // Delegate to the concrete method
        SubprocessExecutor::execute_with_tools(self, code, language, options).await
    }

    async fn reset(&self) -> Result<(), ExecutorError> {
        let mut guard = self.child.lock().await;
        // Kill existing process
        if let Some(ref mut proc) = guard.as_mut() {
            let _ = proc.child.kill().await;
        }
        // Spawn fresh process
        *guard = Some(Self::spawn_python(&self.config, &self.nonce).await?);
        tracing::debug!("subprocess executor reset — fresh Python process");
        Ok(())
    }

    async fn shutdown(&self) -> Result<(), ExecutorError> {
        let mut guard = self.child.lock().await;
        if let Some(ref mut proc) = guard.as_mut() {
            let _ = proc.child.kill().await;
        }
        *guard = None;
        tracing::debug!("subprocess executor shut down");
        Ok(())
    }

    async fn is_alive(&self) -> bool {
        let mut guard = self.child.lock().await;
        match guard.as_mut() {
            None => false,
            Some(proc) => {
                // Check if the child process has actually exited
                match proc.child.try_wait() {
                    Ok(Some(_status)) => {
                        // Process has exited — clear the guard so reset() can respawn
                        *guard = None;
                        false
                    }
                    Ok(None) => true, // Still running
                    Err(_) => {
                        // Can't query status — assume dead
                        *guard = None;
                        false
                    }
                }
            }
        }
    }
}

/// Kill the child process on drop to prevent zombie processes.
impl Drop for SubprocessExecutor {
    fn drop(&mut self) {
        // Use try_lock to avoid blocking in the destructor.
        // If the lock is held (e.g. during execution), the child will
        // be cleaned up when the process exits.
        if let Ok(mut guard) = self.child.try_lock() {
            if let Some(ref mut proc) = guard.as_mut() {
                // Best-effort kill — can't await in Drop, so use start_kill
                let _ = proc.child.start_kill();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn make_executor() -> Result<SubprocessExecutor, ExecutorError> {
        SubprocessExecutor::new(ExecutorConfig {
            execution_timeout_secs: 10,
            max_output_bytes: 1_000_000,
            memory_limit_mb: 256,
            working_directory: None,
            allow_network: false,
        })
        .await
    }

    #[tokio::test]
    async fn basic_execution() {
        let exec = match make_executor().await {
            Ok(e) => e,
            Err(_) => {
                eprintln!("Skipping test: Python not available");
                return;
            }
        };

        let result = exec
            .execute("print('hello world')", Language::Python)
            .await
            .unwrap();
        assert!(!result.is_error);
        assert_eq!(result.stdout.trim(), "hello world");
    }

    #[tokio::test]
    async fn state_persists_across_calls() {
        let exec = match make_executor().await {
            Ok(e) => e,
            Err(_) => {
                eprintln!("Skipping test: Python not available");
                return;
            }
        };

        // Set a variable
        let r1 = exec
            .execute("x = 42", Language::Python)
            .await
            .unwrap();
        assert!(!r1.is_error);

        // Read it back
        let r2 = exec
            .execute("print(x)", Language::Python)
            .await
            .unwrap();
        assert!(!r2.is_error);
        assert_eq!(r2.stdout.trim(), "42");
    }

    #[tokio::test]
    async fn error_handling() {
        let exec = match make_executor().await {
            Ok(e) => e,
            Err(_) => {
                eprintln!("Skipping test: Python not available");
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
            Ok(e) => e,
            Err(_) => {
                eprintln!("Skipping test: Python not available");
                return;
            }
        };

        exec.execute("y = 99", Language::Python).await.unwrap();
        exec.reset().await.unwrap();

        let result = exec
            .execute("print(y)", Language::Python)
            .await
            .unwrap();
        assert!(result.is_error);
        assert!(result.stderr.contains("NameError"));
    }

    #[tokio::test]
    async fn multiline_code() {
        let exec = match make_executor().await {
            Ok(e) => e,
            Err(_) => {
                eprintln!("Skipping test: Python not available");
                return;
            }
        };

        let code = "
for i in range(3):
    print(f'item {i}')
";
        let result = exec.execute(code, Language::Python).await.unwrap();
        assert!(!result.is_error);
        assert!(result.stdout.contains("item 0"));
        assert!(result.stdout.contains("item 1"));
        assert!(result.stdout.contains("item 2"));
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
            Ok(e) => e,
            Err(_) => {
                eprintln!("Skipping test: Python not available");
                return;
            }
        };

        // First, inject the bridge code into the Python session
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

        // Inject bridge code
        let r1 = exec
            .execute_with_tools(&bridge_code, Language::Python, &options)
            .await
            .unwrap();
        assert!(!r1.is_error, "Bridge injection failed: {:?}", r1);

        // Now call the bridged tool
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
