//! WASM-sandboxed Python code execution for the CodeAct agent loop.
//!
//! This crate provides a [`CodeExecutor`] trait and a Wasmtime-based
//! implementation for running LLM-generated Python code in a true
//! WASM sandbox (memory limits, no filesystem beyond preopened dirs,
//! no network, no subprocess spawning).
//!
//! # Architecture
//!
//! ```text
//! CodeActStrategy (hive-loop)
//!     │
//!     ▼
//! SessionRegistry ── manages per-conversation WASM sessions
//!     │
//!     ▼
//! CodeExecutor trait
//!     │
//!     └── WasmExecutor  (Wasmtime + CPython WASI)
//! ```
//!
//! # Session Lifecycle
//!
//! Each conversation gets a persistent Python execution session. Variables,
//! imports, and state survive across code blocks within the same conversation.
//! Sessions are reaped after an idle timeout or when the conversation ends.

pub mod executor;
pub mod session;
pub mod tool_bridge;
pub mod wasm_executor;

pub use executor::{
    CodeExecutor, ExecutionResult, ExecutorConfig, ExecutorError, Language,
};
pub use session::{Session, SessionConfig, SessionRegistry, WasmRuntime};
pub use wasm_executor::WasmExecutor;
pub use tool_bridge::{
    BridgedToolInfo, CodeActToolMode, ExecutionOptions, ToolCallHandler,
    ToolCallRequest, ToolCallResponse, tool_id_to_python_name,
};

/// Resolved paths to the CPython WASI runtime.
#[derive(Debug, Clone)]
pub struct PythonWasmPaths {
    /// Path to the `python.wasm` binary.
    pub wasm_binary: std::path::PathBuf,
    /// Path to the Python standard library directory (e.g. `lib/python3.12`).
    pub stdlib_dir: std::path::PathBuf,
}

/// Discover the CPython WASI runtime from well-known locations.
///
/// Resolution order:
/// 1. `PYTHON_WASM_PATH` + `PYTHON_WASM_STDLIB` environment variables
/// 2. `{hivemind_home}/runtimes/python-wasm/bin/python.wasm` + `lib/python3.12`
/// 3. Sibling of current executable: `python-wasm/bin/python.wasm` + `lib/python3.12`
/// 4. macOS: `Contents/Resources/python-wasm/` inside the .app bundle
///
/// Returns `None` if no valid runtime is found.
pub fn resolve_python_wasm(hivemind_home: Option<&std::path::Path>) -> Option<PythonWasmPaths> {
    use std::path::PathBuf;

    // Helper: check that both paths exist
    let check = |wasm: PathBuf, stdlib: PathBuf| -> Option<PythonWasmPaths> {
        if wasm.exists() && stdlib.exists() {
            Some(PythonWasmPaths {
                wasm_binary: wasm,
                stdlib_dir: stdlib,
            })
        } else {
            None
        }
    };

    // 1. Environment variables (highest priority)
    if let (Ok(wasm), Ok(stdlib)) = (
        std::env::var("PYTHON_WASM_PATH"),
        std::env::var("PYTHON_WASM_STDLIB"),
    ) {
        if let Some(paths) = check(PathBuf::from(&wasm), PathBuf::from(&stdlib)) {
            tracing::debug!("python.wasm resolved from env vars");
            return Some(paths);
        }
    }

    // 2. HiveMind data directory
    if let Some(home) = hivemind_home {
        let runtime_dir = home.join("runtimes").join("python-wasm");
        let wasm = runtime_dir.join("bin").join("python.wasm");
        // Try multiple Python versions
        for version in &["python3.13", "python3.12", "python3.11"] {
            let stdlib = runtime_dir.join("lib").join(version);
            if let Some(paths) = check(wasm.clone(), stdlib) {
                tracing::debug!(dir = %runtime_dir.display(), "python.wasm resolved from hivemind home");
                return Some(paths);
            }
        }
    }

    // 3. Sibling of current executable
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            let runtime_dir = exe_dir.join("python-wasm");
            let wasm = runtime_dir.join("bin").join("python.wasm");
            for version in &["python3.13", "python3.12", "python3.11"] {
                let stdlib = runtime_dir.join("lib").join(version);
                if let Some(paths) = check(wasm.clone(), stdlib) {
                    tracing::debug!(dir = %runtime_dir.display(), "python.wasm resolved from exe sibling");
                    return Some(paths);
                }
            }

            // 4. macOS .app bundle: Contents/Resources/python-wasm/
            #[cfg(target_os = "macos")]
            {
                if let Some(resources) = exe_dir
                    .parent() // Contents/MacOS/ → Contents/
                    .map(|contents| contents.join("Resources").join("python-wasm"))
                {
                    let wasm = resources.join("bin").join("python.wasm");
                    for version in &["python3.13", "python3.12", "python3.11"] {
                        let stdlib = resources.join("lib").join(version);
                        if let Some(paths) = check(wasm.clone(), stdlib) {
                            tracing::debug!(dir = %resources.display(), "python.wasm resolved from macOS Resources");
                            return Some(paths);
                        }
                    }
                }
            }
        }
    }

    None
}
