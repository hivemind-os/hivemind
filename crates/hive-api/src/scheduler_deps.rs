//! Implementations of the scheduler's optional trait dependencies
//! (`SchedulerToolExecutor`, `SchedulerAgentRunner`, `SchedulerNotifier`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use serde_json::Value;

use hive_agents::{AgentMessage, AgentRole, AgentSpec, ControlSignal, SupervisorEvent};
use hive_chat::{ChatService, SessionModality};
use hive_classification::DataClass;
use hive_contracts::config::Persona;
use hive_contracts::permissions::PermissionRule;
use hive_contracts::ToolApproval;
use hive_core::{load_personas, EventBus};
use hive_mcp::{McpCatalogStore, McpService, SessionMcpManager};
use hive_model::sanitize_tool_name;
use hive_scheduler::{
    SchedulerAgentRunner, SchedulerNotifier, SchedulerToolExecutor, TaskCompletionNotification,
    TaskRunStatus,
};
use hive_tools::ToolRegistry;

// ──────────────────────────────────────────────────────────────────────
// Tool Executor
// ──────────────────────────────────────────────────────────────────────

pub(crate) struct SchedulerToolExecutorImpl {
    registry: parking_lot::RwLock<ToolRegistry>,
    /// Reverse map: sanitized tool name → canonical tool ID.
    reverse_map: parking_lot::RwLock<HashMap<String, String>>,
    connector_registry: Option<Arc<hive_connectors::ConnectorRegistry>>,
    connector_audit_log: Option<Arc<hive_connectors::ConnectorAuditLog>>,
    #[allow(dead_code)]
    mcp: Arc<McpService>,
    mcp_catalog: McpCatalogStore,
    session_mcp: Arc<SessionMcpManager>,
}

impl SchedulerToolExecutorImpl {
    pub fn new(
        connector_registry: Option<Arc<hive_connectors::ConnectorRegistry>>,
        connector_audit_log: Option<Arc<hive_connectors::ConnectorAuditLog>>,
        mcp: Arc<McpService>,
        mcp_catalog: McpCatalogStore,
        event_bus: EventBus,
    ) -> Self {
        let mut session_mcp = SessionMcpManager::from_configs(
            "__scheduler__".to_string(),
            &mcp.server_configs_sync(),
            event_bus,
            mcp.global_sandbox_config(),
        );
        if let Some(ne) = mcp.node_env() {
            session_mcp = session_mcp.with_node_env(ne);
        }
        if let Some(pe) = mcp.python_env() {
            session_mcp = session_mcp.with_python_env(pe);
        }
        let session_mcp = Arc::new(session_mcp);
        let registry = build_connector_tools(&connector_registry, &connector_audit_log, None);
        let reverse_map = build_reverse_map(&registry);
        Self {
            registry: parking_lot::RwLock::new(registry),
            reverse_map: parking_lot::RwLock::new(reverse_map),
            connector_registry,
            connector_audit_log,
            mcp,
            mcp_catalog,
            session_mcp,
        }
    }

    /// Rebuild the tool registry including MCP bridge tools.
    /// Call after MCP connections are established.
    pub async fn refresh(&self) {
        let mut reg =
            build_connector_tools(&self.connector_registry, &self.connector_audit_log, None);
        let enabled_ids = self.session_mcp.enabled_server_ids().await;
        hive_tools::register_mcp_tools(
            &mut reg,
            &self.mcp_catalog,
            &self.session_mcp,
            &enabled_ids,
        )
        .await;
        let rmap = build_reverse_map(&reg);
        *self.registry.write() = reg;
        *self.reverse_map.write() = rmap;
    }
}

/// Build a reverse map from sanitized tool names to canonical IDs.
fn build_reverse_map(registry: &ToolRegistry) -> HashMap<String, String> {
    let defs = registry.list_definitions();
    let mut map = HashMap::with_capacity(defs.len());
    for def in &defs {
        let sanitized = sanitize_tool_name(&def.id);
        if sanitized != def.id {
            map.insert(sanitized, def.id.clone());
        }
    }
    map
}

fn build_connector_tools(
    connector_registry: &Option<Arc<hive_connectors::ConnectorRegistry>>,
    connector_audit_log: &Option<Arc<hive_connectors::ConnectorAuditLog>>,
    persona_id: Option<&str>,
) -> ToolRegistry {
    let mut registry = ToolRegistry::new();

    if let Some(cr) = connector_registry {
        // Communication tools
        let _ = registry.register(Arc::new(hive_tools::ListConnectorsTool::new(
            Arc::clone(cr),
            persona_id.unwrap_or("system/general").to_string(),
        )));
        let _ = registry.register(Arc::new(hive_tools::CommListChannelsTool::new(Arc::clone(cr))));
        let _ = registry.register(Arc::new(hive_tools::CommSendMessageTool::new(Arc::clone(cr))));
        let _ = registry.register(Arc::new(hive_tools::CommReadMessagesTool::new(Arc::clone(cr))));

        // Calendar tools
        let _ =
            registry.register(Arc::new(hive_tools::CalendarListEventsTool::new(Arc::clone(cr))));
        let _ =
            registry.register(Arc::new(hive_tools::CalendarCreateEventTool::new(Arc::clone(cr))));
        let _ =
            registry.register(Arc::new(hive_tools::CalendarUpdateEventTool::new(Arc::clone(cr))));
        let _ =
            registry.register(Arc::new(hive_tools::CalendarDeleteEventTool::new(Arc::clone(cr))));
        let _ = registry
            .register(Arc::new(hive_tools::CalendarCheckAvailabilityTool::new(Arc::clone(cr))));

        // Drive tools
        let _ = registry.register(Arc::new(hive_tools::DriveListFilesTool::new(Arc::clone(cr))));
        let _ = registry.register(Arc::new(hive_tools::DriveReadFileTool::new(Arc::clone(cr))));
        let _ = registry.register(Arc::new(hive_tools::DriveSearchFilesTool::new(Arc::clone(cr))));
        let _ = registry.register(Arc::new(hive_tools::DriveUploadFileTool::new(Arc::clone(cr))));
        let _ = registry.register(Arc::new(hive_tools::DriveShareFileTool::new(Arc::clone(cr))));

        // Contacts tools
        let _ = registry.register(Arc::new(hive_tools::ContactsListTool::new(Arc::clone(cr))));
        let _ = registry.register(Arc::new(hive_tools::ContactsSearchTool::new(Arc::clone(cr))));
        let _ = registry.register(Arc::new(hive_tools::ContactsGetTool::new(Arc::clone(cr))));

        hive_tools::register_connector_service_tools(&mut registry, cr, persona_id);
    }

    if let Some(audit) = connector_audit_log {
        let _ =
            registry.register(Arc::new(hive_tools::CommSearchMessagesTool::new(Arc::clone(audit))));
    }

    registry
}

#[async_trait]
impl SchedulerToolExecutor for SchedulerToolExecutorImpl {
    async fn execute_tool(&self, tool_id: &str, arguments: Value) -> Result<Value, String> {
        // Try resolving the tool_id first in case it's sanitized
        let canonical = self.resolve_tool_id(tool_id).unwrap_or_else(|| tool_id.to_string());
        let tool = {
            let reg = self.registry.read();
            reg.get(&canonical).ok_or_else(|| format!("tool not found: {tool_id}"))?
        };
        let result = tool.execute(arguments).await.map_err(|e| e.to_string())?;
        Ok(result.output)
    }

    fn list_tool_ids(&self) -> Vec<String> {
        self.registry.read().list_definitions().into_iter().map(|d| d.id).collect()
    }

    fn is_tool_available(&self, tool_id: &str) -> bool {
        let canonical = self.resolve_tool_id(tool_id);
        let id = canonical.as_deref().unwrap_or(tool_id);
        self.registry.read().get(id).is_some()
    }

    fn resolve_tool_id(&self, raw_id: &str) -> Option<String> {
        let reg = self.registry.read();

        // 1. Exact match — already canonical
        if reg.get(raw_id).is_some() {
            return Some(raw_id.to_string());
        }

        // 2. Strip common prefixes added by model providers (e.g. "functions.")
        let stripped =
            raw_id.strip_prefix("functions.").or_else(|| raw_id.strip_prefix("function."));
        if let Some(stripped) = stripped {
            if reg.get(stripped).is_some() {
                return Some(stripped.to_string());
            }
            // Also try sanitized reverse lookup on the stripped form
            let rmap = self.reverse_map.read();
            if let Some(canonical) = rmap.get(stripped) {
                return Some(canonical.clone());
            }
        }

        // 3. Sanitized reverse map lookup (e.g. `comm_send_external_message` → `comm.send_external_message`)
        let rmap = self.reverse_map.read();
        if let Some(canonical) = rmap.get(raw_id) {
            return Some(canonical.clone());
        }

        // 4. Try sanitizing the raw_id and looking up (handles double-sanitization)
        let sanitized = sanitize_tool_name(raw_id);
        if let Some(canonical) = rmap.get(&sanitized) {
            return Some(canonical.clone());
        }

        None
    }

    fn get_tool_approval(&self, tool_id: &str) -> Option<ToolApproval> {
        let reg = self.registry.read();
        reg.get(tool_id).map(|t| t.definition().approval)
    }
}

// ──────────────────────────────────────────────────────────────────────
// Agent Runner
// ──────────────────────────────────────────────────────────────────────

pub(crate) struct SchedulerAgentRunnerImpl {
    chat: OnceLock<Arc<ChatService>>,
    personas_dir: PathBuf,
}

impl SchedulerAgentRunnerImpl {
    pub fn new(personas_dir: PathBuf) -> Self {
        Self { chat: OnceLock::new(), personas_dir }
    }

    /// Provide the `ChatService` reference after construction.
    pub fn set_chat(&self, chat: Arc<ChatService>) {
        let _ = self.chat.set(chat);
    }
}

#[async_trait]
impl SchedulerAgentRunner for SchedulerAgentRunnerImpl {
    async fn run_agent(
        &self,
        persona_id: &str,
        task: &str,
        friendly_name: Option<String>,
        async_exec: bool,
        timeout_secs: u64,
        _permissions: Option<Vec<PermissionRule>>,
    ) -> Result<Option<String>, String> {
        let chat = self.chat.get().ok_or("scheduler agent runner: ChatService not initialised")?;

        let personas = load_personas(&self.personas_dir).unwrap_or_default();
        let persona = resolve_persona(persona_id, &personas);

        // Create a dedicated session for this scheduled run.
        let session = chat
            .create_session(
                SessionModality::default(),
                Some(format!("Scheduled: {}", friendly_name.as_deref().unwrap_or(persona_id))),
                Some(persona_id.to_string()),
            )
            .await
            .map_err(|e| format!("create session: {e}"))?;

        let supervisor = chat
            .get_or_create_supervisor(&session.id)
            .await
            .map_err(|e| format!("get supervisor: {e}"))?;

        let spec = AgentSpec {
            id: persona.id.clone(),
            name: persona.name.clone(),
            friendly_name: friendly_name.unwrap_or_else(|| persona.name.clone()),
            description: persona.description.clone(),
            role: AgentRole::Custom("scheduled".to_string()),
            model: persona.preferred_models.as_ref().and_then(|m| m.first().cloned()),
            preferred_models: None,
            loop_strategy: Some(persona.loop_strategy.clone()),
            tool_execution_mode: Some(persona.tool_execution_mode),
            system_prompt: persona.system_prompt.clone(),
            allowed_tools: persona.allowed_tools.clone(),
            avatar: persona.avatar.clone(),
            color: persona.color.clone(),
            data_class: DataClass::Internal,
            keep_alive: false,
            idle_timeout_secs: None,
            tool_limits: None,
            persona_id: Some(persona.id.clone()),
            workflow_managed: false,
                shadow_mode: false,
        };

        let agent_id = supervisor
            .spawn_agent(spec, None, Some(session.id.clone()), None, None)
            .await
            .map_err(|e| format!("spawn agent: {e}"))?;

        // Send the task.
        let handle = supervisor.send_handle();
        handle
            .send_to_agent(
                &agent_id,
                AgentMessage::Task {
                    content: task.to_string(),
                    from: Some("scheduler".to_string()),
                },
            )
            .await
            .map_err(|e| format!("send task: {e}"))?;

        // Async mode: fire-and-forget — return immediately without waiting.
        if async_exec {
            return Ok(Some(format!("agent spawned asynchronously: {agent_id}")));
        }

        // Wait for completion or timeout.
        let mut rx = supervisor.subscribe();
        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(timeout_secs);

        loop {
            match tokio::time::timeout_at(deadline, rx.recv()).await {
                Ok(Ok(SupervisorEvent::AgentCompleted { agent_id: ref cid, ref result }))
                    if *cid == agent_id =>
                {
                    return Ok(Some(result.clone()));
                }
                Ok(Ok(_)) => continue,
                Ok(Err(_)) => return Err("event channel closed".to_string()),
                Err(_) => {
                    let _ = handle
                        .send_to_agent(&agent_id, AgentMessage::Control(ControlSignal::Kill))
                        .await;
                    return Err(format!("agent timed out after {timeout_secs}s"));
                }
            }
        }
    }
}

/// Resolve a persona by id or name (case-insensitive fallback), falling back
/// to the "system/general" persona or a built-in default.
fn resolve_persona(name: &str, personas: &[Persona]) -> Persona {
    personas
        .iter()
        .find(|p| p.id == name || p.name == name)
        .or_else(|| {
            personas
                .iter()
                .find(|p| p.id.eq_ignore_ascii_case(name) || p.name.eq_ignore_ascii_case(name))
        })
        .cloned()
        .unwrap_or_else(|| {
            personas
                .iter()
                .find(|p| p.id == "system/general")
                .cloned()
                .unwrap_or_else(Persona::default_persona)
        })
}

// ──────────────────────────────────────────────────────────────────────
// Notifier
// ──────────────────────────────────────────────────────────────────────

pub(crate) struct SchedulerNotifierImpl {
    event_bus: EventBus,
}

impl SchedulerNotifierImpl {
    pub fn new(event_bus: EventBus) -> Self {
        Self { event_bus }
    }
}

#[async_trait]
impl SchedulerNotifier for SchedulerNotifierImpl {
    async fn notify_agent(
        &self,
        agent_id: &str,
        notification: TaskCompletionNotification,
    ) -> Result<(), String> {
        let payload = serde_json::to_value(&notification).unwrap_or_default();
        let topic = match notification.status {
            TaskRunStatus::Success => format!("scheduler.task.completed.agent.{agent_id}"),
            TaskRunStatus::Failure => format!("scheduler.task.failed.agent.{agent_id}"),
        };
        let _ = self.event_bus.publish(topic, "scheduler", payload);
        Ok(())
    }

    async fn notify_session(
        &self,
        session_id: &str,
        notification: TaskCompletionNotification,
    ) -> Result<(), String> {
        let payload = serde_json::to_value(&notification).unwrap_or_default();
        let topic = match notification.status {
            TaskRunStatus::Success => format!("scheduler.task.completed.{session_id}"),
            TaskRunStatus::Failure => format!("scheduler.task.failed.{session_id}"),
        };
        let _ = self.event_bus.publish(topic, "scheduler", payload);
        Ok(())
    }
}
