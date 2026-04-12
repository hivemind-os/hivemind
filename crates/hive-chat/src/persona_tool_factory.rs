use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use hive_agents::{AgentError, PersonaToolFactory};
use hive_contracts::{Persona, SandboxConfig};
use hive_core::EventBus;
use hive_mcp::{McpCatalogStore, McpService, SessionMcpManager};
use hive_skills::SkillCatalog;
use hive_skills_service::SkillsService;
use hive_tools::ToolRegistry;
use parking_lot::Mutex;

use crate::chat::build_session_tools;

/// Factory that builds persona-specific tool registries and skill catalogs.
///
/// When a child agent's persona differs from the session persona, the
/// [`AgentSupervisor`](hive_agents::AgentSupervisor) calls this factory to
/// produce an isolated `ToolRegistry` backed by its own `SessionMcpManager`
/// (with only that persona's MCP server configs), connector tools scoped to
/// the persona, and a persona-specific skill catalog.
#[derive(Clone)]
pub struct ChatPersonaToolFactory {
    personas: Arc<Mutex<Vec<Persona>>>,
    mcp: Option<Arc<McpService>>,
    mcp_catalog: Option<McpCatalogStore>,
    event_bus: EventBus,
    sandbox_config: Arc<parking_lot::RwLock<SandboxConfig>>,
    workspace_path: String,
    daemon_addr: String,
    hivemind_home: Arc<PathBuf>,
    process_manager: Arc<hive_process::ProcessManager>,
    connector_registry: Arc<hive_connectors::ConnectorRegistry>,
    connector_audit_log: Option<Arc<hive_connectors::ConnectorAuditLog>>,
    connector_service: Option<Arc<dyn hive_connectors::ConnectorServiceHandle>>,
    scheduler: Arc<hive_scheduler::SchedulerService>,
    workflow_service: Option<Arc<hive_workflow_service::WorkflowService>>,
    shell_env: Arc<parking_lot::RwLock<HashMap<String, String>>>,
    detected_shells: Arc<hive_contracts::DetectedShells>,
    skills_service: Arc<Mutex<Option<Arc<SkillsService>>>>,
    model_router: Option<Arc<hive_model::ModelRouter>>,
    web_search_config: Arc<hive_contracts::WebSearchConfig>,
}

impl ChatPersonaToolFactory {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        personas: Arc<Mutex<Vec<Persona>>>,
        mcp: Option<Arc<McpService>>,
        mcp_catalog: Option<McpCatalogStore>,
        event_bus: EventBus,
        sandbox_config: Arc<parking_lot::RwLock<SandboxConfig>>,
        workspace_path: String,
        daemon_addr: String,
        hivemind_home: Arc<PathBuf>,
        process_manager: Arc<hive_process::ProcessManager>,
        connector_registry: Arc<hive_connectors::ConnectorRegistry>,
        connector_audit_log: Option<Arc<hive_connectors::ConnectorAuditLog>>,
        connector_service: Option<Arc<dyn hive_connectors::ConnectorServiceHandle>>,
        scheduler: Arc<hive_scheduler::SchedulerService>,
        workflow_service: Option<Arc<hive_workflow_service::WorkflowService>>,
        shell_env: Arc<parking_lot::RwLock<HashMap<String, String>>>,
        detected_shells: Arc<hive_contracts::DetectedShells>,
        skills_service: Arc<Mutex<Option<Arc<SkillsService>>>>,
        model_router: Option<Arc<hive_model::ModelRouter>>,
        web_search_config: Arc<hive_contracts::WebSearchConfig>,
    ) -> Self {
        Self {
            personas,
            mcp,
            mcp_catalog,
            event_bus,
            sandbox_config,
            workspace_path,
            daemon_addr,
            hivemind_home,
            process_manager,
            connector_registry,
            connector_audit_log,
            connector_service,
            scheduler,
            workflow_service,
            shell_env,
            detected_shells,
            skills_service,
            model_router,
            web_search_config,
        }
    }

    /// Build a `SessionMcpManager` scoped to the given persona's MCP configs.
    fn build_persona_mcp(
        &self,
        persona_id: &str,
        session_id: &str,
    ) -> Option<Arc<SessionMcpManager>> {
        let mcp = self.mcp.as_ref()?;
        let configs = self.mcp_configs_for_persona(persona_id);
        if configs.is_empty() {
            return None;
        }
        let mut mgr = SessionMcpManager::from_configs(
            session_id.to_string(),
            &configs,
            self.event_bus.clone(),
            Arc::clone(&self.sandbox_config),
        );
        if let Some(ne) = mcp.node_env() {
            mgr = mgr.with_node_env(ne);
        }
        if let Some(pe) = mcp.python_env() {
            mgr = mgr.with_python_env(pe);
        }
        Some(Arc::new(mgr))
    }

    /// Return only the MCP server configs belonging to the given persona.
    fn mcp_configs_for_persona(&self, persona_id: &str) -> Vec<hive_core::McpServerConfig> {
        let personas = self.personas.lock();
        let Some(persona) = personas.iter().find(|p| p.id == persona_id) else {
            return Vec::new();
        };
        let mut seen_keys = std::collections::HashSet::new();
        persona.mcp_servers.iter().filter(|s| seen_keys.insert(s.cache_key())).cloned().collect()
    }

    /// Build a skill catalog scoped to the given persona.
    async fn skill_catalog_for_persona(&self, persona_id: &str) -> Option<Arc<SkillCatalog>> {
        let skills = self.skills_service.lock().clone()?;
        match skills.catalog_for_persona(persona_id).await {
            Ok(catalog) if !catalog.is_empty() => Some(catalog),
            Ok(_) => None,
            Err(error) => {
                tracing::warn!("failed to build skill catalog for persona {persona_id}: {error}");
                None
            }
        }
    }
}

#[async_trait]
impl PersonaToolFactory for ChatPersonaToolFactory {
    async fn build_tools_for_persona(
        &self,
        persona_id: &str,
        session_id: &str,
    ) -> Result<(Arc<ToolRegistry>, Option<Arc<SkillCatalog>>), AgentError> {
        let persona_mcp = self.build_persona_mcp(persona_id, session_id);

        // Resolve the persona's model preferences for tools like web search:
        // secondary_models first (cheaper), falling back to preferred_models.
        let persona_models = {
            let personas = self.personas.lock();
            personas
                .iter()
                .find(|p| p.id == persona_id)
                .and_then(|p| p.secondary_models.clone().or_else(|| p.preferred_models.clone()))
        };

        let tools = build_session_tools(
            &self.workspace_path,
            &["*".to_string()],
            None,
            &self.daemon_addr,
            Some(session_id),
            &self.hivemind_home,
            self.mcp_catalog.as_ref(),
            persona_mcp.as_ref(),
            Arc::clone(&self.process_manager),
            Arc::clone(&self.connector_registry),
            self.connector_audit_log.clone(),
            self.connector_service.clone(),
            Arc::clone(&self.scheduler),
            None,
            self.workflow_service.clone(),
            self.shell_env.clone(),
            self.sandbox_config.clone(),
            Arc::clone(&self.detected_shells),
            Some(persona_id),
            self.model_router.clone(),
            persona_models,
            Some(&*self.web_search_config),
        )
        .await;

        let skills = self.skill_catalog_for_persona(persona_id).await;

        Ok((tools, skills))
    }
}
