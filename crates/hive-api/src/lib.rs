pub mod afk;
pub mod auth_middleware;
pub mod canvas_ws;
pub mod provider_auth;
pub mod routes;
mod scheduler_deps;
pub mod services;
mod workflow_deps;

// Re-export modules from extracted crates for backward compatibility.
pub use hive_chat as chat;
pub use hive_chat::bridge;
pub use hive_chat::session_log;

pub use chat::{
    ApprovalStreamEvent, ChatMemoryItem, ChatMessage, ChatMessageRole, ChatMessageStatus,
    ChatRunState, ChatRuntimeConfig, ChatService, ChatServiceError, ChatSessionSnapshot,
    ChatSessionSummary, InterruptMode, InterruptRequest, SendMessageRequest, SendMessageResponse,
    SessionEvent, SessionModality, ToolApprovalRequest, ToolApprovalResponse, ToolInvocationError,
    WorkspaceEntry, WorkspaceFileContent,
};
pub use hive_agents::{BotConfig, BotMode, BotSummary};
pub use hive_contracts;
pub use hive_contracts::{ModelRouterSnapshot, SaveFileRequest};
pub use hive_inference::DownloadProgress;
pub use hive_local_models::{
    HardwareSummary, HubRepoFilesResult, InstallModelRequest, LocalModelError, LocalModelService,
    LocalModelSummary,
};
pub use hive_loop::LoopEvent;
pub use hive_risk::{
    FlaggedSpan, PromptInjectionReview, RiskScanRecord, RiskService, RiskServiceError, RiskVerdict,
    ScanActionTaken, ScanDecision, ScanRecommendation, ScanSummary,
};
pub use hive_scheduler::{
    CreateTaskRequest, ListTasksFilter, ScheduledTask, SchedulerConfig, SchedulerError,
    SchedulerService, TaskAction, TaskRun, TaskRunStatus, TaskSchedule, TaskStatus,
    UpdateTaskRequest,
};
pub use hive_skills_service::{SkillsService, SkillsServiceError};

use arc_swap::ArcSwap;
use axum::{
    http::StatusCode,
    routing::{delete, get, post, put},
    Router,
};
use hive_agents::TelemetrySnapshot;
use hive_contracts::config::{AfkConfig, UserStatus, WebSearchConfig};
use hive_core::{
    bundled_persona_skill_names, bundled_persona_yamls, discover_paths, load_personas,
    migrate_personas_from_config, AuditLogger, EventBus, EventLog, HiveMindConfig,
    QueuedSubscriber,
};
use hive_knowledge::KnowledgeGraph;
use hive_mcp::{McpCatalogStore, McpService, McpServiceError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Instant;
use tokio::sync::Notify;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::cors::CorsLayer;
use tracing::Instrument;

/// Shared async HTTP client for API handlers to avoid per-request allocation.
pub(crate) fn shared_api_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("failed to build shared reqwest client")
    })
}

/// Aggregate MCP server configs from all personas, deduplicating by cache key.
pub(crate) fn collect_all_mcp_configs(config: &HiveMindConfig) -> Vec<hive_core::McpServerConfig> {
    collect_mcp_configs(&config.personas)
}

/// Aggregate MCP server configs from a list of personas, deduplicating by cache key.
pub(crate) fn collect_mcp_configs(
    personas: &[hive_contracts::Persona],
) -> Vec<hive_core::McpServerConfig> {
    use std::collections::HashSet;
    let mut seen_keys = HashSet::new();
    let mut result = Vec::new();

    for persona in personas {
        for s in &persona.mcp_servers {
            let key = s.cache_key();
            if seen_keys.insert(key) {
                result.push(s.clone());
            }
        }
    }

    result
}

/// Collect MCP server configs for a single persona (+ global backward-compat
/// servers), deduplicating by cache key.  Used when building per-session MCP
/// Collect MCP server configs for a single persona, deduplicating by cache key.
/// Used when building per-session MCP managers that should only expose the
/// active persona's servers.
#[allow(dead_code)]
pub(crate) fn collect_persona_mcp_configs(
    persona: Option<&hive_contracts::Persona>,
) -> Vec<hive_core::McpServerConfig> {
    match persona {
        Some(persona) => {
            use std::collections::HashSet;
            let mut seen_keys = HashSet::new();
            let mut result = Vec::new();
            for s in &persona.mcp_servers {
                let key = s.cache_key();
                if seen_keys.insert(key) {
                    result.push(s.clone());
                }
            }
            result
        }
        None => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Runtime user-status tracking (AFK feature)
// ---------------------------------------------------------------------------

/// Runtime state for user availability status, separate from persisted config.
#[derive(Clone)]
pub struct UserStatusRuntime {
    inner: Arc<parking_lot::Mutex<UserStatusInner>>,
    status_tx: tokio::sync::broadcast::Sender<UserStatus>,
}

struct UserStatusInner {
    status: UserStatus,
    last_heartbeat: Instant,
    /// True when the user explicitly set their status (e.g. manual "Away").
    /// Heartbeats will not override a manually-set status.
    manual_override: bool,
    /// True once any client has sent at least one heartbeat.
    has_ever_received_heartbeat: bool,
    /// When the daemon (and this runtime) was created.
    daemon_started_at: Instant,
}

impl Default for UserStatusRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl UserStatusRuntime {
    pub fn new() -> Self {
        let (tx, _) = tokio::sync::broadcast::channel(16);
        Self {
            inner: Arc::new(parking_lot::Mutex::new(UserStatusInner {
                status: UserStatus::Active,
                last_heartbeat: Instant::now(),
                manual_override: false,
                has_ever_received_heartbeat: false,
                daemon_started_at: Instant::now(),
            })),
            status_tx: tx,
        }
    }

    pub fn get(&self) -> UserStatus {
        self.inner.lock().status
    }

    pub fn set(&self, status: UserStatus) {
        let mut inner = self.inner.lock();
        // Setting to Active clears the manual override (user is "back").
        // Any other explicit status change is a manual override that heartbeats won't undo.
        inner.manual_override = status != UserStatus::Active;
        if inner.status != status {
            inner.status = status;
            let _ = self.status_tx.send(status);
        }
    }

    /// Record a UI heartbeat. Reverts auto-idle/away back to Active,
    /// but never overrides a manually-set status.
    pub fn heartbeat(&self) -> UserStatus {
        let mut inner = self.inner.lock();
        inner.last_heartbeat = Instant::now();
        inner.has_ever_received_heartbeat = true;
        if !inner.manual_override
            && (inner.status == UserStatus::Idle || inner.status == UserStatus::Away)
        {
            inner.status = UserStatus::Active;
            let _ = self.status_tx.send(UserStatus::Active);
        }
        inner.status
    }

    /// Check elapsed time since last heartbeat and auto-transition if configured.
    pub fn check_auto_transitions(&self, config: &AfkConfig) {
        let mut inner = self.inner.lock();
        let elapsed = inner.last_heartbeat.elapsed();

        // Only auto-transition from Active → Idle → Away (never touch DND or manual Away)
        if inner.status == UserStatus::Active {
            if let Some(idle_secs) = config.auto_idle_after_secs {
                if elapsed >= std::time::Duration::from_secs(idle_secs) {
                    inner.status = UserStatus::Idle;
                    let _ = self.status_tx.send(UserStatus::Idle);
                }
            }
        }
        if inner.status == UserStatus::Idle {
            if let Some(away_secs) = config.auto_away_after_secs {
                if elapsed >= std::time::Duration::from_secs(away_secs) {
                    inner.status = UserStatus::Away;
                    let _ = self.status_tx.send(UserStatus::Away);
                }
            }
        }
    }

    /// If no desktop client has ever connected and the grace period has
    /// elapsed since daemon start, auto-transition to Away.
    pub fn check_no_client_transition(&self, config: &AfkConfig) {
        let mut inner = self.inner.lock();
        if inner.has_ever_received_heartbeat || inner.manual_override {
            return;
        }
        if inner.status != UserStatus::Active {
            return;
        }
        if let Some(grace_secs) = config.no_client_grace_period_secs {
            let elapsed = inner.daemon_started_at.elapsed();
            if elapsed >= std::time::Duration::from_secs(grace_secs) {
                inner.status = UserStatus::Away;
                let _ = self.status_tx.send(UserStatus::Away);
            }
        }
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<UserStatus> {
        self.status_tx.subscribe()
    }
}

#[derive(Clone)]
pub struct AppState {
    pub start_time: Instant,
    pub shutdown: Arc<Notify>,
    /// Crypto-secure token generated on daemon startup.  Clients must
    /// present this as `Authorization: Bearer <token>` on every
    /// non-exempt API request.
    pub auth_token: String,
    pub config: Arc<ArcSwap<HiveMindConfig>>,
    pub config_path: PathBuf,
    pub personas_dir: PathBuf,
    pub hivemind_home: PathBuf,
    pub audit: AuditLogger,
    pub event_bus: EventBus,
    pub chat: Arc<ChatService>,
    pub skills: Arc<SkillsService>,
    pub mcp: Arc<McpService>,
    pub mcp_catalog: McpCatalogStore,
    pub local_models: Option<Arc<LocalModelService>>,
    pub scheduler: Arc<SchedulerService>,
    pub workflows: Arc<hive_workflow_service::WorkflowService>,
    pub trigger_manager: Arc<hive_workflow_service::TriggerManager>,
    pub knowledge_graph_path: Arc<PathBuf>,
    pub entity_graph: Arc<hive_core::EntityGraph>,
    pub runtime_manager: Option<Arc<hive_inference::RuntimeManager>>,
    pub pending_device_codes:
        Arc<parking_lot::Mutex<HashMap<String, provider_auth::DeviceCodeResponse>>>,
    pub pending_oauth_meta: Arc<parking_lot::Mutex<HashMap<String, OAuthPendingMeta>>>,
    pub canvas_sessions: canvas_ws::CanvasSessionRegistry,
    pub connectors: Option<Arc<hive_connectors::ConnectorService>>,
    pub event_log: Option<Arc<EventLog>>,
    /// Deferred workflow wiring — consumed once inside the Tokio runtime.
    pub pending_workflow_wiring: Arc<parking_lot::Mutex<Option<WorkflowWiring>>>,
    /// Runtime user availability status (AFK feature).
    pub user_status: UserStatusRuntime,
    /// Tracks interactions forwarded to communication channels.
    pub forwarded_interactions: afk::ForwardedStore,
    /// Per-service log collector (tracing layer).
    pub service_log_collector: hive_core::ServiceLogCollector,
    /// Central registry of daemon background services.
    pub service_registry: Arc<services::ServiceRegistry>,
    /// Plugin registry for managing installed plugins.
    pub plugin_registry: Arc<hive_plugins::PluginRegistry>,
    /// Plugin host for running plugin processes.
    pub plugin_host: Arc<hive_plugins::PluginHost>,
    /// Concrete scheduler tool executor for post-init refresh (MCP tools).
    pub(crate) scheduler_tool_executor: Arc<scheduler_deps::SchedulerToolExecutorImpl>,
    /// Managed Python environment for agent shell commands.
    pub python_env: Arc<hive_python_env::PythonEnvManager>,
    /// Managed Node.js environment for MCP servers.
    pub node_env: Arc<hive_node_env::NodeEnvManager>,
    /// Shared env vars injected into shell.execute (updated by Python env setup).
    pub(crate) shell_env: Arc<parking_lot::RwLock<HashMap<String, String>>>,
    /// Sandbox configuration (hot-reloadable).
    pub(crate) sandbox_config: Arc<parking_lot::RwLock<hive_contracts::SandboxConfig>>,
}

/// Components needed to wire up the workflow service extension traits.
/// Created during AppState::new() but consumed in start_background() where
/// we have access to the Tokio runtime.
pub struct WorkflowWiring {
    tool_executor: Arc<workflow_deps::WorkflowToolExecutorImpl>,
    agent_runner: Arc<workflow_deps::WorkflowAgentRunnerImpl>,
    interaction_gate: Arc<workflow_deps::WorkflowInteractionGateImpl>,
    task_scheduler: Arc<workflow_deps::WorkflowTaskSchedulerImpl>,
    prompt_renderer: Arc<workflow_deps::WorkflowPromptRendererImpl>,
}

/// Metadata stashed during an OAuth device code flow, keyed by device_code.
/// Used by the poll handler to auto-create/update the channel on success.
#[derive(Clone)]
pub struct OAuthPendingMeta {
    pub channel_id: String,
    pub provider: hive_contracts::connectors::ConnectorProvider,
    pub client_id: String,
    pub email: String,
}

/// Try to find and load all configured embedding models into the RuntimeManager.
///
/// For each model in the config, tries:
/// 1. Registry lookup by hub_repo + runtime
/// 2. Well-known path under storage directory
/// 3. Vendor directory (dev/test builds)
fn ensure_embedding_models_loaded(
    config: &hive_contracts::EmbeddingConfig,
    registry: &dyn hive_inference::ModelRegistryStore,
    runtime_manager: &Arc<hive_inference::RuntimeManager>,
    storage_path: &std::path::Path,
) {
    for model_def in &config.models {
        if runtime_manager.is_loaded(&model_def.model_id) {
            continue;
        }
        if let Err(e) = try_load_embedding_model(
            &model_def.model_id,
            &model_def.hub_repo,
            model_def.runtime,
            registry,
            runtime_manager,
            storage_path,
        ) {
            tracing::warn!(
                model_id = %model_def.model_id,
                error = %e,
                "embedding model not found — run setup wizard or `cargo xtask fetch-models`"
            );
        }
    }
}

fn try_load_embedding_model(
    model_id: &str,
    hub_repo: &str,
    runtime: hive_contracts::InferenceRuntimeKind,
    registry: &dyn hive_inference::ModelRegistryStore,
    runtime_manager: &Arc<hive_inference::RuntimeManager>,
    storage_path: &std::path::Path,
) -> anyhow::Result<()> {
    // Strategy 1: Look in the model registry.
    if let Ok(models) = registry.list_by_runtime(runtime) {
        for model in &models {
            if model.hub_repo == hub_repo
                && model.status == hive_inference::ModelStatus::Available
                && model.local_path.exists()
            {
                runtime_manager.load_model(model_id, &model.local_path, runtime)?;
                tracing::info!(
                    model_id,
                    path = %model.local_path.display(),
                    "auto-loaded embedding model from registry"
                );
                return Ok(());
            }
        }
    }

    // Strategy 2: Well-known path: <storage>/<repo_sanitized>/onnx_model.onnx
    let well_known = storage_path.join(hub_repo.replace('/', "_")).join("onnx_model.onnx");
    if well_known.exists() {
        runtime_manager.load_model(model_id, &well_known, runtime)?;
        tracing::info!(
            model_id,
            path = %well_known.display(),
            "auto-loaded embedding model from well-known path"
        );
        return Ok(());
    }

    // Strategy 3: Vendor directory (dev/test).
    let vendor_path = std::path::Path::new("vendor").join(model_id).join("model.onnx");
    if vendor_path.exists() {
        runtime_manager.load_model(model_id, &vendor_path, runtime)?;
        tracing::info!(
            model_id,
            path = %vendor_path.display(),
            "auto-loaded embedding model from vendor directory"
        );
        return Ok(());
    }

    anyhow::bail!("model files not found for '{model_id}'")
}

impl AppState {
    pub fn new(
        config: HiveMindConfig,
        audit: AuditLogger,
        event_bus: EventBus,
        shutdown: Arc<Notify>,
        auth_token: String,
    ) -> anyhow::Result<Self> {
        let paths = discover_paths()?;
        let knowledge_graph_path = Arc::new(paths.knowledge_graph_path.clone());
        let entity_graph_path = paths.hivemind_home.join("entity-graph.db");
        let entity_graph = Arc::new(
            hive_core::EntityGraph::open(&entity_graph_path)
                .map_err(|e| anyhow::anyhow!("failed to open entity graph: {e}"))?,
        );

        // Create local model service first so we can share its registry with
        // the model router.
        let storage_path = config
            .local_models
            .storage_path
            .clone()
            .unwrap_or_else(|| paths.hivemind_home.join("models"));
        // Read HF token from the global secret store (not from config).
        let hf_token = hive_core::secret_store::load("hf_token");
        let local_models = Arc::new(
            LocalModelService::new(
                paths.local_models_db_path,
                storage_path.clone(),
                hf_token.as_deref(),
            )
            .unwrap_or_else(|_| {
                LocalModelService::with_in_memory_registry(
                    storage_path.clone(),
                    hf_token.as_deref(),
                )
                .expect("in-memory local model service")
            }),
        );

        // Configure LLM HTTP client timeouts before first use.
        hive_model::configure_timeouts(
            config.models.request_timeout_secs,
            config.models.stream_timeout_secs,
        );

        let runtime_manager = Arc::new(if config.local_models.isolate_runtimes {
            let worker_binary = std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|d| d.join("hive-runtime-worker")))
                .unwrap_or_else(|| std::path::PathBuf::from("hive-runtime-worker"));
            tracing::info!(binary = %worker_binary.display(), "using isolated runtime workers");
            hive_inference::RuntimeManager::new_isolated(
                config.local_models.max_loaded_models,
                worker_binary,
                config.models.request_timeout_secs.map(std::time::Duration::from_secs),
            )
        } else {
            hive_inference::RuntimeManager::new(config.local_models.max_loaded_models)
        });

        let model_router = chat::build_model_router_from_config(
            &config,
            Some(local_models.registry()),
            Some(&runtime_manager),
        )?;
        // Share the model router with SkillsService for server-side skill auditing.
        let skill_model_router = Arc::new(arc_swap::ArcSwap::from(Arc::clone(&model_router)));
        let canvas_sessions = canvas_ws::CanvasSessionRegistry::new();

        let sandbox_config: Arc<parking_lot::RwLock<hive_contracts::SandboxConfig>> =
            Arc::new(parking_lot::RwLock::new(config.security.sandbox.clone()));

        // Managed Python environment for agent shell commands.
        let python_config = hive_python_env::PythonEnvConfig {
            enabled: config.python.enabled,
            python_version: config.python.python_version.clone(),
            base_packages: config.python.base_packages.clone(),
            auto_detect_workspace_deps: config.python.auto_detect_workspace_deps,
            uv_version: config.python.uv_version.clone(),
        };
        let python_env = Arc::new(hive_python_env::PythonEnvManager::new(
            paths.hivemind_home.clone(),
            python_config,
        ));

        // Managed Node.js environment for MCP servers.
        let node_config = hive_node_env::NodeEnvConfig {
            enabled: config.node.enabled,
            node_version: config.node.node_version.clone(),
        };
        let node_env =
            Arc::new(hive_node_env::NodeEnvManager::new(paths.hivemind_home.clone(), node_config));

        // Load personas from individual files early so that persona-defined
        // MCP servers are included in the global McpService and catalog
        // discovery.  Migration from config.yaml and bundled-persona seeding
        // must happen first so the files exist on disk.
        if !config.personas.is_empty() {
            let migrated =
                migrate_personas_from_config(&paths.personas_dir, &config.personas).unwrap_or(0);
            if migrated > 0 {
                tracing::info!(
                    "migrated {} persona(s) from config.yaml to {}",
                    migrated,
                    paths.personas_dir.display()
                );
            }
        }
        match hive_core::seed_bundled_personas(&paths.personas_dir) {
            Ok(n) if n > 0 => tracing::info!("seeded/updated {n} bundled persona(s)"),
            Err(e) => tracing::warn!(error = %e, "failed to seed bundled personas"),
            _ => {}
        }
        let personas = load_personas(&paths.personas_dir).unwrap_or_default();

        let all_mcp_configs = collect_mcp_configs(&personas);
        let mcp = Arc::new(
            McpService::from_configs(
                &all_mcp_configs,
                event_bus.clone(),
                Arc::clone(&sandbox_config),
            )
            .with_node_env(Arc::clone(&node_env))
            .with_python_env(Arc::clone(&python_env)),
        );
        let mcp_catalog = McpCatalogStore::new(&paths.hivemind_home);

        // ── Migrate legacy keyring entries to global secret store ────
        {
            let provider_ids: Vec<String> =
                config.models.providers.iter().map(|p| p.id.clone()).collect();
            let connectors_path = paths.hivemind_home.join("connectors.yaml");
            let connector_ids: Vec<String> = connectors_path
                .exists()
                .then(|| std::fs::read_to_string(&connectors_path).ok())
                .flatten()
                .and_then(|yaml| serde_yaml::from_str::<Vec<serde_json::Value>>(&yaml).ok())
                .map(|vals| {
                    vals.iter()
                        .filter_map(|v| v.get("id").and_then(|id| id.as_str()).map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            hive_core::secret_store::migrate_legacy(
                &provider_ids,
                &connector_ids,
                config.hf_token.as_deref(),
            );
        }

        let connector_service = {
            let connectors_dir = paths.hivemind_home.join("connectors");
            match hive_connectors::ConnectorService::new(&connectors_dir) {
                Ok(mut svc) => {
                    svc.set_event_bus(Arc::new(event_bus.clone()));
                    let connectors_path = paths.hivemind_home.join("connectors.yaml");
                    if connectors_path.exists() {
                        if let Ok(yaml) = std::fs::read_to_string(&connectors_path) {
                            if let Ok(configs) =
                                serde_yaml::from_str::<Vec<hive_connectors::ConnectorConfig>>(&yaml)
                            {
                                if let Err(e) = svc.load_connectors(configs) {
                                    tracing::warn!("failed to load connectors: {e}");
                                }
                            }
                        }
                    }
                    Some(Arc::new(svc))
                }
                Err(e) => {
                    tracing::warn!("failed to initialize connector service: {e}");
                    None
                }
            }
        };

        let connector_registry = connector_service.as_ref().map(|cs| cs.registry());
        let connector_audit_log = connector_service.as_ref().map(|cs| cs.audit_log());

        // Build scheduler with its trait dependencies.
        let tool_executor = Arc::new(scheduler_deps::SchedulerToolExecutorImpl::new(
            connector_registry.clone(),
            connector_audit_log.clone(),
            Arc::clone(&mcp),
            mcp_catalog.clone(),
            event_bus.clone(),
        ));
        let agent_runner =
            Arc::new(scheduler_deps::SchedulerAgentRunnerImpl::new(paths.personas_dir.clone()));
        let notifier = Arc::new(scheduler_deps::SchedulerNotifierImpl::new(event_bus.clone()));

        let mut scheduler = match SchedulerService::new(
            paths.hivemind_home.join("scheduler.db"),
            event_bus.clone(),
            config.api.bind.clone(),
            SchedulerConfig::default(),
        ) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("scheduler database failed to open ({e}) — falling back to ephemeral in-memory scheduler; previously scheduled tasks will not be available");
                SchedulerService::in_memory_with_addr(
                    event_bus.clone(),
                    config.api.bind.clone(),
                    SchedulerConfig::default(),
                )
                .expect("in-memory scheduler")
            }
        };
        let scheduler_tool_executor = Arc::clone(&tool_executor);
        scheduler.set_tool_executor(tool_executor);
        scheduler.set_agent_runner(
            agent_runner.clone() as Arc<dyn hive_scheduler::SchedulerAgentRunner>
        );
        scheduler.set_notifier(notifier);
        scheduler.set_auth_token(auth_token.clone());
        let scheduler = Arc::new(scheduler);

        // Build workflow service.
        let wf_db_path = paths.hivemind_home.join("workflows.db");
        let mut wf_service =
            hive_workflow_service::WorkflowService::new(&wf_db_path, Some(event_bus.clone()))
                .map_err(|e| {
                    tracing::error!(
                        path = %wf_db_path.display(),
                        error = %e,
                        "failed to open workflow database — workflow data will NOT be persisted"
                    );
                    e
                })?;
        wf_service.set_workspaces_base_dir(paths.hivemind_home.join("workflows"));
        let workflows = Arc::new(wf_service);

        // Clone connector refs for workflow tool executor (before they're moved to ChatService).
        let wf_connector_registry = connector_registry.clone();
        let wf_connector_audit_log = connector_audit_log.clone();

        // Build the connector service handle for data-class enforcement in tools.
        let connector_svc_handle: Option<Arc<dyn hive_connectors::ConnectorServiceHandle>> =
            connector_service
                .as_ref()
                .map(|cs| Arc::clone(cs) as Arc<dyn hive_connectors::ConnectorServiceHandle>);

        // Shared environment variables for shell commands (updated when managed
        // Python environment becomes available).
        let shell_env: Arc<parking_lot::RwLock<HashMap<String, String>>> =
            Arc::new(parking_lot::RwLock::new(HashMap::new()));

        // Detect available shells on the system (done once at startup).
        let detected_shells = Arc::new(hive_tools::detect_shells());

        let chat = Arc::new(ChatService::with_model_router(
            audit.clone(),
            event_bus.clone(),
            ChatRuntimeConfig::default(),
            paths.hivemind_home.clone(),
            paths.knowledge_graph_path,
            config.security.prompt_injection.clone(),
            config.security.command_policy.clone(),
            paths.risk_ledger_path,
            model_router,
            canvas_sessions.clone(),
            config.compaction.clone(),
            config.api.bind.clone(),
            config.embedding.clone(),
            Some(Arc::clone(&mcp)),
            Some(mcp_catalog.clone()),
            connector_registry,
            connector_audit_log,
            connector_svc_handle,
            Arc::clone(&scheduler),
            Arc::clone(&shell_env),
            Arc::clone(&sandbox_config),
            Arc::clone(&detected_shells),
            config.tool_limits.clone(),
        ));
        chat.set_default_permissions(config.security.default_permissions.clone());
        chat.set_runtime_manager(Arc::clone(&runtime_manager));
        chat.update_web_search_config(resolve_web_search_keyring(&config.web_search));

        // Complete the scheduler agent runner wiring now that ChatService exists.
        agent_runner.set_chat(Arc::clone(&chat));

        // Auto-load all configured embedding models so that workspace indexing
        // and vector search work out of the box.
        ensure_embedding_models_loaded(
            &config.embedding,
            local_models.registry(),
            &runtime_manager,
            &storage_path,
        );

        // Personas were already loaded earlier (before McpService construction)
        // so that persona-defined MCP servers are included in the global catalog.
        // Now push them into the chat service.
        chat.update_personas(personas);

        let skills_data_dir =
            dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("hivemind");
        let skills = Arc::new(SkillsService::new(
            config.skills.clone(),
            skills_data_dir,
            paths.personas_dir.clone(),
            Some(skill_model_router),
        )?);

        // Sync bundled skills into the index DB so they appear in the UI and
        // runtime catalog. `seed_bundled_personas` wrote the files to disk
        // above; now we ensure each one has a corresponding `installed_skills`
        // row.
        for &(persona_id, _) in bundled_persona_yamls() {
            for skill_name in bundled_persona_skill_names(persona_id) {
                let persona_path = persona_id.replace('/', std::path::MAIN_SEPARATOR_STR);
                let skill_dir =
                    paths.personas_dir.join(&persona_path).join("skills").join(skill_name);
                let skill_md_path = skill_dir.join("SKILL.md");
                match std::fs::read_to_string(&skill_md_path) {
                    Ok(content) => match hive_skills::parse_skill_md(&content) {
                        Ok(parsed) => {
                            if let Err(e) = skills.sync_bundled_skill(
                                persona_id,
                                parsed.manifest,
                                &skill_dir.to_string_lossy(),
                            ) {
                                tracing::warn!(
                                    persona_id, skill_name,
                                    error = %e,
                                    "failed to sync bundled skill into index"
                                );
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                persona_id, skill_name,
                                error = %e,
                                "failed to parse bundled SKILL.md"
                            );
                        }
                    },
                    Err(e) => {
                        tracing::warn!(
                            persona_id, skill_name,
                            error = %e,
                            "failed to read bundled SKILL.md"
                        );
                    }
                }
            }
        }

        chat.set_skills_service(Arc::clone(&skills));
        chat.set_workflow_service(Arc::clone(&workflows));
        chat.set_entity_graph(Arc::clone(&entity_graph));

        // Wire up workflow extension traits now that ChatService exists.
        // The actual async wiring is deferred to start_background() since
        // AppState::new() runs outside the Tokio runtime.
        let pending_workflow_wiring = {
            let tool_executor = Arc::new(workflow_deps::WorkflowToolExecutorImpl::new(
                wf_connector_registry,
                wf_connector_audit_log,
                Arc::clone(&mcp),
                mcp_catalog.clone(),
                event_bus.clone(),
            ));
            let wf_agent_runner =
                Arc::new(workflow_deps::WorkflowAgentRunnerImpl::new(paths.personas_dir.clone()));
            wf_agent_runner.set_chat(Arc::clone(&chat));
            wf_agent_runner.set_entity_graph(Arc::clone(&entity_graph));
            let interaction_gate =
                Arc::new(workflow_deps::WorkflowInteractionGateImpl::new(event_bus.clone()));
            let task_scheduler =
                Arc::new(workflow_deps::WorkflowTaskSchedulerImpl::new(Arc::clone(&scheduler)));
            let prompt_renderer = Arc::new(workflow_deps::WorkflowPromptRendererImpl::new(
                paths.personas_dir.clone(),
            ));
            Arc::new(parking_lot::Mutex::new(Some(WorkflowWiring {
                tool_executor,
                agent_runner: wf_agent_runner,
                interaction_gate,
                task_scheduler,
                prompt_renderer,
            })))
        };

        // Wire up router rebuilder so downloaded models become available.
        {
            let chat_clone = Arc::clone(&chat);
            let config_clone = config.clone();
            let embedding_config_clone = config.embedding.clone();
            let registry_clone = local_models.registry().clone();
            let runtime_clone = Arc::clone(&runtime_manager);
            let storage_clone = storage_path.clone();
            local_models.set_router_rebuilder(Arc::new(move || {
                // Rebuild the chat model router.
                match chat::build_model_router_from_config(
                    &config_clone,
                    Some(&registry_clone),
                    Some(&runtime_clone),
                ) {
                    Ok(new_router) => {
                        chat_clone.swap_router(new_router);
                        tracing::info!("Model router rebuilt after local model install");
                    }
                    Err(e) => {
                        tracing::error!("Failed to rebuild model router after install: {e}");
                    }
                }
                // Also try to load any newly-available embedding models.
                ensure_embedding_models_loaded(
                    &embedding_config_clone,
                    &registry_clone,
                    &runtime_clone,
                    &storage_clone,
                );
            }));
        }
        // Migrate legacy file-based GitHub token to the OS keyring.
        let token_path = paths.hivemind_home.join("github-token");
        if token_path.exists() {
            if let Ok(token) = std::fs::read_to_string(&token_path) {
                let token = token.trim().to_string();
                if !token.is_empty() {
                    hive_core::secret_store::save("github:oauth-token", &token);
                    tracing::info!("migrated GitHub token from file to OS keyring");
                }
            }
            let _ = std::fs::remove_file(&token_path);
        }

        // Create the persistent event log (pre-runtime — writer spawned in
        // start_background).
        let event_log = match EventLog::open(paths.hivemind_home.join("event_log.db")) {
            Ok(log) => {
                let log = Arc::new(log);
                // Seed the EventBus counter from the DB so new event IDs
                // don't collide with previously persisted events.
                let max_id = log.max_event_id();
                if max_id > 0 {
                    event_bus.set_next_id(max_id + 1);
                }
                event_bus.register_subscriber(Arc::clone(&log) as Arc<dyn QueuedSubscriber>);
                Some(log)
            }
            Err(e) => {
                tracing::warn!("failed to open event log: {e}");
                None
            }
        };

        // Create the trigger manager for auto-launching workflows.
        // Wiring to workflow service is deferred to start_background().
        let trigger_manager = Arc::new(hive_workflow_service::TriggerManager::new(
            event_bus.clone(),
            Arc::clone(workflows.store()),
        ));

        // Clone event_bus before it is moved into AppState so the
        // plugin host handler can publish notifications.
        let plugin_notify_bus = event_bus.clone();

        // Clone scheduler for the plugin host handler closure.
        let plugin_scheduler = scheduler.clone();

        Ok(Self {
            start_time: Instant::now(),
            shutdown,
            auth_token,
            config: Arc::new(ArcSwap::from_pointee(config)),
            config_path: paths.config_path,
            personas_dir: paths.personas_dir,
            hivemind_home: paths.hivemind_home.clone(),
            audit,
            event_bus,
            chat,
            skills,
            mcp,
            mcp_catalog,
            local_models: Some(local_models),
            scheduler,
            workflows,
            trigger_manager,
            knowledge_graph_path,
            entity_graph,
            runtime_manager: Some(runtime_manager),
            pending_device_codes: Arc::new(parking_lot::Mutex::new(HashMap::new())),
            pending_oauth_meta: Arc::new(parking_lot::Mutex::new(HashMap::new())),
            canvas_sessions,
            connectors: connector_service,
            event_log,
            pending_workflow_wiring,
            user_status: UserStatusRuntime::new(),
            forwarded_interactions: Arc::new(parking_lot::Mutex::new(HashMap::new())),
            service_log_collector: hive_core::ServiceLogCollector::new(),
            service_registry: Arc::new(services::ServiceRegistry::new()),
            plugin_registry: {
                let plugins_dir = paths.hivemind_home.join("plugins");
                let registry = Arc::new(hive_plugins::PluginRegistry::new(plugins_dir));
                let _ = registry.load();
                registry
            },
            plugin_host: {
                let plugins_dir = paths.hivemind_home.join("plugins");
                let data_dir = paths.hivemind_home.join("plugin-data");
                let mut host = hive_plugins::PluginHost::new(plugins_dir, data_dir.clone());

                // Set up host handler for plugin→host API calls (secrets via OS keyring,
                // store via filesystem).
                let store_base = data_dir;
                let notify_event_bus = plugin_notify_bus.clone();
                let scheduler_for_handler = plugin_scheduler.clone();
                host = host.with_host_handler(Arc::new(move |method: &str, params: serde_json::Value| {
                    let method = method.to_string();
                    let store_base = store_base.clone();
                    let notify_bus = notify_event_bus.clone();
                    let scheduler = scheduler_for_handler.clone();
                    Box::pin(async move {
                        match method.as_str() {
                            "host/secretGet" => {
                                let key = params["key"].as_str().unwrap_or("");
                                let plugin_id = params["pluginId"].as_str().unwrap_or("unknown");
                                let scoped_key = format!("plugin:{}:{}", plugin_id, key);
                                let value = hive_core::secret_store::load(&scoped_key);
                                Ok(serde_json::json!({ "value": value }))
                            }
                            "host/secretSet" => {
                                let key = params["key"].as_str().unwrap_or("");
                                let value = params["value"].as_str().unwrap_or("");
                                let plugin_id = params["pluginId"].as_str().unwrap_or("unknown");
                                let scoped_key = format!("plugin:{}:{}", plugin_id, key);
                                hive_core::secret_store::save(&scoped_key, value);
                                Ok(serde_json::json!({}))
                            }
                            "host/secretDelete" => {
                                let key = params["key"].as_str().unwrap_or("");
                                let plugin_id = params["pluginId"].as_str().unwrap_or("unknown");
                                let scoped_key = format!("plugin:{}:{}", plugin_id, key);
                                hive_core::secret_store::delete(&scoped_key);
                                Ok(serde_json::json!({}))
                            }
                            "host/secretHas" => {
                                let key = params["key"].as_str().unwrap_or("");
                                let plugin_id = params["pluginId"].as_str().unwrap_or("unknown");
                                let scoped_key = format!("plugin:{}:{}", plugin_id, key);
                                let exists = hive_core::secret_store::load(&scoped_key).is_some();
                                Ok(serde_json::json!({ "exists": exists }))
                            }
                            "host/storeGet" => {
                                let key = params["key"].as_str().unwrap_or("");
                                let plugin_id = params["pluginId"].as_str().unwrap_or("unknown");
                                let dir = store_base.join(plugin_id);
                                let path = dir.join(format!("{}.json", key));
                                match std::fs::read_to_string(&path) {
                                    Ok(raw) => {
                                        let val = serde_json::from_str::<serde_json::Value>(&raw)
                                            .unwrap_or(serde_json::Value::Null);
                                        Ok(serde_json::json!({ "value": val }))
                                    }
                                    Err(_) => Ok(serde_json::json!({ "value": null })),
                                }
                            }
                            "host/storeSet" => {
                                let key = params["key"].as_str().unwrap_or("");
                                let value = params.get("value").cloned().unwrap_or(serde_json::Value::Null);
                                let plugin_id = params["pluginId"].as_str().unwrap_or("unknown");
                                let dir = store_base.join(plugin_id);
                                std::fs::create_dir_all(&dir)?;
                                let path = dir.join(format!("{}.json", key));
                                std::fs::write(&path, serde_json::to_string(&value)?)?;
                                Ok(serde_json::json!({}))
                            }
                            "host/storeDelete" => {
                                let key = params["key"].as_str().unwrap_or("");
                                let plugin_id = params["pluginId"].as_str().unwrap_or("unknown");
                                let dir = store_base.join(plugin_id);
                                let path = dir.join(format!("{}.json", key));
                                let _ = std::fs::remove_file(&path);
                                Ok(serde_json::json!({}))
                            }
                            "host/storeKeys" => {
                                let plugin_id = params["pluginId"].as_str().unwrap_or("unknown");
                                let dir = store_base.join(plugin_id);
                                let keys: Vec<String> = std::fs::read_dir(&dir)
                                    .ok()
                                    .map(|entries| {
                                        entries
                                            .filter_map(|e| e.ok())
                                            .filter_map(|e| {
                                                let p = e.path();
                                                if p.extension().and_then(|x| x.to_str()) == Some("json") {
                                                    p.file_stem().map(|s| s.to_string_lossy().to_string())
                                                } else {
                                                    None
                                                }
                                            })
                                            .collect()
                                    })
                                    .unwrap_or_default();
                                Ok(serde_json::json!({ "keys": keys }))
                            }
                            "host/notify" => {
                                let plugin_id = params["pluginId"].as_str().unwrap_or("unknown");
                                let title = params["title"].as_str().unwrap_or("");
                                let body = params["body"].as_str().unwrap_or("");
                                tracing::info!(
                                    plugin_id,
                                    title,
                                    body,
                                    "Plugin notification"
                                );
                                let _ = notify_bus.publish(
                                    "plugin.notification",
                                    format!("plugin:{}", plugin_id),
                                    serde_json::json!({
                                        "pluginId": plugin_id,
                                        "title": title,
                                        "body": body,
                                    }),
                                );
                                Ok(serde_json::json!({}))
                            }
                            "host/httpFetch" => {
                                anyhow::bail!("host/httpFetch not yet implemented in host handler")
                            }
                            "host/schedule" => {
                                let id = params["id"].as_str().unwrap_or("").to_string();
                                let interval_secs = params["intervalSeconds"].as_u64().unwrap_or(60);
                                let plugin_id = params["pluginId"].as_str().unwrap_or("unknown").to_string();

                                // Convert intervalSeconds to a cron expression (minimum 1 minute granularity)
                                let minutes = std::cmp::max(1, interval_secs / 60);
                                let cron_expr = if minutes >= 60 {
                                    let hours = minutes / 60;
                                    format!("0 */{} * * *", hours)
                                } else {
                                    format!("*/{} * * * *", minutes)
                                };

                                let task_name = format!("plugin:{}:{}", plugin_id, id);
                                let request = CreateTaskRequest {
                                    name: task_name,
                                    description: Some(format!(
                                        "Scheduled by plugin '{}' (interval: {}s)",
                                        plugin_id, interval_secs
                                    )),
                                    schedule: TaskSchedule::Cron {
                                        expression: cron_expr,
                                    },
                                    action: TaskAction::EmitEvent {
                                        topic: "plugin.scheduled_task".to_string(),
                                        payload: serde_json::json!({
                                            "pluginId": plugin_id,
                                            "taskId": id,
                                        }),
                                    },
                                    owner_session_id: None,
                                    owner_agent_id: Some(plugin_id),
                                    max_retries: None,
                                    retry_delay_ms: None,
                                };

                                match scheduler.create_task(request) {
                                    Ok(task) => Ok(serde_json::json!({
                                        "taskId": task.id,
                                        "ok": true,
                                    })),
                                    Err(e) => {
                                        anyhow::bail!("Failed to schedule task: {}", e)
                                    }
                                }
                            }
                            "host/unschedule" => {
                                let id = params["id"].as_str().unwrap_or("").to_string();
                                let plugin_id = params["pluginId"].as_str().unwrap_or("unknown").to_string();
                                let task_name = format!("plugin:{}:{}", plugin_id, id);

                                // Find the task by name and cancel it
                                let tasks = scheduler.list_tasks();
                                let task = tasks.iter().find(|t| t.name == task_name);
                                match task {
                                    Some(t) => {
                                        match scheduler.cancel_task(&t.id) {
                                            Ok(_) => Ok(serde_json::json!({ "ok": true })),
                                            Err(e) => {
                                                anyhow::bail!("Failed to unschedule task: {}", e)
                                            }
                                        }
                                    }
                                    None => {
                                        // Task not found — not an error, just a no-op
                                        Ok(serde_json::json!({ "ok": true, "found": false }))
                                    }
                                }
                            }
                            _ => {
                                anyhow::bail!("Unknown host method: {}", method)
                            }
                        }
                    })
                }));

                Arc::new(host)
            },
            scheduler_tool_executor,
            python_env,
            node_env,
            shell_env,
            sandbox_config,
        })
    }

    /// Start background services (scheduler tick loop). Must be called from
    /// within a tokio runtime context.
    #[allow(clippy::await_holding_lock)]
    pub async fn start_background(&self) {
        // Start the event log background writer now that we're inside a runtime.
        if let Some(ref log) = self.event_log {
            log.start_writer();
        }
        self.scheduler.start_background_loop();

        // Wire trigger manager dependencies before starting the event loop
        // so that triggers can launch workflows and mark messages as read.
        {
            let tm = &self.trigger_manager;
            tm.set_workflow_service(Arc::clone(&self.workflows)).await;
            if let Some(ref connector_svc) = self.connectors {
                tm.set_connector_service(Arc::clone(connector_svc)).await;
            }
            if let Some(ref log) = self.event_log {
                tm.set_event_log(Arc::clone(log)).await;
            }
        }

        // Wire up workflow service extension traits now that we have a Tokio runtime.
        // This must complete before trigger replay/start so auto-launched workflows
        // have all runtime dependencies configured.
        if let Some(wiring) = self.pending_workflow_wiring.lock().take() {
            wiring.tool_executor.refresh().await;
            self.workflows.set_tool_executor(wiring.tool_executor).await;
            self.workflows.set_agent_runner(wiring.agent_runner).await;
            self.workflows.set_interaction_gate(wiring.interaction_gate).await;
            self.workflows.set_task_scheduler(wiring.task_scheduler).await;
            let event_gate_registrar: Arc<dyn hive_workflow_service::WorkflowEventGateRegistrar> =
                self.trigger_manager.clone();
            self.workflows.set_event_gate_registrar(event_gate_registrar).await;
            self.workflows.set_prompt_renderer(wiring.prompt_renderer).await;
            self.workflows.set_entity_graph(Arc::clone(&self.entity_graph));

            // Restore chat sessions into memory BEFORE recovering workflows.
            // Recovered workflows may reference a parent_session_id that must
            // already be present in the in-memory sessions map.
            if let Err(error) = self.chat.restore_sessions().await {
                tracing::error!("failed to restore chat sessions: {error}");
            }

            // Now that all deps are wired, recover orphaned workflows.
            match self.workflows.recover().await {
                Ok(0) => {}
                Ok(n) => tracing::info!("recovered {n} orphaned workflow instance(s)"),
                Err(e) => tracing::error!("workflow recovery failed: {e}"),
            }

            // Backfill entity graph with existing workflow instances.
            self.workflows.backfill_entity_graph(&self.entity_graph);

            // Re-register event gate subscriptions for instances that were
            // waiting on events when the previous process exited.
            self.trigger_manager.recover_event_gates().await;
        }

        // Register triggers for all existing workflow definitions.
        // This must complete before the event listener starts so no events
        // are evaluated against an incomplete set of triggers.
        {
            let tm = &self.trigger_manager;
            let wf = &self.workflows;

            // Seed bundled (factory-shipped) workflows before registering
            // triggers so that bundled definitions are available.
            match wf.seed_bundled_workflows().await {
                Ok(n) if n > 0 => tracing::info!("seeded/updated {n} bundled workflow(s)"),
                Err(e) => tracing::warn!(error = %e, "failed to seed bundled workflows"),
                _ => {}
            }

            match wf.list_definitions().await {
                Ok(defs) => {
                    let mut registered = 0usize;
                    let mut seen_definition_ids = std::collections::HashSet::new();
                    for summary in &defs {
                        // Register one active trigger set per immutable definition id.
                        // We always load by id so trigger selection does not depend on
                        // list ordering across versions.
                        if !seen_definition_ids.insert(summary.id.clone()) {
                            continue;
                        }

                        match wf.get_definition_by_id(&summary.id).await {
                            Ok((def, _yaml)) => {
                                if def.archived || def.triggers_paused {
                                    continue;
                                }
                                tm.register_definition(&def).await;
                                registered += 1;
                            }
                            Err(e) => {
                                tracing::warn!(
                                    definition_id = %summary.id,
                                    "failed to load definition by id for trigger registration: {e}"
                                );
                            }
                        }
                    }
                    if registered > 0 {
                        tracing::info!(
                            "registered triggers for {registered} workflow definition(s)"
                        );
                    }
                }
                Err(e) => {
                    tracing::error!("failed to list definitions for trigger registration: {e}");
                }
            }
        }

        // Now start the trigger manager event loop — all definitions are
        // already registered so no events will be missed.
        {
            let tm = Arc::clone(&self.trigger_manager);
            tokio::spawn(async move {
                tm.start().await;
            });
        }

        // Start connector background polling for inbound messages.
        if let Some(ref connector_svc) = self.connectors {
            connector_svc.start_background_poll();
        }

        // Forward plugin events into the core EventBus so they can trigger
        // workflows and appear in the SSE event stream.
        {
            let mut plugin_rx = self.plugin_host.subscribe_events();
            let bus = self.event_bus.clone();
            tokio::spawn(async move {
                loop {
                    match plugin_rx.recv().await {
                        Ok(evt) => {
                            let topic = format!("plugin.event.{}", evt.event_type);
                            let source = format!("plugin:{}", evt.plugin_id);
                            if let Err(e) = bus.publish(&topic, &source, evt.payload) {
                                tracing::warn!(
                                    error = %e,
                                    topic,
                                    "failed to publish plugin event to EventBus"
                                );
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!(skipped = n, "plugin event subscriber lagged");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
        }

        // Forward plugin status changes to the ServiceRegistry so
        // FlightDeck receives real-time status updates via SSE.
        {
            let mut plugin_status_rx = self.plugin_host.subscribe_status();
            let service_status_tx = self.service_registry.status_sender();
            tokio::spawn(async move {
                loop {
                    match plugin_status_rx.recv().await {
                        Ok(change) => {
                            let service_id = format!("plugin:{}", change.plugin_id);
                            let service_status = match change.status.state.as_str() {
                                "connected" | "syncing" => hive_contracts::ServiceStatus::Running,
                                "connecting" => hive_contracts::ServiceStatus::Starting,
                                "error" => hive_contracts::ServiceStatus::Error,
                                "disconnected" | "stopped" => hive_contracts::ServiceStatus::Stopped,
                                _ => hive_contracts::ServiceStatus::Running,
                            };
                            let error = if change.status.state == "error" {
                                change.status.message.clone()
                            } else {
                                None
                            };
                            let _ = service_status_tx.send(services::ServiceStatusEvent {
                                service_id,
                                status: service_status,
                                error,
                            });
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!(skipped = n, "plugin status subscriber lagged");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
        }

        // Restore bots and backfill entity graph in the background.
        // (Sessions were already restored synchronously above, before
        // workflow recovery.)
        {
            let chat = Arc::clone(&self.chat);
            let eg = Arc::clone(&self.entity_graph);
            tokio::spawn(async move {
                if let Err(error) = chat.restore_bots().await {
                    tracing::error!("failed to restore bots: {error}");
                }
                // Bridge bot supervisor events into their backing sessions.
                chat.spawn_bot_session_bridge();
                // Backfill entity graph with restored sessions
                chat.backfill_entity_graph(&eg).await;
            });
        }

        // Set up managed Python environment in the background.
        let python_svc: Arc<dyn hive_contracts::DaemonService> =
            Arc::new(services::PythonEnvDaemonService::new(
                Arc::clone(&self.python_env),
                Arc::clone(&self.shell_env),
            ));
        {
            let svc = Arc::clone(&python_svc);
            tokio::spawn(
                async move {
                    svc.start().await.ok();
                }
                .instrument(tracing::info_span!("service", service = "python-env")),
            );
        }

        // Set up managed Node.js environment in the background.
        let node_svc: Arc<dyn hive_contracts::DaemonService> =
            Arc::new(services::NodeEnvDaemonService::new(
                Arc::clone(&self.node_env),
                Arc::clone(&self.shell_env),
            ));
        {
            let svc = Arc::clone(&node_svc);
            tokio::spawn(
                async move {
                    svc.start().await.ok();
                }
                .instrument(tracing::info_span!("service", service = "node-env")),
            );
        }

        // Start AFK forwarding service.
        let afk_svc: Arc<dyn hive_contracts::DaemonService> =
            Arc::new(services::AfkForwarderDaemonService::new(
                Arc::clone(&self.config),
                self.user_status.clone(),
                Arc::clone(&self.chat),
                self.connectors.clone(),
                Arc::clone(&self.forwarded_interactions),
                self.event_bus.clone(),
            ));
        // Start the AFK service in the background.
        {
            let svc = Arc::clone(&afk_svc);
            tokio::spawn(async move {
                svc.start().await.ok();
            });
        }

        // Refresh MCP tool catalog in the background (no global connections — sessions connect lazily).
        let mcp = Arc::clone(&self.mcp);
        let mcp_catalog = self.mcp_catalog.clone();
        let sched_tool_exec = Arc::clone(&self.scheduler_tool_executor);
        let startup_event_bus = self.event_bus.clone();
        tokio::spawn(async move {
            let configs = mcp.server_configs().await;
            let enabled: Vec<_> = configs.iter().filter(|c| c.enabled).collect();
            tracing::info!(
                total = configs.len(),
                enabled = enabled.len(),
                "MCP catalog refresh started"
            );
            // Best-effort catalog discovery for each enabled server.
            for cfg in &enabled {
                match mcp.discover_and_catalog(&cfg.id, &mcp_catalog).await {
                    Ok(entry) => {
                        tracing::info!(
                            server_id = %cfg.id,
                            tools = entry.tools.len(),
                            resources = entry.resources.len(),
                            prompts = entry.prompts.len(),
                            "MCP catalog: server discovered"
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            server_id = %cfg.id,
                            error = %e,
                            "MCP catalog: discovery failed for server (non-fatal)"
                        );
                    }
                }
            }
            let all_tools = mcp_catalog.all_cataloged_tools().await;
            tracing::info!(cataloged_tools = all_tools.len(), "MCP tool catalog refresh complete");

            // Notify listeners that the catalog has been refreshed.
            if let Err(e) = startup_event_bus.publish(
                "mcp.catalog.refreshed",
                "hive-mcp",
                serde_json::json!({ "toolCount": all_tools.len() }),
            ) {
                tracing::debug!(error = %e, "failed to publish mcp.catalog.refreshed event");
            }

            sched_tool_exec.refresh().await;
            tracing::info!("scheduler tool executor refreshed with MCP tools");
        });

        // ── Register all services with the service registry ─────────────

        // Core services
        self.service_registry
            .register(Arc::new(services::SchedulerDaemonService::new(Arc::clone(&self.scheduler))));
        self.service_registry.register(Arc::new(services::TriggerManagerDaemonService::new(
            Arc::clone(&self.trigger_manager),
        )));
        if let Some(ref log) = self.event_log {
            self.service_registry
                .register(Arc::new(services::EventLogDaemonService::new(Arc::clone(log))));
        }
        self.service_registry.register(afk_svc);
        self.service_registry
            .register(Arc::new(services::ChatDaemonService::new(Arc::clone(&self.chat))));
        self.service_registry
            .register(Arc::new(services::WorkflowDaemonService::new(Arc::clone(&self.workflows))));

        // Agents
        self.service_registry
            .register(Arc::new(services::BotSupervisorDaemonService::new(Arc::clone(&self.chat))));

        // Connectors
        if let Some(ref connector_svc) = self.connectors {
            self.service_registry.register(Arc::new(services::ConnectorDaemonService::new(
                Arc::clone(connector_svc),
            )));
            // Register a per-connector listener service for each connector
            // with communication enabled, so they appear individually in FlightDeck.
            for (cid, cname) in connector_svc.communication_connector_ids() {
                self.service_registry.register(Arc::new(
                    services::ConnectorListenerDaemonService::new(
                        Arc::clone(connector_svc),
                        cid,
                        cname,
                    ),
                ));
            }
        }

        // MCP servers — register in the background since list_servers()
        // may need to wait for auto-connect to finish.
        {
            let mcp = Arc::clone(&self.mcp);
            let registry = Arc::clone(&self.service_registry);
            tokio::spawn(async move {
                let servers = mcp.list_servers().await;
                for srv in servers {
                    registry.register(Arc::new(services::McpServerDaemonService::new(
                        Arc::clone(&mcp),
                        srv.id.clone(),
                        srv.id.clone(),
                    )));
                }
            });
        }

        // Inference
        self.service_registry.register(Arc::new(services::InferenceDaemonService::new(
            self.runtime_manager.is_some(),
        )));

        // Python environment
        self.service_registry.register(python_svc);

        // Node.js environment
        self.service_registry.register(node_svc);

        // Plugin services — register each installed plugin so it appears in FlightDeck.
        // Also spawn and activate enabled plugins so they actually run.
        {
            let plugins = self.plugin_registry.list();
            for plugin in &plugins {
                let plugin_id = plugin.manifest.plugin_id();
                let display_name = plugin.manifest.hivemind.display_name.clone();
                let svc = services::PluginDaemonService::new(
                    plugin_id,
                    display_name,
                    Arc::clone(&self.plugin_host),
                );
                self.service_registry.register(Arc::new(svc));
            }

            // Auto-start enabled plugins
            let host = self.plugin_host.clone();
            let registry = self.plugin_registry.clone();
            tokio::spawn(async move {
                for plugin in registry.list() {
                    if !plugin.enabled {
                        continue;
                    }
                    let plugin_id = plugin.manifest.plugin_id();
                    let entry_point = &plugin.manifest.main;
                    let config = plugin.config.clone();
                    let install_path = plugin.install_path.clone();
                    let meta = plugin.manifest.hivemind.clone();
                    let has_loop = meta.permissions.iter().any(|p| p == "loop:background");

                    match host
                        .spawn(&plugin_id, &install_path, entry_point, config.clone(), Some(&meta))
                        .await
                    {
                        Ok(_) => {
                            if let Err(e) = host.activate(&plugin_id, Some(config)).await {
                                tracing::warn!(plugin_id, error = %e, "failed to activate plugin");
                                continue;
                            }
                            if has_loop {
                                if let Err(e) = host.start_loop(&plugin_id).await {
                                    tracing::warn!(plugin_id, error = %e, "failed to start plugin loop");
                                }
                            }
                            tracing::info!(plugin_id, "plugin auto-started");
                        }
                        Err(e) => {
                            tracing::warn!(plugin_id, error = %e, "failed to spawn plugin");
                        }
                    }
                }
            });
        }
    }

    /// Stop all background services so the tokio runtime can shut down cleanly.
    pub async fn shutdown(&self) {
        tracing::info!("stopping background services");
        // Stop connector polling first so IDLE tasks wake up immediately.
        if let Some(ref connectors) = self.connectors {
            connectors.stop_polling();
        }
        self.trigger_manager.stop().await;
        self.scheduler.stop().await;
        // Stop all running plugin processes.
        self.plugin_host.stop_all().await;
        // Disconnect all MCP servers (kills child processes + cancels tasks).
        // Per-session MCP managers handle their own connections.
        {
            let sessions = self.chat.list_sessions().await;
            for session in &sessions {
                if let Ok(Some(session_mcp)) = self.chat.get_session_mcp(&session.id).await {
                    session_mcp.disconnect_all().await;
                }
            }
        }
        tracing::info!("background services stopped");
    }

    pub fn with_chat(
        config: HiveMindConfig,
        audit: AuditLogger,
        event_bus: EventBus,
        shutdown: Arc<Notify>,
        chat: Arc<ChatService>,
    ) -> Self {
        let sandbox_config: Arc<parking_lot::RwLock<hive_contracts::SandboxConfig>> =
            Arc::new(parking_lot::RwLock::new(hive_contracts::SandboxConfig::default()));
        let mcp = Arc::new(McpService::from_configs(
            &collect_all_mcp_configs(&config),
            event_bus.clone(),
            Arc::clone(&sandbox_config),
        ));
        let scheduler = Arc::new(
            SchedulerService::in_memory(event_bus.clone(), SchedulerConfig::default())
                .expect("in-memory scheduler service"),
        );
        let skills_data_dir = std::env::temp_dir().join(format!(
            "hive-api-skills-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let skills_personas_dir = std::env::temp_dir().join("hive-test-agents");
        let skills = Arc::new(
            SkillsService::new(config.skills.clone(), skills_data_dir, skills_personas_dir, None)
                .expect("test skills service should initialize"),
        );
        chat.set_skills_service(Arc::clone(&skills));
        chat.set_default_permissions(config.security.default_permissions.clone());
        chat.update_personas(config.personas.clone());
        let workflows = Arc::new(
            hive_workflow_service::WorkflowService::in_memory()
                .expect("in-memory workflow service"),
        );
        let entity_graph = Arc::new(hive_core::EntityGraph::in_memory().unwrap());
        chat.set_workflow_service(Arc::clone(&workflows));
        chat.set_entity_graph(Arc::clone(&entity_graph));

        let personas_dir = std::env::temp_dir().join("hive-test-agents");
        let mcp_catalog = McpCatalogStore::with_path(std::env::temp_dir().join(
            format!("hive-test-mcp-catalog-{}.json",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()),
        ));
        let pending_workflow_wiring = {
            let tool_executor = Arc::new(workflow_deps::WorkflowToolExecutorImpl::new(
                None,
                None,
                Arc::clone(&mcp),
                mcp_catalog.clone(),
                event_bus.clone(),
            ));
            let wf_agent_runner =
                Arc::new(workflow_deps::WorkflowAgentRunnerImpl::new(personas_dir.clone()));
            wf_agent_runner.set_chat(Arc::clone(&chat));
            wf_agent_runner.set_entity_graph(Arc::clone(&entity_graph));
            let interaction_gate =
                Arc::new(workflow_deps::WorkflowInteractionGateImpl::new(event_bus.clone()));
            let task_scheduler =
                Arc::new(workflow_deps::WorkflowTaskSchedulerImpl::new(Arc::clone(&scheduler)));
            let prompt_renderer =
                Arc::new(workflow_deps::WorkflowPromptRendererImpl::new(personas_dir.clone()));
            Arc::new(parking_lot::Mutex::new(Some(WorkflowWiring {
                tool_executor,
                agent_runner: wf_agent_runner,
                interaction_gate,
                task_scheduler,
                prompt_renderer,
            })))
        };
        let scheduler_tool_executor = Arc::new(scheduler_deps::SchedulerToolExecutorImpl::new(
            None,
            None,
            Arc::clone(&mcp),
            McpCatalogStore::with_path(std::env::temp_dir().join(
                format!("hive-test-sched-catalog-{}.json",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_nanos()),
            )),
            event_bus.clone(),
        ));
        Self {
            start_time: Instant::now(),
            shutdown,
            auth_token: "test-token".to_string(),
            config: Arc::new(ArcSwap::from_pointee(config)),
            config_path: PathBuf::from("test-config.yaml"),
            personas_dir,
            hivemind_home: PathBuf::from("."),
            audit,
            trigger_manager: Arc::new(hive_workflow_service::TriggerManager::new(
                event_bus.clone(),
                Arc::clone(workflows.store()),
            )),
            event_bus,
            chat,
            skills,
            mcp,
            mcp_catalog,
            local_models: None,
            scheduler,
            workflows,
            knowledge_graph_path: Arc::new(PathBuf::from("test-kg.db")),
            entity_graph,
            runtime_manager: None,
            pending_device_codes: Arc::new(parking_lot::Mutex::new(HashMap::new())),
            pending_oauth_meta: Arc::new(parking_lot::Mutex::new(HashMap::new())),
            canvas_sessions: canvas_ws::CanvasSessionRegistry::new(),
            connectors: None,
            event_log: None,
            pending_workflow_wiring,
            user_status: UserStatusRuntime::new(),
            forwarded_interactions: Arc::new(parking_lot::Mutex::new(HashMap::new())),
            service_log_collector: hive_core::ServiceLogCollector::new(),
            service_registry: Arc::new(services::ServiceRegistry::new()),
            plugin_registry: Arc::new(hive_plugins::PluginRegistry::new(std::env::temp_dir().join("hive-test-plugins"))),
            plugin_host: Arc::new(hive_plugins::PluginHost::new(
                std::env::temp_dir().join("hive-test-plugins"),
                std::env::temp_dir().join("hive-test-plugin-data"),
            )),
            scheduler_tool_executor,
            python_env: Arc::new(hive_python_env::PythonEnvManager::new(
                PathBuf::from("."),
                hive_python_env::PythonEnvConfig { enabled: false, ..Default::default() },
            )),
            node_env: Arc::new(hive_node_env::NodeEnvManager::new(
                PathBuf::from("."),
                hive_node_env::NodeEnvConfig { enabled: false, ..Default::default() },
            )),
            shell_env: Arc::new(parking_lot::RwLock::new(HashMap::new())),
            sandbox_config,
        }
    }

    /// Replace the service log collector with one wired into the tracing stack.
    pub fn set_service_log_collector(&mut self, collector: hive_core::ServiceLogCollector) {
        self.service_log_collector = collector;
    }

    #[cfg(test)]
    pub fn with_local_models(mut self, service: LocalModelService) -> Self {
        self.local_models = Some(Arc::new(service));
        self
    }

    /// Apply new configuration at runtime without restarting.
    ///
    /// Rebuilds the model router, updates the HF token on the hub client,
    /// reconciles MCP servers, and stores the new config.
    pub async fn apply_config(&self, new_config: &HiveMindConfig) {
        // 0. Clear the per-key read cache so stale provider secrets are not
        //    re-used after a config change.
        hive_core::secret_cache::invalidate_all_cached_secrets();

        // 0b. Re-read the secret store blob from the OS keyring.
        //     Now that all secret writes go through the daemon API, this is
        //     mainly a safety measure — e.g. after legacy migration or
        //     manual keyring edits.  `invalidate_cache()` preserves the
        //     existing in-memory cache if the keyring read returns empty
        //     (e.g. macOS keychain access denied).
        hive_core::secret_store::invalidate_cache();

        // 1. Rebuild model router from the new config.
        let registry = self.local_models.as_ref().map(|lm| lm.registry());
        match chat::build_model_router_from_config(
            new_config,
            registry,
            self.runtime_manager.as_ref(),
        ) {
            Ok(new_router) => {
                self.chat.swap_router(new_router);
                tracing::info!("Model router rebuilt from updated config");
            }
            Err(e) => {
                tracing::error!("Failed to rebuild model router from new config: {e}");
            }
        }

        // 2. Update HF token on the hub client (read from secret store).
        if let Some(lm) = &self.local_models {
            let hf_token = hive_core::secret_store::load("hf_token");
            lm.update_hub_token(hf_token.as_deref());
        }

        // 3. Reconcile MCP server list (aggregated from all personas).
        //    Load personas from individual files so persona-defined MCP servers
        //    are included in the catalog (config.personas may be empty after
        //    migration to per-file storage).
        let personas = load_personas(&self.personas_dir).unwrap_or_default();
        let all_mcp_configs = collect_mcp_configs(&personas);
        self.reconcile_mcp_servers(all_mcp_configs).await;

        if let Err(error) = self.skills.update_config(new_config.skills.clone()).await {
            tracing::error!("Failed to update skills config from new config: {error}");
        }

        // 4. Store the updated config so readers see the new values.
        self.config.store(Arc::new(new_config.clone()));

        // 5. Update default permission rules from security config.
        self.chat.update_default_permissions(new_config.security.default_permissions.clone()).await;
        self.chat.update_compaction_config(new_config.compaction.clone());
        self.chat.update_web_search_config(resolve_web_search_keyring(&new_config.web_search));
        // Update sandbox configuration.
        *self.sandbox_config.write() = new_config.security.sandbox.clone();
        self.chat.update_personas(personas);

        // 6. Reload connectors from connectors.yaml.
        if let Some(connectors) = &self.connectors {
            let connectors_path = self.hivemind_home.join("connectors.yaml");
            if connectors_path.exists() {
                match std::fs::read_to_string(&connectors_path).ok().and_then(|yaml| {
                    serde_yaml::from_str::<Vec<hive_connectors::ConnectorConfig>>(&yaml).ok()
                }) {
                    Some(configs) => {
                        if let Err(e) = connectors.load_connectors(configs) {
                            tracing::warn!("failed to reload connectors: {e}");
                        } else {
                            tracing::info!("Connectors reloaded");
                            // Restart polling so new config takes effect immediately
                            // and any prior backoff state is reset.
                            connectors.start_background_poll();
                        }
                    }
                    None => {
                        tracing::warn!("failed to parse connectors.yaml during config reload");
                    }
                }
            }
        }
    }

    /// Reconcile the MCP subsystem after MCP server configs change.
    ///
    /// Updates the global MCP service, all session MCP managers, and spawns
    /// background catalog discovery. Call this after any mutation that may add,
    /// remove, or change MCP server configs (e.g. persona save/reset/copy).
    pub(crate) async fn reconcile_mcp_servers(
        &self,
        all_mcp_configs: Vec<hive_core::McpServerConfig>,
    ) {
        // Update the global MCP service with the new server list.
        self.mcp.update_servers(&all_mcp_configs).await;

        // Update all existing session MCP managers (per-persona filtering).
        self.chat.update_session_mcp_configs().await;

        // Refresh the catalog for enabled servers in the background.
        let catalog = self.mcp_catalog.clone();
        let mcp = Arc::clone(&self.mcp);
        let servers = all_mcp_configs;
        let event_bus = self.event_bus.clone();
        tokio::spawn(async move {
            for server_cfg in &servers {
                if !server_cfg.enabled {
                    continue;
                }
                match mcp.discover_and_catalog(&server_cfg.id, &catalog).await {
                    Ok(entry) => {
                        tracing::info!(
                            server_id = %server_cfg.id,
                            tools = entry.tools.len(),
                            "MCP catalog: server re-discovered after persona change"
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            server_id = %server_cfg.id,
                            error = %e,
                            "MCP catalog: discovery after persona change failed (non-fatal)"
                        );
                    }
                }
            }
            // Clean up catalog entries for removed servers.
            let keys: Vec<String> = servers.iter().map(|s| s.cache_key()).collect();
            catalog.retain_keys(&keys).await;

            // Notify listeners that the catalog has been refreshed.
            let all_tools = catalog.all_cataloged_tools().await;
            if let Err(e) = event_bus.publish(
                "mcp.catalog.refreshed",
                "hive-mcp",
                serde_json::json!({ "toolCount": all_tools.len() }),
            ) {
                tracing::debug!(error = %e, "failed to publish mcp.catalog.refreshed event");
            }
        });
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HealthResponse {
    pub ok: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StatusResponse {
    pub version: String,
    pub uptime_secs: f64,
    pub pid: u32,
    pub platform: String,
    pub bind: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ShutdownResponse {
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ValidationResponse {
    pub valid: bool,
}

// AgentSummary from hive_agents is used directly as the API response type.

// ── Flight Deck response types ───────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct SystemHealthSnapshot {
    pub version: String,
    pub uptime_secs: f64,
    pub pid: u32,
    pub platform: String,
    pub active_session_count: usize,
    pub active_agent_count: usize,
    pub active_workflow_count: usize,
    pub mcp_connected_count: usize,
    pub mcp_total_count: usize,
    pub total_llm_calls: u32,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub knowledge_node_count: i64,
    pub knowledge_edge_count: i64,
    pub local_model_count: usize,
    pub loaded_model_count: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SessionTelemetryEntry {
    pub session_id: String,
    pub title: String,
    pub state: hive_contracts::ChatRunState,
    pub telemetry: TelemetrySnapshot,
}

pub fn build_router(state: AppState) -> Router {
    use routes::*;
    Router::new()
        .route("/healthz", get(daemon::healthz))
        .route("/api/v1/daemon/status", get(daemon::status))
        .route("/api/v1/daemon/shutdown", post(daemon::shutdown))
        // Flight Deck aggregate endpoints
        .route("/api/v1/system/health", get(daemon::api_system_health))
        // ── Services dashboard ──────────────────────────────────────────
        .route("/api/v1/services", get(daemon::api_list_services))
        .route("/api/v1/services/events", get(daemon::api_services_events))
        .route("/api/v1/services/{service_id}/logs", get(daemon::api_service_logs))
        .route("/api/v1/services/{service_id}/restart", post(daemon::api_restart_service))
        .route("/api/v1/agents", get(daemon::api_all_agents))
        .route("/api/v1/chat/sessions/telemetry", get(daemon::api_all_sessions_telemetry))
        .route("/api/v1/local-models/{model_id}/load", post(daemon::api_local_model_load))
        .route("/api/v1/local-models/{model_id}/unload", post(daemon::api_local_model_unload))
        .route("/api/v1/config/get", get(config::get_config))
        .route(
            "/api/v1/config/personas",
            get(config::api_list_personas).put(config::api_save_personas),
        )
        .route("/api/v1/config/personas/copy", post(config::api_copy_persona))
        .route("/api/v1/config/personas/{id}/reset", post(config::api_reset_persona))
        .route(
            "/api/v1/config/personas/{id}/prompts/{prompt_id}/render",
            post(config::api_render_prompt_template),
        )
        .route("/api/v1/config", put(config::update_config))
        .route("/api/v1/config/validate", get(config::validate_config))
        .route("/api/v1/model/router", get(config::get_model_router))
        // ── Secrets (OS keyring) ─────────────────────────────────
        .route(
            "/api/v1/secrets/{key}",
            get(secrets::api_load_secret)
                .put(secrets::api_save_secret)
                .delete(secrets::api_delete_secret),
        )
        .route(
            "/api/v1/chat/sessions",
            get(sessions::list_chat_sessions).post(sessions::create_chat_session),
        )
        .route(
            "/api/v1/chat/sessions/{session_id}",
            get(sessions::get_chat_session)
                .delete(sessions::delete_chat_session)
                .patch(sessions::rename_chat_session),
        )
        .route("/api/v1/chat/sessions/{session_id}/messages", post(sessions::send_chat_message))
        .route("/api/v1/chat/sessions/{session_id}/persona", put(sessions::set_session_persona))
        .route("/api/v1/chat/sessions/{session_id}/upload", post(sessions::upload_file_to_session))
        .route(
            "/api/v1/chat/sessions/{session_id}/link-workspace",
            post(sessions::link_session_workspace),
        )
        .route(
            "/api/v1/chat/sessions/{session_id}/workspace/files",
            get(sessions::list_workspace_files),
        )
        .route(
            "/api/v1/chat/sessions/{session_id}/workspace/file",
            get(sessions::read_workspace_file).put(sessions::save_workspace_file),
        )
        .route(
            "/api/v1/chat/sessions/{session_id}/workspace/directory",
            post(sessions::create_workspace_directory),
        )
        .route(
            "/api/v1/chat/sessions/{session_id}/workspace/entry",
            delete(sessions::delete_workspace_entry),
        )
        .route(
            "/api/v1/chat/sessions/{session_id}/workspace/move",
            post(sessions::move_workspace_entry),
        )
        .route(
            "/api/v1/chat/sessions/{session_id}/workspace/audit",
            get(sessions::api_get_workspace_audit).post(sessions::api_audit_workspace_file),
        )
        .route(
            "/api/v1/chat/sessions/{session_id}/workspace/classification",
            get(sessions::api_get_classification).put(sessions::api_set_classification_default),
        )
        .route(
            "/api/v1/chat/sessions/{session_id}/workspace/classification/override",
            put(sessions::api_set_classification_override)
                .delete(sessions::api_clear_classification_override),
        )
        .route(
            "/api/v1/chat/sessions/{session_id}/workspace/index-status/stream",
            get(sessions::api_workspace_index_status_stream),
        )
        .route(
            "/api/v1/chat/sessions/{session_id}/workspace/index-status",
            get(sessions::api_workspace_indexed_files),
        )
        .route(
            "/api/v1/chat/sessions/{session_id}/workspace/reindex",
            post(sessions::api_workspace_reindex_file),
        )
        .route(
            "/api/v1/chat/sessions/{session_id}/tool-approval",
            post(sessions::api_chat_tool_approval),
        )
        .route("/api/v1/chat/sessions/{session_id}/recluster", post(sessions::api_recluster_canvas))
        .route(
            "/api/v1/chat/sessions/{session_id}/propose-layout",
            post(sessions::api_propose_layout),
        )
        .route("/api/v1/skills/discover", post(skills::api_discover_skills))
        .route(
            "/api/v1/skills/sources",
            get(skills::api_get_skill_sources).put(skills::api_set_skill_sources),
        )
        .route("/api/v1/skills/rebuild-index", post(skills::api_rebuild_skill_index))
        .route("/api/v1/skills/{name}/audit", post(skills::api_audit_skill))
        .route("/api/v1/skills/{name}/audit/stream", post(skills::api_audit_skill_stream))
        // Per-persona skill routes (persona_id is percent-encoded, e.g. system%2Fgeneral)
        .route("/api/v1/personas/{persona_id}/skills", get(skills::api_list_persona_skills))
        .route(
            "/api/v1/personas/{persona_id}/skills/{name}/install",
            post(skills::api_install_persona_skill),
        )
        .route(
            "/api/v1/personas/{persona_id}/skills/{name}",
            delete(skills::api_uninstall_persona_skill),
        )
        .route(
            "/api/v1/personas/{persona_id}/skills/{name}/enabled",
            put(skills::api_set_persona_skill_enabled),
        )
        .route(
            "/api/v1/chat/sessions/{session_id}/interaction",
            post(sessions::api_chat_interaction_response),
        )
        .route(
            "/api/v1/chat/sessions/{session_id}/permissions",
            get(sessions::get_session_permissions).put(sessions::update_session_permissions),
        )
        .route("/api/v1/chat/sessions/{session_id}/stream", get(sessions::api_chat_stream))
        .route("/api/v1/chat/sessions/{session_id}/agents", get(agents::api_list_session_agents))
        .route(
            "/api/v1/chat/sessions/{session_id}/agents/stream",
            get(agents::api_agent_stage_stream),
        )
        .route(
            "/api/v1/chat/sessions/{session_id}/agents/{agent_id}/pause",
            post(agents::api_pause_agent),
        )
        .route(
            "/api/v1/chat/sessions/{session_id}/agents/{agent_id}/resume",
            post(agents::api_resume_agent),
        )
        .route(
            "/api/v1/chat/sessions/{session_id}/agents/{agent_id}/kill",
            post(agents::api_kill_agent),
        )
        .route(
            "/api/v1/chat/sessions/{session_id}/agents/{agent_id}/restart",
            post(agents::api_restart_agent),
        )
        .route(
            "/api/v1/chat/sessions/{session_id}/agents/{agent_id}/events",
            get(agents::api_agent_events),
        )
        .route("/api/v1/chat/sessions/{session_id}/events", get(agents::api_session_events))
        // Background process management
        .route("/api/v1/processes", get(processes::api_list_processes))
        .route("/api/v1/processes/{process_id}/status", get(processes::api_process_status))
        .route("/api/v1/processes/{process_id}/kill", post(processes::api_kill_process))
        .route(
            "/api/v1/chat/sessions/{session_id}/processes",
            get(processes::api_list_session_processes),
        )
        .route(
            "/api/v1/chat/sessions/{session_id}/processes/events",
            get(processes::api_process_event_stream),
        )
        .route(
            "/api/v1/chat/sessions/{session_id}/agents/{agent_id}/interaction",
            post(agents::api_agent_interaction_response),
        )
        .route(
            "/api/v1/chat/sessions/{session_id}/agents/telemetry",
            get(agents::api_agent_telemetry),
        )
        .route("/api/v1/pending-approvals", get(sessions::api_pending_approvals))
        .route("/api/v1/pending-questions", get(sessions::api_all_pending_questions))
        .route(
            "/api/v1/chat/sessions/{session_id}/pending-questions",
            get(sessions::api_pending_questions),
        )
        .route("/api/v1/approval-events", get(sessions::api_approval_event_stream))
        .route(
            "/api/v1/status",
            get(sessions::api_get_user_status).put(sessions::api_set_user_status),
        )
        .route("/api/v1/status/heartbeat", post(sessions::api_status_heartbeat))
        .route("/api/v1/status/events", get(sessions::api_status_event_stream))
        .route(
            "/api/v1/chat/sessions/{session_id}/interrupt",
            post(sessions::interrupt_chat_session),
        )
        .route("/api/v1/chat/sessions/{session_id}/resume", post(sessions::resume_chat_session))
        .route("/api/v1/chat/sessions/{session_id}/memory", get(sessions::get_chat_session_memory))
        .route("/api/v1/chat/sessions/{session_id}/risk-scans", get(sessions::list_risk_scans))
        .route("/api/v1/memory/search", get(sessions::search_memory))
        .route("/api/v1/mcp/servers", get(mcp::list_mcp_servers))
        .route("/api/v1/mcp/servers/{server_id}/connect", post(mcp::connect_mcp_server))
        .route("/api/v1/mcp/servers/{server_id}/disconnect", post(mcp::disconnect_mcp_server))
        .route("/api/v1/mcp/servers/{server_id}/tools", get(mcp::list_mcp_tools))
        .route("/api/v1/mcp/servers/{server_id}/resources", get(mcp::list_mcp_resources))
        .route("/api/v1/mcp/servers/{server_id}/prompts", get(mcp::list_mcp_prompts))
        .route("/api/v1/mcp/servers/{server_id}/logs", get(mcp::get_mcp_server_logs))
        .route("/api/v1/mcp/notifications", get(mcp::list_mcp_notifications))
        .route("/api/v1/mcp/events", get(mcp::api_mcp_event_stream))
        .route("/api/v1/mcp/catalog", get(mcp::get_mcp_catalog))
        .route("/api/v1/mcp/catalog/refresh", post(mcp::refresh_mcp_catalog_all))
        .route("/api/v1/mcp/test-connect", post(mcp::test_connect_mcp))
        .route("/api/v1/mcp/servers/{server_id}/install-runtime", post(mcp::install_mcp_runtime))
        .route("/api/v1/mcp/catalog/{server_id}/refresh", post(mcp::refresh_mcp_catalog_server))
        // Session-scoped MCP endpoints
        .route("/api/v1/sessions/{session_id}/mcp/servers", get(mcp::list_session_mcp_servers))
        .route(
            "/api/v1/sessions/{session_id}/mcp/servers/{server_id}/connect",
            post(mcp::connect_session_mcp_server),
        )
        .route(
            "/api/v1/sessions/{session_id}/mcp/servers/{server_id}/disconnect",
            post(mcp::disconnect_session_mcp_server),
        )
        .route(
            "/api/v1/sessions/{session_id}/mcp/servers/{server_id}/logs",
            get(mcp::get_session_mcp_server_logs),
        )
        .route("/api/v1/tools", get(tools::list_tools))
        .route("/api/v1/tools/{tool_id}/invoke", post(tools::invoke_tool))
        // ── Plugins ─────────────────────────────────────────────────
        .route("/api/v1/plugins", get(plugins::api_list_plugins))
        .route("/api/v1/plugins/link", post(plugins::api_link_local))
        .route("/api/v1/plugins/install", post(plugins::api_install_npm))
        .route("/api/v1/plugins/{plugin_id}/config-schema", get(plugins::api_get_config_schema))
        .route("/api/v1/plugins/{plugin_id}/config", post(plugins::api_save_config))
        .route("/api/v1/plugins/{plugin_id}/enabled", post(plugins::api_set_enabled))
        .route("/api/v1/plugins/{plugin_id}", delete(plugins::api_uninstall))
        .route("/api/v1/local-models", get(local_models::api_list_local_models))
        .route("/api/v1/local-models/install", post(local_models::api_install_local_model))
        .route("/api/v1/local-models/downloads", get(local_models::api_list_downloads))
        .route(
            "/api/v1/local-models/downloads/{model_id}",
            delete(local_models::api_remove_download),
        )
        .route("/api/v1/local-models/search", get(local_models::api_search_hub_models))
        .route("/api/v1/local-models/hardware", get(local_models::api_get_hardware))
        .route(
            "/api/v1/local-models/hub/{repo_id}/files",
            get(local_models::api_list_hub_repo_files),
        )
        .route(
            "/api/v1/local-models/{model_id}",
            get(local_models::api_get_local_model).delete(local_models::api_remove_local_model),
        )
        .route("/api/v1/local-models/{model_id}/params", put(local_models::api_update_model_params))
        .route(
            "/api/v1/scheduler/tasks",
            get(scheduler::list_scheduler_tasks).post(scheduler::create_scheduler_task),
        )
        .route(
            "/api/v1/scheduler/tasks/{task_id}",
            get(scheduler::get_scheduler_task)
                .put(scheduler::update_scheduler_task)
                .delete(scheduler::delete_scheduler_task),
        )
        .route("/api/v1/scheduler/tasks/{task_id}/cancel", post(scheduler::cancel_scheduler_task))
        .route("/api/v1/scheduler/tasks/{task_id}/runs", get(scheduler::list_scheduler_task_runs))
        .route("/api/v1/scheduler/events", get(scheduler::api_scheduler_event_stream))
        .route(
            "/api/v1/knowledge/nodes",
            get(knowledge::kg_list_nodes).post(knowledge::kg_create_node),
        )
        .route(
            "/api/v1/knowledge/nodes/{node_id}",
            get(knowledge::kg_get_node)
                .put(knowledge::kg_update_node)
                .delete(knowledge::kg_delete_node),
        )
        .route("/api/v1/knowledge/nodes/{node_id}/edges", get(knowledge::kg_get_node_edges))
        .route("/api/v1/knowledge/nodes/{node_id}/neighbors", get(knowledge::kg_get_neighbors))
        .route("/api/v1/knowledge/edges", post(knowledge::kg_create_edge))
        .route("/api/v1/knowledge/edges/{edge_id}", delete(knowledge::kg_delete_edge))
        .route("/api/v1/knowledge/search", get(knowledge::kg_search))
        .route("/api/v1/knowledge/search/vector", get(knowledge::kg_vector_search))
        .route("/api/v1/knowledge/search/workspace", get(knowledge::workspace_search))
        .route(
            "/api/v1/knowledge/search/workspace/semantic",
            get(knowledge::workspace_semantic_search),
        )
        .route("/api/v1/knowledge/embedding-models", get(knowledge::kg_list_embedding_models))
        .route("/api/v1/knowledge/stats", get(knowledge::kg_stats))
        // Provider auth (GitHub device flow)
        .route("/api/v1/auth/github/device-code", post(auth::github_start_device_flow))
        .route("/api/v1/auth/github/poll", post(auth::github_poll_token))
        .route("/api/v1/auth/github/save-token", post(auth::github_save_token))
        .route("/api/v1/auth/github/status", get(auth::github_auth_status))
        .route("/api/v1/auth/github/models", get(auth::github_list_models))
        .route("/api/v1/auth/github/disconnect", post(auth::github_disconnect))
        // Bots
        .route("/api/v1/bots", get(bots::api_list_bots).post(bots::api_launch_bot))
        .route("/api/v1/bots/launch-with-prompt", post(bots::api_launch_bot_with_prompt))
        .route("/api/v1/bots/stream", get(bots::api_bots_stream))
        .route("/api/v1/bots/telemetry", get(bots::api_bot_telemetry))
        .route("/api/v1/bots/{agent_id}/message", post(bots::api_message_bot))
        .route("/api/v1/bots/{agent_id}/send-prompt", post(bots::api_send_prompt_to_bot))
        .route("/api/v1/bots/{agent_id}/deactivate", post(bots::api_deactivate_bot))
        .route("/api/v1/bots/{agent_id}/activate", post(bots::api_activate_bot))
        .route("/api/v1/bots/{agent_id}", delete(bots::api_delete_bot))
        .route("/api/v1/bots/{agent_id}/events", get(bots::api_bot_events))
        .route("/api/v1/bots/{agent_id}/interaction", post(bots::api_bot_interaction))
        .route(
            "/api/v1/bots/{agent_id}/permissions",
            get(bots::api_get_bot_permissions).put(bots::api_set_bot_permissions),
        )
        .route("/api/v1/bots/{agent_id}/workspace/files", get(bots::api_bot_workspace_files))
        .route("/api/v1/bots/{agent_id}/workspace/file", get(bots::api_bot_workspace_file))
        // Canvas WebSocket for real-time spatial sync
        .route("/api/v1/canvas/{session_id}/ws", get(canvas_ws::canvas_ws_handler))
        // Connectors (formerly communication channels)
        .route(
            "/api/v1/config/connectors",
            get(connectors::api_list_channels).put(connectors::api_save_channels),
        )
        .route("/api/v1/config/connectors/{channel_id}/test", post(connectors::api_test_channel))
        .route(
            "/api/v1/config/connectors/{channel_id}/discover",
            post(connectors::api_channel_discover),
        )
        .route(
            "/api/v1/config/connectors/{channel_id}/channels",
            get(connectors::api_connector_list_channels),
        )
        .route(
            "/api/v1/config/connectors/{channel_id}/oauth/start",
            post(connectors::api_channel_oauth_start),
        )
        .route(
            "/api/v1/config/connectors/{channel_id}/oauth/poll",
            post(connectors::api_channel_oauth_poll),
        )
        .route("/api/v1/comms/send", post(connectors::api_comm_send))
        .route("/api/v1/comms/read", get(connectors::api_comm_read))
        .route("/api/v1/comms/audit", get(connectors::api_comm_audit))
        // Workflows
        .route(
            "/api/v1/workflows/definitions",
            get(workflows::wf_list_definitions).post(workflows::wf_save_definition),
        )
        .route("/api/v1/workflows/definitions/copy", post(workflows::wf_copy_definition))
        .route("/api/v1/workflows/definitions/{name}", get(workflows::wf_get_latest_definition))
        .route("/api/v1/workflows/definitions/{name}/reset", post(workflows::wf_reset_definition))
        .route(
            "/api/v1/workflows/definitions/{name}/{version}/archive",
            post(workflows::wf_archive_definition),
        )
        .route(
            "/api/v1/workflows/definitions/{name}/{version}/triggers-paused",
            post(workflows::wf_set_triggers_paused),
        )
        .route("/api/v1/workflows/definitions/by-id/{id}", get(workflows::wf_get_definition_by_id))
        .route(
            "/api/v1/workflows/definitions/{name}/{version}",
            get(workflows::wf_get_definition).delete(workflows::wf_delete_definition),
        )
        .route(
            "/api/v1/workflows/definitions/{name}/{version}/dependents",
            get(workflows::wf_check_definition_dependents),
        )
        .route(
            "/api/v1/workflows/instances",
            get(workflows::wf_list_instances).post(workflows::wf_launch_instance),
        )
        .route(
            "/api/v1/workflows/instances/{instance_id}",
            get(workflows::wf_get_instance).delete(workflows::wf_delete_instance),
        )
        .route(
            "/api/v1/workflows/instances/{instance_id}/pause",
            post(workflows::wf_pause_instance),
        )
        .route(
            "/api/v1/workflows/instances/{instance_id}/resume",
            post(workflows::wf_resume_instance),
        )
        .route("/api/v1/workflows/instances/{instance_id}/kill", post(workflows::wf_kill_instance))
        .route(
            "/api/v1/workflows/instances/{instance_id}/archive",
            post(workflows::wf_archive_instance),
        )
        .route(
            "/api/v1/workflows/instances/{instance_id}/permissions",
            put(workflows::wf_update_permissions),
        )
        .route(
            "/api/v1/workflows/instances/{instance_id}/steps/{step_id}/respond",
            post(workflows::wf_respond_to_gate),
        )
        .route("/api/v1/workflows/events", get(workflows::wf_event_stream))
        .route("/api/v1/workflows/topics", get(workflows::wf_list_topics))
        .route("/api/v1/workflows/triggers/active", get(workflows::wf_list_active_triggers))
        .route("/api/v1/workflows/ai-assist", post(workflows::wf_ai_assist))
        .route(
            "/api/v1/workflows/attachments/{workflow_id}/{version}",
            get(workflows::wf_list_attachments).post(workflows::wf_upload_attachment),
        )
        .route(
            "/api/v1/workflows/attachments/{workflow_id}/{version}/{attachment_id}",
            delete(workflows::wf_delete_attachment),
        )
        .route(
            "/api/v1/workflows/attachments/{workflow_id}/{from_version}/copy/{to_version}",
            post(workflows::wf_copy_attachments),
        )
        // Agent Kits (export/import)
        .route("/api/v1/agent-kits/export", post(agent_kits::api_export_agent_kit))
        .route("/api/v1/agent-kits/preview", post(agent_kits::api_preview_agent_kit))
        .route("/api/v1/agent-kits/import", post(agent_kits::api_import_agent_kit))
        // Entity ownership graph
        .route("/api/v1/entity-graph", get(entity_graph::api_list_entities))
        .route("/api/v1/entity-graph/{entity_type}/{entity_id}", get(entity_graph::api_get_entity))
        .route(
            "/api/v1/entity-graph/{entity_type}/{entity_id}/children",
            get(entity_graph::api_entity_children),
        )
        .route(
            "/api/v1/entity-graph/{entity_type}/{entity_id}/ancestors",
            get(entity_graph::api_entity_ancestors),
        )
        .route(
            "/api/v1/entity-graph/{entity_type}/{entity_id}/descendants",
            get(entity_graph::api_entity_descendants),
        )
        // Unified pending interactions
        .route("/api/v1/pending-interactions", get(interactions::api_all_pending_interactions))
        .route(
            "/api/v1/pending-interaction-counts",
            get(interactions::api_pending_interaction_counts),
        )
        .route("/api/v1/interactions/stream", get(interactions::api_interactions_stream))
        // Python managed environment
        .route("/api/v1/python/status", get(python::python_status))
        .route("/api/v1/python/status/stream", get(python::python_status_stream))
        .route("/api/v1/python/reinstall", post(python::python_reinstall))
        // Node.js managed environment
        .route("/api/v1/node/status", get(node::node_status))
        .route("/api/v1/node/status/stream", get(node::node_status_stream))
        .route("/api/v1/node/reinstall", post(node::node_reinstall))
        // Event log & recordings
        .route("/api/v1/events", get(events::api_query_events).delete(events::api_prune_events))
        .route("/api/v1/events/publish", post(events::api_publish_event))
        .route("/api/v1/events/stream", get(events::api_event_bus_stream))
        .route(
            "/api/v1/events/recordings",
            get(events::api_list_recordings).post(events::api_start_recording),
        )
        .route(
            "/api/v1/events/recordings/{recording_id}",
            get(events::api_get_recording).delete(events::api_delete_recording),
        )
        .route("/api/v1/events/recordings/{recording_id}/stop", post(events::api_stop_recording))
        .route("/api/v1/events/recordings/{recording_id}/export", get(events::api_export_recording))
        .layer(CatchPanicLayer::new())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth_middleware::require_daemon_token,
        ))
        .layer(
            CorsLayer::new()
                .allow_origin(tower_http::cors::AllowOrigin::predicate(
                    |origin: &axum::http::HeaderValue,
                     _request_parts: &axum::http::request::Parts| {
                        let Ok(origin) = origin.to_str() else {
                            return false;
                        };
                        // Allow Tauri webview origins and localhost dev servers.
                        origin.starts_with("tauri://")
                            || origin.starts_with("http://localhost")
                            || origin.starts_with("http://127.0.0.1")
                            || origin.starts_with("https://tauri.localhost")
                    },
                ))
                .allow_methods(tower_http::cors::Any)
                .allow_headers(tower_http::cors::Any),
        )
        .with_state(state)
}

//  Error converters (used by route modules)

pub(crate) fn chat_error(error: ChatServiceError) -> (StatusCode, String) {
    match error {
        ChatServiceError::SessionNotFound { .. } | ChatServiceError::AgentNotFound { .. } => {
            (StatusCode::NOT_FOUND, error.to_string())
        }
        ChatServiceError::BadRequest { .. } => (StatusCode::BAD_REQUEST, error.to_string()),
        ChatServiceError::KnowledgeGraphFailed { .. }
        | ChatServiceError::RiskScanFailed { .. }
        | ChatServiceError::Internal { .. } => {
            (StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
        }
    }
}

pub(crate) fn mcp_error(error: McpServiceError) -> (StatusCode, String) {
    match error {
        McpServiceError::ServerNotFound { .. } => (StatusCode::NOT_FOUND, error.to_string()),
        McpServiceError::Disabled { .. } => (StatusCode::FORBIDDEN, error.to_string()),
        McpServiceError::NotConnected { .. } | McpServiceError::Connecting { .. } => {
            (StatusCode::SERVICE_UNAVAILABLE, error.to_string())
        }
        McpServiceError::ConnectionFailed { .. }
        | McpServiceError::RequestFailed { .. }
        | McpServiceError::ProtocolError { .. } => (StatusCode::BAD_GATEWAY, error.to_string()),
        McpServiceError::RequestTimeout { .. } => (StatusCode::GATEWAY_TIMEOUT, error.to_string()),
        McpServiceError::RuntimeNotInstalled { .. } => {
            (StatusCode::FAILED_DEPENDENCY, error.to_string())
        }
    }
}

pub(crate) fn tool_error(error: ToolInvocationError) -> (StatusCode, String) {
    match error {
        ToolInvocationError::ToolUnavailable { .. } => (StatusCode::NOT_FOUND, error.to_string()),
        ToolInvocationError::ToolDenied { .. } => (StatusCode::FORBIDDEN, error.to_string()),
        ToolInvocationError::ToolApprovalRequired { .. } => {
            (StatusCode::UNPROCESSABLE_ENTITY, error.to_string())
        }
        ToolInvocationError::ToolExecutionFailed { .. } => {
            (StatusCode::BAD_GATEWAY, error.to_string())
        }
    }
}

pub(crate) fn skills_error(error: SkillsServiceError) -> (StatusCode, String) {
    match error {
        SkillsServiceError::SourceNotFound(_) => (StatusCode::NOT_FOUND, error.to_string()),
        SkillsServiceError::InvalidPath(_) | SkillsServiceError::Parse(_) => {
            (StatusCode::BAD_REQUEST, error.to_string())
        }
        SkillsServiceError::Audit(_) => (StatusCode::BAD_GATEWAY, error.to_string()),
        SkillsServiceError::Config(_) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
        SkillsServiceError::Index(_)
        | SkillsServiceError::Source(_)
        | SkillsServiceError::LocalSource(_)
        | SkillsServiceError::Io { .. } => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

pub(crate) fn scheduler_error(error: SchedulerError) -> (StatusCode, String) {
    match error {
        SchedulerError::TaskNotFound { .. } => (StatusCode::NOT_FOUND, error.to_string()),
        SchedulerError::Database(_) | SchedulerError::Internal(_) => {
            (StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
        }
    }
}

pub(crate) fn kg_error(e: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

pub(crate) fn open_kg(path: &PathBuf) -> Result<KnowledgeGraph, (StatusCode, String)> {
    KnowledgeGraph::open(path).map_err(kg_error)
}

pub(crate) fn clamp_limit(limit: Option<usize>, default_limit: usize) -> usize {
    limit.unwrap_or(default_limit).clamp(1, 50)
}

pub(crate) fn workflow_error(e: hive_workflow::WorkflowError) -> (StatusCode, String) {
    let status = match &e {
        hive_workflow::WorkflowError::DefinitionNotFound { .. }
        | hive_workflow::WorkflowError::InstanceNotFound { .. }
        | hive_workflow::WorkflowError::StepNotFound { .. } => StatusCode::NOT_FOUND,
        hive_workflow::WorkflowError::InvalidDefinition { .. }
        | hive_workflow::WorkflowError::InvalidState { .. }
        | hive_workflow::WorkflowError::Expression(_)
        | hive_workflow::WorkflowError::ValidationError { .. } => StatusCode::BAD_REQUEST,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (status, e.to_string())
}

/// Resolve `keyring:KEY` references in a [`WebSearchConfig`] by reading the
/// secret from the OS keyring and replacing the reference with the literal value.
fn resolve_web_search_keyring(config: &WebSearchConfig) -> WebSearchConfig {
    let mut resolved = config.clone();
    if let Some(ref key) = resolved.api_key {
        if let Some(keyring_key) = key.strip_prefix("keyring:") {
            resolved.api_key = hive_core::secret_store::load(keyring_key);
        }
    }
    resolved
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use hive_agents::{AgentRole, AgentSpec, AgentSummary};
    use hive_classification::DataClass;
    use hive_contracts::{
        config::Persona, FileAuditRecord, FileAuditStatus, WorkspaceClassification,
    };
    use hive_core::{
        validate_config as validate_hivemind_config, CapabilityConfig, ModelProviderConfig,
        ModelsConfig, ProviderAuthConfig, ProviderKindConfig, ProviderOptionsConfig,
    };
    use hive_tools::{ToolDefinition, ToolResult};
    use serde::de::DeserializeOwned;
    use serde_json::json;
    use tempfile::tempdir_in;
    use tower::util::ServiceExt;

    use crate::routes::sessions::AuditStatusResponse;

    fn unique_temp_path(prefix: &str, extension: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "{prefix}-{}-{}.{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos(),
            extension
        ))
    }

    /// Build a test config that includes a mock provider so chat tests can
    /// exercise the full message-processing pipeline without real LLM calls.
    fn test_config() -> HiveMindConfig {
        let mut cfg = HiveMindConfig::default();
        // Insert a mock provider for tests that need to process chat messages.
        cfg.models.providers.insert(
            0,
            ModelProviderConfig {
                id: "test-mock".to_string(),
                name: Some("Test Mock".to_string()),
                kind: ProviderKindConfig::Mock,
                base_url: None,
                auth: ProviderAuthConfig::None,
                models: vec!["chat-fast".to_string()],
                capabilities: std::collections::BTreeSet::new(),
                model_capabilities: std::collections::BTreeMap::from([(
                    "chat-fast".to_string(),
                    std::collections::BTreeSet::from([CapabilityConfig::Chat]),
                )]),
                channel_class: hive_classification::ChannelClass::Internal,
                priority: 100,
                enabled: true,
                options: ProviderOptionsConfig {
                    response_prefix: Some("HiveMind OS test route".to_string()),
                    ..ProviderOptionsConfig::default()
                },
            },
        );
        cfg
    }

    fn app_state() -> AppState {
        let audit_path = unique_temp_path("hive-api-test", "log");
        let cfg = test_config();

        let audit = AuditLogger::new(audit_path).expect("audit logger");
        let bus = EventBus::new(32);
        let chat = Arc::new(ChatService::new(
            audit.clone(),
            bus.clone(),
            ChatRuntimeConfig {
                step_delay: std::time::Duration::from_millis(10),
                ..ChatRuntimeConfig::default()
            },
            unique_temp_path("hive-api-home", "dir"),
            unique_temp_path("hive-api-memory", "db"),
            cfg.security.prompt_injection.clone(),
            unique_temp_path("hive-api-risk", "db"),
            canvas_ws::CanvasSessionRegistry::new(),
        ));
        // Rebuild router from test config so our test-mock provider is active.
        let router = chat::build_model_router_from_config(&cfg, None, None)
            .expect("test config should build a valid model router");
        chat.swap_router(router);

        AppState::with_chat(cfg, audit, bus, Arc::new(Notify::new()), chat)
    }

    async fn read_json<T: DeserializeOwned>(response: axum::response::Response) -> T {
        let body = to_bytes(response.into_body(), usize::MAX).await.expect("response body");
        serde_json::from_slice(&body).expect("valid json response")
    }

    fn authed_request(method: &str, uri: &str) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header("Authorization", "Bearer test-token")
            .body(Body::empty())
            .unwrap()
    }

    fn authed_json_request(method: &str, uri: &str, body: impl Into<Body>) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header("Authorization", "Bearer test-token")
            .header("content-type", "application/json")
            .body(body.into())
            .unwrap()
    }

    async fn post_json(
        app: &Router,
        uri: String,
        body: serde_json::Value,
    ) -> axum::response::Response {
        app.clone()
            .oneshot(authed_json_request("POST", &uri, Body::from(body.to_string())))
            .await
            .expect("post response")
    }

    async fn put_json(
        app: &Router,
        uri: String,
        body: serde_json::Value,
    ) -> axum::response::Response {
        app.clone()
            .oneshot(authed_json_request("PUT", &uri, Body::from(body.to_string())))
            .await
            .expect("put response")
    }

    async fn delete_uri(app: &Router, uri: String) -> axum::response::Response {
        app.clone().oneshot(authed_request("DELETE", &uri)).await.expect("delete response")
    }

    #[tokio::test]
    async fn healthz_returns_ok() {
        let app = build_router(app_state());
        let response = app
            .oneshot(Request::builder().uri("/healthz").body(Body::empty()).unwrap())
            .await
            .expect("healthz response");

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn shutdown_endpoint_accepts_request() {
        let app = build_router(app_state());
        let response = app
            .oneshot(authed_request("POST", "/api/v1/daemon/shutdown"))
            .await
            .expect("shutdown response");

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn session_agent_endpoints_list_and_report_telemetry() {
        let state = app_state();
        let session = state
            .chat
            .create_session(SessionModality::Linear, Some("Agents".to_string()), None)
            .await
            .expect("create session");
        let app = build_router(state.clone());

        let response = app
            .clone()
            .oneshot(authed_request("GET", &format!("/api/v1/chat/sessions/{}/agents", session.id)))
            .await
            .expect("list agents response");
        assert_eq!(response.status(), StatusCode::OK);
        let agents: Vec<AgentSummary> = read_json(response).await;
        assert!(agents.is_empty());

        let response = app
            .clone()
            .oneshot(authed_request(
                "GET",
                &format!("/api/v1/chat/sessions/{}/agents/telemetry", session.id),
            ))
            .await
            .expect("telemetry response");
        assert_eq!(response.status(), StatusCode::OK);
        let telemetry: TelemetrySnapshot = read_json(response).await;
        assert_eq!(telemetry.total.model_calls, 0);
        assert_eq!(telemetry.total.tool_calls, 0);
    }

    #[tokio::test]
    async fn session_agent_control_endpoints_return_not_found_for_missing_agent() {
        let state = app_state();
        let session = state
            .chat
            .create_session(SessionModality::Linear, Some("Agents".to_string()), None)
            .await
            .expect("create session");
        let app = build_router(state.clone());

        for action in ["pause", "resume", "kill"] {
            let response = app
                .clone()
                .oneshot(authed_request(
                    "POST",
                    &format!("/api/v1/chat/sessions/{}/agents/missing/{action}", session.id),
                ))
                .await
                .expect("agent control response");
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
        }
    }

    #[tokio::test]
    async fn session_agent_list_endpoint_returns_spawned_agents() {
        let state = app_state();
        let session = state
            .chat
            .create_session(SessionModality::Linear, Some("Agents".to_string()), None)
            .await
            .expect("create session");
        let supervisor =
            state.chat.get_or_create_supervisor(&session.id).await.expect("get supervisor");
        supervisor
            .spawn_agent(
                AgentSpec {
                    id: "planner".to_string(),
                    name: "Planner".to_string(),
                    friendly_name: "eager_turing".to_string(),
                    description: "Plans the work".to_string(),
                    role: AgentRole::Planner,
                    model: None,
                    preferred_models: None,
                    loop_strategy: None,
                    tool_execution_mode: None,
                    system_prompt: "Plan the work".to_string(),
                    allowed_tools: Vec::new(),
                    avatar: None,
                    color: None,
                    data_class: hive_classification::DataClass::Public,
                    keep_alive: false,
                    idle_timeout_secs: None,
                    tool_limits: None,
                    persona_id: None,
                    workflow_managed: false,
                },
                None,
                None,
                None,
                None,
            )
            .await
            .expect("spawn agent");

        let app = build_router(state.clone());
        let response = app
            .oneshot(authed_request("GET", &format!("/api/v1/chat/sessions/{}/agents", session.id)))
            .await
            .expect("list spawned agents response");
        assert_eq!(response.status(), StatusCode::OK);
        let agents: Vec<AgentSummary> = read_json(response).await;
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].agent_id, "planner");
        assert_eq!(agents[0].spec.id, "planner");
    }

    #[tokio::test]
    async fn config_personas_endpoint_returns_default_persona() {
        let app = build_router(app_state());
        let response = app
            .oneshot(authed_request("GET", "/api/v1/config/personas"))
            .await
            .expect("personas response");

        assert_eq!(response.status(), StatusCode::OK);
        let personas: Vec<Persona> = read_json(response).await;
        assert_eq!(personas, vec![Persona::default_persona()]);
    }

    #[tokio::test]
    async fn config_personas_endpoint_saves_personas() {
        let mut state = app_state();
        let personas_dir = unique_temp_path("hive-api-agents", "d");
        std::fs::create_dir_all(&personas_dir).expect("create temp agents dir");
        state.personas_dir = personas_dir.clone();
        let app = build_router(state);

        let response = put_json(
            &app,
            "/api/v1/config/personas".to_string(),
            json!([
                {
                    "id": "user/planner",
                    "name": "Planner",
                    "description": "Plans tasks before execution.",
                    "system_prompt": "Think before acting.",
                    "loop_strategy": "plan_then_execute",
                    "preferred_model": "gpt-5",
                    "allowed_tools": ["filesystem.read"],
                    "avatar": "🗺️",
                    "color": "#94e2d5"
                }
            ]),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        // Verify the file was written
        let persona_dir = personas_dir.join("user").join("planner");
        let persona_file = persona_dir.join("persona.yaml");
        assert!(persona_file.exists(), "persona file should exist");
        let contents = std::fs::read_to_string(&persona_file).expect("read persona file");
        assert!(contents.contains("planner"));

        let response = app
            .oneshot(authed_request("GET", "/api/v1/config/personas"))
            .await
            .expect("personas response");
        assert_eq!(response.status(), StatusCode::OK);
        let personas: Vec<Persona> = read_json(response).await;
        assert_eq!(personas.len(), 2);
        assert_eq!(personas[0], Persona::default_persona());
        assert_eq!(personas[1].id, "user/planner");

        let _ = std::fs::remove_dir_all(&personas_dir);
    }

    #[tokio::test]
    async fn chat_session_processes_message() {
        let app = build_router(app_state());
        let response = app
            .clone()
            .oneshot(authed_request("POST", "/api/v1/chat/sessions"))
            .await
            .expect("create session response");
        let session: ChatSessionSnapshot = read_json(response).await;

        let response = app
            .clone()
            .oneshot(authed_json_request(
                "POST",
                &format!("/api/v1/chat/sessions/{}/messages", session.id),
                Body::from(r#"{"content":"hello hivemind"}"#),
            ))
            .await
            .expect("send message response");
        assert_eq!(response.status(), StatusCode::OK);
        let send_response: SendMessageResponse = read_json(response).await;
        match send_response {
            SendMessageResponse::Queued { session } => {
                assert_eq!(session.messages.len(), 1);
            }
            other => panic!("expected queued response, got {other:?}"),
        }

        let mut snapshot = None;
        for _ in 0..20 {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            let response = app
                .clone()
                .oneshot(authed_request("GET", &format!("/api/v1/chat/sessions/{}", session.id)))
                .await
                .expect("get session response");
            let candidate: ChatSessionSnapshot = read_json(response).await;
            if candidate.state == ChatRunState::Idle {
                snapshot = Some(candidate);
                break;
            }
        }
        let snapshot = snapshot.expect("chat session should eventually go idle");

        assert_eq!(snapshot.state, ChatRunState::Idle);
        assert_eq!(snapshot.messages.len(), 2);
        assert_eq!(snapshot.messages[0].data_class, Some(hive_classification::DataClass::Internal));
        assert_eq!(snapshot.messages[1].provider_id.as_deref(), Some("test-mock"));
        assert!(!snapshot.messages[1].content.is_empty());
        assert!(!snapshot.workspace_path.is_empty());
        assert!(!snapshot.workspace_linked);
    }

    #[tokio::test]
    async fn tools_list_includes_filesystem_tools() {
        let app = build_router(app_state());
        let response =
            app.oneshot(authed_request("GET", "/api/v1/tools")).await.expect("tools list response");
        assert_eq!(response.status(), StatusCode::OK);
        let tools: Vec<ToolDefinition> = read_json(response).await;
        let ids = tools.iter().map(|tool| tool.id.as_str()).collect::<Vec<_>>();
        assert!(ids.contains(&"core.ask_user"));
        assert!(ids.contains(&"core.activate_skill"));
        assert!(ids.contains(&"filesystem.read"));
        assert!(ids.contains(&"filesystem.list"));
        assert!(ids.contains(&"filesystem.exists"));
        assert!(ids.contains(&"filesystem.write"));
        assert!(ids.contains(&"filesystem.search"));
        assert!(ids.contains(&"filesystem.glob"));
    }

    #[tokio::test]
    async fn workspace_routes_can_save_read_and_list_files() {
        let app = build_router(app_state());
        let response = app
            .clone()
            .oneshot(authed_request("POST", "/api/v1/chat/sessions"))
            .await
            .expect("create session response");
        let session: ChatSessionSnapshot = read_json(response).await;

        let response = put_json(
            &app,
            format!("/api/v1/chat/sessions/{}/workspace/file?path=notes%2Ftodo.txt", session.id),
            json!({ "content": "hello workspace" }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        let response = app
            .clone()
            .oneshot(authed_request(
                "GET",
                &format!(
                    "/api/v1/chat/sessions/{}/workspace/file?path=notes%2Ftodo.txt",
                    session.id
                ),
            ))
            .await
            .expect("read workspace file response");
        assert_eq!(response.status(), StatusCode::OK);
        let file: WorkspaceFileContent = read_json(response).await;
        assert_eq!(file.path, "notes/todo.txt");
        assert_eq!(file.content, "hello workspace");
        assert!(!file.is_binary);
        assert_eq!(file.mime_type, "text/plain");

        let response = app
            .oneshot(authed_request(
                "GET",
                &format!("/api/v1/chat/sessions/{}/workspace/files", session.id),
            ))
            .await
            .expect("list workspace files response");
        assert_eq!(response.status(), StatusCode::OK);
        let entries: Vec<WorkspaceEntry> = read_json(response).await;
        // children is None (lazy-loaded), so just verify the directory entry exists
        assert!(entries.iter().any(|entry| entry.path == "notes" && entry.is_dir));
    }

    #[tokio::test]
    async fn workspace_routes_can_create_move_and_delete_entries() {
        let app = build_router(app_state());
        let response = app
            .clone()
            .oneshot(authed_request("POST", "/api/v1/chat/sessions"))
            .await
            .expect("create session response");
        let session: ChatSessionSnapshot = read_json(response).await;

        let response = app
            .clone()
            .oneshot(authed_request(
                "POST",
                &format!(
                    "/api/v1/chat/sessions/{}/workspace/directory?path=docs%2Fdrafts",
                    session.id
                ),
            ))
            .await
            .expect("create directory response");
        assert_eq!(response.status(), StatusCode::CREATED);

        let response = put_json(
            &app,
            format!(
                "/api/v1/chat/sessions/{}/workspace/file?path=docs%2Fdrafts%2Ftodo.txt",
                session.id
            ),
            json!({ "content": "ship it" }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        let response = post_json(
            &app,
            format!("/api/v1/chat/sessions/{}/workspace/move", session.id),
            json!({
                "from": "docs/drafts/todo.txt",
                "to": "archive/todo.txt"
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        let response = app
            .clone()
            .oneshot(authed_request(
                "GET",
                &format!(
                    "/api/v1/chat/sessions/{}/workspace/file?path=archive%2Ftodo.txt",
                    session.id
                ),
            ))
            .await
            .expect("read moved workspace file response");
        assert_eq!(response.status(), StatusCode::OK);
        let file: WorkspaceFileContent = read_json(response).await;
        assert_eq!(file.path, "archive/todo.txt");
        assert_eq!(file.content, "ship it");

        let response = delete_uri(
            &app,
            format!("/api/v1/chat/sessions/{}/workspace/entry?path=archive", session.id),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        let response = app
            .oneshot(authed_request(
                "GET",
                &format!("/api/v1/chat/sessions/{}/workspace/files", session.id),
            ))
            .await
            .expect("list workspace files response");
        assert_eq!(response.status(), StatusCode::OK);
        let entries: Vec<WorkspaceEntry> = read_json(response).await;
        assert!(!entries.iter().any(|entry| entry.path == "archive"));
    }

    #[tokio::test]
    async fn workspace_audit_routes_report_status_and_record() {
        let app = build_router(app_state());
        let response = app
            .clone()
            .oneshot(authed_request("POST", "/api/v1/chat/sessions"))
            .await
            .expect("create session response");
        let session: ChatSessionSnapshot = read_json(response).await;

        let response = put_json(
            &app,
            format!("/api/v1/chat/sessions/{}/workspace/file?path=src%2Fmain.rs", session.id),
            json!({ "content": "fn main() {}\n" }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        let response = app
            .clone()
            .oneshot(authed_request(
                "GET",
                &format!("/api/v1/chat/sessions/{}/workspace/audit?path=src%2Fmain.rs", session.id),
            ))
            .await
            .expect("get unaudited workspace audit response");
        assert_eq!(response.status(), StatusCode::OK);
        let audit_status: AuditStatusResponse = read_json(response).await;
        assert!(audit_status.record.is_none());
        assert_eq!(audit_status.status, FileAuditStatus::Unaudited);

        let response = post_json(
            &app,
            format!("/api/v1/chat/sessions/{}/workspace/audit?path=src%2Fmain.rs", session.id),
            json!({ "model": "audit-model-v1" }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let audit_record: FileAuditRecord = read_json(response).await;
        assert_eq!(audit_record.path, "src/main.rs");
        assert_eq!(audit_record.model_used, "audit-model-v1");

        let response = app
            .clone()
            .oneshot(authed_request(
                "GET",
                &format!("/api/v1/chat/sessions/{}/workspace/audit?path=src%2Fmain.rs", session.id),
            ))
            .await
            .expect("get audited workspace audit response");
        assert_eq!(response.status(), StatusCode::OK);
        let audit_status: AuditStatusResponse = read_json(response).await;
        let stored_record = audit_status.record.expect("stored audit record");
        assert_eq!(stored_record.path, audit_record.path);
        assert_eq!(stored_record.content_hash, audit_record.content_hash);
        assert_eq!(stored_record.model_used, audit_record.model_used);
        assert_eq!(audit_status.status, FileAuditStatus::Safe);

        let response = put_json(
            &app,
            format!("/api/v1/chat/sessions/{}/workspace/file?path=src%2Fmain.rs", session.id),
            json!({ "content": "fn main() { println!(\"changed\"); }\n" }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        let response = app
            .clone()
            .oneshot(authed_request(
                "GET",
                &format!("/api/v1/chat/sessions/{}/workspace/audit?path=src%2Fmain.rs", session.id),
            ))
            .await
            .expect("get stale workspace audit response");
        assert_eq!(response.status(), StatusCode::OK);
        let audit_status: AuditStatusResponse = read_json(response).await;
        assert!(audit_status.record.is_some());
        assert_eq!(audit_status.status, FileAuditStatus::Stale);
    }

    #[tokio::test]
    async fn workspace_classification_routes_support_crud() {
        let app = build_router(app_state());
        let response = app
            .clone()
            .oneshot(authed_request("POST", "/api/v1/chat/sessions"))
            .await
            .expect("create session response");
        let session: ChatSessionSnapshot = read_json(response).await;

        let response = app
            .clone()
            .oneshot(authed_request(
                "GET",
                &format!("/api/v1/chat/sessions/{}/workspace/classification", session.id),
            ))
            .await
            .expect("get classification response");
        assert_eq!(response.status(), StatusCode::OK);
        let classification: WorkspaceClassification = read_json(response).await;
        assert_eq!(classification.default, DataClass::Internal);
        assert!(classification.overrides.is_empty());

        let response = put_json(
            &app,
            format!("/api/v1/chat/sessions/{}/workspace/classification", session.id),
            json!({ "default": "CONFIDENTIAL" }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        let response = put_json(
            &app,
            format!(
                "/api/v1/chat/sessions/{}/workspace/classification/override?path=src%2Fmain.rs",
                session.id
            ),
            json!({ "class": "RESTRICTED" }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        let response = app
            .clone()
            .oneshot(authed_request(
                "GET",
                &format!("/api/v1/chat/sessions/{}/workspace/classification", session.id),
            ))
            .await
            .expect("get updated classification response");
        assert_eq!(response.status(), StatusCode::OK);
        let classification: WorkspaceClassification = read_json(response).await;
        assert_eq!(classification.default, DataClass::Confidential);
        assert_eq!(classification.overrides.get("src/main.rs"), Some(&DataClass::Restricted));

        let response = app
            .clone()
            .oneshot(authed_request(
                "DELETE",
                &format!(
                    "/api/v1/chat/sessions/{}/workspace/classification/override?path=src%2Fmain.rs",
                    session.id
                ),
            ))
            .await
            .expect("clear classification override response");
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        let response = app
            .oneshot(authed_request(
                "GET",
                &format!("/api/v1/chat/sessions/{}/workspace/classification", session.id),
            ))
            .await
            .expect("get cleared classification response");
        assert_eq!(response.status(), StatusCode::OK);
        let classification: WorkspaceClassification = read_json(response).await;
        assert_eq!(classification.default, DataClass::Confidential);
        assert!(classification.overrides.is_empty());
    }

    #[tokio::test]
    async fn tool_question_direct_invoke_returns_error() {
        let app = build_router(app_state());
        let response = post_json(
            &app,
            "/api/v1/tools/core.ask_user/invoke".to_string(),
            json!({
                "input": { "question": "hello?" },
                "dataClass": "INTERNAL"
            }),
        )
        .await;
        // core.ask_user is handled by the interaction gate, not direct execution
        assert!(response.status().is_server_error());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn filesystem_tools_read_list_and_exists() {
        let app = build_router(app_state());
        let root = std::env::current_dir().expect("current dir");
        let temp_dir = tempdir_in(&root).expect("tempdir in root");
        let file_path = temp_dir.path().join("sample.txt");
        std::fs::write(&file_path, "hello").expect("write file");
        let nested_dir = temp_dir.path().join("nested");
        std::fs::create_dir_all(&nested_dir).expect("nested dir");
        std::fs::write(nested_dir.join("find.txt"), "searchable text").expect("write nested file");

        let rel_file =
            file_path.strip_prefix(&root).expect("strip prefix").to_string_lossy().to_string();
        let rel_dir = temp_dir
            .path()
            .strip_prefix(&root)
            .expect("strip prefix")
            .to_string_lossy()
            .to_string();

        let response = post_json(
            &app,
            "/api/v1/tools/filesystem.read/invoke".to_string(),
            json!({
                "input": { "path": rel_file },
                "dataClass": "INTERNAL"
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let result: ToolResult = read_json(response).await;
        assert_eq!(result.output["content"], "1: hello");

        let response = post_json(
            &app,
            "/api/v1/tools/filesystem.list/invoke".to_string(),
            json!({
                "input": { "path": rel_dir },
                "dataClass": "INTERNAL"
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let result: ToolResult = read_json(response).await;
        let entries = result.output["entries"].as_array().expect("entries array");
        assert!(entries.iter().any(|entry| entry["name"] == "sample.txt"));

        let response = post_json(
            &app,
            "/api/v1/tools/filesystem.exists/invoke".to_string(),
            json!({
                "input": { "path": rel_file },
                "dataClass": "INTERNAL"
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let result: ToolResult = read_json(response).await;
        assert_eq!(result.output["exists"], true);

        let response = post_json(
            &app,
            "/api/v1/tools/filesystem.exists/invoke".to_string(),
            json!({
                "input": { "path": format!("{}\\missing.txt", rel_dir) },
                "dataClass": "INTERNAL"
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let result: ToolResult = read_json(response).await;
        assert_eq!(result.output["exists"], false);

        let response = post_json(
            &app,
            "/api/v1/tools/filesystem.search/invoke".to_string(),
            json!({
                "input": { "path": rel_dir, "query": "searchable", "limit": 5 },
                "dataClass": "INTERNAL"
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let result: ToolResult = read_json(response).await;
        let matches = result.output["matches"].as_array().expect("matches array");
        assert!(!matches.is_empty());

        let glob_pattern = format!("{}/**/*.txt", rel_dir.replace('\\', "/"));
        let response = post_json(
            &app,
            "/api/v1/tools/filesystem.glob/invoke".to_string(),
            json!({
                "input": { "pattern": glob_pattern, "limit": 10 },
                "dataClass": "INTERNAL"
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let result: ToolResult = read_json(response).await;
        let matches = result.output["matches"].as_array().expect("glob matches");
        assert!(matches.iter().any(|value| value.as_str().unwrap_or("").ends_with("sample.txt")));
    }

    #[tokio::test]
    async fn filesystem_write_requires_approval() {
        let app = build_router(app_state());
        let root = std::env::current_dir().expect("current dir");
        let temp_dir = tempdir_in(&root).expect("tempdir in root");
        let rel_file = temp_dir
            .path()
            .join("write.txt")
            .strip_prefix(&root)
            .expect("strip prefix")
            .to_string_lossy()
            .to_string();

        let response = post_json(
            &app,
            "/api/v1/tools/filesystem.write/invoke".to_string(),
            json!({
                "input": { "path": rel_file, "content": "blocked" },
                "dataClass": "INTERNAL"
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn chat_session_queues_follow_up_messages() {
        let app = build_router(app_state());
        let response = app
            .clone()
            .oneshot(authed_request("POST", "/api/v1/chat/sessions"))
            .await
            .expect("create session response");
        let session: ChatSessionSnapshot = read_json(response).await;

        let first_uri = format!("/api/v1/chat/sessions/{}/messages", session.id);
        let second_uri = first_uri.clone();
        let _ = app
            .clone()
            .oneshot(authed_json_request(
                "POST",
                &first_uri,
                Body::from(r#"{"content":"first queued command"}"#),
            ))
            .await
            .expect("first message response");
        let response = app
            .clone()
            .oneshot(authed_json_request(
                "POST",
                &second_uri,
                Body::from(r#"{"content":"second queued command"}"#),
            ))
            .await
            .expect("second message response");
        let send_response: SendMessageResponse = read_json(response).await;
        let snapshot = match send_response {
            SendMessageResponse::Queued { session } => session,
            other => panic!("expected queued response, got {other:?}"),
        };
        assert!(snapshot.queued_count >= 1);

        let mut snapshot = None;
        for _ in 0..20 {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            let response = app
                .clone()
                .oneshot(authed_request("GET", &format!("/api/v1/chat/sessions/{}", session.id)))
                .await
                .expect("get session response");
            let candidate: ChatSessionSnapshot = read_json(response).await;
            if candidate.state == ChatRunState::Idle {
                snapshot = Some(candidate);
                break;
            }
        }
        let snapshot = snapshot.expect("chat session should eventually go idle");

        let assistant_messages = snapshot
            .messages
            .iter()
            .filter(|message| message.role == ChatMessageRole::Assistant)
            .count();
        assert_eq!(assistant_messages, 2);
        assert_eq!(snapshot.state, ChatRunState::Idle);
    }

    #[test]
    fn config_driven_router_snapshot_reflects_provider_bindings() {
        let mut config = HiveMindConfig::default();
        config.models = ModelsConfig {
            providers: vec![ModelProviderConfig {
                id: "openrouter".to_string(),
                name: Some("OpenRouter".to_string()),
                kind: ProviderKindConfig::OpenAiCompatible,
                base_url: Some("http://127.0.0.1:43180".to_string()),
                auth: ProviderAuthConfig::None,
                models: vec!["test-model".to_string()],
                capabilities: [CapabilityConfig::Chat].into_iter().collect(),
                model_capabilities: Default::default(),
                channel_class: hive_classification::ChannelClass::Internal,
                priority: 100,
                enabled: true,
                options: ProviderOptionsConfig::default(),
            }],
            request_timeout_secs: None,
            stream_timeout_secs: None,
        };
        validate_hivemind_config(&config).expect("provider-backed config should validate");

        let snapshot = chat::build_model_router_from_config(&config, None, None)
            .expect("config should build model router")
            .snapshot();
        assert_eq!(snapshot.providers[0].id, "openrouter");
        assert_eq!(snapshot.providers[0].name.as_deref(), Some("OpenRouter"));
    }

    #[tokio::test]
    async fn hard_interrupt_stops_current_run() {
        let audit_path = unique_temp_path("hive-api-interrupt-test", "log");
        let audit = AuditLogger::new(audit_path).expect("audit logger");
        let bus = EventBus::new(32);
        let chat = Arc::new(ChatService::new(
            audit.clone(),
            bus.clone(),
            ChatRuntimeConfig {
                step_delay: std::time::Duration::from_millis(60),
                ..ChatRuntimeConfig::default()
            },
            unique_temp_path("hive-api-interrupt-home", "dir"),
            unique_temp_path("hive-api-interrupt-memory", "db"),
            HiveMindConfig::default().security.prompt_injection,
            unique_temp_path("hive-api-interrupt-risk", "db"),
            canvas_ws::CanvasSessionRegistry::new(),
        ));
        let app = build_router(AppState::with_chat(
            HiveMindConfig::default(),
            audit,
            bus,
            Arc::new(Notify::new()),
            chat,
        ));

        let response = app
            .clone()
            .oneshot(authed_request("POST", "/api/v1/chat/sessions"))
            .await
            .expect("create session response");
        let session: ChatSessionSnapshot = read_json(response).await;

        let _ = app
            .clone()
            .oneshot(authed_json_request(
                "POST",
                &format!("/api/v1/chat/sessions/{}/messages", session.id),
                Body::from(r#"{"content":"stop me quickly"}"#),
            ))
            .await
            .expect("message response");

        let response = app
            .clone()
            .oneshot(authed_json_request(
                "POST",
                &format!("/api/v1/chat/sessions/{}/interrupt", session.id),
                Body::from(r#"{"mode":"hard"}"#),
            ))
            .await
            .expect("interrupt response");
        assert_eq!(response.status(), StatusCode::OK);

        tokio::time::sleep(std::time::Duration::from_millis(90)).await;

        let response = app
            .oneshot(authed_request("GET", &format!("/api/v1/chat/sessions/{}", session.id)))
            .await
            .expect("get session response");
        let snapshot: ChatSessionSnapshot = read_json(response).await;

        assert_eq!(snapshot.state, ChatRunState::Interrupted);
        assert!(snapshot.messages.iter().any(|message| {
            message.role == ChatMessageRole::User
                && message.status == ChatMessageStatus::Interrupted
        }));
        assert!(!snapshot
            .messages
            .iter()
            .any(|message| message.role == ChatMessageRole::Assistant));
    }

    #[tokio::test]
    async fn call_mcp_tool_unknown_server_returns_not_found() {
        let app = build_router(app_state());
        let response = post_json(
            &app,
            "/api/v1/mcp/servers/no-such-server/tools/some_tool/call".to_string(),
            json!({ "input": {} }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    // ── Local models hub integration tests ──────────────────────────

    mod hub_integration {
        use super::*;
        use hive_inference::{HubClient, HubSearchResult};
        use hive_local_models::LocalModelService;
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;

        fn mock_hf_server(response_body: &str) -> (String, std::thread::JoinHandle<()>) {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let base_url = format!("http://{addr}");
            let body = response_body.to_string();

            let handle = std::thread::spawn(move || {
                if let Ok((mut stream, _)) = listener.accept() {
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut request_line = String::new();
                    reader.read_line(&mut request_line).ok();
                    loop {
                        let mut line = String::new();
                        reader.read_line(&mut line).ok();
                        if line.trim().is_empty() {
                            break;
                        }
                    }
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body,
                    );
                    stream.write_all(response.as_bytes()).ok();
                }
            });

            (base_url, handle)
        }

        fn mock_hf_error_server(status: u16, body: &str) -> (String, std::thread::JoinHandle<()>) {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let base_url = format!("http://{addr}");
            let body = body.to_string();

            let handle = std::thread::spawn(move || {
                if let Ok((mut stream, _)) = listener.accept() {
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut request_line = String::new();
                    reader.read_line(&mut request_line).ok();
                    loop {
                        let mut line = String::new();
                        reader.read_line(&mut line).ok();
                        if line.trim().is_empty() {
                            break;
                        }
                    }
                    let response = format!(
                        "HTTP/1.1 {status} Error\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body,
                    );
                    stream.write_all(response.as_bytes()).ok();
                }
            });

            (base_url, handle)
        }

        fn app_state_with_hub(mock_base_url: &str) -> AppState {
            let dir = std::env::temp_dir().join("hive-api-hub-test");
            let _ = std::fs::create_dir_all(&dir);
            let hub = HubClient::new().with_base_url(format!("{mock_base_url}/api"));
            let service = LocalModelService::with_hub_client(dir, hub).expect("service");
            app_state().with_local_models(service)
        }

        #[tokio::test]
        async fn search_endpoint_returns_models() {
            let body = r#"[
                {
                    "_id": "abc1", "id": "google/gemma-2b", "modelId": "google/gemma-2b",
                    "author": "google", "tags": ["gguf"],
                    "downloads": 100000, "likes": 500, "private": false,
                    "pipeline_tag": "text-generation", "createdAt": "2025-01-01T00:00:00.000Z"
                }
            ]"#;

            let (base_url, handle) = mock_hf_server(body);
            let app = build_router(app_state_with_hub(&base_url));

            let response = app
                .oneshot(authed_request("GET", "/api/v1/local-models/search?query=gemma&limit=5"))
                .await
                .expect("search response");

            assert_eq!(response.status(), StatusCode::OK);
            let result: HubSearchResult = read_json(response).await;
            assert_eq!(result.models.len(), 1);
            assert_eq!(result.models[0].id, "google/gemma-2b");
            assert_eq!(result.models[0].downloads, 100000);

            handle.join().ok();
        }

        #[tokio::test]
        async fn search_endpoint_serialized_field_names_match_frontend() {
            let body = r#"[
                {
                    "_id": "abc2", "id": "TheBloke/Llama-2-7B-GGUF", "modelId": "TheBloke/Llama-2-7B-GGUF",
                    "author": "TheBloke", "tags": ["gguf", "llama"],
                    "downloads": 50000, "likes": 200, "private": false,
                    "pipeline_tag": "text-generation", "library_name": "transformers",
                    "createdAt": "2024-01-01T00:00:00.000Z"
                }
            ]"#;

            let (base_url, handle) = mock_hf_server(body);
            let app = build_router(app_state_with_hub(&base_url));

            let response = app
                .oneshot(authed_request("GET", "/api/v1/local-models/search?query=llama"))
                .await
                .expect("search response");

            assert_eq!(response.status(), StatusCode::OK);
            let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

            // Verify the exact JSON field names that the frontend expects
            let model = &json["models"][0];
            assert_eq!(model["id"], "TheBloke/Llama-2-7B-GGUF");
            assert_eq!(model["author"], "TheBloke");
            assert_eq!(model["downloads"], 50000);
            assert_eq!(model["likes"], 200);
            assert_eq!(model["pipeline_tag"], "text-generation");
            assert!(json["total"].is_number());

            handle.join().ok();
        }

        #[tokio::test]
        async fn search_endpoint_hf_error_returns_bad_gateway() {
            let (base_url, handle) = mock_hf_error_server(500, "HF internal error");
            let app = build_router(app_state_with_hub(&base_url));

            let response = app
                .oneshot(authed_request("GET", "/api/v1/local-models/search?query=test"))
                .await
                .expect("search response");

            assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
            let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let error_text = String::from_utf8(body.to_vec()).unwrap();
            assert!(error_text.contains("500"), "error should contain HF status: {error_text}");

            handle.join().ok();
        }

        #[tokio::test]
        async fn search_endpoint_without_local_models_returns_503() {
            let app = build_router(app_state());

            let response = app
                .oneshot(authed_request("GET", "/api/v1/local-models/search?query=test"))
                .await
                .expect("search response");

            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        }

        #[tokio::test]
        async fn hub_files_endpoint_returns_files() {
            let body = r#"[
                { "type": "file", "path": "llama-2-7b.Q4_K_M.gguf", "size": 4000000000 },
                { "type": "file", "path": "README.md", "size": 1024 }
            ]"#;

            let (base_url, handle) = mock_hf_server(body);
            let app = build_router(app_state_with_hub(&base_url));

            let response = app
                .oneshot(authed_request(
                    "GET",
                    "/api/v1/local-models/hub/TheBloke%2FLlama-2-7B-GGUF/files",
                ))
                .await
                .expect("files response");

            assert_eq!(response.status(), StatusCode::OK);
            let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

            assert_eq!(json["repo_id"], "TheBloke/Llama-2-7B-GGUF");
            assert_eq!(json["files"].as_array().unwrap().len(), 2);
            assert_eq!(json["files"][0]["filename"], "llama-2-7b.Q4_K_M.gguf");
            assert_eq!(json["files"][0]["size"], 4000000000u64);

            handle.join().ok();
        }
    }

    // ======================================================================
    // Session deletion – agent & workflow cleanup
    // ======================================================================

    /// Helper: create an AppState whose ChatService has the workflow service wired up.
    fn app_state_with_workflows() -> AppState {
        let state = app_state();
        // `with_chat()` creates an in-memory WorkflowService but doesn't connect
        // it to ChatService.  Wire it up so delete_session exercises the
        // workflow-cleanup path.
        state.chat.set_workflow_service(Arc::clone(&state.workflows));
        state
    }

    fn test_agent_spec(id: &str, name: &str) -> AgentSpec {
        AgentSpec {
            id: id.to_string(),
            name: name.to_string(),
            friendly_name: format!("test_{id}"),
            description: format!("Test agent {name}"),
            role: AgentRole::Planner,
            model: None,
            preferred_models: None,
            loop_strategy: None,
            tool_execution_mode: None,
            system_prompt: "You are a test agent".to_string(),
            allowed_tools: Vec::new(),
            avatar: None,
            color: None,
            data_class: DataClass::Public,
            keep_alive: false,
            idle_timeout_secs: None,
            tool_limits: None,
            persona_id: None,
            workflow_managed: false,
        }
    }

    fn simple_workflow_yaml() -> &'static str {
        r#"
name: user/test-wf
version: "1.0"
description: "A minimal test workflow"
variables:
  type: object
  properties:
    result:
      type: string
      default: ""
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs:
      - name: greeting
        type: string
        required: true
    outputs:
      greeting: "{{trigger.greeting}}"
    next: [finish]

  - id: finish
    type: control_flow
    control:
      kind: end_workflow
output:
  greeting: "{{steps.start.outputs.greeting}}"
"#
    }

    #[tokio::test]
    async fn delete_session_terminates_child_agents() {
        let state = app_state_with_workflows();
        let session = state
            .chat
            .create_session(SessionModality::Linear, Some("AgentCleanup".to_string()), None)
            .await
            .expect("create session");

        let supervisor =
            state.chat.get_or_create_supervisor(&session.id).await.expect("get supervisor");

        // Spawn two agents (one as a child of the other)
        supervisor
            .spawn_agent(test_agent_spec("parent-agent", "Parent"), None, None, None, None)
            .await
            .expect("spawn parent");
        supervisor
            .spawn_agent(
                test_agent_spec("child-agent", "Child"),
                Some("parent-agent".to_string()),
                None,
                None,
                None,
            )
            .await
            .expect("spawn child");

        assert_eq!(supervisor.get_all_agents().len(), 2, "both agents should exist before delete");

        // Delete the session
        state.chat.delete_session(&session.id, false).await.expect("delete session");

        // After deletion, the supervisor should have no agents
        assert!(
            supervisor.get_all_agents().is_empty(),
            "all agents should be killed after session deletion"
        );
    }

    #[tokio::test]
    async fn delete_session_cleans_up_workflow_instances() {
        let state = app_state_with_workflows();
        let session = state
            .chat
            .create_session(SessionModality::Linear, Some("WorkflowCleanup".to_string()), None)
            .await
            .expect("create session");

        // Register a workflow definition and launch two instances tied to this session
        state.workflows.save_definition(simple_workflow_yaml()).await.expect("save definition");

        let wf_id1 = state
            .workflows
            .launch(
                "user/test-wf",
                Some("1.0"),
                json!({"greeting": "hello"}),
                &session.id,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("launch workflow 1");
        let wf_id2 = state
            .workflows
            .launch(
                "user/test-wf",
                Some("1.0"),
                json!({"greeting": "world"}),
                &session.id,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("launch workflow 2");

        // Verify instances exist
        let filter = hive_workflow::types::InstanceFilter {
            parent_session_id: Some(session.id.clone()),
            ..Default::default()
        };
        let before = state.workflows.list_instances(&filter).await.expect("list before");
        assert_eq!(before.items.len(), 2, "two workflow instances should exist before delete");

        // Delete the session
        state.chat.delete_session(&session.id, false).await.expect("delete session");

        // Both instances should be gone
        let after = state.workflows.list_instances(&filter).await.expect("list after");
        assert!(
            after.items.is_empty(),
            "all workflow instances should be deleted after session deletion, found: {}",
            after.items.len()
        );

        // Direct lookup should also fail
        assert!(state.workflows.get_instance(wf_id1).await.is_err());
        assert!(state.workflows.get_instance(wf_id2).await.is_err());
    }

    #[tokio::test]
    async fn delete_session_does_not_affect_other_sessions() {
        let state = app_state_with_workflows();

        // Create two sessions with agents and workflows
        let session_a = state
            .chat
            .create_session(SessionModality::Linear, Some("SessionA".to_string()), None)
            .await
            .expect("create session A");
        let session_b = state
            .chat
            .create_session(SessionModality::Linear, Some("SessionB".to_string()), None)
            .await
            .expect("create session B");

        let sup_a = state.chat.get_or_create_supervisor(&session_a.id).await.expect("sup A");
        let sup_b = state.chat.get_or_create_supervisor(&session_b.id).await.expect("sup B");

        sup_a
            .spawn_agent(test_agent_spec("agent-a", "AgentA"), None, None, None, None)
            .await
            .expect("spawn agent A");
        sup_b
            .spawn_agent(test_agent_spec("agent-b", "AgentB"), None, None, None, None)
            .await
            .expect("spawn agent B");

        state.workflows.save_definition(simple_workflow_yaml()).await.expect("save definition");
        let _wf_a = state
            .workflows
            .launch(
                "user/test-wf",
                Some("1.0"),
                json!({"greeting": "a"}),
                &session_a.id,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("launch wf A");
        let _wf_b = state
            .workflows
            .launch(
                "user/test-wf",
                Some("1.0"),
                json!({"greeting": "b"}),
                &session_b.id,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("launch wf B");

        // Delete session A only
        state.chat.delete_session(&session_a.id, false).await.expect("delete session A");

        // Session B's agent should still be alive
        assert_eq!(sup_b.get_all_agents().len(), 1, "session B agent should survive");

        // Session B's workflow should still exist
        let filter_b = hive_workflow::types::InstanceFilter {
            parent_session_id: Some(session_b.id.clone()),
            ..Default::default()
        };
        let remaining = state.workflows.list_instances(&filter_b).await.expect("list B");
        assert_eq!(remaining.items.len(), 1, "session B workflow should survive");
    }

    #[tokio::test]
    async fn delete_session_via_http_returns_no_content() {
        let state = app_state_with_workflows();
        let session = state
            .chat
            .create_session(SessionModality::Linear, Some("HttpDelete".to_string()), None)
            .await
            .expect("create session");

        let app = build_router(state.clone());

        let response = delete_uri(&app, format!("/api/v1/chat/sessions/{}", session.id)).await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        // Session should be gone
        let list_resp = app
            .clone()
            .oneshot(authed_request("GET", "/api/v1/chat/sessions"))
            .await
            .expect("list sessions");
        let sessions: Vec<serde_json::Value> = read_json(list_resp).await;
        assert!(
            sessions.iter().all(|s| s["id"] != session.id),
            "deleted session should not appear in session list"
        );
    }

    #[tokio::test]
    async fn delete_session_http_cleans_up_agents_and_workflows() {
        let state = app_state_with_workflows();
        let session = state
            .chat
            .create_session(SessionModality::Linear, Some("FullCleanup".to_string()), None)
            .await
            .expect("create session");

        // Spawn an agent
        let supervisor = state.chat.get_or_create_supervisor(&session.id).await.expect("sup");
        supervisor
            .spawn_agent(test_agent_spec("http-agent", "HttpAgent"), None, None, None, None)
            .await
            .expect("spawn agent");

        // Launch a workflow
        state.workflows.save_definition(simple_workflow_yaml()).await.expect("save def");
        let wf_id = state
            .workflows
            .launch(
                "user/test-wf",
                Some("1.0"),
                json!({"greeting": "hi"}),
                &session.id,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("launch wf");

        // Delete via HTTP
        let app = build_router(state.clone());
        let response = delete_uri(&app, format!("/api/v1/chat/sessions/{}", session.id)).await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        // Agent should be gone
        assert!(
            supervisor.get_all_agents().is_empty(),
            "agents should be cleaned up via HTTP delete"
        );

        // Workflow instance should be gone
        assert!(
            state.workflows.get_instance(wf_id).await.is_err(),
            "workflow instance should be cleaned up via HTTP delete"
        );
    }

    #[tokio::test]
    async fn delete_nonexistent_session_returns_not_found() {
        let state = app_state_with_workflows();
        let app = build_router(state);

        let response = delete_uri(&app, "/api/v1/chat/sessions/nonexistent-id".to_string()).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
