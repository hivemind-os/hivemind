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
    ToolCallRequest, ToolCallResponse,
};
