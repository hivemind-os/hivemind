//! # hive-plugins
//!
//! Plugin host for Hivemind TypeScript connector plugins.
//!
//! This crate manages the lifecycle of external plugin processes that
//! communicate via JSON-RPC 2.0 over stdio. Plugins provide:
//!
//! - Agent-callable tools (MCP-compatible `tools/list` + `tools/call`)
//! - Configuration schemas (rendered as forms in the desktop UI)
//! - Background polling loops for incoming messages
//! - Lifecycle hooks (activate/deactivate)
//!
//! ## Architecture
//!
//! ```text
//! PluginRegistry ──── discovers/installs/uninstalls plugins
//! PluginHost ──────── spawns & manages plugin processes
//! PluginBridgeTool ── wraps plugin tools for the agent ToolRegistry
//! MessageRouter ───── routes plugin messages into ConnectorService
//! ```

pub mod bridge;
pub mod config_schema;
pub mod host;
pub mod manifest;
pub mod message_router;
pub mod protocol;
pub mod registry;

pub use bridge::PluginBridgeTool;
pub use config_schema::ConfigSchema;
pub use host::{PluginHost, PluginProcess};
pub use manifest::PluginManifest;
pub use message_router::PluginMessageRouter;
pub use registry::PluginRegistry;
