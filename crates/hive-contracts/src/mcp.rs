use crate::config::McpTransportConfig;
use hive_classification::ChannelClass;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum McpConnectionStatus {
    Disconnected,
    Connecting,
    Connected,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerSnapshot {
    pub id: String,
    pub transport: McpTransportConfig,
    #[serde(alias = "channelClass")]
    pub channel_class: ChannelClass,
    pub enabled: bool,
    #[serde(alias = "autoConnect")]
    pub auto_connect: bool,
    pub reactive: bool,
    pub status: McpConnectionStatus,
    #[serde(alias = "lastError")]
    pub last_error: Option<String>,
    #[serde(alias = "toolCount")]
    pub tool_count: usize,
    #[serde(alias = "resourceCount")]
    pub resource_count: usize,
    #[serde(alias = "promptCount")]
    pub prompt_count: usize,
    #[serde(skip_serializing_if = "Option::is_none", alias = "sandboxStatus")]
    pub sandbox_status: Option<McpSandboxStatus>,
}

/// Runtime sandbox status for an MCP server, included in snapshots
/// so the UI can show the effective sandbox policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpSandboxStatus {
    /// Whether the sandbox is actually active for this server.
    pub active: bool,
    /// Where the sandbox config came from: "per-server", "global", or "none".
    pub source: String,
    #[serde(alias = "allowNetwork")]
    pub allow_network: bool,
    #[serde(alias = "readWorkspace")]
    pub read_workspace: bool,
    #[serde(alias = "writeWorkspace")]
    pub write_workspace: bool,
    #[serde(default, alias = "extraReadPaths")]
    pub extra_read_paths: Vec<String>,
    #[serde(default, alias = "extraWritePaths")]
    pub extra_write_paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolInfo {
    pub name: String,
    pub description: String,
    #[serde(alias = "inputSchema")]
    pub input_schema: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResourceInfo {
    pub uri: String,
    pub name: String,
    pub description: Option<String>,
    #[serde(alias = "mimeType")]
    pub mime_type: Option<String>,
    pub size: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPromptArgumentInfo {
    pub name: String,
    pub description: Option<String>,
    pub required: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPromptInfo {
    pub name: String,
    pub description: Option<String>,
    pub arguments: Vec<McpPromptArgumentInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpCallToolResult {
    pub content: String,
    #[serde(alias = "isError")]
    pub is_error: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpConnectedTool {
    #[serde(alias = "serverId")]
    pub server_id: String,
    #[serde(alias = "channelClass")]
    pub channel_class: ChannelClass,
    pub tool: McpToolInfo,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum McpNotificationKind {
    Cancelled,
    Progress,
    LoggingMessage,
    ResourceUpdated,
    ResourceListChanged,
    ToolListChanged,
    PromptListChanged,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpNotificationEvent {
    #[serde(alias = "serverId")]
    pub server_id: String,
    pub kind: McpNotificationKind,
    pub payload: Value,
    #[serde(alias = "timestampMs")]
    pub timestamp_ms: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerLog {
    #[serde(alias = "timestampMs")]
    pub timestamp_ms: u128,
    pub message: String,
}

/// A catalog entry storing discovered tools, resources, and prompts for an
/// MCP server.  Persisted to disk so that sessions can register bridge tools
/// without connecting first.
///
/// Keyed by `cache_key` — a content hash of the server's transport identity
/// (transport type + command + args + url).  Two personas with identical
/// server configs share the same catalog entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpCatalogEntry {
    #[serde(alias = "serverId")]
    pub server_id: String,
    /// Content-addressed cache key (sha256 of transport identity).
    #[serde(default, alias = "cacheKey")]
    pub cache_key: String,
    #[serde(alias = "channelClass")]
    pub channel_class: ChannelClass,
    pub tools: Vec<McpToolInfo>,
    pub resources: Vec<McpResourceInfo>,
    pub prompts: Vec<McpPromptInfo>,
    /// Unix-epoch milliseconds when this entry was last refreshed.
    #[serde(alias = "lastUpdatedMs")]
    pub last_updated_ms: u128,
}

/// The full on-disk catalog: one entry per known MCP server.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpCatalog {
    pub entries: Vec<McpCatalogEntry>,
}
