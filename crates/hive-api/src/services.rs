//! Daemon service adapters and registry.
//!
//! Each background subsystem gets a thin adapter struct that implements
//! [`DaemonService`] without modifying the original service code.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use hive_contracts::{DaemonService, ServiceCategory, ServiceSnapshot, ServiceStatus};
use parking_lot::Mutex;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tracing::{error, info};

/// Merge environment variables into the shared `shell_env` map.
///
/// For `PATH`, the new value's first component (the managed runtime's bin dir)
/// is **prepended** to the existing `shell_env` PATH rather than overwriting it.
/// This lets both Node.js and Python runtimes coexist on PATH.
/// All other keys are inserted/overwritten normally.
pub fn merge_runtime_env(
    shell_env: &parking_lot::RwLock<std::collections::HashMap<String, String>>,
    vars: std::collections::HashMap<String, String>,
) {
    let mut env = shell_env.write();
    for (k, v) in vars {
        if k == "PATH" {
            // The manager computed PATH as "<bin_dir><sep><system_PATH>".
            // Extract just the bin_dir prefix (everything before the first
            // separator) and prepend it to whatever is already in shell_env.
            let sep = if cfg!(windows) { ';' } else { ':' };
            let bin_dir = v.split(sep).next().unwrap_or(&v);
            if let Some(existing) = env.get("PATH").cloned() {
                // Avoid duplicates: don't prepend if it's already there.
                if !existing.split(sep).any(|p| p == bin_dir) {
                    env.insert(k, format!("{bin_dir}{sep}{existing}"));
                }
            } else {
                env.insert(k, v);
            }
        } else {
            env.insert(k, v);
        }
    }
}

// ── Status helpers ──────────────────────────────────────────────────

const STATUS_RUNNING: u8 = 0;
const STATUS_STOPPED: u8 = 1;
const STATUS_STARTING: u8 = 2;
const STATUS_STOPPING: u8 = 3;
const STATUS_ERROR: u8 = 4;

fn status_from_u8(v: u8) -> ServiceStatus {
    match v {
        STATUS_RUNNING => ServiceStatus::Running,
        STATUS_STOPPED => ServiceStatus::Stopped,
        STATUS_STARTING => ServiceStatus::Starting,
        STATUS_STOPPING => ServiceStatus::Stopping,
        _ => ServiceStatus::Error,
    }
}

// ── Service Registry ────────────────────────────────────────────────

/// Event emitted when a service's status changes.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ServiceStatusEvent {
    pub service_id: String,
    pub status: ServiceStatus,
    pub error: Option<String>,
}

/// Central registry holding all daemon services.
pub struct ServiceRegistry {
    services: parking_lot::RwLock<Vec<Arc<dyn DaemonService>>>,
    status_tx: broadcast::Sender<ServiceStatusEvent>,
}

impl Default for ServiceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ServiceRegistry {
    pub fn new() -> Self {
        let (status_tx, _) = broadcast::channel(64);
        Self { services: parking_lot::RwLock::new(Vec::new()), status_tx }
    }

    pub fn register(&self, service: Arc<dyn DaemonService>) {
        self.services.write().push(service);
    }

    pub fn list(&self) -> Vec<ServiceSnapshot> {
        self.services.read().iter().map(|s| s.snapshot()).collect()
    }

    pub fn get(&self, id: &str) -> Option<Arc<dyn DaemonService>> {
        self.services.read().iter().find(|s| s.service_id() == id).cloned()
    }

    /// Restart a service by id and emit status events.
    pub async fn restart(&self, id: &str) -> anyhow::Result<()> {
        let svc = self.get(id).ok_or_else(|| anyhow::anyhow!("unknown service: {id}"))?;

        {
            let _span = tracing::info_span!("service", service = %id).entered();
            info!("restarting service");
        }

        self.emit(id, ServiceStatus::Stopping, None);
        if let Err(e) = svc.stop().await {
            let _span = tracing::info_span!("service", service = %id).entered();
            error!(error = %e, "service stop failed");
            self.emit(id, ServiceStatus::Error, Some(e.to_string()));
            return Err(e);
        }

        self.emit(id, ServiceStatus::Starting, None);
        if let Err(e) = svc.start().await {
            let _span = tracing::info_span!("service", service = %id).entered();
            error!(error = %e, "service start failed");
            self.emit(id, ServiceStatus::Error, Some(e.to_string()));
            return Err(e);
        }

        {
            let _span = tracing::info_span!("service", service = %id).entered();
            info!("service restarted");
        }
        self.emit(id, ServiceStatus::Running, None);
        Ok(())
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ServiceStatusEvent> {
        self.status_tx.subscribe()
    }

    fn emit(&self, service_id: &str, status: ServiceStatus, error: Option<String>) {
        let _ = self.status_tx.send(ServiceStatusEvent {
            service_id: service_id.to_string(),
            status,
            error,
        });
    }
}

// ── Scheduler Adapter ───────────────────────────────────────────────

pub struct SchedulerDaemonService {
    inner: Arc<hive_scheduler::SchedulerService>,
}

impl SchedulerDaemonService {
    pub fn new(inner: Arc<hive_scheduler::SchedulerService>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl DaemonService for SchedulerDaemonService {
    fn service_id(&self) -> &str {
        "scheduler"
    }

    fn display_name(&self) -> String {
        "Task Scheduler".into()
    }

    fn category(&self) -> ServiceCategory {
        ServiceCategory::Core
    }

    fn status(&self) -> ServiceStatus {
        if self.inner.is_running() {
            ServiceStatus::Running
        } else {
            ServiceStatus::Stopped
        }
    }

    async fn start(&self) -> anyhow::Result<()> {
        self.inner.start_background_loop();
        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        self.inner.stop().await;
        Ok(())
    }
}

// ── Trigger Manager Adapter ─────────────────────────────────────────

pub struct TriggerManagerDaemonService {
    inner: Arc<hive_workflow_service::TriggerManager>,
}

impl TriggerManagerDaemonService {
    pub fn new(inner: Arc<hive_workflow_service::TriggerManager>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl DaemonService for TriggerManagerDaemonService {
    fn service_id(&self) -> &str {
        "trigger-manager"
    }

    fn display_name(&self) -> String {
        "Trigger Manager".into()
    }

    fn category(&self) -> ServiceCategory {
        ServiceCategory::Core
    }

    fn status(&self) -> ServiceStatus {
        if self.inner.is_running() {
            ServiceStatus::Running
        } else {
            ServiceStatus::Stopped
        }
    }

    async fn start(&self) -> anyhow::Result<()> {
        self.inner.start().await;
        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        self.inner.stop().await;
        Ok(())
    }
}

// ── Connector Service Adapter ───────────────────────────────────────

/// Single global connector service — manages the overall polling lifecycle.
pub struct ConnectorDaemonService {
    inner: Arc<hive_connectors::ConnectorService>,
}

impl ConnectorDaemonService {
    pub fn new(inner: Arc<hive_connectors::ConnectorService>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl DaemonService for ConnectorDaemonService {
    fn service_id(&self) -> &str {
        "connector-polling"
    }

    fn display_name(&self) -> String {
        "Connector Polling".into()
    }

    fn category(&self) -> ServiceCategory {
        ServiceCategory::Connector
    }

    fn status(&self) -> ServiceStatus {
        if self.inner.is_polling() {
            ServiceStatus::Running
        } else {
            ServiceStatus::Stopped
        }
    }

    async fn start(&self) -> anyhow::Result<()> {
        self.inner.start_background_poll();
        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        self.inner.stop_polling();
        Ok(())
    }
}

/// Per-connector listener service — one row per connector in FlightDeck.
///
/// Delegates start/stop to the global polling service but reports status
/// and errors specific to this connector.
pub struct ConnectorListenerDaemonService {
    connector_svc: Arc<hive_connectors::ConnectorService>,
    connector_id: String,
    connector_name: String,
}

impl ConnectorListenerDaemonService {
    pub fn new(
        connector_svc: Arc<hive_connectors::ConnectorService>,
        connector_id: String,
        connector_name: String,
    ) -> Self {
        Self { connector_svc, connector_id, connector_name }
    }
}

#[async_trait]
impl DaemonService for ConnectorListenerDaemonService {
    fn service_id(&self) -> &str {
        &self.connector_id
    }

    fn display_name(&self) -> String {
        format!("{} Listener", self.connector_name)
    }

    fn category(&self) -> ServiceCategory {
        ServiceCategory::Connector
    }

    fn status(&self) -> ServiceStatus {
        if self.connector_svc.is_polling() {
            ServiceStatus::Running
        } else {
            ServiceStatus::Stopped
        }
    }

    async fn start(&self) -> anyhow::Result<()> {
        // Starting an individual connector listener restarts the whole poll loop,
        // which re-spawns tasks for all eligible connectors.
        self.connector_svc.start_background_poll();
        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        self.connector_svc.stop_polling();
        Ok(())
    }
}

// ── MCP Server Adapter (per server) ─────────────────────────────────

pub struct McpServerDaemonService {
    mcp: Arc<hive_mcp::McpService>,
    server_id: String,
    server_name: String,
    cached_status: Arc<Mutex<ServiceStatus>>,
    cached_error: Arc<Mutex<Option<String>>>,
}

impl McpServerDaemonService {
    pub fn new(mcp: Arc<hive_mcp::McpService>, server_id: String, server_name: String) -> Self {
        Self {
            mcp,
            server_id,
            server_name,
            cached_status: Arc::new(Mutex::new(ServiceStatus::Stopped)),
            cached_error: Arc::new(Mutex::new(None)),
        }
    }

    fn service_id_string(&self) -> String {
        format!("mcp:{}", self.server_id)
    }
}

#[async_trait]
impl DaemonService for McpServerDaemonService {
    fn service_id(&self) -> &str {
        // SAFETY: service_id must return &str, but our id is dynamic.
        // We leak a small string per MCP server (bounded by config count).
        // This is acceptable because MCP servers are registered once at startup.
        Box::leak(self.service_id_string().into_boxed_str())
    }

    fn display_name(&self) -> String {
        format!("MCP: {}", self.server_name)
    }

    fn category(&self) -> ServiceCategory {
        ServiceCategory::Mcp
    }

    fn status(&self) -> ServiceStatus {
        *self.cached_status.lock()
    }

    async fn start(&self) -> anyhow::Result<()> {
        let svc_id = self.service_id_string();
        {
            let _span = tracing::info_span!("service", service = %svc_id).entered();
            info!(server_id = %self.server_id, "connecting MCP server");
        }
        match self.mcp.connect(&self.server_id).await {
            Ok(_) => {
                let _span = tracing::info_span!("service", service = %svc_id).entered();
                info!(server_id = %self.server_id, "MCP server connected");
                *self.cached_status.lock() = ServiceStatus::Running;
                *self.cached_error.lock() = None;
                Ok(())
            }
            Err(e) => {
                let msg = e.to_string();
                let _span = tracing::info_span!("service", service = %svc_id).entered();
                error!(server_id = %self.server_id, error = %msg, "MCP server connection failed");
                *self.cached_status.lock() = ServiceStatus::Error;
                *self.cached_error.lock() = Some(msg.clone());
                Err(anyhow::anyhow!("{msg}"))
            }
        }
    }

    async fn stop(&self) -> anyhow::Result<()> {
        let svc_id = self.service_id_string();
        {
            let _span = tracing::info_span!("service", service = %svc_id).entered();
            info!(server_id = %self.server_id, "disconnecting MCP server");
        }
        self.mcp.disconnect(&self.server_id).await.map_err(|e| anyhow::anyhow!("{e}"))?;
        let _span = tracing::info_span!("service", service = %svc_id).entered();
        info!(server_id = %self.server_id, "MCP server disconnected");
        *self.cached_status.lock() = ServiceStatus::Stopped;
        *self.cached_error.lock() = None;
        Ok(())
    }

    fn last_error(&self) -> Option<String> {
        self.cached_error.lock().clone()
    }
}

// ── Event Log Adapter ───────────────────────────────────────────────

pub struct EventLogDaemonService {
    inner: Arc<hive_core::EventLog>,
    status: AtomicU8,
}

impl EventLogDaemonService {
    pub fn new(inner: Arc<hive_core::EventLog>) -> Self {
        Self { inner, status: AtomicU8::new(STATUS_RUNNING) }
    }
}

#[async_trait]
impl DaemonService for EventLogDaemonService {
    fn service_id(&self) -> &str {
        "event-log"
    }

    fn display_name(&self) -> String {
        "Event Log Writer".into()
    }

    fn category(&self) -> ServiceCategory {
        ServiceCategory::Core
    }

    fn status(&self) -> ServiceStatus {
        status_from_u8(self.status.load(Ordering::SeqCst))
    }

    async fn start(&self) -> anyhow::Result<()> {
        self.inner.start_writer();
        self.status.store(STATUS_RUNNING, Ordering::SeqCst);
        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        // EventLog writer task ends when the sender is dropped, which only
        // happens when the EventLog itself is dropped. We can't cleanly stop
        // it independently, so we mark it as stopped for the UI.
        self.status.store(STATUS_STOPPED, Ordering::SeqCst);
        Ok(())
    }
}

// ── AFK Forwarder Adapter ───────────────────────────────────────────

pub struct AfkForwarderDaemonService {
    config: Arc<arc_swap::ArcSwap<hive_contracts::HiveMindConfig>>,
    user_status: crate::UserStatusRuntime,
    chat: Arc<hive_chat::ChatService>,
    connectors: Option<Arc<hive_connectors::ConnectorService>>,
    forwarded: crate::afk::ForwardedStore,
    event_bus: hive_core::EventBus,
    handle: Mutex<Option<JoinHandle<()>>>,
    status: AtomicU8,
}

impl AfkForwarderDaemonService {
    pub fn new(
        config: Arc<arc_swap::ArcSwap<hive_contracts::HiveMindConfig>>,
        user_status: crate::UserStatusRuntime,
        chat: Arc<hive_chat::ChatService>,
        connectors: Option<Arc<hive_connectors::ConnectorService>>,
        forwarded: crate::afk::ForwardedStore,
        event_bus: hive_core::EventBus,
    ) -> Self {
        Self {
            config,
            user_status,
            chat,
            connectors,
            forwarded,
            event_bus,
            handle: Mutex::new(None),
            status: AtomicU8::new(STATUS_STOPPED),
        }
    }
}

#[async_trait]
impl DaemonService for AfkForwarderDaemonService {
    fn service_id(&self) -> &str {
        "afk-forwarder"
    }

    fn display_name(&self) -> String {
        "AFK Forwarder".into()
    }

    fn category(&self) -> ServiceCategory {
        ServiceCategory::Core
    }

    fn status(&self) -> ServiceStatus {
        // Check if the task has finished unexpectedly.
        let guard = self.handle.lock();
        if let Some(ref h) = *guard {
            if h.is_finished() {
                return ServiceStatus::Stopped;
            }
        }
        status_from_u8(self.status.load(Ordering::SeqCst))
    }

    async fn start(&self) -> anyhow::Result<()> {
        let h = crate::afk::spawn_afk_forwarder(
            Arc::clone(&self.config),
            self.user_status.clone(),
            Arc::clone(&self.chat),
            self.connectors.clone(),
            Arc::clone(&self.forwarded),
            self.event_bus.clone(),
        );
        *self.handle.lock() = Some(h);
        self.status.store(STATUS_RUNNING, Ordering::SeqCst);
        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        if let Some(h) = self.handle.lock().take() {
            h.abort();
        }
        self.status.store(STATUS_STOPPED, Ordering::SeqCst);
        Ok(())
    }
}

// ── Chat Service Adapter ────────────────────────────────────────────

pub struct ChatDaemonService {
    inner: Arc<hive_chat::ChatService>,
    status: AtomicU8,
}

impl ChatDaemonService {
    pub fn new(inner: Arc<hive_chat::ChatService>) -> Self {
        Self { inner, status: AtomicU8::new(STATUS_RUNNING) }
    }
}

#[async_trait]
impl DaemonService for ChatDaemonService {
    fn service_id(&self) -> &str {
        "chat"
    }

    fn display_name(&self) -> String {
        "Chat Service".into()
    }

    fn category(&self) -> ServiceCategory {
        ServiceCategory::Core
    }

    fn status(&self) -> ServiceStatus {
        status_from_u8(self.status.load(Ordering::SeqCst))
    }

    async fn start(&self) -> anyhow::Result<()> {
        // Soft start: re-run session/bot restoration.
        self.inner.restore_sessions().await.map_err(|e| anyhow::anyhow!("{e}"))?;
        self.inner.restore_bots().await.map_err(|e| anyhow::anyhow!("{e}"))?;
        self.status.store(STATUS_RUNNING, Ordering::SeqCst);
        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        // ChatService doesn't have a stop method — it's always available.
        // Mark as stopped for the UI.
        self.status.store(STATUS_STOPPED, Ordering::SeqCst);
        Ok(())
    }
}

// ── Bot Supervisor Adapter ──────────────────────────────────────────

pub struct BotSupervisorDaemonService {
    chat: Arc<hive_chat::ChatService>,
}

impl BotSupervisorDaemonService {
    pub fn new(chat: Arc<hive_chat::ChatService>) -> Self {
        Self { chat }
    }
}

#[async_trait]
impl DaemonService for BotSupervisorDaemonService {
    fn service_id(&self) -> &str {
        "bot-supervisor"
    }

    fn display_name(&self) -> String {
        "Bot Supervisor".into()
    }

    fn category(&self) -> ServiceCategory {
        ServiceCategory::Agents
    }

    fn status(&self) -> ServiceStatus {
        if self.chat.has_bot_supervisor() {
            ServiceStatus::Running
        } else {
            ServiceStatus::Stopped
        }
    }

    async fn start(&self) -> anyhow::Result<()> {
        self.chat.get_or_create_bot_supervisor().await.map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        self.chat.shutdown_bot_supervisor().await;
        Ok(())
    }
}

// ── Workflow Service Adapter ────────────────────────────────────────

pub struct WorkflowDaemonService {
    inner: Arc<hive_workflow_service::WorkflowService>,
    status: AtomicU8,
}

impl WorkflowDaemonService {
    pub fn new(inner: Arc<hive_workflow_service::WorkflowService>) -> Self {
        Self { inner, status: AtomicU8::new(STATUS_RUNNING) }
    }
}

#[async_trait]
impl DaemonService for WorkflowDaemonService {
    fn service_id(&self) -> &str {
        "workflows"
    }

    fn display_name(&self) -> String {
        "Workflow Engine".into()
    }

    fn category(&self) -> ServiceCategory {
        ServiceCategory::Core
    }

    fn status(&self) -> ServiceStatus {
        status_from_u8(self.status.load(Ordering::SeqCst))
    }

    async fn start(&self) -> anyhow::Result<()> {
        // Soft start: re-run workflow recovery.
        match self.inner.recover().await {
            Ok(n) if n > 0 => info!("recovered {n} orphaned workflow(s)"),
            Err(e) => {
                error!("workflow recovery failed: {e}");
                self.status.store(STATUS_ERROR, Ordering::SeqCst);
                return Err(anyhow::anyhow!("{e}"));
            }
            _ => {}
        }
        self.status.store(STATUS_RUNNING, Ordering::SeqCst);
        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        self.status.store(STATUS_STOPPED, Ordering::SeqCst);
        Ok(())
    }
}

// ── Inference Adapter ───────────────────────────────────────────────

pub struct InferenceDaemonService {
    status: AtomicU8,
}

impl InferenceDaemonService {
    pub fn new(has_runtime: bool) -> Self {
        Self { status: AtomicU8::new(if has_runtime { STATUS_RUNNING } else { STATUS_STOPPED }) }
    }
}

#[async_trait]
impl DaemonService for InferenceDaemonService {
    fn service_id(&self) -> &str {
        "inference"
    }

    fn display_name(&self) -> String {
        "Local Inference".into()
    }

    fn category(&self) -> ServiceCategory {
        ServiceCategory::Inference
    }

    fn status(&self) -> ServiceStatus {
        status_from_u8(self.status.load(Ordering::SeqCst))
    }

    async fn start(&self) -> anyhow::Result<()> {
        let _span = tracing::info_span!("service", service = "inference").entered();
        info!("local inference service started");
        self.status.store(STATUS_RUNNING, Ordering::SeqCst);
        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        let _span = tracing::info_span!("service", service = "inference").entered();
        info!("local inference service stopped");
        self.status.store(STATUS_STOPPED, Ordering::SeqCst);
        Ok(())
    }
}

// ── Python Environment Adapter ──────────────────────────────────────

pub struct PythonEnvDaemonService {
    inner: Arc<hive_python_env::PythonEnvManager>,
    shell_env: Arc<parking_lot::RwLock<std::collections::HashMap<String, String>>>,
    last_error: Mutex<Option<String>>,
}

impl PythonEnvDaemonService {
    pub fn new(
        inner: Arc<hive_python_env::PythonEnvManager>,
        shell_env: Arc<parking_lot::RwLock<std::collections::HashMap<String, String>>>,
    ) -> Self {
        Self { inner, shell_env, last_error: Mutex::new(None) }
    }
}

#[async_trait]
impl DaemonService for PythonEnvDaemonService {
    fn service_id(&self) -> &str {
        "python-env"
    }

    fn display_name(&self) -> String {
        "Python Environment".into()
    }

    fn category(&self) -> ServiceCategory {
        ServiceCategory::Core
    }

    fn status(&self) -> ServiceStatus {
        match self.inner.status_blocking() {
            Some(hive_python_env::PythonEnvStatus::Ready { .. }) => ServiceStatus::Running,
            Some(hive_python_env::PythonEnvStatus::Installing { .. }) => ServiceStatus::Starting,
            Some(hive_python_env::PythonEnvStatus::Failed { .. }) => ServiceStatus::Error,
            Some(hive_python_env::PythonEnvStatus::Disabled) => ServiceStatus::Stopped,
            Some(hive_python_env::PythonEnvStatus::NotInstalled) => ServiceStatus::Stopped,
            None => ServiceStatus::Starting,
        }
    }

    async fn start(&self) -> anyhow::Result<()> {
        {
            let _span = tracing::info_span!("service", service = "python-env").entered();
            info!("ensuring Python environment");
        }
        match self.inner.ensure_default_env().await {
            Ok(env_info) => {
                if let Some(vars) = self.inner.shell_env_vars(None).await {
                    merge_runtime_env(&self.shell_env, vars);
                }
                *self.last_error.lock() = None;
                let _span = tracing::info_span!("service", service = "python-env").entered();
                info!(venv = %env_info.venv_path.display(), "Python environment ready");
                Ok(())
            }
            Err(hive_python_env::PythonEnvError::Disabled) => {
                let _span = tracing::info_span!("service", service = "python-env").entered();
                info!("Python environment disabled");
                *self.last_error.lock() = None;
                Ok(())
            }
            Err(e) => {
                let msg = e.to_string();
                let _span = tracing::info_span!("service", service = "python-env").entered();
                error!(error = %msg, "Python environment setup failed");
                *self.last_error.lock() = Some(msg.clone());
                Err(anyhow::anyhow!(msg))
            }
        }
    }

    async fn stop(&self) -> anyhow::Result<()> {
        let _span = tracing::info_span!("service", service = "python-env").entered();
        info!("Python environment stopped");
        Ok(())
    }

    fn last_error(&self) -> Option<String> {
        self.last_error.lock().clone()
    }
}

// ── Node.js Environment Adapter ─────────────────────────────────────

pub struct NodeEnvDaemonService {
    inner: Arc<hive_node_env::NodeEnvManager>,
    shell_env: Arc<parking_lot::RwLock<std::collections::HashMap<String, String>>>,
    last_error: Mutex<Option<String>>,
}

impl NodeEnvDaemonService {
    pub fn new(
        inner: Arc<hive_node_env::NodeEnvManager>,
        shell_env: Arc<parking_lot::RwLock<std::collections::HashMap<String, String>>>,
    ) -> Self {
        Self { inner, shell_env, last_error: Mutex::new(None) }
    }
}

#[async_trait]
impl DaemonService for NodeEnvDaemonService {
    fn service_id(&self) -> &str {
        "node-env"
    }

    fn display_name(&self) -> String {
        "Node.js Environment".into()
    }

    fn category(&self) -> ServiceCategory {
        ServiceCategory::Core
    }

    fn status(&self) -> ServiceStatus {
        match self.inner.status_blocking() {
            Some(hive_node_env::NodeEnvStatus::Ready { .. }) => ServiceStatus::Running,
            Some(hive_node_env::NodeEnvStatus::Installing { .. }) => ServiceStatus::Starting,
            Some(hive_node_env::NodeEnvStatus::Failed { .. }) => ServiceStatus::Error,
            Some(hive_node_env::NodeEnvStatus::Disabled) => ServiceStatus::Stopped,
            Some(hive_node_env::NodeEnvStatus::NotInstalled) => ServiceStatus::Stopped,
            None => ServiceStatus::Starting,
        }
    }

    async fn start(&self) -> anyhow::Result<()> {
        {
            let _span = tracing::info_span!("service", service = "node-env").entered();
            info!("detecting existing Node.js installation");
        }

        // First check if a previous installation exists on disk.
        self.inner.detect_existing().await;
        if matches!(self.inner.status().await, hive_node_env::NodeEnvStatus::Ready { .. }) {
            if let Some(vars) = self.inner.shell_env_vars().await {
                merge_runtime_env(&self.shell_env, vars);
            }
            let _span = tracing::info_span!("service", service = "node-env").entered();
            info!("Node.js environment already installed");
            *self.last_error.lock() = None;
            return Ok(());
        }

        // If enabled but not yet installed, download now.
        match self.inner.ensure_node().await {
            Ok(dist_dir) => {
                if let Some(vars) = self.inner.shell_env_vars().await {
                    merge_runtime_env(&self.shell_env, vars);
                }
                *self.last_error.lock() = None;
                let _span = tracing::info_span!("service", service = "node-env").entered();
                info!(path = %dist_dir.display(), "Node.js environment ready");
                Ok(())
            }
            Err(hive_node_env::NodeEnvError::Disabled) => {
                let _span = tracing::info_span!("service", service = "node-env").entered();
                info!("Node.js environment disabled");
                *self.last_error.lock() = None;
                Ok(())
            }
            Err(e) => {
                let msg = e.to_string();
                let _span = tracing::info_span!("service", service = "node-env").entered();
                error!(error = %msg, "Node.js environment setup failed");
                *self.last_error.lock() = Some(msg.clone());
                Err(anyhow::anyhow!(msg))
            }
        }
    }

    async fn stop(&self) -> anyhow::Result<()> {
        let _span = tracing::info_span!("service", service = "node-env").entered();
        info!("Node.js environment stopped");
        Ok(())
    }

    fn last_error(&self) -> Option<String> {
        self.last_error.lock().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hive_node_env::{NodeEnvConfig, NodeEnvManager};
    use std::path::Path;
    use tempfile::TempDir;

    /// Create a fake Node.js distribution directory that `detect_existing()` will
    /// recognise as a valid installation.
    fn create_fake_node_dist(hivemind_home: &Path, version: &str) {
        let platform = match std::env::consts::OS {
            "macos" => "darwin",
            "linux" => "linux",
            "windows" => "win",
            os => panic!("unsupported OS for test: {os}"),
        };
        let arch = match std::env::consts::ARCH {
            "x86_64" => "x64",
            "aarch64" => "arm64",
            a => panic!("unsupported arch for test: {a}"),
        };
        let dir = hivemind_home
            .join("runtimes")
            .join("node")
            .join(format!("node-v{version}-{platform}-{arch}"));
        if cfg!(target_os = "windows") {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("node.exe"), b"fake").unwrap();
        } else {
            let bin = dir.join("bin");
            std::fs::create_dir_all(&bin).unwrap();
            std::fs::write(bin.join("node"), b"fake").unwrap();
        }
    }

    #[tokio::test]
    async fn daemon_start_with_existing_install() {
        let tmp = TempDir::new().unwrap();
        let config = NodeEnvConfig::default();
        let version = config.node_version.clone();
        create_fake_node_dist(tmp.path(), &version);

        let mgr = Arc::new(NodeEnvManager::new(tmp.path().to_path_buf(), config));
        let svc = NodeEnvDaemonService::new(
            Arc::clone(&mgr),
            Arc::new(parking_lot::RwLock::new(std::collections::HashMap::new())),
        );

        svc.start().await.expect("start should succeed");
        assert_eq!(svc.status(), ServiceStatus::Running);
        assert!(svc.last_error().is_none());
    }

    #[tokio::test]
    async fn daemon_start_disabled() {
        let tmp = TempDir::new().unwrap();
        let config = NodeEnvConfig { enabled: false, ..Default::default() };
        let mgr = Arc::new(NodeEnvManager::new(tmp.path().to_path_buf(), config));
        let svc = NodeEnvDaemonService::new(
            mgr,
            Arc::new(parking_lot::RwLock::new(std::collections::HashMap::new())),
        );

        svc.start().await.expect("start should succeed when disabled");
        assert_eq!(svc.status(), ServiceStatus::Stopped);
    }

    #[tokio::test]
    async fn daemon_status_mapping() {
        let tmp = TempDir::new().unwrap();
        let config = NodeEnvConfig::default();
        let version = config.node_version.clone();

        let mgr = Arc::new(NodeEnvManager::new(tmp.path().to_path_buf(), config));
        let svc = NodeEnvDaemonService::new(
            Arc::clone(&mgr),
            Arc::new(parking_lot::RwLock::new(std::collections::HashMap::new())),
        );

        // Initially not installed → Stopped
        assert_eq!(svc.status(), ServiceStatus::Stopped);

        // After detecting an existing install → Running
        create_fake_node_dist(tmp.path(), &version);
        mgr.detect_existing().await;
        assert_eq!(svc.status(), ServiceStatus::Running);
    }

    #[tokio::test]
    async fn daemon_start_populates_shell_env() {
        let tmp = TempDir::new().unwrap();
        let config = NodeEnvConfig::default();
        let version = config.node_version.clone();
        create_fake_node_dist(tmp.path(), &version);

        let mgr = Arc::new(NodeEnvManager::new(tmp.path().to_path_buf(), config));
        let shell_env = Arc::new(parking_lot::RwLock::new(std::collections::HashMap::new()));
        let svc = NodeEnvDaemonService::new(Arc::clone(&mgr), Arc::clone(&shell_env));

        svc.start().await.expect("start should succeed");
        assert_eq!(svc.status(), ServiceStatus::Running);

        let env = shell_env.read();
        assert!(
            env.contains_key("PATH"),
            "shell_env should contain PATH after Node.js env is ready"
        );
    }

    #[test]
    fn merge_runtime_env_preserves_both_paths() {
        let shell_env = parking_lot::RwLock::new(std::collections::HashMap::new());
        let sep = if cfg!(windows) { ';' } else { ':' };

        // First runtime sets PATH
        let mut vars1 = std::collections::HashMap::new();
        vars1.insert("PATH".to_string(), format!("/opt/python/bin{sep}/usr/bin"));
        merge_runtime_env(&shell_env, vars1);

        {
            let env = shell_env.read();
            assert!(env.get("PATH").unwrap().starts_with("/opt/python/bin"));
        }

        // Second runtime merges its bin dir without overwriting the first
        let mut vars2 = std::collections::HashMap::new();
        vars2.insert("PATH".to_string(), format!("/opt/node/bin{sep}/usr/bin"));
        merge_runtime_env(&shell_env, vars2);

        {
            let env = shell_env.read();
            let path = env.get("PATH").unwrap();
            assert!(path.contains("/opt/node/bin"), "PATH should contain node bin dir: {path}");
            assert!(
                path.contains("/opt/python/bin"),
                "PATH should still contain python bin dir: {path}"
            );
        }

        // Calling again with same bin dir should not duplicate
        let mut vars3 = std::collections::HashMap::new();
        vars3.insert("PATH".to_string(), format!("/opt/node/bin{sep}/usr/bin"));
        merge_runtime_env(&shell_env, vars3);

        {
            let env = shell_env.read();
            let path = env.get("PATH").unwrap();
            let count = path.matches("/opt/node/bin").count();
            assert_eq!(count, 1, "node bin dir should appear exactly once: {path}");
        }
    }
}
