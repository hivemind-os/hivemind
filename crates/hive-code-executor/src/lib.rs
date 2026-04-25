//! WASM-sandboxed Python code execution for the CodeAct agent loop.
//!
//! This crate provides a [`CodeExecutor`] trait and implementations for
//! running LLM-generated Python code in a sandboxed environment.
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
//!     ├── SubprocessExecutor  (MVP: Python subprocess with sandbox)
//!     └── WasmExecutor        (target: Wasmtime + Pyodide)
//! ```
//!
//! # Session Lifecycle
//!
//! Each conversation gets a persistent Python execution session. Variables,
//! imports, and state survive across code blocks within the same conversation.
//! Sessions are reaped after an idle timeout or when the conversation ends.

pub mod executor;
pub mod session;
pub mod subprocess;
pub mod tool_bridge;

pub use executor::{
    CodeExecutor, ExecutionResult, ExecutorConfig, ExecutorError, Language,
};
pub use session::{Session, SessionConfig, SessionRegistry};
pub use subprocess::SubprocessExecutor;
pub use tool_bridge::{
    BridgedToolInfo, CodeActToolMode, ExecutionOptions, ToolCallHandler,
    ToolCallRequest, ToolCallResponse,
};
