pub use hive_contracts::{
    ChannelClass, McpCallToolResult, McpCatalog, McpCatalogEntry, McpConnectedTool,
    McpConnectionStatus, McpNotificationEvent, McpNotificationKind, McpPromptArgumentInfo,
    McpPromptInfo, McpResourceInfo, McpSandboxStatus, McpServerLog, McpServerSnapshot, McpToolInfo,
    McpTransportConfig,
};
use hive_core::{EventBus, McpServerConfig};
use rmcp::model::{
    CallToolRequestParam, LoggingLevel, LoggingMessageNotificationParam, ProgressNotificationParam,
    Prompt, PromptArgument, ReadResourceRequestParam, Resource, ServerCapabilities,
    SetLevelRequestParam, SubscribeRequestParam, Tool,
};
use rmcp::transport::SseTransport;
use rmcp::{ClientHandler, Peer, RoleClient, ServiceError, ServiceExt};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

pub mod catalog;
pub mod runtime;
pub mod session_mcp;
pub mod streamable_http;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tokio::io::AsyncBufReadExt;
use tokio::process::Command;
use tokio::sync::{Mutex, RwLock};

pub use catalog::{CatalogedTool, McpCatalogStore};
pub use session_mcp::SessionMcpManager;

const MAX_NOTIFICATIONS: usize = 200;
const MAX_SERVER_LOGS: usize = 200;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Error)]
pub enum McpServiceError {
    #[error("mcp server `{server_id}` was not found")]
    ServerNotFound { server_id: String },
    #[error("mcp server `{server_id}` is disabled")]
    Disabled { server_id: String },
    #[error("mcp server `{server_id}` is not connected")]
    NotConnected { server_id: String },
    #[error("mcp server `{server_id}` is already connecting")]
    Connecting { server_id: String },
    #[error("mcp server `{server_id}` failed to connect: {detail}")]
    ConnectionFailed { server_id: String, detail: String },
    #[error("mcp server `{server_id}` request failed: {detail}")]
    RequestFailed { server_id: String, detail: String },
    #[error("mcp server `{server_id}` request timed out")]
    RequestTimeout { server_id: String },
    #[error("mcp server `{server_id}` protocol error: {detail}")]
    ProtocolError { server_id: String, detail: String },
    #[error("mcp server `{server_id}` requires {runtime} which is not installed: {install_hint}")]
    RuntimeNotInstalled {
        server_id: String,
        runtime: String,
        install_hint: String,
        can_auto_install: bool,
    },
}

pub(crate) struct McpServerState {
    pub(crate) config: McpServerConfig,
    pub(crate) status: McpConnectionStatus,
    pub(crate) last_error: Option<String>,
    pub(crate) client: Option<rmcp::service::RunningService<RoleClient, McpClientHandler>>,
    /// Handle to the stdio child process (if any). Kept alive so
    /// `kill_on_drop` can clean up on disconnect.
    pub(crate) child_process: Option<tokio::process::Child>,
    pub(crate) logs: VecDeque<McpServerLog>,
    pub(crate) tools: Vec<McpToolInfo>,
    pub(crate) resources: Vec<McpResourceInfo>,
    pub(crate) prompts: Vec<McpPromptInfo>,
    /// Server capabilities reported during initialization.
    pub(crate) capabilities: Option<ServerCapabilities>,
    /// Per-server lifecycle guard — serializes connect/disconnect operations.
    pub(crate) lifecycle: Arc<tokio::sync::Mutex<()>>,
    /// Effective sandbox status, populated during connect.
    pub(crate) sandbox_status: Option<McpSandboxStatus>,
    /// Event bus for publishing realtime log events.
    pub(crate) event_bus: EventBus,
}

impl McpServerState {
    fn new(config: McpServerConfig, event_bus: EventBus) -> Self {
        Self {
            config,
            status: McpConnectionStatus::Disconnected,
            last_error: None,
            client: None,
            child_process: None,
            logs: VecDeque::new(),
            tools: Vec::new(),
            resources: Vec::new(),
            prompts: Vec::new(),
            capabilities: None,
            lifecycle: Arc::new(tokio::sync::Mutex::new(())),
            sandbox_status: None,
            event_bus,
        }
    }

    fn push_log(&mut self, message: impl Into<String>) {
        let log_entry = McpServerLog { timestamp_ms: now_ms(), message: message.into() };
        // Publish realtime log event to the event bus.
        let _ = self.event_bus.publish(
            "mcp.server.log",
            "hive-mcp",
            json!({
                "serverId": self.config.id,
                "log": {
                    "timestampMs": log_entry.timestamp_ms,
                    "message": log_entry.message,
                }
            }),
        );
        if self.logs.len() >= MAX_SERVER_LOGS {
            self.logs.pop_front();
        }
        self.logs.push_back(log_entry);
    }
}

/// Build a `reqwest::Client` with default headers resolved from `McpHeaderValue`.
/// `SecretRef` values are loaded from the OS keystore via `hive_core::secret_store`.
fn build_http_client_with_headers(
    headers: &std::collections::BTreeMap<String, hive_core::McpHeaderValue>,
) -> Result<reqwest::Client, McpServiceError> {
    use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

    let mut header_map = HeaderMap::new();
    for (name, value) in headers {
        let resolved = match value {
            hive_core::McpHeaderValue::Plain(v) => v.clone(),
            hive_core::McpHeaderValue::SecretRef(key) => hive_core::secret_store::load(key)
                .ok_or_else(|| McpServiceError::ConnectionFailed {
                    server_id: String::new(),
                    detail: format!("secret reference '{key}' not found in OS keystore"),
                })?,
        };
        let header_name = HeaderName::from_bytes(name.as_bytes()).map_err(|e| {
            McpServiceError::ConnectionFailed {
                server_id: String::new(),
                detail: format!("invalid header name `{name}`: {e}"),
            }
        })?;
        let header_value =
            HeaderValue::from_str(&resolved).map_err(|e| McpServiceError::ConnectionFailed {
                server_id: String::new(),
                detail: format!("invalid header value for `{name}`: {e}"),
            })?;
        header_map.insert(header_name, header_value);
    }

    reqwest::Client::builder()
        .default_headers(header_map)
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| McpServiceError::ConnectionFailed {
            server_id: String::new(),
            detail: format!("failed to build HTTP client with headers: {e}"),
        })
}

#[derive(Clone)]
pub struct McpService {
    pub(crate) servers: Arc<RwLock<HashMap<String, McpServerState>>>,
    pub(crate) event_bus: EventBus,
    pub(crate) notifications: Arc<Mutex<VecDeque<McpNotificationEvent>>>,
    /// Global sandbox config from `security.sandbox`.  When a server has no
    /// per-server `McpSandboxConfig`, this config controls sandboxing.
    pub(crate) global_sandbox: Arc<parking_lot::RwLock<hive_contracts::SandboxConfig>>,
    /// Managed Node.js environment handle (if enabled).
    pub(crate) node_env: Option<Arc<hive_node_env::NodeEnvManager>>,
    /// Managed Python environment handle (if enabled).
    pub(crate) python_env: Option<Arc<hive_python_env::PythonEnvManager>>,
}

impl McpService {
    /// Create an McpService from an explicit list of server configs.
    pub fn from_configs(
        configs: &[McpServerConfig],
        event_bus: EventBus,
        global_sandbox: Arc<parking_lot::RwLock<hive_contracts::SandboxConfig>>,
    ) -> Self {
        let mut servers = HashMap::new();
        for server in configs {
            servers
                .insert(server.id.clone(), McpServerState::new(server.clone(), event_bus.clone()));
        }

        Self {
            servers: Arc::new(RwLock::new(servers)),
            event_bus,
            notifications: Arc::new(Mutex::new(VecDeque::new())),
            global_sandbox,
            node_env: None,
            python_env: None,
        }
    }

    /// Set the managed Node.js environment handle.
    pub fn with_node_env(mut self, node_env: Arc<hive_node_env::NodeEnvManager>) -> Self {
        self.node_env = Some(node_env);
        self
    }

    /// Set the managed Python environment handle.
    pub fn with_python_env(mut self, python_env: Arc<hive_python_env::PythonEnvManager>) -> Self {
        self.python_env = Some(python_env);
        self
    }

    pub async fn list_servers(&self) -> Vec<McpServerSnapshot> {
        let servers = self.servers.read().await;
        servers.values().map(Self::snapshot_for).collect()
    }

    /// Return a clone of all server configurations.
    pub async fn server_configs(&self) -> Vec<McpServerConfig> {
        let servers = self.servers.read().await;
        servers.values().map(|s| s.config.clone()).collect()
    }

    /// Return a clone of all server configurations (sync version for non-async contexts).
    /// Uses `try_read` which succeeds when no write lock is held — safe in construction contexts.
    pub fn server_configs_sync(&self) -> Vec<McpServerConfig> {
        match self.servers.try_read() {
            Ok(servers) => servers.values().map(|s| s.config.clone()).collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Return the shared global sandbox config Arc.
    pub fn global_sandbox_config(&self) -> Arc<parking_lot::RwLock<hive_contracts::SandboxConfig>> {
        Arc::clone(&self.global_sandbox)
    }

    /// Return the managed Node.js environment handle, if set.
    pub fn node_env(&self) -> Option<Arc<hive_node_env::NodeEnvManager>> {
        self.node_env.clone()
    }

    /// Return the managed Python environment handle, if set.
    pub fn python_env(&self) -> Option<Arc<hive_python_env::PythonEnvManager>> {
        self.python_env.clone()
    }

    pub async fn connect(&self, server_id: &str) -> Result<McpServerSnapshot, McpServiceError> {
        self.connect_impl(server_id, None, true).await
    }

    /// Connect to a server, optionally providing a workspace path for sandbox support.
    pub async fn connect_with_workspace(
        &self,
        server_id: &str,
        workspace_path: Option<&std::path::Path>,
    ) -> Result<McpServerSnapshot, McpServiceError> {
        self.connect_impl(server_id, workspace_path, true).await
    }

    /// Internal connect implementation.
    /// When `use_sandbox` is false, sandbox wrapping is skipped (used for
    /// catalog discovery where we only list tool schemas, not execute tools).
    async fn connect_impl(
        &self,
        server_id: &str,
        workspace_path: Option<&std::path::Path>,
        use_sandbox: bool,
    ) -> Result<McpServerSnapshot, McpServiceError> {
        tracing::info!(
            server_id = %server_id,
            workspace_path = ?workspace_path,
            use_sandbox = use_sandbox,
            "connect_impl called"
        );
        // Acquire per-server lifecycle lock to serialize connect/disconnect.
        let lifecycle_guard = {
            let servers = self.servers.read().await;
            let state = servers.get(server_id).ok_or_else(|| McpServiceError::ServerNotFound {
                server_id: server_id.to_string(),
            })?;
            Arc::clone(&state.lifecycle)
        };
        let _lifecycle = lifecycle_guard.lock().await;

        let config = {
            let mut servers = self.servers.write().await;
            let state = servers.get_mut(server_id).ok_or_else(|| {
                McpServiceError::ServerNotFound { server_id: server_id.to_string() }
            })?;

            if !state.config.enabled {
                return Err(McpServiceError::Disabled { server_id: server_id.to_string() });
            }

            match state.status {
                McpConnectionStatus::Connected => return Ok(Self::snapshot_for(state)),
                McpConnectionStatus::Connecting => {
                    return Err(McpServiceError::Connecting { server_id: server_id.to_string() });
                }
                _ => {}
            }

            state.status = McpConnectionStatus::Connecting;
            state.last_error = None;
            state.config.clone()
        };

        let handler = McpClientHandler::new(
            server_id.to_string(),
            self.event_bus.clone(),
            Arc::clone(&self.notifications),
            self.clone(),
        );

        let service_result = match config.transport {
            McpTransportConfig::Stdio => {
                let servers = Arc::clone(&self.servers);
                let sid = server_id.to_string();
                let result: Result<_, McpServiceError> = async {
                    let (command, args) = parse_command(&config)?;

                    // ── Runtime detection ────────────────────────────
                    // Check if the required runtime is available before
                    // attempting to spawn the process.
                    let detected_runtime = runtime::detect_runtime(&command);
                    let node_env_ready = match &self.node_env {
                        Some(ne) => {
                            matches!(ne.status().await, hive_node_env::NodeEnvStatus::Ready { .. })
                        }
                        None => false,
                    };
                    let python_env_ready = match &self.python_env {
                        Some(pe) => matches!(
                            pe.status().await,
                            hive_python_env::PythonEnvStatus::Ready { .. }
                        ),
                        None => false,
                    };
                    let runtime_status =
                        runtime::check_runtime(detected_runtime, node_env_ready, python_env_ready);

                    // For unknown runtimes, additionally check the actual command on PATH.
                    let runtime_status = if detected_runtime == runtime::McpRuntime::Unknown {
                        if runtime::find_on_path(&command).is_some() {
                            runtime::RuntimeStatus::Available {
                                path: std::path::PathBuf::from(&command),
                            }
                        } else {
                            runtime_status
                        }
                    } else {
                        runtime_status
                    };

                    // If not available, return an actionable error.
                    match &runtime_status {
                        runtime::RuntimeStatus::NotInstalled { install_hint } => {
                            return Err(McpServiceError::RuntimeNotInstalled {
                                server_id: sid.clone(),
                                runtime: detected_runtime.to_string(),
                                install_hint: install_hint.clone(),
                                can_auto_install: false,
                            });
                        }
                        runtime::RuntimeStatus::Manageable { runtime } => {
                            return Err(McpServiceError::RuntimeNotInstalled {
                                server_id: sid.clone(),
                                runtime: runtime.to_string(),
                                install_hint: runtime::install_hint(*runtime).to_string(),
                                can_auto_install: true,
                            });
                        }
                        _ => {} // Available or ManagedAvailable — proceed
                    }

                    // Collect managed runtime env vars to inject.
                    let mut runtime_env: HashMap<String, String> = HashMap::new();
                    if matches!(detected_runtime, runtime::McpRuntime::Node) {
                        if let Some(ref ne) = self.node_env {
                            if let Some(vars) = ne.shell_env_vars().await {
                                runtime_env.extend(vars);
                            }
                        }
                    }
                    if matches!(detected_runtime, runtime::McpRuntime::Python) {
                        if let Some(ref pe) = self.python_env {
                            if let Some(vars) = pe.shell_env_vars(None).await {
                                runtime_env.extend(vars);
                            }
                        }
                    }

                    // Log: spawning
                    {
                        let mut servers = servers.write().await;
                        if let Some(state) = servers.get_mut(&sid) {
                            state.push_log(format!("spawning `{command}` {}", args.join(" ")));
                        }
                    }

                    // Build the command, optionally wrapped in a sandbox.
                    // Per-server McpSandboxConfig takes precedence; otherwise
                    // fall back to the global SandboxConfig (same as shell tool).
                    let (sandboxed, sandbox_status) =
                        if use_sandbox && config.transport == McpTransportConfig::Stdio {
                            if let Some(ref per_server) = config.sandbox {
                                // Explicit per-server config
                                let cmd = if per_server.enabled {
                                    build_mcp_sandbox_command_from_per_server(
                                        &command,
                                        &args,
                                        per_server,
                                        workspace_path,
                                    )
                                } else {
                                    None
                                };
                                // active reflects whether wrapping actually succeeded,
                                // not just the config intent.
                                let status = McpSandboxStatus {
                                    active: cmd.is_some(),
                                    source: "per-server".to_string(),
                                    allow_network: per_server.allow_network,
                                    read_workspace: per_server.read_workspace,
                                    write_workspace: per_server.write_workspace,
                                    extra_read_paths: per_server.extra_read_paths.clone(),
                                    extra_write_paths: per_server.extra_write_paths.clone(),
                                };
                                (cmd, Some(status))
                            } else {
                                // No per-server config — use global sandbox
                                let global = self.global_sandbox.read().clone();
                                let cmd = if global.enabled {
                                    build_mcp_sandbox_command_from_global(
                                        &command,
                                        &args,
                                        &global,
                                        workspace_path,
                                    )
                                } else {
                                    None
                                };
                                // active reflects whether wrapping actually succeeded.
                                let status = McpSandboxStatus {
                                    active: cmd.is_some(),
                                    source: "global".to_string(),
                                    allow_network: global.allow_network,
                                    read_workspace: true,
                                    write_workspace: true,
                                    extra_read_paths: global.extra_read_paths.clone(),
                                    extra_write_paths: global.extra_write_paths.clone(),
                                };
                                (cmd, Some(status))
                            }
                        } else {
                            let status = McpSandboxStatus {
                                active: false,
                                source: "none".to_string(),
                                allow_network: true,
                                read_workspace: true,
                                write_workspace: true,
                                extra_read_paths: Vec::new(),
                                extra_write_paths: Vec::new(),
                            };
                            (None, Some(status))
                        };

                    // Store sandbox status on server state.
                    {
                        let mut servers = servers.write().await;
                        if let Some(state) = servers.get_mut(&sid) {
                            state.sandbox_status = sandbox_status;
                        }
                    }

                    // _temp_files must live until child is spawned.
                    let _temp_files: Vec<tempfile::TempPath>;

                    // On Windows, Command::new() resolves bare command names
                    // using the *parent* process's PATH, not the child's
                    // environment.  Pre-resolve the command against the
                    // effective child PATH so managed runtimes are found.
                    #[cfg(target_os = "windows")]
                    let command = {
                        // Build effective PATH: runtime_env PATH takes
                        // precedence over config env PATH, both override
                        // the system PATH.
                        let effective_path = runtime_env
                            .get("PATH")
                            .or_else(|| runtime_env.get("Path"))
                            .cloned()
                            .or_else(|| {
                                resolve_env(&config).ok().and_then(|pairs| {
                                    pairs.into_iter().find(|(k, _)| k.eq_ignore_ascii_case("PATH")).map(|(_, v)| v)
                                })
                            });
                        if let Some(ref path_var) = effective_path {
                            runtime::resolve_command_in_path(&command, path_var)
                                .map(|p| p.to_string_lossy().into_owned())
                                .unwrap_or(command)
                        } else {
                            command
                        }
                    };

                    let mut cmd = if let Some((program, sandbox_args, temps)) = sandboxed {
                        _temp_files = temps;
                        let mut c = Command::new(&program);
                        c.args(&sandbox_args);
                        {
                            let mut servers = servers.write().await;
                            if let Some(state) = servers.get_mut(&sid) {
                                state.push_log("sandbox enabled for this server");
                            }
                        }
                        c
                    } else {
                        _temp_files = Vec::new();
                        let mut c = Command::new(&command);
                        c.args(&args);
                        c
                    };

                    cmd.kill_on_drop(true)
                        .stdin(std::process::Stdio::piped())
                        .stdout(std::process::Stdio::piped())
                        .stderr(std::process::Stdio::piped());
                    // Set cwd to the workspace (or /tmp) so the sandboxed
                    // process starts in a directory it is allowed to read.
                    // Without this, it inherits the daemon's cwd which may
                    // be inside $HOME and thus blocked by the sandbox.
                    if use_sandbox {
                        if let Some(ws) = workspace_path {
                            cmd.current_dir(ws);
                        } else {
                            cmd.current_dir(std::env::temp_dir());
                        }
                    }
                    // On macOS, Rust uses posix_spawn with
                    // POSIX_SPAWN_CLOEXEC_DEFAULT which can produce
                    // spurious EBADF when the daemon's standard fds
                    // point to /dev/null.  A pre_exec hook (even a
                    // no-op) forces the fork+exec path instead.
                    #[cfg(unix)]
                    unsafe {
                        cmd.pre_exec(|| Ok(()))
                    };
                    for (key, value) in resolve_env(&config)? {
                        cmd.env(key, value);
                    }
                    // Inject managed runtime environment (PATH etc.).
                    for (key, value) in &runtime_env {
                        cmd.env(key, value);
                    }

                    #[cfg(target_os = "windows")]
                    {
                        const CREATE_NO_WINDOW: u32 = 0x08000000;
                        cmd.creation_flags(CREATE_NO_WINDOW);
                    }

                    let mut child =
                        cmd.spawn().map_err(|error| McpServiceError::ConnectionFailed {
                            server_id: sid.clone(),
                            detail: format!("failed to spawn `{command}`: {error}"),
                        })?;

                    let pid = child.id().unwrap_or(0);

                    // Log: process started
                    {
                        let mut servers = servers.write().await;
                        if let Some(state) = servers.get_mut(&sid) {
                            state.push_log(format!("process started (pid {pid})"));
                        }
                    }

                    let child_stdout =
                        child.stdout.take().ok_or_else(|| McpServiceError::ConnectionFailed {
                            server_id: sid.clone(),
                            detail: "stdout was not set to piped".to_string(),
                        })?;
                    let child_stdin =
                        child.stdin.take().ok_or_else(|| McpServiceError::ConnectionFailed {
                            server_id: sid.clone(),
                            detail: "stdin was not set to piped".to_string(),
                        })?;
                    let child_stderr =
                        child.stderr.take().ok_or_else(|| McpServiceError::ConnectionFailed {
                            server_id: sid.clone(),
                            detail: "stderr was not set to piped".to_string(),
                        })?;

                    // Spawn background task to read stderr into the log buffer.
                    let stderr_servers = Arc::clone(&servers);
                    let stderr_sid = sid.clone();
                    tokio::spawn(async move {
                        let mut reader = tokio::io::BufReader::new(child_stderr);
                        let mut buf = String::new();
                        const MAX_LINE_LEN: usize = 65_536; // 64 KB
                        loop {
                            buf.clear();
                            match reader.read_line(&mut buf).await {
                                Ok(0) => break, // EOF
                                Ok(_) => {
                                    let line = if buf.len() > MAX_LINE_LEN {
                                        tracing::warn!(
                                            server_id = %stderr_sid,
                                            len = buf.len(),
                                            "MCP server stderr line truncated"
                                        );
                                        &buf[..MAX_LINE_LEN]
                                    } else {
                                        buf.trim_end_matches('\n').trim_end_matches('\r')
                                    };
                                    let mut servers = stderr_servers.write().await;
                                    if let Some(state) = servers.get_mut(&stderr_sid) {
                                        state.push_log(format!("[stderr] {line}"));
                                    }
                                }
                                Err(_) => break,
                            }
                        }
                    });

                    // Log: handshake starting
                    {
                        let mut servers = servers.write().await;
                        if let Some(state) = servers.get_mut(&sid) {
                            state.push_log("protocol handshake starting");
                        }
                    }

                    let service = tokio::time::timeout(
                        CONNECT_TIMEOUT,
                        handler.serve((child_stdout, child_stdin)),
                    )
                    .await
                    .map_err(|_| McpServiceError::ConnectionFailed {
                        server_id: sid.clone(),
                        detail: format!(
                            "connection timed out after {}s",
                            CONNECT_TIMEOUT.as_secs()
                        ),
                    })?
                    .map_err(|error| McpServiceError::ConnectionFailed {
                        server_id: sid.clone(),
                        detail: format!("protocol handshake with `{command}` failed: {error}"),
                    })?;

                    Ok((service, Some(child)))
                }
                .await;
                result
            }
            McpTransportConfig::Sse => {
                let result: Result<_, McpServiceError> = async {
                    let url =
                        config.url.clone().ok_or_else(|| McpServiceError::ConnectionFailed {
                            server_id: server_id.to_string(),
                            detail: "missing url".to_string(),
                        })?;
                    let transport = if config.headers.is_empty() {
                        SseTransport::start(url).await
                    } else {
                        let client = build_http_client_with_headers(&config.headers)?;
                        SseTransport::start_with_client(url, client).await
                    }
                    .map_err(|error| McpServiceError::ConnectionFailed {
                        server_id: server_id.to_string(),
                        detail: error.to_string(),
                    })?;
                    handler.serve(transport).await.map(|s| (s, None)).map_err(|error| {
                        McpServiceError::ConnectionFailed {
                            server_id: server_id.to_string(),
                            detail: error.to_string(),
                        }
                    })
                }
                .await;
                result
            }
            McpTransportConfig::StreamableHttp => {
                let result: Result<_, McpServiceError> = async {
                    let url =
                        config.url.clone().ok_or_else(|| McpServiceError::ConnectionFailed {
                            server_id: server_id.to_string(),
                            detail: "missing url".to_string(),
                        })?;
                    let transport = if config.headers.is_empty() {
                        streamable_http::StreamableHttpTransport::new(url)
                    } else {
                        let client = build_http_client_with_headers(&config.headers)?;
                        streamable_http::StreamableHttpTransport::new_with_client(url, client)
                    }
                    .map_err(|error| McpServiceError::ConnectionFailed {
                        server_id: server_id.to_string(),
                        detail: error.to_string(),
                    })?;
                    handler.serve(transport).await.map(|s| (s, None)).map_err(|error| {
                        McpServiceError::ConnectionFailed {
                            server_id: server_id.to_string(),
                            detail: error.to_string(),
                        }
                    })
                }
                .await;
                result
            }
        };

        let (service, child) = match service_result {
            Ok(pair) => pair,
            Err(error) => {
                let mut servers = self.servers.write().await;
                if let Some(state) = servers.get_mut(server_id) {
                    state.push_log(format!("error: {error}"));
                    state.status = McpConnectionStatus::Error;
                    state.last_error = Some(error.to_string());
                }
                if let Err(e) = self.event_bus.publish(
                    "mcp.server.error",
                    "hive-mcp",
                    json!({ "serverId": server_id, "error": error.to_string() }),
                ) {
                    tracing::warn!(error = %e, "failed to publish mcp.server.error event");
                }
                return Err(error);
            }
        };

        {
            let mut servers = self.servers.write().await;
            let state = servers.get_mut(server_id).ok_or_else(|| {
                McpServiceError::ServerNotFound { server_id: server_id.to_string() }
            })?;
            state.push_log("connected");
            state.status = McpConnectionStatus::Connected;
            state.last_error = None;
            // Capture server capabilities from the initialize handshake.
            state.capabilities = Some(service.peer_info().capabilities.clone());
            state.client = Some(service);
            state.child_process = child;
        }

        if let Err(e) = self.event_bus.publish(
            "mcp.server.connected",
            "hive-mcp",
            json!({ "serverId": server_id }),
        ) {
            tracing::warn!(error = %e, "failed to publish mcp.server.connected event");
        }

        // If the server advertises logging capability, request log messages at
        // `debug` level so they are forwarded to the server log buffer.
        {
            let servers = self.servers.read().await;
            if let Some(state) = servers.get(server_id) {
                let supports_logging =
                    state.capabilities.as_ref().map(|c| c.logging.is_some()).unwrap_or(false);
                if supports_logging {
                    if let Some(client) = &state.client {
                        let peer = client.peer().clone();
                        let sid = server_id.to_string();
                        tokio::spawn(async move {
                            if let Err(e) = peer
                                .set_level(SetLevelRequestParam { level: LoggingLevel::Debug })
                                .await
                            {
                                tracing::debug!(
                                    server_id = %sid,
                                    error = %e,
                                    "failed to set MCP logging level"
                                );
                            }
                        });
                    }
                }
            }
        }

        // Discover tools and resources (best-effort, 10s timeout each).
        const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(10);

        let tools = match tokio::time::timeout(DISCOVERY_TIMEOUT, self.list_tools(server_id)).await
        {
            Ok(Ok(t)) => t,
            Ok(Err(e)) => {
                let mut servers = self.servers.write().await;
                if let Some(state) = servers.get_mut(server_id) {
                    state.push_log(format!("tool discovery failed: {e}"));
                }
                vec![]
            }
            Err(_) => {
                let mut servers = self.servers.write().await;
                if let Some(state) = servers.get_mut(server_id) {
                    state.push_log("tool discovery timed out");
                }
                vec![]
            }
        };

        let resources =
            match tokio::time::timeout(DISCOVERY_TIMEOUT, self.list_resources(server_id)).await {
                Ok(Ok(r)) => r,
                Ok(Err(e)) => {
                    let mut servers = self.servers.write().await;
                    if let Some(state) = servers.get_mut(server_id) {
                        state.push_log(format!("resource discovery failed: {e}"));
                    }
                    vec![]
                }
                Err(_) => {
                    let mut servers = self.servers.write().await;
                    if let Some(state) = servers.get_mut(server_id) {
                        state.push_log("resource discovery timed out");
                    }
                    vec![]
                }
            };

        let prompts =
            match tokio::time::timeout(DISCOVERY_TIMEOUT, self.list_prompts(server_id)).await {
                Ok(Ok(p)) => p,
                Ok(Err(e)) => {
                    let mut servers = self.servers.write().await;
                    if let Some(state) = servers.get_mut(server_id) {
                        state.push_log(format!("prompt discovery failed: {e}"));
                    }
                    vec![]
                }
                Err(_) => {
                    let mut servers = self.servers.write().await;
                    if let Some(state) = servers.get_mut(server_id) {
                        state.push_log("prompt discovery timed out");
                    }
                    vec![]
                }
            };

        let snapshot = {
            let mut servers = self.servers.write().await;
            let state = servers.get_mut(server_id).ok_or_else(|| {
                McpServiceError::ServerNotFound { server_id: server_id.to_string() }
            })?;
            state.push_log(format!(
                "discovered {} tools, {} resources, {} prompts",
                tools.len(),
                resources.len(),
                prompts.len()
            ));
            Self::snapshot_for(state)
        };

        Ok(snapshot)
    }

    pub async fn disconnect(&self, server_id: &str) -> Result<McpServerSnapshot, McpServiceError> {
        // Acquire per-server lifecycle lock to serialize connect/disconnect.
        let lifecycle_guard = {
            let servers = self.servers.read().await;
            let state = servers.get(server_id).ok_or_else(|| McpServiceError::ServerNotFound {
                server_id: server_id.to_string(),
            })?;
            Arc::clone(&state.lifecycle)
        };
        let _lifecycle = lifecycle_guard.lock().await;

        let (service, child_process) = {
            let mut servers = self.servers.write().await;
            let state = servers.get_mut(server_id).ok_or_else(|| {
                McpServiceError::ServerNotFound { server_id: server_id.to_string() }
            })?;
            let service = state.client.take().ok_or_else(|| McpServiceError::NotConnected {
                server_id: server_id.to_string(),
            })?;
            let child = state.child_process.take();
            state.status = McpConnectionStatus::Disconnected;
            state.last_error = None;
            state.tools.clear();
            state.resources.clear();
            state.prompts.clear();
            (service, child)
        };

        let _ = service.cancel().await;

        // Give stdio servers a moment to clean up before the child is dropped.
        if let Some(mut child) = child_process {
            match tokio::time::timeout(Duration::from_millis(500), child.wait()).await {
                Ok(Ok(_)) => {} // Exited cleanly
                _ => {
                    let _ = child.kill().await;
                }
            }
        }

        let snapshot = {
            let servers = self.servers.read().await;
            let state = servers.get(server_id).ok_or_else(|| McpServiceError::ServerNotFound {
                server_id: server_id.to_string(),
            })?;
            Self::snapshot_for(state)
        };

        if let Err(e) = self.event_bus.publish(
            "mcp.server.disconnected",
            "hive-mcp",
            json!({ "serverId": server_id }),
        ) {
            tracing::warn!(error = %e, "failed to publish mcp.server.disconnected event");
        }

        Ok(snapshot)
    }

    pub async fn list_tools(&self, server_id: &str) -> Result<Vec<McpToolInfo>, McpServiceError> {
        let peer = self.peer_for(server_id).await?;
        let tools = peer
            .list_all_tools()
            .await
            .map_err(|error| map_request_error(server_id, error))?
            .into_iter()
            .map(tool_to_info)
            .collect::<Vec<_>>();

        self.update_tools(server_id, tools.clone()).await?;
        Ok(tools)
    }

    pub async fn list_resources(
        &self,
        server_id: &str,
    ) -> Result<Vec<McpResourceInfo>, McpServiceError> {
        let peer = self.peer_for(server_id).await?;
        let resources = peer
            .list_all_resources()
            .await
            .map_err(|error| map_request_error(server_id, error))?
            .into_iter()
            .map(resource_to_info)
            .collect::<Vec<_>>();

        self.update_resources(server_id, resources.clone()).await?;
        Ok(resources)
    }

    pub async fn list_prompts(
        &self,
        server_id: &str,
    ) -> Result<Vec<McpPromptInfo>, McpServiceError> {
        let peer = self.peer_for(server_id).await?;
        let prompts = peer
            .list_all_prompts()
            .await
            .map_err(|error| map_request_error(server_id, error))?
            .into_iter()
            .map(prompt_to_info)
            .collect::<Vec<_>>();

        self.update_prompts(server_id, prompts.clone()).await?;
        Ok(prompts)
    }

    pub async fn list_notifications(&self, limit: usize) -> Vec<McpNotificationEvent> {
        let limit = limit.clamp(1, 100);
        let notifications = self.notifications.lock().await;
        let start = notifications.len().saturating_sub(limit);
        notifications.iter().skip(start).cloned().collect()
    }

    pub async fn get_server_logs(
        &self,
        server_id: &str,
    ) -> Result<Vec<McpServerLog>, McpServiceError> {
        let servers = self.servers.read().await;
        let state = servers
            .get(server_id)
            .ok_or_else(|| McpServiceError::ServerNotFound { server_id: server_id.to_string() })?;
        Ok(state.logs.iter().cloned().collect())
    }

    /// Call a tool on a connected MCP server.
    pub async fn call_tool(
        &self,
        server_id: &str,
        tool_name: &str,
        arguments: serde_json::Map<String, Value>,
    ) -> Result<McpCallToolResult, McpServiceError> {
        let peer = self.peer_for(server_id).await?;
        let result = peer
            .call_tool(CallToolRequestParam {
                name: tool_name.to_string().into(),
                arguments: Some(arguments),
            })
            .await
            .map_err(|error| map_request_error(server_id, error))?;

        let content = result
            .content
            .iter()
            .filter_map(|c| match &c.raw {
                rmcp::model::RawContent::Text(text) => Some(text.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");

        Ok(McpCallToolResult { content, is_error: result.is_error.unwrap_or(false) })
    }

    /// Return every tool from every *connected* server, together with the
    /// server ID and channel class so the bridge layer can wrap them.
    pub async fn connected_tools(&self) -> Vec<McpConnectedTool> {
        let servers = self.servers.read().await;
        let mut out = Vec::new();
        for (id, state) in servers.iter() {
            if state.status != McpConnectionStatus::Connected {
                continue;
            }
            for tool in &state.tools {
                out.push(McpConnectedTool {
                    server_id: id.clone(),
                    channel_class: state.config.channel_class,
                    tool: tool.clone(),
                });
            }
        }
        out
    }

    /// Check whether a connected server declared resource support.
    pub async fn server_supports_resources(&self, server_id: &str) -> bool {
        let servers = self.servers.read().await;
        servers
            .get(server_id)
            .and_then(|s| s.capabilities.as_ref())
            .and_then(|c| c.resources.as_ref())
            .is_some()
    }

    /// Check whether a connected server declared resource subscription support.
    pub async fn server_supports_subscribe(&self, server_id: &str) -> bool {
        let servers = self.servers.read().await;
        servers
            .get(server_id)
            .and_then(|s| s.capabilities.as_ref())
            .and_then(|c| c.resources.as_ref())
            .and_then(|r| r.subscribe)
            .unwrap_or(false)
    }

    /// Return server IDs of connected servers that support resources.
    pub async fn connected_resource_servers(&self) -> Vec<String> {
        let servers = self.servers.read().await;
        servers
            .iter()
            .filter(|(_, s)| {
                s.status == McpConnectionStatus::Connected
                    && s.capabilities.as_ref().and_then(|c| c.resources.as_ref()).is_some()
            })
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Return server IDs of connected servers that have `reactive: true`.
    pub async fn reactive_server_ids(&self) -> Vec<String> {
        let servers = self.servers.read().await;
        servers
            .iter()
            .filter(|(_, s)| s.status == McpConnectionStatus::Connected && s.config.reactive)
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Return the configured [`ChannelClass`] for a server, if it exists.
    pub async fn server_channel_class(&self, server_id: &str) -> Option<ChannelClass> {
        let servers = self.servers.read().await;
        servers.get(server_id).map(|s| s.config.channel_class)
    }

    /// Read a resource from an MCP server by URI.
    pub async fn read_resource(
        &self,
        server_id: &str,
        uri: &str,
    ) -> Result<String, McpServiceError> {
        let peer = self.peer_for(server_id).await?;
        let result = peer
            .read_resource(ReadResourceRequestParam { uri: uri.to_string() })
            .await
            .map_err(|error| map_request_error(server_id, error))?;

        use rmcp::model::ResourceContents;
        let content = result
            .contents
            .iter()
            .map(|c| match c {
                ResourceContents::TextResourceContents { text, .. } => text.clone(),
                ResourceContents::BlobResourceContents { blob, .. } => {
                    format!("[binary resource, base64 ({} chars)]\n{}", blob.len(), blob)
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        Ok(content)
    }

    /// Subscribe to change notifications for a resource URI.
    pub async fn subscribe_resource(
        &self,
        server_id: &str,
        uri: &str,
    ) -> Result<(), McpServiceError> {
        let peer = self.peer_for(server_id).await?;
        peer.subscribe(SubscribeRequestParam { uri: uri.to_string() })
            .await
            .map_err(|error| map_request_error(server_id, error))?;
        Ok(())
    }

    /// Drain all pending notifications, returning them and clearing the buffer.
    pub async fn drain_notifications(&self) -> Vec<McpNotificationEvent> {
        let mut notifications = self.notifications.lock().await;
        notifications.drain(..).collect()
    }

    /// Ensure a server is connected.  If already connected, returns the
    /// current snapshot.  Otherwise, attempts to connect.
    pub async fn ensure_connected(
        &self,
        server_id: &str,
    ) -> Result<McpServerSnapshot, McpServiceError> {
        self.ensure_connected_with_workspace(server_id, None).await
    }

    /// Ensure a server is connected, with optional workspace path for sandbox.
    pub async fn ensure_connected_with_workspace(
        &self,
        server_id: &str,
        workspace_path: Option<&std::path::Path>,
    ) -> Result<McpServerSnapshot, McpServiceError> {
        let is_connected = {
            let servers = self.servers.read().await;
            servers
                .get(server_id)
                .map(|s| s.status == McpConnectionStatus::Connected)
                .unwrap_or(false)
        };
        if is_connected {
            let servers = self.servers.read().await;
            if let Some(state) = servers.get(server_id) {
                return Ok(Self::snapshot_for(state));
            }
        }
        self.connect_with_workspace(server_id, workspace_path).await
    }

    /// Disconnect all currently connected servers.
    pub async fn disconnect_all(&self) {
        let ids: Vec<String> = {
            let servers = self.servers.read().await;
            servers
                .iter()
                .filter(|(_, s)| s.status == McpConnectionStatus::Connected)
                .map(|(id, _)| id.clone())
                .collect()
        };
        for id in ids {
            if let Err(e) = self.disconnect(&id).await {
                tracing::debug!(server_id = %id, error = %e, "disconnect_all: failed to disconnect");
            }
        }
    }

    async fn peer_for(&self, server_id: &str) -> Result<Peer<RoleClient>, McpServiceError> {
        let servers = self.servers.read().await;
        let state = servers
            .get(server_id)
            .ok_or_else(|| McpServiceError::ServerNotFound { server_id: server_id.to_string() })?;
        let client = state
            .client
            .as_ref()
            .ok_or_else(|| McpServiceError::NotConnected { server_id: server_id.to_string() })?;
        Ok(client.peer().clone())
    }

    async fn update_tools(
        &self,
        server_id: &str,
        tools: Vec<McpToolInfo>,
    ) -> Result<(), McpServiceError> {
        let mut servers = self.servers.write().await;
        let state = servers
            .get_mut(server_id)
            .ok_or_else(|| McpServiceError::ServerNotFound { server_id: server_id.to_string() })?;
        state.tools = tools;
        Ok(())
    }

    async fn update_resources(
        &self,
        server_id: &str,
        resources: Vec<McpResourceInfo>,
    ) -> Result<(), McpServiceError> {
        let mut servers = self.servers.write().await;
        let state = servers
            .get_mut(server_id)
            .ok_or_else(|| McpServiceError::ServerNotFound { server_id: server_id.to_string() })?;
        state.resources = resources;
        Ok(())
    }

    async fn update_prompts(
        &self,
        server_id: &str,
        prompts: Vec<McpPromptInfo>,
    ) -> Result<(), McpServiceError> {
        let mut servers = self.servers.write().await;
        let state = servers
            .get_mut(server_id)
            .ok_or_else(|| McpServiceError::ServerNotFound { server_id: server_id.to_string() })?;
        state.prompts = prompts;
        Ok(())
    }

    /// Reconcile the server list with new configuration.
    ///
    /// Disconnects and removes servers no longer present in the config,
    /// adds new servers, updates the config of existing servers, and
    /// reconnects or disconnects servers whose config changed.
    pub async fn update_servers(&self, new_configs: &[McpServerConfig]) {
        use std::collections::HashSet;

        let new_ids: HashSet<String> = new_configs.iter().map(|c| c.id.clone()).collect();

        // Collect old configs for change detection.
        let old_configs: HashMap<String, McpServerConfig> = {
            let servers_read = self.servers.read().await;
            servers_read.iter().map(|(id, state)| (id.clone(), state.config.clone())).collect()
        };

        // Disconnect and remove servers that are no longer in config.
        let removed: Vec<String> = {
            let servers = self.servers.read().await;
            servers.keys().filter(|id| !new_ids.contains(*id)).cloned().collect()
        };
        for id in &removed {
            // Acquire lifecycle lock to avoid racing with in-flight connect.
            let lifecycle_guard = {
                let servers = self.servers.read().await;
                servers.get(id).map(|state| Arc::clone(&state.lifecycle))
            };
            if let Some(guard) = lifecycle_guard {
                let _lifecycle = guard.lock().await;
            }

            let (service, child_process) = {
                let mut servers = self.servers.write().await;
                if let Some(mut state) = servers.remove(id) {
                    (state.client.take(), state.child_process.take())
                } else {
                    (None, None)
                }
            };
            if let Some(svc) = service {
                let _ = svc.cancel().await;
            }
            // Give stdio servers a moment to clean up.
            if let Some(mut child) = child_process {
                match tokio::time::timeout(Duration::from_millis(500), child.wait()).await {
                    Ok(Ok(_)) => {}
                    _ => {
                        let _ = child.kill().await;
                    }
                }
            }
            if let Err(e) =
                self.event_bus.publish("mcp.server.removed", "hive-mcp", json!({ "serverId": id }))
            {
                tracing::warn!(error = %e, "failed to publish mcp.server.removed event");
            }
        }

        // Add new servers and update configs for existing ones.
        {
            let mut servers = self.servers.write().await;
            for config in new_configs {
                if let Some(state) = servers.get_mut(&config.id) {
                    state.config = config.clone();
                } else {
                    servers.insert(
                        config.id.clone(),
                        McpServerState::new(config.clone(), self.event_bus.clone()),
                    );
                }
            }
        }

        // Reconcile: disconnect disabled servers, reconnect servers whose config changed.
        let mut to_disconnect = Vec::new();
        let mut to_reconnect = Vec::new();
        {
            let servers = self.servers.read().await;
            for (id, state) in servers.iter() {
                if let Some(old) = old_configs.get(id) {
                    let config_changed = old.transport != state.config.transport
                        || old.command != state.config.command
                        || old.url != state.config.url
                        || old.args != state.config.args
                        || old.env != state.config.env;

                    if state.status == McpConnectionStatus::Connected {
                        if !state.config.enabled {
                            to_disconnect.push(id.clone());
                        } else if config_changed {
                            to_reconnect.push(id.clone());
                        }
                    }
                }
            }
        }

        for id in &to_disconnect {
            if let Err(e) = self.disconnect(id).await {
                tracing::warn!(server_id = %id, error = %e, "failed to disconnect disabled MCP server");
            }
        }
        for id in &to_reconnect {
            tracing::info!(server_id = %id, "reconnecting MCP server due to config change");
            let _ = self.disconnect(id).await;
            if let Err(e) = self.connect(id).await {
                tracing::warn!(server_id = %id, error = %e, "failed to reconnect MCP server after config change");
            }
        }
    }

    /// Briefly connect to a server, discover its tools/resources/prompts,
    /// store them in the catalog, then disconnect.  This is used when an
    /// MCP server is first configured so that sessions can register bridge
    /// tools without maintaining a long-lived connection.
    ///
    /// If `catalog` is `None` the discovery results are only stored in the
    /// in-memory server state (the legacy behaviour).
    pub async fn discover_and_catalog(
        &self,
        server_id: &str,
        catalog: &McpCatalogStore,
    ) -> Result<McpCatalogEntry, McpServiceError> {
        // Connect WITHOUT sandbox — discovery only reads tool schemas, it
        // does not execute tools.  Sandboxing is enforced later when the
        // per-session SessionMcpManager actually calls a tool.
        self.connect_impl(server_id, None, false).await?;

        // Read the discovered data from server state.
        let (tools, resources, prompts, channel_class) = {
            let servers = self.servers.read().await;
            let state = servers.get(server_id).ok_or_else(|| McpServiceError::ServerNotFound {
                server_id: server_id.to_string(),
            })?;
            (
                state.tools.clone(),
                state.resources.clone(),
                state.prompts.clone(),
                state.config.channel_class,
            )
        };

        // Persist to catalog.
        let cache_key = {
            let servers = self.servers.read().await;
            servers.get(server_id).map(|s| s.config.cache_key()).unwrap_or_default()
        };
        catalog
            .upsert(
                server_id,
                &cache_key,
                channel_class,
                tools.clone(),
                resources.clone(),
                prompts.clone(),
            )
            .await;

        // Disconnect — we only needed to discover.
        if let Err(e) = self.disconnect(server_id).await {
            tracing::debug!(server_id = %server_id, error = %e, "disconnect after catalog discovery failed (non-fatal)");
        }

        Ok(McpCatalogEntry {
            server_id: server_id.to_string(),
            cache_key,
            channel_class,
            tools,
            resources,
            prompts,
            last_updated_ms: now_ms(),
        })
    }

    /// Refresh the catalog for all enabled servers.
    /// Errors on individual servers are logged but do not stop others.
    pub async fn refresh_catalog(&self, catalog: &McpCatalogStore) {
        let configs: Vec<McpServerConfig> = {
            let servers = self.servers.read().await;
            servers.values().filter(|s| s.config.enabled).map(|s| s.config.clone()).collect()
        };

        for config in &configs {
            tracing::info!(server_id = %config.id, "refreshing MCP catalog");
            match self.discover_and_catalog(&config.id, catalog).await {
                Ok(entry) => {
                    tracing::info!(
                        server_id = %config.id,
                        tools = entry.tools.len(),
                        resources = entry.resources.len(),
                        prompts = entry.prompts.len(),
                        "catalog refreshed"
                    );
                }
                Err(e) => {
                    tracing::warn!(server_id = %config.id, error = %e, "catalog refresh failed");
                }
            }
        }

        // Remove catalog entries for servers that no longer exist.
        let keys: Vec<String> = configs.iter().map(|c| c.cache_key()).collect();
        catalog.retain_keys(&keys).await;
    }

    fn snapshot_for(state: &McpServerState) -> McpServerSnapshot {
        McpServerSnapshot {
            id: state.config.id.clone(),
            transport: state.config.transport,
            channel_class: state.config.channel_class,
            enabled: state.config.enabled,
            auto_connect: state.config.auto_connect,
            reactive: state.config.reactive,
            status: state.status,
            last_error: state.last_error.clone(),
            tool_count: state.tools.len(),
            resource_count: state.resources.len(),
            prompt_count: state.prompts.len(),
            sandbox_status: state.sandbox_status.clone(),
        }
    }
}

pub(crate) struct McpClientHandler {
    server_id: String,
    event_bus: EventBus,
    notifications: Arc<Mutex<VecDeque<McpNotificationEvent>>>,
    peer: Option<Peer<RoleClient>>,
    /// Reference to the owning service for cache refresh on notifications.
    service: Option<McpService>,
}

impl McpClientHandler {
    fn new(
        server_id: String,
        event_bus: EventBus,
        notifications: Arc<Mutex<VecDeque<McpNotificationEvent>>>,
        service: McpService,
    ) -> Self {
        Self { server_id, event_bus, notifications, peer: None, service: Some(service) }
    }

    async fn record_notification(&self, kind: McpNotificationKind, payload: Value) {
        let event = McpNotificationEvent {
            server_id: self.server_id.clone(),
            kind,
            payload,
            timestamp_ms: now_ms(),
        };

        {
            let mut notifications = self.notifications.lock().await;

            // Deduplicate: the same notification can arrive via both the GET
            // SSE listener and a POST response stream.  Skip if an identical
            // (server_id + kind + payload) event already exists among recent
            // entries — content equality is sufficient since genuinely
            // different events will have different payloads.
            let dominated = notifications.iter().rev().take(20).any(|n| {
                n.server_id == event.server_id && n.kind == event.kind && n.payload == event.payload
            });
            if dominated {
                return;
            }

            if notifications.len() >= MAX_NOTIFICATIONS {
                notifications.pop_front();
            }
            notifications.push_back(event.clone());
        }

        let payload = serde_json::to_value(&event).unwrap_or_else(|error| {
            json!({
                "serverId": self.server_id.clone(),
                "kind": format!("{:?}", kind),
                "error": error.to_string(),
            })
        });
        if let Err(e) = self.event_bus.publish("mcp.notification", "hive-mcp", payload) {
            tracing::warn!(error = %e, "failed to publish mcp.notification event");
        }
    }
}

impl ClientHandler for McpClientHandler {
    fn on_cancelled(
        &self,
        params: rmcp::model::CancelledNotificationParam,
    ) -> impl std::future::Future<Output = ()> + Send + '_ {
        let payload = serde_json::to_value(&params).unwrap_or_else(|error| {
            tracing::warn!("failed to serialize MCP notification params: {error}");
            json!({ "error": error.to_string() })
        });
        async move { self.record_notification(McpNotificationKind::Cancelled, payload).await }
    }

    fn on_progress(
        &self,
        params: ProgressNotificationParam,
    ) -> impl std::future::Future<Output = ()> + Send + '_ {
        let payload = serde_json::to_value(&params).unwrap_or_else(|error| {
            tracing::warn!("failed to serialize MCP notification params: {error}");
            json!({ "error": error.to_string() })
        });
        async move { self.record_notification(McpNotificationKind::Progress, payload).await }
    }

    fn on_logging_message(
        &self,
        params: LoggingMessageNotificationParam,
    ) -> impl std::future::Future<Output = ()> + Send + '_ {
        // Format log entry for the server log buffer.
        let level = format!("{:?}", params.level).to_lowercase();
        let logger_prefix = params.logger.as_deref().map(|l| format!("{l}: ")).unwrap_or_default();
        let data_str = match &params.data {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        let log_message = format!("[mcp:{level}] {logger_prefix}{data_str}");

        let payload = serde_json::to_value(&params).unwrap_or_else(|error| {
            tracing::warn!("failed to serialize MCP notification params: {error}");
            json!({ "error": error.to_string() })
        });
        let server_id = self.server_id.clone();
        let service = self.service.clone();
        async move {
            self.record_notification(McpNotificationKind::LoggingMessage, payload).await;
            // Also push into the server log buffer for unified display.
            if let Some(svc) = &service {
                let mut servers = svc.servers.write().await;
                if let Some(state) = servers.get_mut(&server_id) {
                    state.push_log(log_message);
                }
            }
        }
    }

    fn on_resource_updated(
        &self,
        params: rmcp::model::ResourceUpdatedNotificationParam,
    ) -> impl std::future::Future<Output = ()> + Send + '_ {
        let payload = serde_json::to_value(&params).unwrap_or_else(|error| {
            tracing::warn!("failed to serialize MCP notification params: {error}");
            json!({ "error": error.to_string() })
        });
        async move { self.record_notification(McpNotificationKind::ResourceUpdated, payload).await }
    }

    async fn on_resource_list_changed(&self) {
        self.record_notification(McpNotificationKind::ResourceListChanged, json!({})).await;
        let service = self.service.clone();
        let server_id = self.server_id.clone();
        let event_bus = self.event_bus.clone();
        tokio::spawn(async move {
            if let Some(svc) = service {
                if let Err(e) = svc.list_resources(&server_id).await {
                    tracing::debug!(server_id = %server_id, error = %e, "failed to refresh resource cache after resource_list_changed");
                }
            }
            if let Err(e) = event_bus.publish(
                "mcp.resources.changed",
                "hive-mcp",
                serde_json::json!({ "serverId": server_id }),
            ) {
                tracing::warn!(error = %e, "failed to publish mcp.resources.changed event");
            }
        });
    }

    async fn on_tool_list_changed(&self) {
        self.record_notification(McpNotificationKind::ToolListChanged, json!({})).await;
        let service = self.service.clone();
        let server_id = self.server_id.clone();
        let event_bus = self.event_bus.clone();
        tokio::spawn(async move {
            if let Some(svc) = service {
                if let Err(e) = svc.list_tools(&server_id).await {
                    tracing::debug!(server_id = %server_id, error = %e, "failed to refresh tool cache after tool_list_changed");
                }
            }
            if let Err(e) = event_bus.publish(
                "mcp.tools.changed",
                "hive-mcp",
                serde_json::json!({ "serverId": server_id }),
            ) {
                tracing::warn!(error = %e, "failed to publish mcp.tools.changed event");
            }
        });
    }

    async fn on_prompt_list_changed(&self) {
        self.record_notification(McpNotificationKind::PromptListChanged, json!({})).await;
        let service = self.service.clone();
        let server_id = self.server_id.clone();
        let event_bus = self.event_bus.clone();
        tokio::spawn(async move {
            if let Some(svc) = service {
                if let Err(e) = svc.list_prompts(&server_id).await {
                    tracing::debug!(server_id = %server_id, error = %e, "failed to refresh prompt cache after prompt_list_changed");
                }
            }
            if let Err(e) = event_bus.publish(
                "mcp.prompts.changed",
                "hive-mcp",
                serde_json::json!({ "serverId": server_id }),
            ) {
                tracing::warn!(error = %e, "failed to publish mcp.prompts.changed event");
            }
        });
    }

    fn get_peer(&self) -> Option<Peer<RoleClient>> {
        self.peer.clone()
    }

    fn set_peer(&mut self, peer: Peer<RoleClient>) {
        self.peer = Some(peer);
    }
}

fn parse_command(config: &McpServerConfig) -> Result<(String, Vec<String>), McpServiceError> {
    let command = config.command.as_deref().ok_or_else(|| McpServiceError::ConnectionFailed {
        server_id: config.id.clone(),
        detail: "missing command".to_string(),
    })?;

    if !config.args.is_empty() {
        return Ok((command.to_string(), config.args.clone()));
    }

    // Parse the command string respecting shell-style quoting.
    let parts = shell_split(command);
    if parts.is_empty() {
        return Err(McpServiceError::ConnectionFailed {
            server_id: config.id.clone(),
            detail: "command is empty".to_string(),
        });
    }

    Ok((parts[0].clone(), parts[1..].to_vec()))
}

/// Simple shell-style tokenizer that respects single and double quotes.
/// Does NOT interpret shell metacharacters (&&, ||, ;, etc.).
fn shell_split(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = input.chars().peekable();
    let mut in_single = false;
    let mut in_double = false;

    while let Some(ch) = chars.next() {
        match ch {
            '\'' if !in_double => {
                in_single = !in_single;
            }
            '"' if !in_single => {
                in_double = !in_double;
            }
            '\\' if in_double || !in_single => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            c if c.is_whitespace() && !in_single && !in_double => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            c => {
                current.push(c);
            }
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn resolve_env(config: &McpServerConfig) -> Result<Vec<(String, String)>, McpServiceError> {
    let mut resolved = Vec::new();
    for (key, value) in &config.env {
        let value = if let Some(variable) = value.strip_prefix("env:") {
            std::env::var(variable).map_err(|error| McpServiceError::ConnectionFailed {
                server_id: config.id.clone(),
                detail: format!("missing env {variable}: {error}"),
            })?
        } else {
            value.clone()
        };
        resolved.push((key.clone(), value));
    }
    Ok(resolved)
}

fn map_request_error(server_id: &str, error: ServiceError) -> McpServiceError {
    match error {
        ServiceError::Timeout { .. } => {
            McpServiceError::RequestTimeout { server_id: server_id.to_string() }
        }
        ServiceError::Transport(ref io_err) => McpServiceError::ProtocolError {
            server_id: server_id.to_string(),
            detail: io_err.to_string(),
        },
        ServiceError::McpError(_)
        | ServiceError::UnexpectedResponse
        | ServiceError::Cancelled { .. } => McpServiceError::RequestFailed {
            server_id: server_id.to_string(),
            detail: error.to_string(),
        },
        _ => McpServiceError::RequestFailed {
            server_id: server_id.to_string(),
            detail: error.to_string(),
        },
    }
}

pub(crate) fn now_ms() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis()
}

fn tool_to_info(tool: Tool) -> McpToolInfo {
    McpToolInfo {
        name: tool.name.to_string(),
        description: tool.description.to_string(),
        input_schema: tool.schema_as_json_value(),
    }
}

fn resource_to_info(resource: Resource) -> McpResourceInfo {
    McpResourceInfo {
        uri: resource.uri.clone(),
        name: resource.name.clone(),
        description: resource.description.clone(),
        mime_type: resource.mime_type.clone(),
        size: resource.size,
    }
}

fn prompt_to_info(prompt: Prompt) -> McpPromptInfo {
    let arguments =
        prompt.arguments.unwrap_or_default().into_iter().map(prompt_argument_to_info).collect();
    McpPromptInfo { name: prompt.name, description: prompt.description, arguments }
}

fn prompt_argument_to_info(argument: PromptArgument) -> McpPromptArgumentInfo {
    McpPromptArgumentInfo {
        name: argument.name,
        description: argument.description,
        required: argument.required,
    }
}

/// Build a sandboxed command for an MCP stdio server using the per-server
/// `McpSandboxConfig`.
///
/// Returns `Some((program, args, temp_files))` if sandbox wrapping succeeded,
/// or `None` if it's unavailable or the platform doesn't support it.
pub(crate) fn build_mcp_sandbox_command_from_per_server(
    command: &str,
    args: &[String],
    sandbox_cfg: &hive_contracts::McpSandboxConfig,
    workspace_path: Option<&std::path::Path>,
) -> Option<(String, Vec<String>, Vec<tempfile::TempPath>)> {
    if !sandbox_cfg.enabled {
        return None;
    }

    let mut builder = hive_sandbox::SandboxPolicy::builder().network(sandbox_cfg.allow_network);

    // Workspace access
    if let Some(ws) = workspace_path {
        if sandbox_cfg.write_workspace {
            builder = builder.allow_read_write(ws);
        } else if sandbox_cfg.read_workspace {
            builder = builder.allow_read(ws);
        }
    }

    // Allow reading the MCP server command and script arguments so the
    // runtime can load them even when they live under $HOME.
    builder = allow_command_paths(command, args, builder);

    // Allow PATH entries under $HOME (nvm, pyenv, etc.)
    builder = allow_home_path_entries(builder);

    // System defaults: allow reads to common system paths
    for p in hive_sandbox::default_system_read_paths() {
        builder = builder.allow_read(p);
    }

    // HiveMind OS home directory — managed Node.js/Python runtimes live here.
    if let Some(hivemind_home) = resolve_hivemind_home() {
        builder = builder.allow_read(hivemind_home);
    }

    // Deny sensitive dot-directories
    for p in hive_sandbox::default_denied_paths() {
        builder = builder.deny(p);
    }

    // Temp directory access (read-write)
    builder = builder.allow_read_write(std::env::temp_dir());

    // Extra user-configured paths
    for p in &sandbox_cfg.extra_read_paths {
        builder = builder.allow_read(std::path::Path::new(p));
    }
    for p in &sandbox_cfg.extra_write_paths {
        builder = builder.allow_read_write(std::path::Path::new(p));
    }

    let policy = builder.build();
    wrap_mcp_command(command, args, &policy)
}

/// Build a sandboxed command for an MCP stdio server using the global
/// `SandboxConfig` (same policy shape as the shell tool).
///
/// Returns `Some((program, args, temp_files))` if sandbox wrapping succeeded,
/// or `None` if it's unavailable or the platform doesn't support it.
pub(crate) fn build_mcp_sandbox_command_from_global(
    command: &str,
    args: &[String],
    sandbox_cfg: &hive_contracts::SandboxConfig,
    workspace_path: Option<&std::path::Path>,
) -> Option<(String, Vec<String>, Vec<tempfile::TempPath>)> {
    if !sandbox_cfg.enabled {
        return None;
    }

    let mut builder = hive_sandbox::SandboxPolicy::builder().network(sandbox_cfg.allow_network);

    // Read-write: workspace / working dir (matches shell tool behaviour)
    if let Some(ws) = workspace_path {
        builder = builder.allow_read_write(ws);
    }

    // Allow reading the MCP server command and script arguments so the
    // runtime can load them even when they live under $HOME.
    builder = allow_command_paths(command, args, builder);

    // Allow PATH entries under $HOME (nvm, pyenv, etc.)
    builder = allow_home_path_entries(builder);

    // Read-write: temp directory
    builder = builder.allow_read_write(std::env::temp_dir());

    // Read-only: system paths
    for p in hive_sandbox::default_system_read_paths() {
        builder = builder.allow_read(p);
    }

    // Read-only: hivemind home (managed Node.js/Python runtimes)
    if let Some(hivemind_home) = resolve_hivemind_home() {
        builder = builder.allow_read(hivemind_home);
    }

    // Denied: sensitive dot-directories
    for p in hive_sandbox::default_denied_paths() {
        builder = builder.deny(p);
    }

    // User-configured extra paths from global config
    for p in &sandbox_cfg.extra_read_paths {
        builder = builder.allow_read(std::path::Path::new(p));
    }
    for p in &sandbox_cfg.extra_write_paths {
        builder = builder.allow_read_write(std::path::Path::new(p));
    }

    let policy = builder.build();
    wrap_mcp_command(command, args, &policy)
}

/// Resolve the hivemind home directory for sandbox read-path inclusion.
/// Managed Node.js and Python runtimes live under this directory.
fn resolve_hivemind_home() -> Option<std::path::PathBuf> {
    std::env::var_os("HIVEMIND_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(|h| std::path::PathBuf::from(h).join(".hivemind"))
        })
        .filter(|p| p.exists())
}

/// Allow read access to the command binary and any arguments that look like
/// absolute file paths.  This ensures the sandbox can load MCP server
/// scripts even when they live under the (otherwise denied) home directory.
fn allow_command_paths(
    command: &str,
    args: &[String],
    mut builder: hive_sandbox::PolicyBuilder,
) -> hive_sandbox::PolicyBuilder {
    let cmd_path = std::path::Path::new(command);
    if cmd_path.is_absolute() {
        if let Some(parent) = cmd_path.parent() {
            builder = builder.allow_read(parent);
        }
    }
    for arg in args {
        let p = std::path::Path::new(arg);
        if p.is_absolute() && (p.exists() || p.extension().is_some()) {
            // Allow the script's parent directory (for the script itself)
            // AND the project root (for node_modules, etc.).
            // Walk up from the script looking for package.json / pyproject.toml
            // as a project root indicator. If none found, allow the parent.
            if let Some(parent) = p.parent() {
                let project_root = find_project_root(parent);
                builder = builder.allow_read(project_root);
            }
        }
    }
    builder
}

/// Walk up from `start` looking for a project root marker file
/// (package.json, pyproject.toml, Cargo.toml, etc.).
/// Returns the directory containing the marker, or `start` if none found.
fn find_project_root(start: &std::path::Path) -> &std::path::Path {
    const MARKERS: &[&str] = &[
        "package.json",
        "pyproject.toml",
        "setup.py",
        "Cargo.toml",
        "go.mod",
        "pom.xml",
        "build.gradle",
    ];
    let mut dir = start;
    loop {
        for marker in MARKERS {
            if dir.join(marker).exists() {
                return dir;
            }
        }
        match dir.parent() {
            Some(parent) if parent != dir => dir = parent,
            _ => return start,
        }
    }
}

/// Allow read access to PATH directories that live under the user's home
/// directory.  Runtimes installed via version managers (nvm, pyenv, rbenv,
/// rustup, etc.) place their binaries under `$HOME`.  The macOS sandbox
/// denies all of `/Users`, so these directories must be explicitly
/// re-allowed for the sandboxed process to find its runtime.
///
/// For PATH entries ending in `/bin`, also allows the parent directory
/// (the runtime installation root) so that `lib/`, `share/`, and other
/// sibling directories are accessible. This is necessary because runtimes
/// like Node.js need `lib/node_modules/` and Python needs `lib/pythonX.Y/`.
fn allow_home_path_entries(
    mut builder: hive_sandbox::PolicyBuilder,
) -> hive_sandbox::PolicyBuilder {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from);
    let home = match home {
        Some(h) => h,
        None => return builder,
    };

    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            if dir.starts_with(&home) {
                builder = builder.allow_read(&dir);
                // If the entry ends in /bin, also allow the parent (runtime root)
                // so lib/, share/, etc. are accessible. Guard: skip if parent is HOME.
                if dir.ends_with("bin") {
                    if let Some(parent) = dir.parent() {
                        if parent != home {
                            builder = builder.allow_read(parent);
                        }
                    }
                }
            }
        }
    }
    builder
}

/// Shared helper: wrap `command args...` through the platform sandbox.
fn wrap_mcp_command(
    command: &str,
    args: &[String],
    policy: &hive_sandbox::SandboxPolicy,
) -> Option<(String, Vec<String>, Vec<tempfile::TempPath>)> {
    use hive_sandbox::SandboxedCommand;

    // Reconstruct the full command line for sandbox_command
    let full_cmd = if args.is_empty() {
        command.to_string()
    } else {
        format!("{} {}", command, args.join(" "))
    };

    match hive_sandbox::sandbox_command(&full_cmd, policy) {
        Ok(SandboxedCommand::Wrapped { program, args, _temp_files }) => {
            Some((program, args, _temp_files))
        }
        Ok(SandboxedCommand::Passthrough) => {
            tracing::warn!("sandbox returned Passthrough — process will run unsandboxed");
            None
        }
        Err(e) => {
            tracing::warn!("sandbox wrapping failed for MCP server, running unsandboxed: {e}");
            None
        }
    }
}

// Keep the old name as a public alias for backward compatibility in tests.
#[allow(dead_code)]
pub(crate) fn build_mcp_sandbox_command(
    command: &str,
    args: &[String],
    sandbox_cfg: &hive_contracts::McpSandboxConfig,
    workspace_path: Option<&std::path::Path>,
) -> Option<(String, Vec<String>, Vec<tempfile::TempPath>)> {
    build_mcp_sandbox_command_from_per_server(command, args, sandbox_cfg, workspace_path)
}

#[cfg(test)]
mod integration_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use hive_core::EventBus;
    use rmcp::model::{
        Annotated, CallToolRequestParam, CallToolResult, Content, ListResourcesResult,
        ListToolsResult, NumberOrString, PaginatedRequestParam, RawResource,
        ReadResourceRequestParam, ReadResourceResult, Resource, ResourceContents,
        ResourceUpdatedNotificationParam, ServerCapabilities, ServerInfo, SubscribeRequestParam,
        Tool,
    };
    use rmcp::{Peer, RoleServer, ServerHandler, ServiceExt};
    use std::collections::VecDeque;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    // ---------------------------------------------------------------
    // Mock MCP Server
    // ---------------------------------------------------------------

    #[derive(Clone)]
    struct MockMcpServer {
        peer: Arc<tokio::sync::Mutex<Option<Peer<RoleServer>>>>,
        tools: Vec<Tool>,
        resources: Vec<Resource>,
        supports_subscribe: bool,
    }

    impl MockMcpServer {
        fn new() -> Self {
            Self {
                peer: Arc::new(tokio::sync::Mutex::new(None)),
                tools: Vec::new(),
                resources: Vec::new(),
                supports_subscribe: false,
            }
        }

        fn with_tool(mut self, name: &str, description: &str) -> Self {
            let schema: Arc<serde_json::Map<String, serde_json::Value>> = Arc::new(
                serde_json::from_value(serde_json::json!({
                    "type": "object",
                    "properties": {}
                }))
                .unwrap(),
            );
            self.tools.push(Tool {
                name: name.to_string().into(),
                description: description.to_string().into(),
                input_schema: schema,
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
    }

    impl ServerHandler for MockMcpServer {
        fn get_info(&self) -> ServerInfo {
            let mut builder = ServerCapabilities::builder().enable_tools().enable_resources();
            if self.supports_subscribe {
                builder = builder.enable_resources_subscribe();
            }
            ServerInfo {
                instructions: Some("Mock MCP server for testing".into()),
                capabilities: builder.build(),
                server_info: rmcp::model::Implementation {
                    name: "mock-mcp-server".into(),
                    version: "0.1.0".into(),
                },
                ..Default::default()
            }
        }

        fn set_peer(&mut self, peer: Peer<RoleServer>) {
            // Use a direct field write since we have &mut self.
            // We can't use async here, so store via std::sync::Mutex workaround.
            // Actually, set_peer gives us &mut self, so we can access the Arc directly.
            let peers = Arc::clone(&self.peer);
            // Fire and forget — the peer will be available by the time tests need it.
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
        ) -> impl std::future::Future<Output = Result<ListToolsResult, rmcp::Error>> + Send + '_
        {
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
            Ok(CallToolResult::success(vec![Content::text(format!(
                "called tool: {}",
                request.name
            ))]))
        }
    }

    // ---------------------------------------------------------------
    // Helpers
    // ---------------------------------------------------------------

    async fn connect_pair(
        server: MockMcpServer,
    ) -> (
        rmcp::service::RunningService<RoleClient, McpClientHandler>,
        rmcp::service::RunningService<RoleServer, MockMcpServer>,
    ) {
        let (client_read, server_write) = tokio::io::duplex(64 * 1024);
        let (server_read, client_write) = tokio::io::duplex(64 * 1024);

        let notifications: Arc<Mutex<VecDeque<McpNotificationEvent>>> =
            Arc::new(Mutex::new(VecDeque::new()));
        let event_bus = EventBus::new(128);

        let dummy_service = McpService::from_configs(
            &[],
            event_bus.clone(),
            Arc::new(parking_lot::RwLock::new(hive_contracts::SandboxConfig::default())),
        );
        let handler = McpClientHandler::new(
            "test-server".to_string(),
            event_bus.clone(),
            Arc::clone(&notifications),
            dummy_service,
        );

        // Must serve both sides concurrently — each side's handshake
        // depends on the other side being ready.
        let (server_result, client_result) = tokio::join!(
            server.serve((server_read, server_write)),
            handler.serve((client_read, client_write)),
        );

        (client_result.unwrap(), server_result.unwrap())
    }

    async fn connect_pair_with_bus(
        server: MockMcpServer,
    ) -> (
        rmcp::service::RunningService<RoleClient, McpClientHandler>,
        rmcp::service::RunningService<RoleServer, MockMcpServer>,
        Arc<Mutex<VecDeque<McpNotificationEvent>>>,
        EventBus,
    ) {
        let (client_read, server_write) = tokio::io::duplex(64 * 1024);
        let (server_read, client_write) = tokio::io::duplex(64 * 1024);

        let notifications: Arc<Mutex<VecDeque<McpNotificationEvent>>> =
            Arc::new(Mutex::new(VecDeque::new()));
        let event_bus = EventBus::new(128);

        let dummy_service = McpService::from_configs(
            &[],
            event_bus.clone(),
            Arc::new(parking_lot::RwLock::new(hive_contracts::SandboxConfig::default())),
        );
        let handler = McpClientHandler::new(
            "test-server".to_string(),
            event_bus.clone(),
            Arc::clone(&notifications),
            dummy_service,
        );

        let (server_result, client_result) = tokio::join!(
            server.serve((server_read, server_write)),
            handler.serve((client_read, client_write)),
        );

        (client_result.unwrap(), server_result.unwrap(), notifications, event_bus)
    }

    // ---------------------------------------------------------------
    // Tests
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn test_handshake_and_capabilities() {
        let server = MockMcpServer::new().with_subscribe();
        let (client, _server_svc) = connect_pair(server).await;

        let info = client.peer_info();
        assert_eq!(info.server_info.name.as_str(), "mock-mcp-server");
        assert!(info.capabilities.tools.is_some());
        assert!(info.capabilities.resources.is_some());
        let res_cap = info.capabilities.resources.as_ref().unwrap();
        assert_eq!(res_cap.subscribe, Some(true));
    }

    #[tokio::test]
    async fn test_tool_discovery() {
        let server = MockMcpServer::new()
            .with_tool("send_notification", "Send a desktop notification")
            .with_tool("read_file", "Read a file from disk");
        let (client, _server_svc) = connect_pair(server).await;

        let tools = client.list_all_tools().await.unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name.as_ref(), "send_notification");
        assert_eq!(tools[1].name.as_ref(), "read_file");
        assert_eq!(tools[0].description.as_ref(), "Send a desktop notification");
    }

    #[tokio::test]
    async fn test_resource_discovery() {
        let server = MockMcpServer::new()
            .with_resource("file:///tmp/test.txt", "test.txt")
            .with_resource("file:///tmp/data.json", "data.json");
        let (client, _server_svc) = connect_pair(server).await;

        let resources = client.list_all_resources().await.unwrap();
        assert_eq!(resources.len(), 2);
        assert_eq!(resources[0].uri, "file:///tmp/test.txt");
        assert_eq!(resources[0].name, "test.txt");
    }

    #[tokio::test]
    async fn test_read_resource() {
        let server = MockMcpServer::new().with_resource("file:///tmp/test.txt", "test.txt");
        let (client, _server_svc) = connect_pair(server).await;

        let result = client
            .read_resource(ReadResourceRequestParam { uri: "file:///tmp/test.txt".into() })
            .await
            .unwrap();
        assert_eq!(result.contents.len(), 1);
        match &result.contents[0] {
            ResourceContents::TextResourceContents { text, .. } => {
                assert_eq!(text, "content of file:///tmp/test.txt");
            }
            _ => panic!("expected text resource"),
        }
    }

    #[tokio::test]
    async fn test_call_tool() {
        let server = MockMcpServer::new().with_tool("greet", "Greet someone");
        let (client, _server_svc) = connect_pair(server).await;

        let result = client
            .call_tool(CallToolRequestParam { name: "greet".into(), arguments: None })
            .await
            .unwrap();
        let text = result
            .content
            .first()
            .and_then(|c| c.raw.as_text())
            .map(|t| t.text.as_str())
            .unwrap_or("");
        assert_eq!(text, "called tool: greet");
    }

    #[tokio::test]
    async fn test_subscribe_resource() {
        let server =
            MockMcpServer::new().with_resource("file:///tmp/test.txt", "test.txt").with_subscribe();
        let (client, _server_svc) = connect_pair(server).await;

        client
            .subscribe(SubscribeRequestParam { uri: "file:///tmp/test.txt".into() })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_subscribe_fails_without_capability() {
        let server = MockMcpServer::new().with_resource("file:///tmp/test.txt", "test.txt");
        let (client, _server_svc) = connect_pair(server).await;

        let result =
            client.subscribe(SubscribeRequestParam { uri: "file:///tmp/test.txt".into() }).await;
        assert!(result.is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_resource_updated_notification() {
        let server =
            MockMcpServer::new().with_resource("file:///tmp/test.txt", "test.txt").with_subscribe();
        let (_client, server_svc, notifications, _bus) = connect_pair_with_bus(server).await;

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let peer = server_svc.peer().clone();
        peer.notify_resource_updated(ResourceUpdatedNotificationParam {
            uri: "file:///tmp/test.txt".into(),
            meta: None,
        })
        .await
        .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let items = notifications.lock().await;
        assert!(!items.is_empty(), "expected at least one notification");
        let notif = &items[0];
        assert_eq!(notif.server_id, "test-server");
        assert!(matches!(notif.kind, McpNotificationKind::ResourceUpdated));
        assert_eq!(notif.payload.get("uri").and_then(|v| v.as_str()), Some("file:///tmp/test.txt"));
    }

    #[tokio::test]
    async fn test_tool_list_changed_notification() {
        let server = MockMcpServer::new().with_tool("greet", "Greet someone");
        let (_client, server_svc, notifications, _bus) = connect_pair_with_bus(server).await;

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let peer = server_svc.peer().clone();
        peer.notify_tool_list_changed().await.unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let items = notifications.lock().await;
        assert!(!items.is_empty(), "expected tool_list_changed notification");
        assert!(matches!(items[0].kind, McpNotificationKind::ToolListChanged));
    }

    #[tokio::test]
    async fn test_resource_list_changed_notification() {
        let server = MockMcpServer::new().with_resource("file:///tmp/test.txt", "test.txt");
        let (_client, server_svc, notifications, _bus) = connect_pair_with_bus(server).await;

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let peer = server_svc.peer().clone();
        peer.notify_resource_list_changed().await.unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let items = notifications.lock().await;
        assert!(!items.is_empty(), "expected resource_list_changed notification");
        assert!(matches!(items[0].kind, McpNotificationKind::ResourceListChanged));
    }

    #[tokio::test]
    async fn test_logging_message_notification() {
        let server = MockMcpServer::new();
        let (_client, server_svc, notifications, _bus) = connect_pair_with_bus(server).await;

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let peer = server_svc.peer().clone();
        peer.notify_logging_message(rmcp::model::LoggingMessageNotificationParam {
            level: rmcp::model::LoggingLevel::Info,
            logger: Some("test-logger".into()),
            data: serde_json::json!("hello from server"),
        })
        .await
        .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let items = notifications.lock().await;
        assert!(!items.is_empty(), "expected logging_message notification");
        assert!(matches!(items[0].kind, McpNotificationKind::LoggingMessage));
        assert_eq!(items[0].payload.get("logger").and_then(|v| v.as_str()), Some("test-logger"));
    }

    #[tokio::test]
    async fn test_progress_notification() {
        let server = MockMcpServer::new();
        let (_client, server_svc, notifications, _bus) = connect_pair_with_bus(server).await;

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let peer = server_svc.peer().clone();
        peer.notify_progress(rmcp::model::ProgressNotificationParam {
            progress_token: NumberOrString::Number(42),
            progress: 50,
            total: Some(100),
        })
        .await
        .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let items = notifications.lock().await;
        assert!(!items.is_empty(), "expected progress notification");
        assert!(matches!(items[0].kind, McpNotificationKind::Progress));
        assert_eq!(items[0].payload.get("progress").and_then(|v| v.as_u64()), Some(50));
        assert_eq!(items[0].payload.get("total").and_then(|v| v.as_u64()), Some(100));
    }

    #[tokio::test]
    async fn test_notification_published_to_event_bus() {
        let server =
            MockMcpServer::new().with_resource("file:///tmp/test.txt", "test.txt").with_subscribe();
        let (_client, server_svc, _notifications, event_bus) = connect_pair_with_bus(server).await;

        let mut sub = event_bus.subscribe_topic("mcp.notification");

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let peer = server_svc.peer().clone();
        peer.notify_resource_updated(ResourceUpdatedNotificationParam {
            uri: "file:///tmp/test.txt".into(),
            meta: None,
        })
        .await
        .unwrap();

        let envelope = tokio::time::timeout(std::time::Duration::from_secs(2), sub.recv())
            .await
            .expect("timed out waiting for event bus notification")
            .expect("event bus recv failed");

        assert_eq!(envelope.topic, "mcp.notification");
        assert_eq!(envelope.payload.get("serverId").and_then(|v| v.as_str()), Some("test-server"));
        assert!(envelope.payload.get("kind").is_some());
    }

    #[tokio::test]
    async fn test_multiple_notifications_in_sequence() {
        let server = MockMcpServer::new()
            .with_tool("tool1", "First tool")
            .with_resource("file:///data.txt", "data.txt")
            .with_subscribe();
        let (_client, server_svc, notifications, _bus) = connect_pair_with_bus(server).await;

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let peer = server_svc.peer().clone();

        peer.notify_tool_list_changed().await.unwrap();
        peer.notify_resource_updated(ResourceUpdatedNotificationParam {
            uri: "file:///data.txt".into(),
            meta: None,
        })
        .await
        .unwrap();
        peer.notify_resource_list_changed().await.unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        let items = notifications.lock().await;
        assert_eq!(items.len(), 3, "expected 3 notifications");
        assert!(matches!(items[0].kind, McpNotificationKind::ToolListChanged));
        assert!(matches!(items[1].kind, McpNotificationKind::ResourceUpdated));
        assert!(matches!(items[2].kind, McpNotificationKind::ResourceListChanged));
    }

    #[tokio::test]
    async fn test_notification_queue_caps_at_max() {
        let server = MockMcpServer::new();
        let (_client, server_svc, notifications, _bus) = connect_pair_with_bus(server).await;

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let peer = server_svc.peer().clone();

        for i in 0..(MAX_NOTIFICATIONS + 10) {
            peer.notify_progress(rmcp::model::ProgressNotificationParam {
                progress_token: NumberOrString::Number(i as u32),
                progress: i as u32,
                total: None,
            })
            .await
            .unwrap();
        }

        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        let items = notifications.lock().await;
        assert_eq!(
            items.len(),
            MAX_NOTIFICATIONS,
            "notification queue should be capped at MAX_NOTIFICATIONS"
        );
        let first = &items[0];
        assert_eq!(first.payload.get("progress").and_then(|v| v.as_u64()), Some(10));
    }

    #[tokio::test]
    async fn test_notification_has_timestamp() {
        let server = MockMcpServer::new();
        let (_client, server_svc, notifications, _bus) = connect_pair_with_bus(server).await;

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let peer = server_svc.peer().clone();
        peer.notify_tool_list_changed().await.unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let items = notifications.lock().await;
        assert!(!items.is_empty());
        assert!(items[0].timestamp_ms > 0, "notification should have a non-zero timestamp");
    }

    #[tokio::test]
    async fn test_event_bus_topic_filtering() {
        let server = MockMcpServer::new();
        let (_client, server_svc, _notifications, event_bus) = connect_pair_with_bus(server).await;

        let mut other_sub = event_bus.subscribe_topic("other.topic");
        let mut mcp_sub = event_bus.subscribe_topic("mcp.notification");

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let peer = server_svc.peer().clone();
        peer.notify_tool_list_changed().await.unwrap();

        let result = tokio::time::timeout(std::time::Duration::from_secs(2), mcp_sub.recv()).await;
        assert!(result.is_ok(), "mcp subscriber should receive notification");

        let result =
            tokio::time::timeout(std::time::Duration::from_millis(200), other_sub.recv()).await;
        assert!(result.is_err(), "other subscriber should not receive mcp notification");
    }
}
