pub mod chat;
pub mod comms;
pub mod config;
pub mod connectors;
pub mod daemon;
pub mod daemon_service;
pub mod hardware;
pub mod interaction;
pub mod local_models;
pub mod mcp;
pub mod model_router;
pub mod models;
pub mod permissions;
pub mod prompt_sanitize;
pub mod risk;
pub mod scheduler;
pub mod shell;
pub mod skills;
pub mod tools;
pub mod workspace_classification;

// Re-export all types for convenience
pub use chat::*;
pub use comms::*;
pub use config::*;
pub use connectors::*;
pub use daemon::*;
pub use daemon_service::*;
pub use hardware::*;
pub use interaction::*;
pub use local_models::*;
pub use mcp::*;
pub use model_router::*;
pub use models::*;
pub use permissions::*;
pub use risk::*;
pub use scheduler::*;
pub use shell::*;
pub use skills::*;
pub use tools::*;
pub use workspace_classification::*;

// Re-export classification types that appear in our public API
pub use hive_classification::{ChannelClass, DataClass};
