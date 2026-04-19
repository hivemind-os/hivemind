#![allow(dead_code)]
//! Plugin host — spawns and manages plugin processes.
//!
//! Each plugin runs as a Node.js child process communicating via
//! JSON-RPC 2.0 over stdin/stdout with Content-Length framing.

use crate::health::{HealthConfig, HealthMonitor, PluginHealth};
use crate::manifest::HivemindMeta;
use crate::protocol::{
    self, HostInfo, InitializeParams, IncomingMessage, JsonRpcRequest, JsonRpcResponse,
    PluginStatus, PluginToolDef,
};
use crate::sandbox::PluginSandbox;
use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{broadcast, mpsc, oneshot, Mutex};
use tracing::{debug, error, info, warn};

/// An event emitted by a plugin via `host/emitEvent`.
#[derive(Debug, Clone)]
pub struct PluginEvent {
    pub plugin_id: String,
    pub event_type: String,
    pub payload: Value,
}

/// A running plugin process.
pub struct PluginProcess {
    plugin_id: String,
    child: Mutex<Option<Child>>,
    stdin_tx: mpsc::Sender<Vec<u8>>,
    pending: Arc<parking_lot::Mutex<HashMap<u64, oneshot::Sender<Result<Value>>>>>,
    status: Arc<parking_lot::RwLock<PluginStatus>>,
    message_tx: mpsc::Sender<IncomingMessage>,
    event_tx: mpsc::Sender<(String, Value)>,
    sandbox: Option<Arc<PluginSandbox>>,
}

/// Host API callback for handling plugin→host requests.
pub type HostHandler = Arc<dyn Fn(&str, Value) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value>> + Send>> + Send + Sync>;

impl PluginProcess {
    /// Send a JSON-RPC request to the plugin and wait for a response.
    pub async fn request(&self, method: &str, params: Option<Value>) -> Result<Value> {
        let req = JsonRpcRequest::new(method, params);
        let id = req.id.as_ref().and_then(|v| v.as_u64()).unwrap_or(0);

        let (tx, rx) = oneshot::channel();
        self.pending.lock().insert(id, tx);

        let json = serde_json::to_string(&req)?;
        let frame = format!("Content-Length: {}\r\n\r\n{}", json.len(), json);
        self.stdin_tx
            .send(frame.into_bytes())
            .await
            .map_err(|_| anyhow::anyhow!("Plugin stdin closed"))?;

        let result = tokio::time::timeout(std::time::Duration::from_secs(30), rx)
            .await
            .context("Plugin request timed out")?
            .context("Plugin response channel dropped")??;

        Ok(result)
    }

    /// Send a JSON-RPC notification to the plugin (no response expected).
    pub async fn notify(&self, method: &str, params: Option<Value>) -> Result<()> {
        let req = JsonRpcRequest::notification(method, params);
        let json = serde_json::to_string(&req)?;
        let frame = format!("Content-Length: {}\r\n\r\n{}", json.len(), json);
        self.stdin_tx
            .send(frame.into_bytes())
            .await
            .map_err(|_| anyhow::anyhow!("Plugin stdin closed"))?;
        Ok(())
    }

    /// Get the current plugin status.
    pub fn status(&self) -> PluginStatus {
        self.status.read().clone()
    }

    /// Kill the plugin process.
    pub async fn kill(&self) {
        if let Some(mut child) = self.child.lock().await.take() {
            let _ = child.kill().await;
        }
    }
}

/// Manages all plugin processes.
pub struct PluginHost {
    plugins_dir: PathBuf,
    data_dir: PathBuf,
    node_path: Option<PathBuf>,
    processes: parking_lot::RwLock<HashMap<String, Arc<PluginProcess>>>,
    host_info: HostInfo,
    host_handler: Option<HostHandler>,
    health: Arc<HealthMonitor>,
    /// Broadcast channel for plugin events emitted via `host/emitEvent`.
    plugin_event_tx: broadcast::Sender<PluginEvent>,
}

impl PluginHost {
    pub fn new(plugins_dir: PathBuf, data_dir: PathBuf) -> Self {
        let (plugin_event_tx, _) = broadcast::channel(256);
        Self {
            plugins_dir,
            data_dir,
            node_path: None,
            processes: parking_lot::RwLock::new(HashMap::new()),
            host_info: HostInfo {
                version: env!("CARGO_PKG_VERSION").into(),
                platform: std::env::consts::OS.into(),
                capabilities: vec![
                    "tools".into(),
                    "loop".into(),
                    "lifecycle".into(),
                    "secrets".into(),
                    "store".into(),
                    "events".into(),
                ],
            },
            host_handler: None,
            health: Arc::new(HealthMonitor::new(HealthConfig::default())),
            plugin_event_tx,
        }
    }

    /// Set the Node.js executable path.
    pub fn with_node_path(mut self, path: PathBuf) -> Self {
        self.node_path = Some(path);
        self
    }

    /// Set the host handler for plugin→host API calls.
    pub fn with_host_handler(mut self, handler: HostHandler) -> Self {
        self.host_handler = Some(handler);
        self
    }

    /// Subscribe to plugin events emitted via `host/emitEvent`.
    /// Returns a broadcast receiver that yields `PluginEvent`s from all
    /// running plugins.
    pub fn subscribe_events(&self) -> broadcast::Receiver<PluginEvent> {
        self.plugin_event_tx.subscribe()
    }

    /// Spawn a plugin process from its package directory.
    pub async fn spawn(
        &self,
        plugin_id: &str,
        package_dir: &Path,
        entry_point: &str,
        config: Value,
        manifest_meta: Option<&HivemindMeta>,
    ) -> Result<Arc<PluginProcess>> {
        let entry_path = package_dir.join(entry_point);
        if !entry_path.exists() {
            anyhow::bail!(
                "Plugin entry point not found: {}",
                entry_path.display()
            );
        }

        let node_path = self
            .node_path
            .clone()
            .unwrap_or_else(|| PathBuf::from("node"));

        info!(plugin_id, entry_path = %entry_path.display(), "Spawning plugin process");

        let mut cmd = Command::new(&node_path);
        cmd.arg(&entry_path)
            .current_dir(package_dir)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }

        let mut child = cmd.spawn().context("Failed to spawn plugin process")?;

        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();

        // Channel for writing to stdin
        let (stdin_tx, mut stdin_rx) = mpsc::channel::<Vec<u8>>(64);

        // Pending request map
        let pending: Arc<parking_lot::Mutex<HashMap<u64, oneshot::Sender<Result<Value>>>>> =
            Arc::new(parking_lot::Mutex::new(HashMap::new()));

        // Message and event channels
        let (message_tx, _message_rx) = mpsc::channel::<IncomingMessage>(256);
        let (event_tx, mut event_rx) = mpsc::channel::<(String, Value)>(256);

        // Shared status for both process struct and reader task
        let status = Arc::new(parking_lot::RwLock::new(PluginStatus {
            state: "connecting".into(),
            message: None,
            progress: None,
        }));

        // Create sandbox from manifest permissions
        let sandbox = manifest_meta.map(|meta| Arc::new(PluginSandbox::new(plugin_id.into(), meta)));

        let process = Arc::new(PluginProcess {
            plugin_id: plugin_id.into(),
            child: Mutex::new(Some(child)),
            stdin_tx,
            pending: pending.clone(),
            status: status.clone(),
            message_tx: message_tx.clone(),
            event_tx: event_tx.clone(),
            sandbox: sandbox.clone(),
        });

        // Stdin writer task
        tokio::spawn(async move {
            let mut stdin = stdin;
            while let Some(data) = stdin_rx.recv().await {
                if stdin.write_all(&data).await.is_err() {
                    break;
                }
                let _ = stdin.flush().await;
            }
        });

        // Stderr reader task
        let pid = plugin_id.to_string();
        tokio::spawn(async move {
            let reader = BufReader::new(stderr);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                debug!(plugin_id = %pid, "[stderr] {}", line);
            }
        });

        // Plugin event forwarder — bridge the per-process mpsc into the
        // shared broadcast channel so external subscribers (e.g. EventBus)
        // receive plugin events.
        let broadcast_tx = self.plugin_event_tx.clone();
        let event_plugin_id = plugin_id.to_string();
        tokio::spawn(async move {
            while let Some((event_type, payload)) = event_rx.recv().await {
                let _ = broadcast_tx.send(PluginEvent {
                    plugin_id: event_plugin_id.clone(),
                    event_type,
                    payload,
                });
            }
        });

        // Stdout reader task — parse JSON-RPC messages
        let pending_clone = pending.clone();
        let process_status = status;
        let msg_tx = message_tx;
        let evt_tx = event_tx;
        let host_handler = self.host_handler.clone();
        let process_for_reader = process.clone();
        let reader_sandbox = sandbox;

        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            let mut header_buf = String::new();

            loop {
                header_buf.clear();
                // Read headers until \r\n\r\n
                let mut found_empty = false;
                let mut content_length: Option<usize> = None;

                loop {
                    header_buf.clear();
                    match reader.read_line(&mut header_buf).await {
                        Ok(0) => return, // EOF
                        Ok(_) => {
                            let trimmed = header_buf.trim();
                            if trimmed.is_empty() {
                                found_empty = true;
                                break;
                            }
                            if let Some(val) = trimmed.strip_prefix("Content-Length:") {
                                content_length = val.trim().parse().ok();
                            }
                        }
                        Err(_) => return,
                    }
                }

                if !found_empty {
                    continue;
                }

                let len = match content_length {
                    Some(l) => l,
                    None => continue,
                };

                let mut body = vec![0u8; len];
                if reader.read_exact(&mut body).await.is_err() {
                    return;
                }

                // Payload size check
                if let Some(ref sandbox) = reader_sandbox {
                    if let Err(reason) = sandbox.check_payload_size(body.len()) {
                        warn!("{}", reason);
                        continue;
                    }
                }

                let Ok(msg) = serde_json::from_slice::<Value>(&body) else {
                    continue;
                };

                // Determine if it's a response, request, or notification
                if msg.get("id").is_some() && (msg.get("result").is_some() || msg.get("error").is_some()) {
                    // Response to our request
                    if let Some(id) = msg["id"].as_u64() {
                        if let Some(tx) = pending_clone.lock().remove(&id) {
                            if let Some(err) = msg.get("error") {
                                let msg_str = err["message"].as_str().unwrap_or("Unknown error");
                                let _ = tx.send(Err(anyhow::anyhow!("{}", msg_str)));
                            } else {
                                let result = msg.get("result").cloned().unwrap_or(Value::Null);
                                let _ = tx.send(Ok(result));
                            }
                        }
                    }
                } else if let Some(method) = msg["method"].as_str() {
                    let params = msg.get("params").cloned().unwrap_or(Value::Null);
                    let has_id = msg.get("id").is_some();

                    // Permission check: verify the host call is allowed
                    if let Some(ref sandbox) = reader_sandbox {
                        let check = sandbox.check_host_call(method, &params);
                        if !check.allowed {
                            warn!("Sandbox denied host call '{}': {}", method, check.reason.as_deref().unwrap_or("denied"));
                            if has_id {
                                let resp = JsonRpcResponse::error(
                                    msg["id"].clone(),
                                    -32003,
                                    check.reason.unwrap_or_default(),
                                );
                                let json = serde_json::to_string(&resp).unwrap();
                                let frame = format!("Content-Length: {}\r\n\r\n{}", json.len(), json);
                                let _ = process_for_reader.stdin_tx.send(frame.into_bytes()).await;
                            }
                            continue;
                        }
                    }

                    // Handle host API calls from the plugin
                    match method {
                        protocol::host_methods::EMIT_MESSAGE => {
                            // Forward to host handler for testing/capture
                            if let Some(ref handler) = host_handler {
                                let h = handler.clone();
                                let p = params.clone();
                                tokio::spawn(async move { let _ = h("host/emitMessage", p).await; });
                            }
                            // Rate limit check for messages
                            if let Some(ref sandbox) = reader_sandbox {
                                if let Err(reason) = sandbox.check_message_rate() {
                                    warn!("{}", reason);
                                    if has_id {
                                        let resp = JsonRpcResponse::error(
                                            msg["id"].clone(),
                                            -32003,
                                            reason,
                                        );
                                        let json = serde_json::to_string(&resp).unwrap();
                                        let frame = format!("Content-Length: {}\r\n\r\n{}", json.len(), json);
                                        let _ = process_for_reader.stdin_tx.send(frame.into_bytes()).await;
                                    }
                                    continue;
                                }
                            }
                            if let Ok(im) = serde_json::from_value::<IncomingMessage>(
                                params.get("message").cloned().unwrap_or(params.clone()),
                            ) {
                                let _ = msg_tx.send(im).await;
                            }
                            if has_id {
                                let resp = JsonRpcResponse::success(msg["id"].clone(), Value::Null);
                                let json = serde_json::to_string(&resp).unwrap();
                                let frame = format!("Content-Length: {}\r\n\r\n{}", json.len(), json);
                                let _ = process_for_reader.stdin_tx.send(frame.into_bytes()).await;
                            }
                        }
                        protocol::host_methods::EMIT_EVENT => {
                            // Forward to host handler for testing/capture
                            if let Some(ref handler) = host_handler {
                                let h = handler.clone();
                                let p = params.clone();
                                tokio::spawn(async move { let _ = h("host/emitEvent", p).await; });
                            }
                            // Rate limit check for events
                            if let Some(ref sandbox) = reader_sandbox {
                                if let Err(reason) = sandbox.check_event_rate() {
                                    warn!("{}", reason);
                                    if has_id {
                                        let resp = JsonRpcResponse::error(
                                            msg["id"].clone(),
                                            -32003,
                                            reason,
                                        );
                                        let json = serde_json::to_string(&resp).unwrap();
                                        let frame = format!("Content-Length: {}\r\n\r\n{}", json.len(), json);
                                        let _ = process_for_reader.stdin_tx.send(frame.into_bytes()).await;
                                    }
                                    continue;
                                }
                            }
                            let event_type = params["eventType"].as_str().unwrap_or("").to_string();
                            let payload = params.get("payload").cloned().unwrap_or(Value::Null);
                            let _ = evt_tx.send((event_type, payload)).await;
                            if has_id {
                                let resp = JsonRpcResponse::success(msg["id"].clone(), Value::Null);
                                let json = serde_json::to_string(&resp).unwrap();
                                let frame = format!("Content-Length: {}\r\n\r\n{}", json.len(), json);
                                let _ = process_for_reader.stdin_tx.send(frame.into_bytes()).await;
                            }
                        }
                        protocol::host_methods::UPDATE_STATUS => {
                            // Forward to host handler for testing/capture
                            if let Some(ref handler) = host_handler {
                                let h = handler.clone();
                                let p = params.clone();
                                tokio::spawn(async move { let _ = h("host/updateStatus", p).await; });
                            }
                            if let Ok(status) = serde_json::from_value::<PluginStatus>(params.clone()) {
                                *process_status.write() = status;
                            }
                            if has_id {
                                let resp = JsonRpcResponse::success(msg["id"].clone(), Value::Null);
                                let json = serde_json::to_string(&resp).unwrap();
                                let frame = format!("Content-Length: {}\r\n\r\n{}", json.len(), json);
                                let _ = process_for_reader.stdin_tx.send(frame.into_bytes()).await;
                            }
                        }
                        protocol::host_methods::LOG => {
                            let level = params["level"].as_str().unwrap_or("info");
                            let log_msg = params["msg"].as_str().unwrap_or("");
                            match level {
                                "debug" => debug!(plugin = %"plugin", "{}", log_msg),
                                "warn" => warn!(plugin = %"plugin", "{}", log_msg),
                                "error" => error!(plugin = %"plugin", "{}", log_msg),
                                _ => info!(plugin = %"plugin", "{}", log_msg),
                            }
                            // Log is a notification, no response needed
                        }
                        _ => {
                            // Delegate to host handler if available
                            if has_id {
                                if let Some(ref handler) = host_handler {
                                    let handler = handler.clone();
                                    let method = method.to_string();
                                    let process_ref = process_for_reader.clone();
                                    let id = msg["id"].clone();
                                    tokio::spawn(async move {
                                        let result = handler(&method, params).await;
                                        let resp = match result {
                                            Ok(val) => JsonRpcResponse::success(id, val),
                                            Err(e) => JsonRpcResponse::error(id, -32000, e.to_string()),
                                        };
                                        let json = serde_json::to_string(&resp).unwrap();
                                        let frame = format!("Content-Length: {}\r\n\r\n{}", json.len(), json);
                                        let _ = process_ref.stdin_tx.send(frame.into_bytes()).await;
                                    });
                                } else {
                                    let resp = JsonRpcResponse::error(
                                        msg["id"].clone(),
                                        -32601,
                                        format!("No host handler for: {}", method),
                                    );
                                    let json = serde_json::to_string(&resp).unwrap();
                                    let frame = format!("Content-Length: {}\r\n\r\n{}", json.len(), json);
                                    let _ = process_for_reader.stdin_tx.send(frame.into_bytes()).await;
                                }
                            }
                        }
                    }
                }
            }
        });

        // Wait for plugin/ready notification (with timeout)
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        // Send initialize
        let init_params = InitializeParams {
            plugin_id: plugin_id.into(),
            config,
            host_info: self.host_info.clone(),
        };
        let init_result = process
            .request(
                protocol::methods::INITIALIZE,
                Some(serde_json::to_value(&init_params)?),
            )
            .await
            .context("Plugin initialization failed")?;

        info!(plugin_id, ?init_result, "Plugin initialized");

        *process.status.write() = PluginStatus {
            state: "connected".into(),
            message: Some("Initialized".into()),
            progress: None,
        };

        // Register with health monitor and report healthy
        self.health.register(plugin_id);
        self.health.report_healthy(plugin_id);

        // Spawn a background task to detect process exit (crash detection)
        let health_clone = self.health.clone();
        let pid = plugin_id.to_string();
        let process_for_monitor = process.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                let mut guard = process_for_monitor.child.lock().await;
                if let Some(ref mut child) = *guard {
                    match child.try_wait() {
                        Ok(Some(status)) => {
                            let code = status.code();
                            drop(guard);
                            health_clone.report_crash(&pid, code);
                            break;
                        }
                        Ok(None) => {} // still running
                        Err(_) => break,
                    }
                } else {
                    break; // child was taken (graceful shutdown)
                }
            }
        });

        self.processes.write().insert(plugin_id.into(), process.clone());
        Ok(process)
    }

    /// Get a running plugin process.
    pub fn get(&self, plugin_id: &str) -> Option<Arc<PluginProcess>> {
        self.processes.read().get(plugin_id).cloned()
    }

    /// List all running plugin IDs.
    pub fn list_running(&self) -> Vec<String> {
        self.processes.read().keys().cloned().collect()
    }

    /// Stop a plugin process.
    pub async fn stop(&self, plugin_id: &str) -> Result<()> {
        self.health.unregister(plugin_id);
        let process = self.processes.write().remove(plugin_id);
        if let Some(process) = process {
            // Try graceful deactivate first
            let _ = process
                .request(protocol::methods::DEACTIVATE, None)
                .await;
            process.kill().await;
            info!(plugin_id, "Plugin stopped");
        }
        Ok(())
    }

    /// Stop all plugin processes.
    pub async fn stop_all(&self) {
        let ids: Vec<String> = self.processes.read().keys().cloned().collect();
        for id in &ids {
            self.health.unregister(id);
        }
        for id in ids {
            let _ = self.stop(&id).await;
        }
    }

    /// Get health info for a specific plugin.
    pub fn get_health(&self, plugin_id: &str) -> Option<PluginHealth> {
        self.health.get_health(plugin_id)
    }

    /// Get health info for all monitored plugins.
    pub fn all_health(&self) -> Vec<PluginHealth> {
        self.health.all_health()
    }

    /// List tools from a running plugin.
    pub async fn list_tools(&self, plugin_id: &str) -> Result<Vec<PluginToolDef>> {
        let process = self
            .get(plugin_id)
            .ok_or_else(|| anyhow::anyhow!("Plugin not running: {}", plugin_id))?;

        let result = process.request(protocol::methods::TOOLS_LIST, None).await?;
        let tools: Vec<PluginToolDef> = serde_json::from_value(
            result.get("tools").cloned().unwrap_or(Value::Array(vec![])),
        )?;
        Ok(tools)
    }

    /// Call a tool on a running plugin.
    pub async fn call_tool(
        &self,
        plugin_id: &str,
        tool_name: &str,
        arguments: Value,
    ) -> Result<Value> {
        let process = self
            .get(plugin_id)
            .ok_or_else(|| anyhow::anyhow!("Plugin not running: {}", plugin_id))?;

        let params = serde_json::json!({
            "name": tool_name,
            "arguments": arguments,
        });
        process
            .request(protocol::methods::TOOLS_CALL, Some(params))
            .await
    }

    /// Get the config schema from a running plugin.
    pub async fn get_config_schema(&self, plugin_id: &str) -> Result<Value> {
        let process = self
            .get(plugin_id)
            .ok_or_else(|| anyhow::anyhow!("Plugin not running: {}", plugin_id))?;
        process
            .request(protocol::methods::CONFIG_SCHEMA, None)
            .await
    }

    /// Activate a plugin (run onActivate hook).
    pub async fn activate(&self, plugin_id: &str, config: Option<Value>) -> Result<()> {
        let process = self
            .get(plugin_id)
            .ok_or_else(|| anyhow::anyhow!("Plugin not running: {}", plugin_id))?;
        let params = config.map(|c| serde_json::json!({ "config": c }));
        process
            .request(protocol::methods::ACTIVATE, params)
            .await?;
        Ok(())
    }

    /// Start the plugin's background loop.
    pub async fn start_loop(&self, plugin_id: &str) -> Result<()> {
        let process = self
            .get(plugin_id)
            .ok_or_else(|| anyhow::anyhow!("Plugin not running: {}", plugin_id))?;
        process
            .request(protocol::methods::START_LOOP, None)
            .await?;
        Ok(())
    }

    /// Stop the plugin's background loop.
    pub async fn stop_loop(&self, plugin_id: &str) -> Result<()> {
        let process = self
            .get(plugin_id)
            .ok_or_else(|| anyhow::anyhow!("Plugin not running: {}", plugin_id))?;
        process
            .request(protocol::methods::STOP_LOOP, None)
            .await?;
        Ok(())
    }
}

impl Drop for PluginHost {
    fn drop(&mut self) {
        // Best-effort sync cleanup
        for (_, process) in self.processes.write().drain() {
            if let Ok(mut guard) = process.child.try_lock() {
                if let Some(ref mut child) = *guard {
                    let _ = child.start_kill();
                }
            }
        }
    }
}
