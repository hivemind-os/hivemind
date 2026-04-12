//! Per-session MCP connection manager.
//!
//! Each chat session (or bot) gets its own `SessionMcpManager` which
//! maintains independent MCP server connections.  Connections are
//! established lazily on first tool use and torn down when the session
//! ends.

use crate::{McpCallToolResult, McpResourceInfo, McpServerLog, McpServerSnapshot, McpServiceError};
use hive_core::{EventBus, McpServerConfig};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Per-session MCP connection manager.
///
/// Wraps an internal `McpService` instance that is private to the session.
/// Connections are created lazily (on first tool call for a given server)
/// and remain open for the lifetime of the session.
#[derive(Clone)]
pub struct SessionMcpManager {
    session_id: String,
    pub(crate) inner: crate::McpService,
    /// Workspace path for this session, used for sandbox policy construction.
    workspace_path: Arc<RwLock<Option<PathBuf>>>,
}

impl SessionMcpManager {
    /// Create from an explicit list of server configs.
    pub fn from_configs(
        session_id: String,
        servers: &[McpServerConfig],
        event_bus: EventBus,
        global_sandbox: Arc<parking_lot::RwLock<hive_contracts::SandboxConfig>>,
    ) -> Self {
        Self {
            session_id,
            inner: crate::McpService::from_configs(servers, event_bus, global_sandbox),
            workspace_path: Arc::new(RwLock::new(None)),
        }
    }

    /// Set the managed Node.js environment handle.
    pub fn with_node_env(mut self, node_env: Arc<hive_node_env::NodeEnvManager>) -> Self {
        self.inner = self.inner.with_node_env(node_env);
        self
    }

    /// Set the managed Python environment handle.
    pub fn with_python_env(mut self, python_env: Arc<hive_python_env::PythonEnvManager>) -> Self {
        self.inner = self.inner.with_python_env(python_env);
        self
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Return the IDs of servers that are enabled in this session's config.
    pub async fn enabled_server_ids(&self) -> Vec<String> {
        let configs = self.inner.server_configs().await;
        configs.into_iter().filter(|c| c.enabled).map(|c| c.id).collect()
    }

    /// Set the workspace path for sandbox policy resolution.
    pub async fn set_workspace_path(&self, path: PathBuf) {
        *self.workspace_path.write().await = Some(path);
    }

    // ── Lazy-connecting tool execution ──────────────────────────────

    /// Lazy-connect helper that passes workspace_path for sandbox support.
    async fn lazy_connect(&self, server_id: &str) -> Result<McpServerSnapshot, McpServiceError> {
        let ws = self.workspace_path.read().await;
        self.inner.ensure_connected_with_workspace(server_id, ws.as_deref()).await
    }

    /// Call a tool on an MCP server, lazily connecting if needed.
    pub async fn call_tool(
        &self,
        server_id: &str,
        tool_name: &str,
        arguments: serde_json::Map<String, Value>,
    ) -> Result<McpCallToolResult, McpServiceError> {
        self.lazy_connect(server_id).await?;
        self.inner.call_tool(server_id, tool_name, arguments).await
    }

    /// List resources from an MCP server, lazily connecting if needed.
    pub async fn list_resources(
        &self,
        server_id: &str,
    ) -> Result<Vec<McpResourceInfo>, McpServiceError> {
        self.lazy_connect(server_id).await?;
        self.inner.list_resources(server_id).await
    }

    /// Read a resource by URI, lazily connecting if needed.
    pub async fn read_resource(
        &self,
        server_id: &str,
        uri: &str,
    ) -> Result<String, McpServiceError> {
        self.lazy_connect(server_id).await?;
        self.inner.read_resource(server_id, uri).await
    }

    /// Subscribe to a resource, lazily connecting if needed.
    pub async fn subscribe_resource(
        &self,
        server_id: &str,
        uri: &str,
    ) -> Result<(), McpServiceError> {
        self.lazy_connect(server_id).await?;
        self.inner.subscribe_resource(server_id, uri).await
    }

    // ── Status & capabilities ───────────────────────────────────────

    /// Check if the server has resource capabilities (requires connection).
    pub async fn server_supports_resources(&self, server_id: &str) -> bool {
        if self.lazy_connect(server_id).await.is_err() {
            return false;
        }
        self.inner.server_supports_resources(server_id).await
    }

    /// Check if the server supports resource subscriptions (requires connection).
    pub async fn server_supports_subscribe(&self, server_id: &str) -> bool {
        if self.lazy_connect(server_id).await.is_err() {
            return false;
        }
        self.inner.server_supports_subscribe(server_id).await
    }

    /// Return the channel class for a server, if configured.
    pub async fn server_channel_class(
        &self,
        server_id: &str,
    ) -> Option<hive_classification::ChannelClass> {
        self.inner.server_channel_class(server_id).await
    }

    /// List all MCP servers and their per-session connection status.
    pub async fn list_servers(&self) -> Vec<McpServerSnapshot> {
        self.inner.list_servers().await
    }

    /// Return server IDs of connected servers that have `reactive: true`.
    pub async fn reactive_server_ids(&self) -> Vec<String> {
        self.inner.reactive_server_ids().await
    }

    /// Return server IDs of connected servers that support resources.
    pub async fn connected_resource_servers(&self) -> Vec<String> {
        self.inner.connected_resource_servers().await
    }

    // ── Explicit connection management ──────────────────────────────

    /// Explicitly connect to a server (e.g. from the UI).
    pub async fn connect(&self, server_id: &str) -> Result<McpServerSnapshot, McpServiceError> {
        let ws = self.workspace_path.read().await;
        self.inner.connect_with_workspace(server_id, ws.as_deref()).await
    }

    /// Explicitly disconnect from a server.
    pub async fn disconnect(&self, server_id: &str) -> Result<McpServerSnapshot, McpServiceError> {
        self.inner.disconnect(server_id).await
    }

    /// Disconnect all connected servers.  Called when the session ends.
    pub async fn disconnect_all(&self) {
        self.inner.disconnect_all().await;
    }

    // ── Logs & notifications ────────────────────────────────────────

    /// Get logs for a specific server in this session.
    pub async fn get_server_logs(
        &self,
        server_id: &str,
    ) -> Result<Vec<McpServerLog>, McpServiceError> {
        self.inner.get_server_logs(server_id).await
    }

    /// Drain pending notifications.
    pub async fn drain_notifications(&self) -> Vec<crate::McpNotificationEvent> {
        self.inner.drain_notifications().await
    }

    /// Update the server list from new configuration (e.g. config change).
    pub async fn update_servers(&self, new_configs: &[McpServerConfig]) {
        self.inner.update_servers(new_configs).await;
    }
}
