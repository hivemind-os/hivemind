//! End-to-end integration tests for the code executor.
//!
//! These tests exercise the FULL execution pipeline:
//! - Code execution via the CodeExecutor TRAIT (not concrete methods)
//! - Tool bridge injection + tool calls from Python back to host
//! - State persistence across calls
//! - Session lifecycle (create, reset, shutdown)
//! - Error handling and edge cases
//!
//! Tests run against real Python — either WASM (if PYTHON_WASM_PATH set)
//! or subprocess fallback. Both backends MUST pass all tests.

use hive_code_executor::{
    BridgedToolInfo, CodeActToolMode, CodeExecutor, ExecutionOptions,
    ExecutorConfig, Language, SubprocessExecutor, ToolCallHandler,
    ToolCallRequest, ToolCallResponse, WasmExecutor,
};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;

// ── Helper: create executors ─────────────────────────────────────────

fn default_config() -> ExecutorConfig {
    ExecutorConfig {
        execution_timeout_secs: 30,
        max_output_bytes: 1_000_000,
        memory_limit_mb: 256,
        working_directory: None,
        allow_network: false,
    }
}

async fn make_subprocess() -> Option<Arc<dyn CodeExecutor>> {
    match SubprocessExecutor::new(default_config()).await {
        Ok(exec) => Some(Arc::new(exec)),
        Err(e) => {
            eprintln!("Skipping subprocess tests: {e}");
            None
        }
    }
}

async fn make_wasm() -> Option<Arc<dyn CodeExecutor>> {
    let wasm_path = std::env::var("PYTHON_WASM_PATH").ok()?;
    let stdlib_path = std::env::var("PYTHON_WASM_STDLIB").ok()?;
    let wasm = PathBuf::from(&wasm_path);
    let stdlib = PathBuf::from(&stdlib_path);
    if !wasm.exists() || !stdlib.exists() {
        eprintln!("Skipping WASM tests: binary not found");
        return None;
    }
    match WasmExecutor::new(default_config(), &wasm, &stdlib).await {
        Ok(exec) => Some(Arc::new(exec)),
        Err(e) => {
            eprintln!("Skipping WASM tests: {e}");
            None
        }
    }
}

/// Get all available executor backends for testing.
async fn all_executors() -> Vec<(&'static str, Arc<dyn CodeExecutor>)> {
    let mut executors = Vec::new();
    if let Some(exec) = make_subprocess().await {
        executors.push(("subprocess", exec));
    }
    if let Some(exec) = make_wasm().await {
        executors.push(("wasm", exec));
    }
    executors
}

// ── Mock tool handler ─────────────────────────────────────────────────

struct MockToolHandler;

#[async_trait::async_trait]
impl ToolCallHandler for MockToolHandler {
    async fn handle_tool_call(&self, req: ToolCallRequest) -> ToolCallResponse {
        match req.tool_id.as_str() {
            "math.add" => {
                let a = req.args["a"].as_f64().unwrap_or(0.0);
                let b = req.args["b"].as_f64().unwrap_or(0.0);
                ToolCallResponse {
                    request_id: req.request_id,
                    result: Some(json!(a + b)),
                    error: None,
                    truncated: false,
                }
            }
            "data.lookup" => {
                let key = req.args["key"].as_str().unwrap_or("unknown");
                let value = match key {
                    "greeting" => "Hello, World!",
                    "answer" => "42",
                    _ => "not found",
                };
                ToolCallResponse {
                    request_id: req.request_id,
                    result: Some(json!(value)),
                    error: None,
                    truncated: false,
                }
            }
            "always.fail" => ToolCallResponse {
                request_id: req.request_id,
                result: None,
                error: Some("this tool always fails".into()),
                truncated: false,
            },
            _ => ToolCallResponse {
                request_id: req.request_id,
                result: None,
                error: Some(format!("unknown tool: {}", req.tool_id)),
                truncated: false,
            },
        }
    }
}

fn test_tools() -> Vec<BridgedToolInfo> {
    vec![
        BridgedToolInfo {
            tool_id: "math.add".into(),
            description: "Add two numbers".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "a": {"type": "number", "description": "First number"},
                    "b": {"type": "number", "description": "Second number"}
                },
                "required": ["a", "b"]
            }),
            mode: CodeActToolMode::Bridged,
        },
        BridgedToolInfo {
            tool_id: "data.lookup".into(),
            description: "Look up a value by key".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "key": {"type": "string", "description": "Lookup key"}
                },
                "required": ["key"]
            }),
            mode: CodeActToolMode::Bridged,
        },
        BridgedToolInfo {
            tool_id: "always.fail".into(),
            description: "A tool that always fails".into(),
            input_schema: json!({
                "type": "object",
                "properties": {},
            }),
            mode: CodeActToolMode::Bridged,
        },
    ]
}

// ── E2E Tests ─────────────────────────────────────────────────────────

/// Test 1: Basic code execution via the TRAIT (not concrete method).
#[tokio::test]
async fn e2e_basic_execution_via_trait() {
    for (name, executor) in all_executors().await {
        let result = executor
            .execute("print('hello from trait')", Language::Python)
            .await
            .unwrap_or_else(|e| panic!("[{name}] basic execution failed: {e}"));
        assert!(
            !result.is_error,
            "[{name}] unexpected error: {}",
            result.stderr
        );
        assert!(
            result.stdout.contains("hello from trait"),
            "[{name}] expected 'hello from trait' in stdout, got: {:?}",
            result.stdout
        );
        executor.shutdown().await.ok();
    }
}

/// Test 2: Tool bridge injection + single tool call from Python.
/// This is the #1 broken path — the whole point of CodeAct.
#[tokio::test]
async fn e2e_tool_call_through_trait() {
    let handler = MockToolHandler;
    let options = ExecutionOptions {
        tool_call_handler: Some(&handler),
    };

    for (name, executor) in all_executors().await {
        // Inject bridge code
        let bridge_code =
            hive_code_executor::tool_bridge::generate_bridge_code(&test_tools());
        let init_result = executor
            .execute_with_tools(&bridge_code, Language::Python, &options)
            .await
            .unwrap_or_else(|e| panic!("[{name}] bridge injection failed: {e}"));
        assert!(
            !init_result.is_error,
            "[{name}] bridge injection error: {}",
            init_result.stderr
        );

        // Call a tool from Python
        let result = executor
            .execute_with_tools(
                "result = math_add(a=3, b=4)\nprint(result)",
                Language::Python,
                &options,
            )
            .await
            .unwrap_or_else(|e| panic!("[{name}] tool call failed: {e}"));
        assert!(
            !result.is_error,
            "[{name}] tool call error: {}",
            result.stderr
        );
        assert!(
            result.stdout.contains("7"),
            "[{name}] expected '7' in output, got: {:?}",
            result.stdout
        );

        executor.shutdown().await.ok();
    }
}

/// Test 3: Multiple tool calls in sequence within one code block.
#[tokio::test]
async fn e2e_multiple_tool_calls_one_block() {
    let handler = MockToolHandler;
    let options = ExecutionOptions {
        tool_call_handler: Some(&handler),
    };

    for (name, executor) in all_executors().await {
        let bridge_code =
            hive_code_executor::tool_bridge::generate_bridge_code(&test_tools());
        executor
            .execute_with_tools(&bridge_code, Language::Python, &options)
            .await
            .unwrap();

        let code = r#"
a = math_add(a=10, b=20)
b = math_add(a=a, b=5)
greeting = data_lookup(key="greeting")
print(f"sum={b}, greeting={greeting}")
"#;
        let result = executor
            .execute_with_tools(code, Language::Python, &options)
            .await
            .unwrap_or_else(|e| panic!("[{name}] multi-tool failed: {e}"));
        assert!(
            !result.is_error,
            "[{name}] multi-tool error: {}",
            result.stderr
        );
        assert!(
            result.stdout.contains("sum=35"),
            "[{name}] expected 'sum=35', got: {:?}",
            result.stdout
        );
        assert!(
            result.stdout.contains("greeting=Hello, World!"),
            "[{name}] expected greeting, got: {:?}",
            result.stdout
        );

        executor.shutdown().await.ok();
    }
}

/// Test 4: Tool call that returns an error — Python should raise RuntimeError.
#[tokio::test]
async fn e2e_tool_call_error_handling() {
    let handler = MockToolHandler;
    let options = ExecutionOptions {
        tool_call_handler: Some(&handler),
    };

    for (name, executor) in all_executors().await {
        let bridge_code =
            hive_code_executor::tool_bridge::generate_bridge_code(&test_tools());
        executor
            .execute_with_tools(&bridge_code, Language::Python, &options)
            .await
            .unwrap();

        let code = r#"
try:
    always_fail()
    print("SHOULD NOT REACH")
except RuntimeError as e:
    print(f"caught: {e}")
"#;
        let result = executor
            .execute_with_tools(code, Language::Python, &options)
            .await
            .unwrap_or_else(|e| panic!("[{name}] error handling failed: {e}"));
        assert!(
            !result.is_error,
            "[{name}] unexpected exec error: {}",
            result.stderr
        );
        assert!(
            result.stdout.contains("caught:"),
            "[{name}] expected 'caught:' in output, got: {:?}",
            result.stdout
        );
        assert!(
            result.stdout.contains("this tool always fails"),
            "[{name}] expected error message, got: {:?}",
            result.stdout
        );
        assert!(
            !result.stdout.contains("SHOULD NOT REACH"),
            "[{name}] exception was not raised"
        );

        executor.shutdown().await.ok();
    }
}

/// Test 5: State persistence — variables survive across execute_with_tools calls.
#[tokio::test]
async fn e2e_state_persists_with_tools() {
    let handler = MockToolHandler;
    let options = ExecutionOptions {
        tool_call_handler: Some(&handler),
    };

    for (name, executor) in all_executors().await {
        let bridge_code =
            hive_code_executor::tool_bridge::generate_bridge_code(&test_tools());
        executor
            .execute_with_tools(&bridge_code, Language::Python, &options)
            .await
            .unwrap();

        // Call 1: store tool result in variable
        executor
            .execute_with_tools(
                "saved_value = math_add(a=100, b=200)",
                Language::Python,
                &options,
            )
            .await
            .unwrap();

        // Call 2: use the variable from call 1
        let result = executor
            .execute_with_tools(
                "print(f'saved={saved_value}')",
                Language::Python,
                &options,
            )
            .await
            .unwrap_or_else(|e| panic!("[{name}] state persistence failed: {e}"));
        assert!(
            !result.is_error,
            "[{name}] state error: {}",
            result.stderr
        );
        assert!(
            result.stdout.contains("saved=300"),
            "[{name}] expected 'saved=300', got: {:?}",
            result.stdout
        );

        executor.shutdown().await.ok();
    }
}

/// Test 6: Reset clears state AND tool bridge.
#[tokio::test]
async fn e2e_reset_clears_everything() {
    let handler = MockToolHandler;
    let options = ExecutionOptions {
        tool_call_handler: Some(&handler),
    };

    for (name, executor) in all_executors().await {
        let bridge_code =
            hive_code_executor::tool_bridge::generate_bridge_code(&test_tools());
        executor
            .execute_with_tools(&bridge_code, Language::Python, &options)
            .await
            .unwrap();

        executor
            .execute_with_tools("x = 42", Language::Python, &options)
            .await
            .unwrap();

        // Reset
        executor.reset().await.unwrap();

        // Variable should be gone
        let result = executor
            .execute("print(x)", Language::Python)
            .await
            .unwrap();
        assert!(
            result.is_error,
            "[{name}] expected NameError after reset, got success: {:?}",
            result.stdout
        );
        assert!(
            result.stderr.contains("NameError"),
            "[{name}] expected NameError, got: {:?}",
            result.stderr
        );

        // Tool bridge should also be gone
        let result = executor
            .execute_with_tools(
                "math_add(a=1, b=2)",
                Language::Python,
                &options,
            )
            .await
            .unwrap();
        assert!(
            result.is_error,
            "[{name}] expected NameError for tool after reset"
        );

        executor.shutdown().await.ok();
    }
}

/// Test 7: Python exception doesn't kill the session.
#[tokio::test]
async fn e2e_exception_recovery() {
    for (name, executor) in all_executors().await {
        let r1 = executor
            .execute("1 / 0", Language::Python)
            .await
            .unwrap();
        assert!(r1.is_error, "[{name}] expected ZeroDivisionError");
        assert!(
            r1.stderr.contains("ZeroDivisionError"),
            "[{name}] expected ZeroDivisionError, got: {}",
            r1.stderr
        );

        // Session should still be alive
        let r2 = executor
            .execute("print('still alive')", Language::Python)
            .await
            .unwrap();
        assert!(
            !r2.is_error,
            "[{name}] session died after exception: {}",
            r2.stderr
        );
        assert!(
            r2.stdout.contains("still alive"),
            "[{name}] expected 'still alive', got: {:?}",
            r2.stdout
        );

        executor.shutdown().await.ok();
    }
}

/// Test 8: Multiline code with loops and data structures.
#[tokio::test]
async fn e2e_complex_code() {
    for (name, executor) in all_executors().await {
        let code = r#"
data = [{"name": "Alice", "score": 95}, {"name": "Bob", "score": 87}]
total = sum(d["score"] for d in data)
avg = total / len(data)
print(f"average={avg}")
names = sorted(d["name"] for d in data)
print(f"names={names}")
"#;
        let result = executor
            .execute(code, Language::Python)
            .await
            .unwrap_or_else(|e| panic!("[{name}] complex code failed: {e}"));
        assert!(
            !result.is_error,
            "[{name}] complex code error: {}",
            result.stderr
        );
        assert!(
            result.stdout.contains("average=91"),
            "[{name}] expected average=91, got: {:?}",
            result.stdout
        );
        assert!(
            result.stdout.contains("Alice") && result.stdout.contains("Bob"),
            "[{name}] expected names, got: {:?}",
            result.stdout
        );

        executor.shutdown().await.ok();
    }
}

/// Test 9: Tool call with no handler configured should produce error.
#[tokio::test]
async fn e2e_tool_call_without_handler() {
    let handler = MockToolHandler;
    let with_handler = ExecutionOptions {
        tool_call_handler: Some(&handler),
    };
    let no_handler = ExecutionOptions::default();

    for (name, executor) in all_executors().await {
        let bridge_code =
            hive_code_executor::tool_bridge::generate_bridge_code(&test_tools());
        executor
            .execute_with_tools(&bridge_code, Language::Python, &with_handler)
            .await
            .unwrap();

        // Call tool WITHOUT handler — should error
        let result = executor
            .execute_with_tools(
                "math_add(a=1, b=2)",
                Language::Python,
                &no_handler,
            )
            .await
            .unwrap();
        assert!(
            result.is_error,
            "[{name}] expected error when calling tool without handler"
        );

        executor.shutdown().await.ok();
    }
}

/// Test 10: Stdlib imports work (json, math, etc).
#[tokio::test]
async fn e2e_stdlib_imports() {
    for (name, executor) in all_executors().await {
        let code = r#"
import json, math
data = json.dumps({"pi": round(math.pi, 4)})
print(data)
"#;
        let result = executor
            .execute(code, Language::Python)
            .await
            .unwrap_or_else(|e| panic!("[{name}] stdlib import failed: {e}"));
        assert!(
            !result.is_error,
            "[{name}] stdlib error: {}",
            result.stderr
        );
        assert!(
            result.stdout.contains("3.1416"),
            "[{name}] expected pi value, got: {:?}",
            result.stdout
        );

        executor.shutdown().await.ok();
    }
}

/// Test 11: is_alive reflects actual state.
#[tokio::test]
async fn e2e_lifecycle() {
    for (name, executor) in all_executors().await {
        assert!(
            executor.is_alive().await,
            "[{name}] executor should be alive initially"
        );

        executor.shutdown().await.unwrap();

        assert!(
            !executor.is_alive().await,
            "[{name}] executor should be dead after shutdown"
        );
    }
}

/// Test 12: Tool result used in computation spanning multiple calls.
#[tokio::test]
async fn e2e_tool_results_in_computation() {
    let handler = MockToolHandler;
    let options = ExecutionOptions {
        tool_call_handler: Some(&handler),
    };

    for (name, executor) in all_executors().await {
        let bridge_code =
            hive_code_executor::tool_bridge::generate_bridge_code(&test_tools());
        executor
            .execute_with_tools(&bridge_code, Language::Python, &options)
            .await
            .unwrap();

        // Get value from tool
        executor
            .execute_with_tools(
                "answer = data_lookup(key='answer')",
                Language::Python,
                &options,
            )
            .await
            .unwrap();

        // Use tool result in computation
        let result = executor
            .execute_with_tools(
                "doubled = int(answer) * 2\nprint(f'doubled={doubled}')",
                Language::Python,
                &options,
            )
            .await
            .unwrap_or_else(|e| panic!("[{name}] computation failed: {e}"));
        assert!(
            !result.is_error,
            "[{name}] computation error: {}",
            result.stderr
        );
        assert!(
            result.stdout.contains("doubled=84"),
            "[{name}] expected 'doubled=84', got: {:?}",
            result.stdout
        );

        executor.shutdown().await.ok();
    }
}
