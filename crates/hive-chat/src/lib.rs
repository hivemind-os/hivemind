//! # hive-chat
//!
//! Chat orchestration for HiveMind OS. This crate owns session state, message handling,
//! agent-loop integration, persona-aware tool wiring, and canvas/session event bridging
//! for interactive conversations.
//!
//! ## Key exports
//!
//! - [`ChatService`] and [`ChatRuntimeConfig`] — run and configure chat sessions.
//! - [`ApprovalStreamEvent`] and [`SessionEvent`] — stream approvals and session updates.
//! - [`CanvasSessionRegistry`] — track canvas sessions and broadcast canvas events.
//! - [`ChatPersonaToolFactory`] and [`bridge`] — assemble persona-scoped tools and backends.
//!
//! ## Crate relationships
//!
//! Builds on `hive-model`, `hive-inference`, `hive-tools`, `hive-mcp`, and
//! `hive-knowledge`, and is re-exported by `hive-api` for HTTP-facing consumers.
//!
//! ## Usage notes
//!
//! Backend features (`candle`, `llama-cpp`, `onnx`) forward to `hive-inference` so callers can opt into local runtime support.

pub(crate) mod bot_service;
pub mod bridge;
pub mod canvas_ws;
mod chat;
pub(crate) mod indexing_service;
pub mod persona_tool_factory;
pub mod session_log;
pub(crate) mod workflow_context;

pub use canvas_ws::CanvasSessionRegistry;
pub use chat::*;
pub use persona_tool_factory::ChatPersonaToolFactory;
