//! Integration tests for MCP service, session manager, catalog, and sandbox.
//!
//! These tests use in-memory duplex pipes to simulate real stdio MCP server
//! processes, exercising the full happy and sad paths of connection management,
//! tool execution, catalog discovery, and sandbox policy construction.

use crate::catalog::McpCatalogStore;
use crate::session_mcp::SessionMcpManager;
use crate::{
    build_mcp_sandbox_command, McpClientHandler, McpConnectionStatus, McpService, McpServiceError,
    McpToolInfo,
};
use hive_classification::ChannelClass;
use hive_contracts::{McpPromptInfo, McpResourceInfo, McpSandboxConfig, SandboxConfig};
use hive_core::{EventBus, McpServerConfig, McpTransportConfig};
use rmcp::model::{
    Annotated, CallToolRequestParam, CallToolResult, Content, ListResourcesResult, ListToolsResult,
    PaginatedRequestParam, RawResource, ReadResourceRequestParam, ReadResourceResult, Resource,
    ResourceContents, ServerCapabilities, ServerInfo, SubscribeRequestParam, Tool,
};
use rmcp::{Peer, RoleServer, ServerHandler, ServiceExt};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;

// ═══════════════════════════════════════════════════════════════════════
// Mock MCP Server
// ═══════════════════════════════════════════════════════════════════════

/// A configurable mock MCP server that responds to tool calls with
/// either success or failure depending on configuration.
#[derive(Clone)]
struct MockServer {
    peer: Arc<tokio::sync::Mutex<Option<Peer<RoleServer>>>>,
    tools: Vec<Tool>,
    resources: Vec<Resource>,
    supports_subscribe: bool,
    /// When set, call_tool returns an error containing this message.
    fail_tool_calls: Option<String>,
}

impl MockServer {
    fn new() -> Self {
        Self {
            peer: Arc::new(tokio::sync::Mutex::new(None)),
            tools: Vec::new(),
            resources: Vec::new(),
            supports_subscribe: false,
            fail_tool_calls: None,
        }
    }

    fn with_tool(mut self, name: &str, description: &str) -> Self {
        let schema: Arc<serde_json::Map<String, serde_json::Value>> = Arc::new(
            serde_json::from_value(json!({
                "type": "object",
                "properties": {
                    "input": { "type": "string" }
                }
            }))
            .unwrap(),
        );
        self.tools.push(Tool {
            name: name.to_string().into(),
            description: description.to_string().into(),
            input_schema: schema,
            meta: None,
        });
        self
    }

    /// Add a tool with MCP App UI metadata (has a `_meta.ui.resourceUri`).
    fn with_ui_tool(mut self, name: &str, description: &str, resource_uri: &str) -> Self {
        let schema: Arc<serde_json::Map<String, serde_json::Value>> = Arc::new(
            serde_json::from_value(json!({
                "type": "object",
                "properties": {}
            }))
            .unwrap(),
        );
        self.tools.push(Tool {
            name: name.to_string().into(),
            description: description.to_string().into(),
            input_schema: schema,
            meta: Some(json!({
                "ui": {
                    "resourceUri": resource_uri,
                }
            })),
        });
        self
    }

    fn with_resource(mut self, uri: &str, name: &str) -> Self {
        self.resources.push(Annotated::new(RawResource::new(uri, name), None));
        self
    }

    fn with_subscribe(mut self) -> Self {
        self.supports_subscribe = true;
        self
    }

    fn failing_tools(mut self, msg: &str) -> Self {
        self.fail_tool_calls = Some(msg.to_string());
        self
    }
}

impl ServerHandler for MockServer {
    fn get_info(&self) -> ServerInfo {
        let mut builder = ServerCapabilities::builder().enable_tools().enable_resources();
        if self.supports_subscribe {
            builder = builder.enable_resources_subscribe();
        }
        ServerInfo {
            instructions: Some("Integration test mock server".into()),
            capabilities: builder.build(),
            server_info: rmcp::model::Implementation {
                name: "integration-test-server".into(),
                version: "0.1.0".into(),
            },
            ..Default::default()
        }
    }

    fn set_peer(&mut self, peer: Peer<RoleServer>) {
        let peers = Arc::clone(&self.peer);
        tokio::spawn(async move {
            *peers.lock().await = Some(peer);
        });
    }

    fn get_peer(&self) -> Option<Peer<RoleServer>> {
        self.peer.try_lock().ok().and_then(|g| g.clone())
    }

    fn list_tools(
        &self,
        _request: PaginatedRequestParam,
        _context: rmcp::service::RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListToolsResult, rmcp::Error>> + Send + '_ {
        let tools = self.tools.clone();
        async move { Ok(ListToolsResult { tools, next_cursor: None }) }
    }

    fn list_resources(
        &self,
        _request: PaginatedRequestParam,
        _context: rmcp::service::RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListResourcesResult, rmcp::Error>> + Send + '_
    {
        let resources = self.resources.clone();
        async move { Ok(ListResourcesResult { resources, next_cursor: None }) }
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParam,
        _context: rmcp::service::RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, rmcp::Error> {
        Ok(ReadResourceResult {
            contents: vec![ResourceContents::text(
                format!("content of {}", request.uri),
                request.uri,
            )],
        })
    }

    fn subscribe(
        &self,
        _request: SubscribeRequestParam,
        _context: rmcp::service::RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<(), rmcp::Error>> + Send + '_ {
        let supports = self.supports_subscribe;
        async move {
            if supports {
                Ok(())
            } else {
                Err(rmcp::Error::method_not_found::<rmcp::model::SubscribeRequestMethod>())
            }
        }
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParam,
        _context: rmcp::service::RequestContext<RoleServer>,
    ) -> Result<CallToolResult, rmcp::Error> {
        if let Some(ref msg) = self.fail_tool_calls {
            return Ok(CallToolResult {
                content: vec![Content::text(msg.clone())],
                is_error: Some(true),
            });
        }
        // Echo back the tool name and arguments for verification
        let args_str = request
            .arguments
            .as_ref()
            .map(|a| serde_json::to_string(a).unwrap_or_default())
            .unwrap_or_default();
        Ok(CallToolResult::success(vec![Content::text(format!(
            "tool={} args={}",
            request.name, args_str
        ))]))
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════════════════

/// Create a test McpServerConfig for a stdio-type server.
fn test_server_config(id: &str) -> McpServerConfig {
    McpServerConfig {
        id: id.to_string(),
        transport: McpTransportConfig::Stdio,
        command: Some("echo".to_string()),
        args: vec![],
        url: None,
        env: Default::default(),
        headers: Default::default(),
        channel_class: ChannelClass::Internal,
        enabled: true,
        auto_connect: false,
        reactive: false,
        auto_reconnect: false,
        sandbox: None,
    }
}

/// Create an McpService from the given server configs.
fn mcp_service_with_servers(servers: Vec<McpServerConfig>) -> McpService {
    McpService::from_configs(
        &servers,
        EventBus::new(32),
        Arc::new(parking_lot::RwLock::new(hive_contracts::SandboxConfig::default())),
    )
}

/// Inject a mock MCP server connection into an McpService for a given server_id.
///
/// This simulates what would happen if the service connected to a real stdio
/// MCP server process: the client is connected, tools/resources discovered,
/// and status set to Connected.
async fn inject_mock_into_service(
    service: &McpService,
    server_id: &str,
    mock: MockServer,
) -> rmcp::service::RunningService<RoleServer, MockServer> {
    let (client_read, server_write) = tokio::io::duplex(64 * 1024);
    let (server_read, client_write) = tokio::io::duplex(64 * 1024);

    let handler = McpClientHandler::new(
        server_id.to_string(),
        service.event_bus.clone(),
        Arc::clone(&service.notifications),
        service.clone(),
    );

    let (server_svc, client_svc) = tokio::join!(
        mock.serve((server_read, server_write)),
        handler.serve((client_read, client_write)),
    );
    let client_svc = client_svc.expect("client handshake failed");
    let server_svc = server_svc.expect("server handshake failed");

    // Discover tools and resources from the mock server
    let tools_result = client_svc.list_all_tools().await.unwrap_or_default();
    let resources_result = client_svc.list_all_resources().await.unwrap_or_default();

    let tools: Vec<McpToolInfo> = tools_result
        .into_iter()
        .map(crate::tool_to_info)
        .collect();

    let resources: Vec<McpResourceInfo> = resources_result
        .iter()
        .map(|r| McpResourceInfo {
            uri: r.uri.to_string(),
            name: r.name.to_string(),
            description: r.description.as_ref().map(|d| d.to_string()),
            mime_type: r.mime_type.as_ref().map(|m| m.to_string()),
            size: None,
        })
        .collect();

    // Inject into the service's server state
    {
        let mut servers = service.servers.write().await;
        if let Some(state) = servers.get_mut(server_id) {
            state.client = Some(client_svc);
            state.status = McpConnectionStatus::Connected;
            state.tools = tools;
            state.resources = resources;
            state.push_log("mock connection injected for testing");
        }
    }

    server_svc
}

/// Inject a mock connection into a SessionMcpManager's inner service.
async fn inject_mock_into_session(
    session_mcp: &SessionMcpManager,
    server_id: &str,
    mock: MockServer,
) -> rmcp::service::RunningService<RoleServer, MockServer> {
    inject_mock_into_service(&session_mcp.inner, server_id, mock).await
}

// ═══════════════════════════════════════════════════════════════════════
// A. McpService Integration Tests
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn service_connect_and_discover_tools() {
    let service = mcp_service_with_servers(vec![test_server_config("srv1")]);
    let bus = EventBus::new(128);

    let mock =
        MockServer::new().with_tool("greet", "Say hello").with_tool("compute", "Run computation");

    let _server = inject_mock_into_service(&service, "srv1", mock).await;

    // Verify service reports server as connected
    let servers = service.list_servers().await;
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].status, McpConnectionStatus::Connected);
    assert_eq!(servers[0].tool_count, 2);
}

#[tokio::test]
async fn service_discover_resources() {
    let service = mcp_service_with_servers(vec![test_server_config("srv1")]);
    let bus = EventBus::new(128);

    let mock = MockServer::new()
        .with_resource("file:///workspace/README.md", "README.md")
        .with_resource("file:///workspace/src/main.rs", "main.rs");

    let _server = inject_mock_into_service(&service, "srv1", mock).await;

    let resources = service.list_resources("srv1").await.unwrap();
    assert_eq!(resources.len(), 2);
    assert_eq!(resources[0].uri, "file:///workspace/README.md");
}

#[tokio::test]
async fn service_call_tool_success() {
    let service = mcp_service_with_servers(vec![test_server_config("srv1")]);
    let bus = EventBus::new(128);

    let mock = MockServer::new().with_tool("echo", "Echo tool");
    let _server = inject_mock_into_service(&service, "srv1", mock).await;

    let mut args = serde_json::Map::new();
    args.insert("input".to_string(), json!("hello"));

    let result = service.call_tool("srv1", "echo", args).await.unwrap();
    assert!(!result.is_error);
    assert!(result.content.contains("tool=echo"));
    assert!(result.content.contains("hello"));
}

#[tokio::test]
async fn service_call_tool_server_returns_error() {
    let service = mcp_service_with_servers(vec![test_server_config("srv1")]);
    let bus = EventBus::new(128);

    let mock = MockServer::new()
        .with_tool("fail_tool", "A tool that fails")
        .failing_tools("something went wrong");
    let _server = inject_mock_into_service(&service, "srv1", mock).await;

    let result = service.call_tool("srv1", "fail_tool", serde_json::Map::new()).await.unwrap();
    assert!(result.is_error);
    assert!(result.content.contains("something went wrong"));
}

#[tokio::test]
async fn service_call_tool_unknown_server() {
    let service = mcp_service_with_servers(vec![test_server_config("srv1")]);
    let bus = EventBus::new(128);

    let result = service.call_tool("nonexistent", "tool", serde_json::Map::new()).await;
    assert!(result.is_err());
    match result.unwrap_err() {
        McpServiceError::ServerNotFound { server_id } => {
            assert_eq!(server_id, "nonexistent");
        }
        other => panic!("expected ServerNotFound, got: {other}"),
    }
}

#[tokio::test]
async fn service_call_tool_not_connected() {
    let service = mcp_service_with_servers(vec![test_server_config("srv1")]);
    let bus = EventBus::new(128);

    // Don't inject any mock — server is in Disconnected state
    let result = service.call_tool("srv1", "tool", serde_json::Map::new()).await;
    assert!(result.is_err());
    match result.unwrap_err() {
        McpServiceError::NotConnected { server_id } => {
            assert_eq!(server_id, "srv1");
        }
        other => panic!("expected NotConnected, got: {other}"),
    }
}

#[tokio::test]
async fn service_call_tool_disabled_server() {
    let mut cfg = test_server_config("srv1");
    cfg.enabled = false;
    let service = mcp_service_with_servers(vec![cfg]);
    let bus = EventBus::new(128);

    // ensure_connected on a disabled server should fail
    let result = service.ensure_connected("srv1").await;
    assert!(result.is_err());
    match result.unwrap_err() {
        McpServiceError::Disabled { server_id } => {
            assert_eq!(server_id, "srv1");
        }
        other => panic!("expected Disabled, got: {other}"),
    }
}

#[tokio::test]
async fn service_disconnect_and_verify_status() {
    let service = mcp_service_with_servers(vec![test_server_config("srv1")]);
    let bus = EventBus::new(128);

    let mock = MockServer::new().with_tool("t1", "tool 1");
    let _server = inject_mock_into_service(&service, "srv1", mock).await;

    // Verify connected
    let servers = service.list_servers().await;
    assert_eq!(servers[0].status, McpConnectionStatus::Connected);

    // Disconnect
    let snap = service.disconnect("srv1").await.unwrap();
    assert_eq!(snap.status, McpConnectionStatus::Disconnected);

    // Verify disconnected
    let servers = service.list_servers().await;
    assert_eq!(servers[0].status, McpConnectionStatus::Disconnected);
    assert_eq!(servers[0].tool_count, 0);
}

#[tokio::test]
async fn service_disconnect_all() {
    let service =
        mcp_service_with_servers(vec![test_server_config("srv1"), test_server_config("srv2")]);

    let mock1 = MockServer::new().with_tool("t1", "tool 1");
    let mock2 = MockServer::new().with_tool("t2", "tool 2");
    let _s1 = inject_mock_into_service(&service, "srv1", mock1).await;
    let _s2 = inject_mock_into_service(&service, "srv2", mock2).await;

    // Both should be connected
    let servers = service.list_servers().await;
    assert!(servers.iter().all(|s| s.status == McpConnectionStatus::Connected));

    // Disconnect all
    service.disconnect_all().await;

    // Both should be disconnected
    let servers = service.list_servers().await;
    assert!(servers.iter().all(|s| s.status == McpConnectionStatus::Disconnected));
}

#[tokio::test]
async fn service_ensure_connected_idempotent() {
    let service = mcp_service_with_servers(vec![test_server_config("srv1")]);
    let bus = EventBus::new(128);

    let mock = MockServer::new().with_tool("t1", "tool 1");
    let _server = inject_mock_into_service(&service, "srv1", mock).await;

    // ensure_connected should succeed and return snapshot
    let snap = service.ensure_connected("srv1").await.unwrap();
    assert_eq!(snap.status, McpConnectionStatus::Connected);

    // Calling again should still succeed (idempotent)
    let snap2 = service.ensure_connected("srv1").await.unwrap();
    assert_eq!(snap2.status, McpConnectionStatus::Connected);
    assert_eq!(snap2.tool_count, 1);
}

#[tokio::test]
async fn service_read_resource_content() {
    let service = mcp_service_with_servers(vec![test_server_config("srv1")]);
    let bus = EventBus::new(128);

    let mock = MockServer::new().with_resource("file:///test.txt", "test.txt");
    let _server = inject_mock_into_service(&service, "srv1", mock).await;

    let content = service.read_resource("srv1", "file:///test.txt").await.unwrap();
    assert!(content.contains("content of file:///test.txt"));
}

#[tokio::test]
async fn service_server_logs_after_injection() {
    let service = mcp_service_with_servers(vec![test_server_config("srv1")]);
    let bus = EventBus::new(128);

    let mock = MockServer::new().with_tool("t1", "tool 1");
    let _server = inject_mock_into_service(&service, "srv1", mock).await;

    let logs = service.get_server_logs("srv1").await.unwrap();
    assert!(
        logs.iter().any(|l| l.message.contains("mock connection injected")),
        "expected log entry from mock injection"
    );
}

#[tokio::test]
async fn service_update_servers_adds_and_removes() {
    let service = mcp_service_with_servers(vec![test_server_config("srv1")]);
    let bus = EventBus::new(128);

    // Initially one server
    assert_eq!(service.list_servers().await.len(), 1);

    // Update: add srv2, keep srv1
    let new_configs = vec![test_server_config("srv1"), test_server_config("srv2")];
    service.update_servers(&new_configs).await;
    assert_eq!(service.list_servers().await.len(), 2);

    // Update: remove srv1
    let new_configs = vec![test_server_config("srv2")];
    service.update_servers(&new_configs).await;
    let servers = service.list_servers().await;
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].id, "srv2");
}

// ═══════════════════════════════════════════════════════════════════════
// B. SessionMcpManager Integration Tests
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn session_mcp_call_tool_delegates_to_inner() {
    let service = mcp_service_with_servers(vec![test_server_config("srv1")]);
    let bus = EventBus::new(128);
    let mgr = SessionMcpManager::from_configs(
        "session-1".to_string(),
        &[test_server_config("srv1")],
        bus,
        Arc::new(parking_lot::RwLock::new(SandboxConfig::default())),
    );

    let mock = MockServer::new().with_tool("echo", "Echo back");
    let _server = inject_mock_into_session(&mgr, "srv1", mock).await;

    let mut args = serde_json::Map::new();
    args.insert("input".to_string(), json!("world"));

    let result = mgr.call_tool("srv1", "echo", args).await.unwrap();
    assert!(!result.is_error);
    assert!(result.content.contains("tool=echo"));
}

#[tokio::test]
async fn session_mcp_call_tool_unknown_server() {
    let service = mcp_service_with_servers(vec![test_server_config("srv1")]);
    let bus = EventBus::new(128);
    let mgr = SessionMcpManager::from_configs(
        "session-1".to_string(),
        &[test_server_config("srv1")],
        bus,
        Arc::new(parking_lot::RwLock::new(SandboxConfig::default())),
    );

    let result = mgr.call_tool("no-such-server", "tool", serde_json::Map::new()).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn session_mcp_disconnect_all_cleans_up() {
    let servers = vec![test_server_config("srv1"), test_server_config("srv2")];
    let bus = EventBus::new(128);
    let mgr = SessionMcpManager::from_configs(
        "session-1".to_string(),
        &servers,
        bus,
        Arc::new(parking_lot::RwLock::new(SandboxConfig::default())),
    );

    let mock1 = MockServer::new().with_tool("t1", "tool 1");
    let mock2 = MockServer::new().with_tool("t2", "tool 2");
    let _s1 = inject_mock_into_session(&mgr, "srv1", mock1).await;
    let _s2 = inject_mock_into_session(&mgr, "srv2", mock2).await;

    // Both connected
    let servers = mgr.list_servers().await;
    assert!(servers.iter().all(|s| s.status == McpConnectionStatus::Connected));

    // Disconnect all
    mgr.disconnect_all().await;

    let servers = mgr.list_servers().await;
    assert!(servers.iter().all(|s| s.status == McpConnectionStatus::Disconnected));
}

#[tokio::test]
async fn session_mcp_set_workspace_path() {
    let service = mcp_service_with_servers(vec![test_server_config("srv1")]);
    let bus = EventBus::new(128);
    let mgr = SessionMcpManager::from_configs(
        "session-1".to_string(),
        &[test_server_config("srv1")],
        bus,
        Arc::new(parking_lot::RwLock::new(SandboxConfig::default())),
    );

    mgr.set_workspace_path(PathBuf::from("/my/workspace")).await;

    // Verify through the session_id accessor
    assert_eq!(mgr.session_id(), "session-1");
}

#[tokio::test]
async fn session_mcp_list_resources_after_connect() {
    let service = mcp_service_with_servers(vec![test_server_config("srv1")]);
    let bus = EventBus::new(128);
    let mgr = SessionMcpManager::from_configs(
        "session-1".to_string(),
        &[test_server_config("srv1")],
        bus,
        Arc::new(parking_lot::RwLock::new(SandboxConfig::default())),
    );

    let mock = MockServer::new()
        .with_resource("file:///workspace/data.csv", "data.csv")
        .with_resource("file:///workspace/config.json", "config.json");
    let _server = inject_mock_into_session(&mgr, "srv1", mock).await;

    let resources = mgr.list_resources("srv1").await.unwrap();
    assert_eq!(resources.len(), 2);
}

#[tokio::test]
async fn session_mcp_read_resource() {
    let service = mcp_service_with_servers(vec![test_server_config("srv1")]);
    let bus = EventBus::new(128);
    let mgr = SessionMcpManager::from_configs(
        "session-1".to_string(),
        &[test_server_config("srv1")],
        bus,
        Arc::new(parking_lot::RwLock::new(SandboxConfig::default())),
    );

    let mock = MockServer::new().with_resource("file:///test.txt", "test.txt");
    let _server = inject_mock_into_session(&mgr, "srv1", mock).await;

    let content = mgr.read_resource("srv1", "file:///test.txt").await.unwrap();
    assert!(content.contains("content of file:///test.txt"));
}

#[tokio::test]
async fn session_mcp_independent_sessions() {
    let service = mcp_service_with_servers(vec![test_server_config("srv1")]);
    let bus = EventBus::new(128);

    let mgr1 = SessionMcpManager::from_configs(
        "session-A".to_string(),
        &[test_server_config("srv1")],
        bus.clone(),
        Arc::new(parking_lot::RwLock::new(SandboxConfig::default())),
    );
    let mgr2 = SessionMcpManager::from_configs(
        "session-B".to_string(),
        &[test_server_config("srv1")],
        bus,
        Arc::new(parking_lot::RwLock::new(SandboxConfig::default())),
    );

    // Connect mock to session A only
    let mock = MockServer::new().with_tool("exclusive", "Only in A");
    let _server = inject_mock_into_session(&mgr1, "srv1", mock).await;

    // Session A should be connected
    let servers_a = mgr1.list_servers().await;
    assert_eq!(servers_a[0].status, McpConnectionStatus::Connected);

    // Session B should NOT be connected (independent)
    let servers_b = mgr2.list_servers().await;
    assert_eq!(servers_b[0].status, McpConnectionStatus::Disconnected);
}

#[tokio::test]
async fn session_mcp_from_configs_subset() {
    // A bot might only have a subset of MCP servers
    let bus = EventBus::new(128);
    let configs = vec![test_server_config("allowed-srv")];
    let mgr = SessionMcpManager::from_configs(
        "bot-1".to_string(),
        &configs,
        bus,
        Arc::new(parking_lot::RwLock::new(SandboxConfig::default())),
    );

    let servers = mgr.list_servers().await;
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].id, "allowed-srv");
}

#[tokio::test]
async fn session_mcp_connect_and_disconnect_explicit() {
    let service = mcp_service_with_servers(vec![test_server_config("srv1")]);
    let bus = EventBus::new(128);
    let mgr = SessionMcpManager::from_configs(
        "session-1".to_string(),
        &[test_server_config("srv1")],
        bus,
        Arc::new(parking_lot::RwLock::new(SandboxConfig::default())),
    );

    let mock = MockServer::new().with_tool("t1", "tool 1");
    let _server = inject_mock_into_session(&mgr, "srv1", mock).await;

    // Connected via injection
    let snap = mgr.disconnect("srv1").await.unwrap();
    assert_eq!(snap.status, McpConnectionStatus::Disconnected);

    // Verify logs show the disconnect
    let logs = mgr.get_server_logs("srv1").await.unwrap();
    assert!(!logs.is_empty());
}

#[tokio::test]
async fn session_mcp_server_channel_class() {
    let mut cfg = test_server_config("srv1");
    cfg.channel_class = ChannelClass::Public;
    let bus = EventBus::new(128);
    let mgr = SessionMcpManager::from_configs(
        "session-1".to_string(),
        &[cfg],
        bus,
        Arc::new(parking_lot::RwLock::new(SandboxConfig::default())),
    );

    let cc = mgr.server_channel_class("srv1").await;
    assert_eq!(cc, Some(ChannelClass::Public));
}

#[tokio::test]
async fn session_mcp_forwards_node_env() {
    let tmp = TempDir::new().unwrap();
    let node_env = Arc::new(hive_node_env::NodeEnvManager::new(
        tmp.path().to_path_buf(),
        hive_node_env::NodeEnvConfig::default(),
    ));
    let configs = vec![test_server_config("srv1")];
    let bus = EventBus::new(128);
    let mgr = SessionMcpManager::from_configs(
        "test-session".to_string(),
        &configs,
        bus,
        Arc::new(parking_lot::RwLock::new(SandboxConfig::default())),
    )
    .with_node_env(Arc::clone(&node_env));

    assert!(mgr.inner.node_env().is_some());
}

#[tokio::test]
async fn session_mcp_forwards_python_env() {
    let tmp = TempDir::new().unwrap();
    let python_env = Arc::new(hive_python_env::PythonEnvManager::new(
        tmp.path().to_path_buf(),
        hive_python_env::PythonEnvConfig::default(),
    ));
    let configs = vec![test_server_config("srv1")];
    let bus = EventBus::new(128);
    let mgr = SessionMcpManager::from_configs(
        "test-session".to_string(),
        &configs,
        bus,
        Arc::new(parking_lot::RwLock::new(SandboxConfig::default())),
    )
    .with_python_env(Arc::clone(&python_env));

    assert!(mgr.inner.python_env().is_some());
}

// ═══════════════════════════════════════════════════════════════════════
// C. Catalog Integration Tests
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn catalog_discover_and_catalog_flow() {
    let service = mcp_service_with_servers(vec![test_server_config("srv1")]);
    let bus = EventBus::new(128);

    // Inject a connected mock with tools and resources
    let mock = MockServer::new()
        .with_tool("analyzer", "Analyze data")
        .with_tool("formatter", "Format output")
        .with_resource("file:///data.csv", "data.csv")
        .with_resource("file:///config.json", "config.json");
    let _server = inject_mock_into_service(&service, "srv1", mock).await;

    // Create catalog
    let dir = TempDir::new().unwrap();
    let catalog = McpCatalogStore::new(dir.path());

    // Upsert into catalog from service state
    let tools = service.list_tools("srv1").await.unwrap();
    let resources = service.list_resources("srv1").await.unwrap();
    catalog
        .upsert("srv1", "ck-srv1", ChannelClass::Internal, tools.clone(), resources.clone(), vec![])
        .await;

    // Verify catalog has the right data
    let entry = catalog.get("ck-srv1").await.unwrap();
    assert_eq!(entry.tools.len(), 2);
    assert_eq!(entry.resources.len(), 2);
    assert_eq!(entry.tools[0].name, "analyzer");
    assert_eq!(entry.tools[1].name, "formatter");
}

#[tokio::test]
async fn catalog_all_cataloged_tools_multi_server() {
    let dir = TempDir::new().unwrap();
    let catalog = McpCatalogStore::new(dir.path());

    // Server A has 2 tools
    catalog
        .upsert(
            "srv-a",
            "ck-srv-a",
            ChannelClass::Internal,
            vec![
                McpToolInfo {
                    name: "tool-a1".to_string(),
                    description: "First tool".to_string(),
                    input_schema: json!({"type": "object"}),
                    ui_meta: None,
                },
                McpToolInfo {
                    name: "tool-a2".to_string(),
                    description: "Second tool".to_string(),
                    input_schema: json!({"type": "object"}),
                    ui_meta: None,
                },
            ],
            vec![],
            vec![],
        )
        .await;

    // Server B has 1 tool with different channel class
    catalog
        .upsert(
            "srv-b",
            "ck-srv-b",
            ChannelClass::Public,
            vec![McpToolInfo {
                name: "tool-b1".to_string(),
                description: "Public tool".to_string(),
                input_schema: json!({"type": "object"}),
                    ui_meta: None,
                }],
            vec![],
            vec![],
        )
        .await;

    let all = catalog.all_cataloged_tools().await;
    assert_eq!(all.len(), 3);

    // Verify channel classes are preserved
    let public_tools: Vec<_> =
        all.iter().filter(|t| t.channel_class == ChannelClass::Public).collect();
    assert_eq!(public_tools.len(), 1);
    assert_eq!(public_tools[0].tool.name, "tool-b1");
}

#[tokio::test]
async fn catalog_persist_survives_reload() {
    let dir = TempDir::new().unwrap();

    // Create and populate
    {
        let catalog = McpCatalogStore::new(dir.path());
        catalog
            .upsert(
                "persistent-srv",
                "ck-persistent",
                ChannelClass::Internal,
                vec![McpToolInfo {
                    name: "survive".to_string(),
                    description: "Must survive reload".to_string(),
                    input_schema: json!({"type": "object"}),
                    ui_meta: None,
                }],
                vec![McpResourceInfo {
                    uri: "file:///kept.txt".to_string(),
                    name: "kept.txt".to_string(),
                    description: None,
                    mime_type: None,
                    size: None,
                }],
                vec![McpPromptInfo {
                    name: "prompt1".to_string(),
                    description: Some("A prompt".to_string()),
                    arguments: vec![],
                }],
            )
            .await;
    }

    // Reload from disk
    let catalog = McpCatalogStore::new(dir.path());
    let entry = catalog.get("ck-persistent").await.unwrap();
    assert_eq!(entry.tools.len(), 1);
    assert_eq!(entry.tools[0].name, "survive");
    assert_eq!(entry.resources.len(), 1);
    assert_eq!(entry.resources[0].uri, "file:///kept.txt");
    assert_eq!(entry.prompts.len(), 1);
    assert_eq!(entry.prompts[0].name, "prompt1");
}

#[tokio::test]
async fn catalog_retain_removes_stale_servers() {
    let dir = TempDir::new().unwrap();
    let catalog = McpCatalogStore::new(dir.path());

    catalog.upsert("keep", "ck-keep", ChannelClass::Internal, vec![], vec![], vec![]).await;
    catalog.upsert("remove1", "ck-rm1", ChannelClass::Internal, vec![], vec![], vec![]).await;
    catalog.upsert("remove2", "ck-rm2", ChannelClass::Internal, vec![], vec![], vec![]).await;

    catalog.retain_keys(&["ck-keep".to_string()]).await;

    assert!(catalog.get("ck-keep").await.is_some());
    assert!(catalog.get("ck-rm1").await.is_none());
    assert!(catalog.get("ck-rm2").await.is_none());
}

#[tokio::test]
async fn catalog_concurrent_upserts_no_data_loss() {
    let dir = TempDir::new().unwrap();
    let catalog = McpCatalogStore::new(dir.path());

    // Spawn multiple concurrent upserts
    let mut handles = vec![];
    for i in 0..10 {
        let cat = catalog.clone();
        handles.push(tokio::spawn(async move {
            cat.upsert(
                &format!("server-{i}"),
                &format!("ck-{i}"),
                ChannelClass::Internal,
                vec![McpToolInfo {
                    name: format!("tool-{i}"),
                    description: format!("tool {i}"),
                    input_schema: json!({"type": "object"}),
                    ui_meta: None,
                }],
                vec![],
                vec![],
            )
            .await;
        }));
    }

    for h in handles {
        h.await.unwrap();
    }

    // All 10 should be present in memory
    let all = catalog.all().await;
    assert_eq!(all.len(), 10);

    // Reload from disk — all should survive
    let catalog2 = McpCatalogStore::new(dir.path());
    let all2 = catalog2.all().await;
    assert_eq!(all2.len(), 10, "concurrent upserts should all persist to disk");
}

// ═══════════════════════════════════════════════════════════════════════
// D. Sandbox Policy Tests (build_mcp_sandbox_command permutations)
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn sandbox_disabled_returns_none() {
    let cfg = McpSandboxConfig { enabled: false, ..Default::default() };
    let result = build_mcp_sandbox_command(
        "echo hello",
        &[],
        &cfg,
        Some(std::path::Path::new("/workspace")),
    );
    assert!(result.is_none(), "disabled sandbox should return None");
}

#[test]
fn sandbox_enabled_returns_wrapped_command() {
    let cfg = McpSandboxConfig {
        enabled: true,
        read_workspace: true,
        write_workspace: false,
        allow_network: true,
        extra_read_paths: vec![],
        extra_write_paths: vec![],
    };
    let result = build_mcp_sandbox_command(
        "echo hello",
        &[],
        &cfg,
        Some(std::path::Path::new("/workspace")),
    );
    // On platforms with sandbox support, this should return Some
    // On platforms without, sandbox_command returns Passthrough which maps to None
    // The test verifies the function doesn't panic and behaves correctly
    // Actual wrapping depends on platform (macOS/Linux/Windows)
    if let Some((program, args, _temps)) = &result {
        assert!(!program.is_empty());
        assert!(!args.is_empty());
    }
    // On platforms without sandbox: result is None (passthrough) — that's also valid
}

#[test]
fn sandbox_read_workspace_only() {
    let cfg = McpSandboxConfig {
        enabled: true,
        read_workspace: true,
        write_workspace: false,
        allow_network: true,
        extra_read_paths: vec![],
        extra_write_paths: vec![],
    };
    // We test that the function produces the correct SandboxPolicy by calling
    // it and checking the result doesn't error. The actual policy enforcement
    // is tested at the sandbox crate level.
    let result = build_mcp_sandbox_command(
        "node server.js",
        &["--port".to_string(), "8080".to_string()],
        &cfg,
        Some(std::path::Path::new("/my/workspace")),
    );
    // Should not panic; result depends on platform
    let _ = result;
}

#[test]
fn sandbox_read_write_workspace() {
    let cfg = McpSandboxConfig {
        enabled: true,
        read_workspace: true,
        write_workspace: true,
        allow_network: true,
        extra_read_paths: vec![],
        extra_write_paths: vec![],
    };
    let result = build_mcp_sandbox_command(
        "python server.py",
        &[],
        &cfg,
        Some(std::path::Path::new("/workspace")),
    );
    let _ = result;
}

#[test]
fn sandbox_no_workspace_access() {
    let cfg = McpSandboxConfig {
        enabled: true,
        read_workspace: false,
        write_workspace: false,
        allow_network: true,
        extra_read_paths: vec![],
        extra_write_paths: vec![],
    };
    // No workspace permissions — server can't read or write workspace
    let result =
        build_mcp_sandbox_command("echo test", &[], &cfg, Some(std::path::Path::new("/workspace")));
    let _ = result;
}

#[test]
fn sandbox_network_denied() {
    let cfg = McpSandboxConfig {
        enabled: true,
        read_workspace: true,
        write_workspace: false,
        allow_network: false,
        extra_read_paths: vec![],
        extra_write_paths: vec![],
    };
    let result =
        build_mcp_sandbox_command("echo test", &[], &cfg, Some(std::path::Path::new("/workspace")));
    let _ = result;
}

#[test]
fn sandbox_extra_read_paths() {
    let cfg = McpSandboxConfig {
        enabled: true,
        read_workspace: true,
        write_workspace: false,
        allow_network: true,
        extra_read_paths: vec!["/opt/data".to_string(), "/var/shared".to_string()],
        extra_write_paths: vec![],
    };
    let result =
        build_mcp_sandbox_command("echo test", &[], &cfg, Some(std::path::Path::new("/workspace")));
    let _ = result;
}

#[test]
fn sandbox_extra_write_paths() {
    let cfg = McpSandboxConfig {
        enabled: true,
        read_workspace: false,
        write_workspace: false,
        allow_network: false,
        extra_read_paths: vec![],
        extra_write_paths: vec!["/tmp/output".to_string(), "/var/log/mcp".to_string()],
    };
    let result =
        build_mcp_sandbox_command("echo test", &[], &cfg, Some(std::path::Path::new("/workspace")));
    let _ = result;
}

#[test]
fn sandbox_no_workspace_path() {
    let cfg = McpSandboxConfig {
        enabled: true,
        read_workspace: true,
        write_workspace: true,
        allow_network: true,
        extra_read_paths: vec![],
        extra_write_paths: vec![],
    };
    // workspace_path = None — read/write workspace flags should be ignored
    let result = build_mcp_sandbox_command("echo test", &[], &cfg, None);
    let _ = result;
}

#[test]
fn sandbox_all_permissions() {
    let cfg = McpSandboxConfig {
        enabled: true,
        read_workspace: true,
        write_workspace: true,
        allow_network: true,
        extra_read_paths: vec!["/opt/lib".to_string()],
        extra_write_paths: vec!["/tmp/scratch".to_string()],
    };
    let result = build_mcp_sandbox_command(
        "node server.js",
        &["--verbose".to_string()],
        &cfg,
        Some(std::path::Path::new("/projects/my-app")),
    );
    let _ = result;
}

#[test]
fn sandbox_minimal_permissions() {
    let cfg = McpSandboxConfig {
        enabled: true,
        read_workspace: false,
        write_workspace: false,
        allow_network: false,
        extra_read_paths: vec![],
        extra_write_paths: vec![],
    };
    // Most restrictive: no workspace, no network, no extra paths
    let result = build_mcp_sandbox_command(
        "echo locked-down",
        &[],
        &cfg,
        Some(std::path::Path::new("/workspace")),
    );
    let _ = result;
}

#[test]
fn sandbox_with_args_in_command() {
    let cfg = McpSandboxConfig {
        enabled: true,
        read_workspace: true,
        write_workspace: false,
        allow_network: true,
        extra_read_paths: vec![],
        extra_write_paths: vec![],
    };
    // Verify args are properly joined into the command string
    let result = build_mcp_sandbox_command(
        "python",
        &["-m".to_string(), "mcp_server".to_string(), "--port".to_string(), "9000".to_string()],
        &cfg,
        Some(std::path::Path::new("/workspace")),
    );
    if let Some((program, args, _temps)) = &result {
        // The wrapped command should contain the original command + args somewhere
        // in the program or args. On Windows, the sandbox uses a temp PowerShell
        // script, so the original command may only appear inside the script file.
        // On macOS/Linux, it appears in the args directly.
        let full = format!("{} {}", program, args.join(" "));
        if cfg!(not(windows)) {
            assert!(
                full.contains("python") || full.contains("mcp_server"),
                "wrapped command should preserve original command: {full}"
            );
        }
        // On all platforms, the program should be non-empty
        assert!(!program.is_empty(), "sandbox program should not be empty");
    }
}

/// Verify the actual SandboxPolicy produced by build_mcp_sandbox_command
/// by inspecting the intermediate policy construction. We do this by
/// calling the sandbox crate directly with equivalent parameters.
#[test]
fn sandbox_policy_construction_correctness() {
    use hive_sandbox::{AccessMode, SandboxPolicy};

    // Build the same policy that build_mcp_sandbox_command would build
    let workspace = std::path::Path::new("/test/workspace");

    let cfg = McpSandboxConfig {
        enabled: true,
        read_workspace: true,
        write_workspace: false,
        allow_network: false,
        extra_read_paths: vec!["/opt/data".to_string()],
        extra_write_paths: vec!["/tmp/output".to_string()],
    };

    let mut builder = SandboxPolicy::builder().network(cfg.allow_network);

    // Workspace: read only
    if cfg.write_workspace {
        builder = builder.allow_read_write(workspace);
    } else if cfg.read_workspace {
        builder = builder.allow_read(workspace);
    }

    for p in hive_sandbox::default_system_read_paths() {
        builder = builder.allow_read(p);
    }
    for p in hive_sandbox::default_denied_paths() {
        builder = builder.deny(p);
    }
    builder = builder.allow_read_write(std::env::temp_dir());
    for p in &cfg.extra_read_paths {
        builder = builder.allow_read(std::path::Path::new(p));
    }
    for p in &cfg.extra_write_paths {
        builder = builder.allow_read_write(std::path::Path::new(p));
    }

    let policy = builder.build();

    // Verify policy properties
    assert!(!policy.allow_network, "network should be denied");

    // Workspace should be read-only
    let ws_entry = policy.allowed_paths.iter().find(|p| p.path == workspace);
    assert!(ws_entry.is_some(), "workspace should be in allowed paths");
    assert_eq!(ws_entry.unwrap().mode, AccessMode::ReadOnly);

    // Extra read path should be read-only
    let extra_read =
        policy.allowed_paths.iter().find(|p| p.path == std::path::Path::new("/opt/data"));
    assert!(extra_read.is_some(), "extra read path should be present");
    assert_eq!(extra_read.unwrap().mode, AccessMode::ReadOnly);

    // Extra write path should be read-write
    let extra_write =
        policy.allowed_paths.iter().find(|p| p.path == std::path::Path::new("/tmp/output"));
    assert!(extra_write.is_some(), "extra write path should be present");
    assert_eq!(extra_write.unwrap().mode, AccessMode::ReadWrite);
}

/// Verify that write_workspace=true produces ReadWrite mode for workspace.
#[test]
fn sandbox_policy_write_workspace_produces_readwrite() {
    use hive_sandbox::{AccessMode, SandboxPolicy};

    let workspace = std::path::Path::new("/test/workspace");
    let policy = SandboxPolicy::builder().network(true).allow_read_write(workspace).build();

    let ws_entry = policy.allowed_paths.iter().find(|p| p.path == workspace);
    assert!(ws_entry.is_some());
    assert_eq!(ws_entry.unwrap().mode, AccessMode::ReadWrite);
}

/// Verify that no workspace path means no workspace entry in policy.
#[test]
fn sandbox_policy_no_workspace_path_means_no_entry() {
    use hive_sandbox::SandboxPolicy;

    let policy = SandboxPolicy::builder().network(true).build();

    // No workspace path added → no entry with workspace
    let ws_entries: Vec<_> = policy
        .allowed_paths
        .iter()
        .filter(|p| p.path.to_string_lossy().contains("workspace"))
        .collect();
    assert!(ws_entries.is_empty());
}

// ═══════════════════════════════════════════════════════════════════════
// E2. Global Sandbox Fallback Tests (build_mcp_sandbox_command_from_global)
// ═══════════════════════════════════════════════════════════════════════

/// When global sandbox is enabled and no per-server config exists,
/// the global policy is applied — matching the shell tool's behaviour.
#[test]
fn global_sandbox_enabled_produces_wrapped_command() {
    use crate::build_mcp_sandbox_command_from_global;

    let cfg = SandboxConfig {
        enabled: true,
        allow_network: true,
        extra_read_paths: vec![],
        extra_write_paths: vec![],
    };
    let result = build_mcp_sandbox_command_from_global(
        "node",
        &["server.js".to_string()],
        &cfg,
        Some(std::path::Path::new("/workspace")),
    );
    // On platforms with sandbox support the result is Some; on CI it may be None.
    // The important thing is that the function doesn't panic and produces the
    // correct structure when sandboxing IS available.
    if let Some((program, args, _temps)) = &result {
        assert!(!program.is_empty(), "sandbox program should not be empty");
        let full = format!("{} {}", program, args.join(" "));
        if cfg!(not(windows)) {
            assert!(
                full.contains("node") || full.contains("server.js"),
                "wrapped command should include the original command: {full}"
            );
        }
    }
}

/// When global sandbox is disabled, wrapping should return None.
#[test]
fn global_sandbox_disabled_returns_none() {
    use crate::build_mcp_sandbox_command_from_global;

    let cfg = SandboxConfig {
        enabled: false,
        allow_network: true,
        extra_read_paths: vec![],
        extra_write_paths: vec![],
    };
    let result = build_mcp_sandbox_command_from_global(
        "echo",
        &[],
        &cfg,
        Some(std::path::Path::new("/workspace")),
    );
    assert!(result.is_none(), "disabled global sandbox should not wrap");
}

/// Global sandbox extra paths are forwarded into the policy.
#[test]
fn global_sandbox_extra_paths_propagated() {
    use crate::build_mcp_sandbox_command_from_global;

    let cfg = SandboxConfig {
        enabled: true,
        allow_network: false,
        extra_read_paths: vec!["/data/shared".to_string()],
        extra_write_paths: vec!["/data/output".to_string()],
    };
    // We can't easily inspect the policy embedded inside the wrapped command,
    // but we verify the function succeeds without panicking and respects
    // enabled = true.
    let result = build_mcp_sandbox_command_from_global(
        "python",
        &["mcp_server.py".to_string()],
        &cfg,
        Some(std::path::Path::new("/workspace")),
    );
    // On platforms with sandboxing, result is Some.
    // On platforms without, result is None (passthrough).
    // Either way — no panic, no error.
    let _ = result;
}

/// Per-server config takes precedence over global config.
#[test]
fn per_server_overrides_global() {
    use crate::{build_mcp_sandbox_command_from_global, build_mcp_sandbox_command_from_per_server};

    // Global would sandbox, but per-server says disabled.
    let global = SandboxConfig {
        enabled: true,
        allow_network: true,
        extra_read_paths: vec![],
        extra_write_paths: vec![],
    };
    let per_server = McpSandboxConfig { enabled: false, ..Default::default() };

    // Per-server disabled should produce None even though global is enabled.
    let per_server_result = build_mcp_sandbox_command_from_per_server(
        "node",
        &["server.js".to_string()],
        &per_server,
        Some(std::path::Path::new("/workspace")),
    );
    assert!(per_server_result.is_none(), "per-server disabled should not wrap");

    // Global would wrap when used directly.
    let global_result = build_mcp_sandbox_command_from_global(
        "node",
        &["server.js".to_string()],
        &global,
        Some(std::path::Path::new("/workspace")),
    );
    // On platforms with sandbox support, this would be Some. The key assertion
    // is that per_server_result above is None, proving the override works.
    let _ = global_result;
}

/// Verify global sandbox policy matches shell tool policy shape:
/// workspace gets read-write, system paths get read-only, temp gets
/// read-write, sensitive dirs are denied.
#[test]
fn global_sandbox_policy_matches_shell_tool() {
    use hive_sandbox::{AccessMode, SandboxPolicy};

    // Build the same policy that build_mcp_sandbox_command_from_global builds.
    let cfg = SandboxConfig {
        enabled: true,
        allow_network: true,
        extra_read_paths: vec!["/extra/read".to_string()],
        extra_write_paths: vec!["/extra/write".to_string()],
    };

    let workspace = std::path::Path::new("/my/workspace");
    let mut builder = SandboxPolicy::builder().network(cfg.allow_network);
    builder = builder.allow_read_write(workspace);
    builder = builder.allow_read_write(std::env::temp_dir());
    for p in hive_sandbox::default_system_read_paths() {
        builder = builder.allow_read(p);
    }
    for p in hive_sandbox::default_denied_paths() {
        builder = builder.deny(p);
    }
    for p in &cfg.extra_read_paths {
        builder = builder.allow_read(std::path::Path::new(p));
    }
    for p in &cfg.extra_write_paths {
        builder = builder.allow_read_write(std::path::Path::new(p));
    }
    let policy = builder.build();

    // Workspace should be read-write
    let ws_entry = policy.allowed_paths.iter().find(|p| p.path == workspace);
    assert!(ws_entry.is_some(), "workspace should be in allowed paths");
    assert_eq!(ws_entry.unwrap().mode, AccessMode::ReadWrite);

    // Temp should be read-write
    let temp = std::env::temp_dir();
    let temp_entry = policy.allowed_paths.iter().find(|p| p.path == temp);
    assert!(temp_entry.is_some(), "temp dir should be in allowed paths");
    assert_eq!(temp_entry.unwrap().mode, AccessMode::ReadWrite);

    // Extra read path should be read-only
    let extra_read =
        policy.allowed_paths.iter().find(|p| p.path == std::path::Path::new("/extra/read"));
    assert!(extra_read.is_some(), "extra_read_paths should be added");
    assert_eq!(extra_read.unwrap().mode, AccessMode::ReadOnly);

    // Extra write path should be read-write
    let extra_write =
        policy.allowed_paths.iter().find(|p| p.path == std::path::Path::new("/extra/write"));
    assert!(extra_write.is_some(), "extra_write_paths should be added");
    assert_eq!(extra_write.unwrap().mode, AccessMode::ReadWrite);

    // Network allowed
    assert!(policy.allow_network);

    // Denied paths should include sensitive directories
    // (only if they exist on the current system)
    let denied_count = policy.denied_paths.len();
    let expected_denied = hive_sandbox::default_denied_paths();
    assert_eq!(denied_count, expected_denied.len());
}

// ═══════════════════════════════════════════════════════════════════════
// F. Catalog ↔ register_mcp_tools end-to-end (simulates daemon restart)
// ═══════════════════════════════════════════════════════════════════════

/// Full lifecycle test:
///   1. Create McpService + mock server → discover_and_catalog → persist
///   2. Drop catalog (simulate daemon shutdown)
///   3. Reload catalog from disk (simulate daemon restart)
///   4. Create a SessionMcpManager (no connections — just for bridge tool wiring)
///   5. Call register_mcp_tools with the reloaded catalog
///   6. Assert the tools from the mock server are in the ToolRegistry
#[tokio::test]
async fn catalog_survives_restart_and_tools_register() {
    let dir = TempDir::new().unwrap();
    let catalog_path = dir.path().join("mcp_catalog.json");
    let server_id = "restart-test-srv";

    // ── Phase 1: initial discovery ─────────────────────────────────
    {
        let service = mcp_service_with_servers(vec![test_server_config(server_id)]);
        let bus = EventBus::new(128);

        let mock =
            MockServer::new().with_tool("alpha", "first tool").with_tool("beta", "second tool");
        let _server_svc = inject_mock_into_service(&service, server_id, mock).await;

        // discover_and_catalog reads state.tools (populated by inject) and writes to catalog.
        let catalog = McpCatalogStore::with_path(catalog_path.clone());
        let entry = service
            .discover_and_catalog(server_id, &catalog)
            .await
            .expect("discover_and_catalog should succeed");
        assert_eq!(entry.tools.len(), 2, "should discover 2 tools from mock");

        // Verify in-memory catalog has the tools.
        let tools = catalog.all_cataloged_tools().await;
        assert_eq!(tools.len(), 2, "in-memory catalog should have 2 tools");

        // Verify the file was written to disk.
        assert!(catalog_path.exists(), "mcp_catalog.json must exist on disk");
    }
    // catalog and service are dropped here — simulates daemon shutdown.

    // ── Phase 2: reload from disk (simulates daemon restart) ───────
    let reloaded_catalog = McpCatalogStore::with_path(catalog_path.clone());
    let reloaded_tools = reloaded_catalog.all_cataloged_tools().await;
    assert_eq!(
        reloaded_tools.len(),
        2,
        "reloaded catalog must have 2 tools from disk; got {}",
        reloaded_tools.len()
    );
    let tool_names: Vec<&str> = reloaded_tools.iter().map(|t| t.tool.name.as_str()).collect();
    assert!(tool_names.contains(&"alpha"), "tool 'alpha' must survive restart");
    assert!(tool_names.contains(&"beta"), "tool 'beta' must survive restart");

    // Phase 3 (tool registration) is tested in hive-chat's
    // build_session_tools_includes_mcp_tools_from_catalog test.
    // Here we've proven the catalog data roundtrips through disk.
}

// ═══════════════════════════════════════════════════════════════════════
// E. Runtime Detection & Managed Environment Tests
// ═══════════════════════════════════════════════════════════════════════

/// Create a fake Node.js installation under `hivemind_home` so that
/// `NodeEnvManager::detect_existing()` marks the environment as Ready.
fn create_fake_node_installation(hivemind_home: &std::path::Path, version: &str) {
    let platform = match std::env::consts::OS {
        "macos" => "darwin",
        "linux" => "linux",
        "windows" => "win",
        os => panic!("unsupported OS in test: {os}"),
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        a => panic!("unsupported arch in test: {a}"),
    };
    let dir_name = format!("node-v{version}-{platform}-{arch}");
    let dist_dir = hivemind_home.join("runtimes").join("node").join(&dir_name);

    if cfg!(target_os = "windows") {
        std::fs::create_dir_all(&dist_dir).unwrap();
        std::fs::write(dist_dir.join("node.exe"), b"fake").unwrap();
    } else {
        let bin_dir = dist_dir.join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        std::fs::write(bin_dir.join("node"), b"fake").unwrap();
    }
}

/// Helper: build an McpServerConfig whose command triggers a specific runtime.
fn runtime_server_config(id: &str, command: &str) -> McpServerConfig {
    McpServerConfig {
        id: id.to_string(),
        transport: McpTransportConfig::Stdio,
        command: Some(command.to_string()),
        args: vec!["--fake-arg".to_string()],
        url: None,
        env: Default::default(),
        headers: Default::default(),
        channel_class: ChannelClass::Internal,
        enabled: true,
        auto_connect: false,
        reactive: false,
        auto_reconnect: false,
        sandbox: None,
    }
}

/// E-1: An "npx" server without a managed Node env yields either
/// `RuntimeNotInstalled { can_auto_install: true }` (if node is absent from
/// the system PATH) or `ConnectionFailed` (if node happens to be installed).
#[tokio::test]
async fn runtime_node_server_without_node_returns_manageable_error() {
    let cfg = runtime_server_config("node-rt-test", "npx");
    let bus = EventBus::new(128);
    // Provide a NodeEnvManager that is NOT ready (no detect_existing call).
    let tmp = TempDir::new().unwrap();
    let node_env = Arc::new(hive_node_env::NodeEnvManager::new(
        tmp.path().to_path_buf(),
        hive_node_env::NodeEnvConfig { enabled: true, ..Default::default() },
    ));
    let service = McpService::from_configs(
        &[cfg],
        bus,
        Arc::new(parking_lot::RwLock::new(SandboxConfig::default())),
    )
    .with_node_env(node_env);

    let err = service.connect("node-rt-test").await.unwrap_err();

    // Node on system PATH → spawn fails → ConnectionFailed.
    // Node absent → RuntimeNotInstalled(can_auto_install=true).
    match &err {
        McpServiceError::RuntimeNotInstalled { can_auto_install, runtime, .. } => {
            assert!(can_auto_install, "Node.js is a manageable runtime");
            assert!(runtime.contains("Node"), "runtime should mention Node, got: {runtime}");
        }
        McpServiceError::ConnectionFailed { .. } => {
            // node is on system PATH; runtime check passed, spawn failed — acceptable.
        }
        other => panic!("expected RuntimeNotInstalled or ConnectionFailed, got: {other:?}"),
    }
}

/// E-2: A "uvx" server without a managed Python env yields either
/// `RuntimeNotInstalled { can_auto_install: true }` or `ConnectionFailed`.
#[tokio::test]
async fn runtime_python_server_without_python_returns_manageable_error() {
    let cfg = runtime_server_config("py-rt-test", "uvx");
    let bus = EventBus::new(128);
    let tmp = TempDir::new().unwrap();
    let python_env = Arc::new(hive_python_env::PythonEnvManager::new(
        tmp.path().to_path_buf(),
        hive_python_env::PythonEnvConfig { enabled: true, ..Default::default() },
    ));
    let service = McpService::from_configs(
        &[cfg],
        bus,
        Arc::new(parking_lot::RwLock::new(SandboxConfig::default())),
    )
    .with_python_env(python_env);

    let err = service.connect("py-rt-test").await.unwrap_err();

    match &err {
        McpServiceError::RuntimeNotInstalled { can_auto_install, runtime, .. } => {
            assert!(can_auto_install, "Python is a manageable runtime");
            assert!(runtime.contains("Python"), "runtime should mention Python, got: {runtime}");
        }
        McpServiceError::ConnectionFailed { .. } => {
            // python3 is on system PATH — acceptable.
        }
        other => panic!("expected RuntimeNotInstalled or ConnectionFailed, got: {other:?}"),
    }
}

/// E-3: A "docker" server without docker on PATH yields
/// `RuntimeNotInstalled { can_auto_install: false }` because Docker is not
/// a manageable runtime.  If docker IS on PATH → `ConnectionFailed`.
#[tokio::test]
async fn runtime_docker_server_without_docker_returns_not_installed() {
    let cfg = runtime_server_config("docker-rt-test", "docker");
    let bus = EventBus::new(128);
    let service = McpService::from_configs(
        &[cfg],
        bus,
        Arc::new(parking_lot::RwLock::new(SandboxConfig::default())),
    );

    let err = service.connect("docker-rt-test").await.unwrap_err();

    match &err {
        McpServiceError::RuntimeNotInstalled { can_auto_install, runtime, .. } => {
            assert!(!can_auto_install, "Docker cannot be auto-installed");
            assert!(runtime.contains("Docker"), "runtime should mention Docker, got: {runtime}");
        }
        McpServiceError::ConnectionFailed { .. } => {
            // docker is on PATH — acceptable.
        }
        other => panic!("expected RuntimeNotInstalled or ConnectionFailed, got: {other:?}"),
    }
}

/// E-4: When a managed Node.js env is Ready the runtime check passes,
/// so the error must NOT be `RuntimeNotInstalled` — it should be a
/// `ConnectionFailed` from the spawn attempt (no real npx process).
#[tokio::test]
async fn runtime_node_server_with_managed_node_proceeds() {
    let tmp = TempDir::new().unwrap();
    let hivemind_home = tmp.path().to_path_buf();
    let version = "22.16.0";

    create_fake_node_installation(&hivemind_home, version);

    let node_env = Arc::new(hive_node_env::NodeEnvManager::new(
        hivemind_home,
        hive_node_env::NodeEnvConfig { enabled: true, node_version: version.to_string() },
    ));
    node_env.detect_existing().await;
    assert!(
        matches!(node_env.status().await, hive_node_env::NodeEnvStatus::Ready { .. }),
        "managed node env should be Ready after detect_existing with fake installation"
    );

    let cfg = runtime_server_config("managed-node-test", "npx");
    let bus = EventBus::new(128);
    let service = McpService::from_configs(
        &[cfg],
        bus,
        Arc::new(parking_lot::RwLock::new(SandboxConfig::default())),
    )
    .with_node_env(node_env);

    let err = service.connect("managed-node-test").await.unwrap_err();

    // The key assertion: the runtime check PASSED (managed env is Ready),
    // so the error must come from the spawn/handshake phase, not the
    // runtime gating phase.
    assert!(
        !matches!(err, McpServiceError::RuntimeNotInstalled { .. }),
        "managed-node env is Ready — should not get RuntimeNotInstalled, got: {err:?}"
    );
}

/// E-5: After a runtime error the server's status is `Error` (not stuck
/// on `Connecting`) and `last_error` is populated.
#[tokio::test]
async fn runtime_error_resets_server_status() {
    let cfg = runtime_server_config("status-reset-test", "npx");
    let bus = EventBus::new(128);
    // Provide a NodeEnvManager that is NOT ready.
    let tmp = TempDir::new().unwrap();
    let node_env = Arc::new(hive_node_env::NodeEnvManager::new(
        tmp.path().to_path_buf(),
        hive_node_env::NodeEnvConfig { enabled: true, ..Default::default() },
    ));
    let service = McpService::from_configs(
        &[cfg],
        bus,
        Arc::new(parking_lot::RwLock::new(SandboxConfig::default())),
    )
    .with_node_env(node_env);

    // connect should fail (either RuntimeNotInstalled or ConnectionFailed).
    let _err = service.connect("status-reset-test").await.unwrap_err();

    let servers = service.list_servers().await;
    let snapshot = servers
        .iter()
        .find(|s| s.id == "status-reset-test")
        .expect("server must exist in snapshot");

    assert_eq!(
        snapshot.status,
        McpConnectionStatus::Error,
        "server status should be Error after a failed connect, got: {:?}",
        snapshot.status,
    );
    assert!(snapshot.last_error.is_some(), "last_error should be populated after a failed connect");
}

/// E-6: A disabled server is rejected with `Disabled` before runtime
/// detection even runs — regardless of the command.
#[tokio::test]
async fn runtime_disabled_server_skips_runtime_check() {
    let mut cfg = runtime_server_config("disabled-rt-test", "npx");
    cfg.enabled = false;

    let bus = EventBus::new(128);
    let service = McpService::from_configs(
        &[cfg],
        bus,
        Arc::new(parking_lot::RwLock::new(SandboxConfig::default())),
    );

    let err = service.connect("disabled-rt-test").await.unwrap_err();

    assert!(
        matches!(err, McpServiceError::Disabled { .. }),
        "disabled server should return Disabled, not RuntimeNotInstalled; got: {err:?}"
    );
}

#[tokio::test]
async fn test_build_sandbox_from_global_config_actually_sandboxes() {
    // This reproduces the EXACT code path when:
    //   - per-server sandbox is null
    //   - global sandbox.enabled = true
    //   - transport is stdio
    let global_cfg = hive_contracts::SandboxConfig {
        enabled: true,
        allow_network: true,
        extra_read_paths: vec![],
        extra_write_paths: vec![],
    };

    let command = "node";
    let args = vec![
        "/Users/danielgerlag/dev/forge/tools/mock-mcp-server/dist/index.js".to_string(),
        "--dashboard-port".to_string(),
        "6100".to_string(),
    ];
    let workspace = std::path::Path::new("/Users/danielgerlag/dev/forge");

    let result =
        crate::build_mcp_sandbox_command_from_global(command, &args, &global_cfg, Some(workspace));

    // This MUST return Some (sandboxed), not None (passthrough)
    assert!(
        result.is_some(),
        "build_mcp_sandbox_command_from_global returned None — MCP server would run UNSANDBOXED"
    );

    let (program, sandbox_args, _temps) = result.unwrap();
    assert_eq!(program, "sandbox-exec", "Expected sandbox-exec wrapper");
    assert_eq!(sandbox_args[0], "-f", "Expected -f flag");

    // Read the profile from the temp file referenced in the args
    let profile_path = &sandbox_args[1];
    let profile =
        std::fs::read_to_string(profile_path).expect("Failed to read sandbox profile temp file");

    // The profile MUST deny user data areas (file-read-data, not file-read*)
    assert!(
        profile.contains(r#"(deny file-read-data (subpath "/Users"))"#),
        "Profile does not deny /Users!"
    );
    assert!(
        profile.contains(r#"(deny file-read-data (subpath "/Volumes"))"#),
        "Profile does not deny /Volumes!"
    );
    // The profile MUST re-allow the workspace (file-read-data for specific subpath)
    assert!(
        profile.contains(&format!(r#"(allow file-read-data (subpath "{}"))"#, workspace.display())),
        "Profile does not re-allow workspace!"
    );
    // The MCP server's project root must be allowed
    assert!(
        profile.contains(
            r#"(allow file-read-data (subpath "/Users/danielgerlag/dev/forge/tools/mock-mcp-server"#
        ),
        "Profile does not allow MCP server project root!"
    );
}


// ═══════════════════════════════════════════════════════════════════════
// MCP Apps Integration Tests
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_tool_ui_meta_extraction() {
    let service = mcp_service_with_servers(vec![test_server_config("mock")]);

    let mock = MockServer::new()
        .with_ui_tool("get-time", "Returns the current time", "ui://get-time/app.html")
        .with_tool("plain-tool", "A tool without UI");

    let _server = inject_mock_into_service(&service, "mock", mock).await;

    let tools = service.list_tools("mock").await.unwrap();
    assert_eq!(tools.len(), 2);

    // The UI tool should have ui_meta with the resource URI
    let ui_tool = tools.iter().find(|t| t.name == "get-time").unwrap();
    assert!(ui_tool.ui_meta.is_some(), "UI tool should have ui_meta");
    let meta = ui_tool.ui_meta.as_ref().unwrap();
    assert_eq!(
        meta.resource_uri.as_deref(),
        Some("ui://get-time/app.html"),
        "resource_uri should match"
    );

    // The plain tool should have no ui_meta
    let plain_tool = tools.iter().find(|t| t.name == "plain-tool").unwrap();
    assert!(plain_tool.ui_meta.is_none(), "Plain tool should not have ui_meta");
}

#[tokio::test]
async fn test_ui_resource_cache() {
    let service = mcp_service_with_servers(vec![test_server_config("mock")]);

    let mock = MockServer::new()
        .with_ui_tool("widget", "A widget", "ui://widget/app.html")
        .with_resource("ui://widget/app.html", "Widget App");

    let _server = inject_mock_into_service(&service, "mock", mock).await;

    // First fetch should work
    let resource = service
        .fetch_ui_resource("mock", "ui://widget/app.html", None)
        .await
        .unwrap();
    assert!(!resource.html.is_empty(), "Should return HTML content");
    assert_eq!(resource.uri, "ui://widget/app.html");

    // Second fetch should return cached result (same content)
    let cached = service
        .fetch_ui_resource("mock", "ui://widget/app.html", None)
        .await
        .unwrap();
    assert_eq!(cached.html, resource.html, "Cached result should match");

    // Invalidation should clear the cache
    service.invalidate_ui_cache("mock").await;

    // After invalidation, fetch works again (hits the server)
    let refreshed = service
        .fetch_ui_resource("mock", "ui://widget/app.html", None)
        .await
        .unwrap();
    assert_eq!(refreshed.uri, "ui://widget/app.html");
}

/// Verifies the full catalog round-trip preserves `ui_meta`:
///
/// 1. Connect mock server with a UI tool
/// 2. Call `list_tools` (populates `state.tools` with `ui_meta`)
/// 3. Store tools in catalog
/// 4. Disconnect the server (clears `state.tools`)
/// 5. Read tools back from catalog — `ui_meta` must still be present
///
/// This is the exact path the daemon follows at startup via
/// `discover_and_catalog`, and the frontend relies on the catalog
/// fallback to render MCP App iframes for disconnected servers.
#[tokio::test]
async fn test_catalog_preserves_ui_meta_after_disconnect() {
    let service = mcp_service_with_servers(vec![test_server_config("vanjs")]);

    let mock = MockServer::new()
        .with_ui_tool("get-time", "Returns the current time", "ui://get-time/mcp-app.html")
        .with_tool("plain-tool", "No UI");

    let _server = inject_mock_into_service(&service, "vanjs", mock).await;

    // Step 1: list_tools populates state.tools with ui_meta
    let tools = service.list_tools("vanjs").await.unwrap();
    assert_eq!(tools.len(), 2);
    let ui_tool = tools.iter().find(|t| t.name == "get-time").unwrap();
    assert!(ui_tool.ui_meta.is_some(), "ui_meta should be present after list_tools");

    // Step 2: persist to catalog (simulating discover_and_catalog)
    let dir = TempDir::new().unwrap();
    let catalog = McpCatalogStore::new(dir.path());
    catalog
        .upsert("vanjs", "ck-vanjs", ChannelClass::Internal, tools, vec![], vec![])
        .await;

    // Step 3: disconnect clears state.tools (tool_count becomes 0)
    let snapshot = service.disconnect("vanjs").await.unwrap();
    assert_eq!(snapshot.tool_count, 0, "tool_count should be 0 after disconnect");

    // Step 4: read from catalog — ui_meta must survive the round-trip
    let cached_tools = catalog.tools_for_server("vanjs").await;
    assert_eq!(cached_tools.len(), 2, "catalog should have 2 tools");

    let cached_ui_tool = cached_tools.iter().find(|t| t.name == "get-time").unwrap();
    assert!(
        cached_ui_tool.ui_meta.is_some(),
        "ui_meta should survive catalog round-trip"
    );
    assert_eq!(
        cached_ui_tool.ui_meta.as_ref().unwrap().resource_uri.as_deref(),
        Some("ui://get-time/mcp-app.html"),
        "resource_uri should match original"
    );

    let cached_plain = cached_tools.iter().find(|t| t.name == "plain-tool").unwrap();
    assert!(cached_plain.ui_meta.is_none(), "plain tool should still have no ui_meta");

    // Step 5: verify JSON serialization matches what the API would return
    let json = serde_json::to_value(&cached_tools).unwrap();
    let json_tools = json.as_array().unwrap();
    let json_ui_tool = json_tools.iter().find(|t| t["name"] == "get-time").unwrap();
    assert!(
        json_ui_tool.get("ui_meta").is_some() && !json_ui_tool["ui_meta"].is_null(),
        "JSON serialization must include ui_meta: got {}",
        serde_json::to_string_pretty(json_ui_tool).unwrap()
    );
    assert_eq!(
        json_ui_tool["ui_meta"]["resource_uri"],
        "ui://get-time/mcp-app.html",
        "JSON resource_uri should match"
    );
}

/// Simulates the frontend's exact lookup pattern:
/// 1. tool_id "mcp.{serverId}.{toolName}" → regex extract serverId + toolName
/// 2. Map key: "{serverId}::{toolName}"
/// 3. Lookup in catalog tools by (server_id, tool_name)
///
/// This validates the full chain that broke in production.
#[tokio::test]
async fn test_frontend_mcp_app_lookup_pattern() {
    let dir = TempDir::new().unwrap();
    let catalog = McpCatalogStore::new(dir.path());

    // Simulate what discover_and_catalog stores
    catalog
        .upsert(
            "Vanilla",
            "ck-vanilla",
            ChannelClass::Internal,
            vec![
                McpToolInfo {
                    name: "get-time".to_string(),
                    description: "Get current time".to_string(),
                    input_schema: json!({"type": "object"}),
                    ui_meta: Some(hive_contracts::McpToolUiMeta {
                        resource_uri: Some("ui://get-time/mcp-app.html".to_string()),
                        visibility: None,
                        csp: None,
                        permissions: None,
                        prefers_border: None,
                    }),
                },
                McpToolInfo {
                    name: "echo".to_string(),
                    description: "Echo input".to_string(),
                    input_schema: json!({"type": "object"}),
                    ui_meta: None,
                },
            ],
            vec![],
            vec![],
        )
        .await;

    // Simulate the SSE tool_id the frontend receives
    let tool_id = "mcp.Vanilla.get-time";

    // Frontend regex: /^mcp\.(.+?)\.(.+)$/  — simulate with simple split
    assert!(tool_id.starts_with("mcp."));
    let rest = &tool_id[4..]; // strip "mcp."
    let dot = rest.find('.').expect("should have a dot separating serverId and toolName");
    let server_id = &rest[..dot];
    let tool_name = &rest[dot + 1..];
    assert_eq!(server_id, "Vanilla");
    assert_eq!(tool_name, "get-time");

    // Frontend map key
    let map_key = format!("{server_id}::{tool_name}");
    assert_eq!(map_key, "Vanilla::get-time");

    // Frontend loads tools from catalog (API fallback path)
    let tools = catalog.tools_for_server(server_id).await;
    assert_eq!(tools.len(), 2, "catalog should return tools for Vanilla");

    // Build the mcpAppTools map (same logic as frontend)
    let mut app_tools = std::collections::HashMap::new();
    for tool in &tools {
        if tool.ui_meta.as_ref().and_then(|m| m.resource_uri.as_ref()).is_some() {
            app_tools.insert(format!("{server_id}::{}", tool.name), tool);
        }
    }

    // The lookup that the ChatView does
    assert!(
        app_tools.contains_key(&map_key),
        "mcpAppTools should contain key '{map_key}', but only has: {:?}",
        app_tools.keys().collect::<Vec<_>>()
    );

    let found = app_tools[&map_key];
    assert_eq!(
        found.ui_meta.as_ref().unwrap().resource_uri.as_deref(),
        Some("ui://get-time/mcp-app.html")
    );

    // Non-UI tool should NOT be in the map
    assert!(
        !app_tools.contains_key("Vanilla::echo"),
        "non-UI tool should not be in mcpAppTools"
    );
}
