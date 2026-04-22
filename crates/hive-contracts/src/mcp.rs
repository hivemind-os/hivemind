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

/// UI metadata for MCP Apps (SEP-1865). Tools with a `resource_uri` have
/// an interactive HTML interface that hosts can render in a sandboxed iframe.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolUiMeta {
    /// URI of the `ui://` resource containing the app HTML.
    #[serde(alias = "resourceUri")]
    pub resource_uri: Option<String>,
    /// Who can access this tool: "model" and/or "app".
    #[serde(default)]
    pub visibility: Option<Vec<String>>,
    /// Content Security Policy declarations.
    pub csp: Option<McpUiCsp>,
    /// Sandbox permissions requested by the UI.
    pub permissions: Option<McpUiPermissions>,
    /// Whether the host should render a border around the app.
    #[serde(alias = "prefersBorder")]
    pub prefers_border: Option<bool>,
}

/// CSP declarations for an MCP App resource.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpUiCsp {
    #[serde(default, alias = "connectDomains")]
    pub connect_domains: Option<Vec<String>>,
    #[serde(default, alias = "resourceDomains")]
    pub resource_domains: Option<Vec<String>>,
    #[serde(default, alias = "frameDomains")]
    pub frame_domains: Option<Vec<String>>,
    #[serde(default, alias = "baseUriDomains")]
    pub base_uri_domains: Option<Vec<String>>,
}

/// Sandbox permissions an MCP App may request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpUiPermissions {
    pub camera: Option<serde_json::Value>,
    pub microphone: Option<serde_json::Value>,
    pub geolocation: Option<serde_json::Value>,
    #[serde(alias = "clipboardWrite")]
    pub clipboard_write: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolInfo {
    pub name: String,
    pub description: String,
    #[serde(alias = "inputSchema")]
    pub input_schema: Value,
    /// MCP Apps UI metadata, extracted from `_meta.ui`.
    #[serde(default, skip_serializing_if = "Option::is_none", alias = "uiMeta")]
    pub ui_meta: Option<McpToolUiMeta>,
}

/// Fetched MCP App UI resource content, ready for rendering in a sandboxed iframe.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpAppResource {
    /// The `ui://` URI of the resource.
    pub uri: String,
    /// The HTML content of the app.
    pub html: String,
    /// UI metadata (CSP, permissions, border preference) from the tool.
    #[serde(skip_serializing_if = "Option::is_none", alias = "uiMeta")]
    pub ui_meta: Option<McpToolUiMeta>,
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
    /// Full raw MCP CallToolResult JSON (for MCP Apps structured forwarding).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpConnectedTool {
    #[serde(alias = "serverId")]
    pub server_id: String,
    #[serde(alias = "channelClass")]
    pub channel_class: ChannelClass,
    pub tool: McpToolInfo,
}

/// Result of reading an MCP resource, with structured content items.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpReadResourceResult {
    pub contents: Vec<McpResourceContent>,
}

/// A single content item from an MCP resource read.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResourceContent {
    pub uri: String,
    #[serde(skip_serializing_if = "Option::is_none", alias = "mimeType")]
    pub mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blob: Option<String>,
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
