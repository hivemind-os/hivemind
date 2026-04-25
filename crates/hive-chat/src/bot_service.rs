use arc_swap::ArcSwap;
use hive_agents::{
    AgentError, AgentMessage, AgentRole, AgentStatus, AgentSummary, AgentSupervisor, BotConfig,
    BotMode, BotSummary, SupervisorEvent,
};
use hive_classification::DataClass;
use hive_contracts::{Persona, SessionPermissions, WorkspaceEntry, WorkspaceFileContent};
use hive_loop::{AgentOrchestrator, BoxFuture, ConversationJournal, LoopExecutor};
use hive_mcp::{McpCatalogStore, McpService};
use hive_model::ModelRouter;
use hive_skills::SkillCatalog;
use hive_skills_service::SkillsService;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::chat::{
    agent_spec_from_persona, build_session_tools, list_workspace_dir,
    normalize_workspace_relative_path, open_graph, read_workspace_file_at, with_default_persona,
    ApprovalStreamEvent, ChatServiceError, SessionEvent,
};
use crate::session_log::SessionLogger;

/// Manages the bot supervisor, bot CRUD, bot persistence, and bot workspace.
#[derive(Clone)]
pub(crate) struct BotService {
    // ── Bot-owned state ────────────────────────────────────
    pub(crate) bot_supervisor: Arc<RwLock<Option<Arc<AgentSupervisor>>>>,
    pub(crate) bot_configs: Arc<RwLock<HashMap<String, BotConfig>>>,
    pub(crate) bot_workspace: Arc<PathBuf>,
    pub(crate) bot_stream_tx: broadcast::Sender<SessionEvent>,
    pub(crate) bot_loggers: Arc<RwLock<HashMap<String, Arc<SessionLogger>>>>,

    // ── Shared dependencies ────────────────────────────────
    pub(crate) loop_executor: Arc<LoopExecutor>,
    pub(crate) model_router: Arc<ArcSwap<ModelRouter>>,
    pub(crate) personas: Arc<Mutex<Vec<Persona>>>,
    pub(crate) knowledge_graph_path: Arc<PathBuf>,
    pub(crate) hivemind_home: Arc<PathBuf>,
    pub(crate) mcp: Option<Arc<McpService>>,
    pub(crate) mcp_catalog: Option<McpCatalogStore>,
    pub(crate) connector_registry: Arc<hive_connectors::ConnectorRegistry>,
    pub(crate) connector_audit_log: Option<Arc<hive_connectors::ConnectorAuditLog>>,
    pub(crate) connector_service: Option<Arc<dyn hive_connectors::ConnectorServiceHandle>>,
    pub(crate) scheduler: Arc<hive_scheduler::SchedulerService>,
    pub(crate) process_manager: Arc<hive_process::ProcessManager>,
    pub(crate) workflow_service: Arc<Mutex<Option<Arc<hive_workflow_service::WorkflowService>>>>,
    pub(crate) daemon_addr: String,
    pub(crate) approval_tx: broadcast::Sender<ApprovalStreamEvent>,
    pub(crate) skills_service: Arc<Mutex<Option<Arc<SkillsService>>>>,
    pub(crate) shell_env: Arc<parking_lot::RwLock<std::collections::HashMap<String, String>>>,
    pub(crate) sandbox_config: Arc<parking_lot::RwLock<hive_contracts::SandboxConfig>>,
    pub(crate) event_bus: hive_core::EventBus,
    pub(crate) web_search_config: Arc<ArcSwap<hive_contracts::WebSearchConfig>>,
    pub(crate) plugin_host: Option<Arc<hive_plugins::PluginHost>>,
    pub(crate) plugin_registry: Option<Arc<hive_plugins::PluginRegistry>>,
}

impl BotService {
    // ── Helpers ─────────────────────────────────────────────

    /// Return (or lazily create) a `SessionLogger` for the given bot, writing
    /// into `<hivemind_home>/sessions/<bot_id>/`.
    async fn get_or_create_logger(&self, bot_id: &str) -> Option<Arc<SessionLogger>> {
        // Fast path – already cached.
        {
            let loggers = self.bot_loggers.read().await;
            if let Some(logger) = loggers.get(bot_id) {
                return Some(Arc::clone(logger));
            }
        }
        // Slow path – create and cache.
        let sessions_root = self.hivemind_home.join("sessions");
        match SessionLogger::new(&sessions_root, bot_id) {
            Ok(logger) => {
                let logger = Arc::new(logger);
                self.bot_loggers.write().await.insert(bot_id.to_string(), Arc::clone(&logger));
                Some(logger)
            }
            Err(e) => {
                tracing::warn!(bot_id, "failed to create bot session logger: {e}");
                None
            }
        }
    }

    #[allow(dead_code)]
    fn available_personas(&self) -> Vec<Persona> {
        let mut personas = self.personas.lock().clone();
        personas.retain(|p| !p.archived);
        with_default_persona(personas)
    }

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

    fn map_agent_error(error: AgentError) -> ChatServiceError {
        match error {
            AgentError::AgentNotFound(agent_id) => ChatServiceError::AgentNotFound { agent_id },
            _ => ChatServiceError::Internal { detail: error.to_string() },
        }
    }

    // ── Bot supervisor ─────────────────────────────────────

    pub async fn get_or_create_bot_supervisor(
        &self,
    ) -> Result<Arc<AgentSupervisor>, ChatServiceError> {
        {
            let guard = self.bot_supervisor.read().await;
            if let Some(sup) = guard.as_ref() {
                return Ok(Arc::clone(sup));
            }
        }

        let workspace = self.bot_workspace.as_ref().clone();
        std::fs::create_dir_all(&workspace).map_err(|e| ChatServiceError::Internal {
            detail: format!("failed to create bots workspace: {e}"),
        })?;

        let orchestrator: Arc<dyn AgentOrchestrator> = Arc::new(BotOrchestrator::new(self.clone()));
        let wf_svc = self.workflow_service.lock().clone();
        let bot_tools = build_session_tools(
            workspace.to_string_lossy().as_ref(),
            &["*".to_string()],
            None,
            &self.daemon_addr,
            None,
            &self.hivemind_home,
            self.mcp_catalog.as_ref(),
            None, // Bot supervisor doesn't use per-session MCP; individual bot sessions do.
            Arc::clone(&self.process_manager),
            Arc::clone(&self.connector_registry),
            self.connector_audit_log.clone(),
            self.connector_service.clone(),
            Arc::clone(&self.scheduler),
            None,
            wf_svc.clone(),
            self.shell_env.clone(),
            self.sandbox_config.clone(),
            Arc::new(hive_contracts::DetectedShells::default()),
            None, // Bot supervisor sees all connectors
            Some(Arc::clone(&self.model_router.load())),
            None, // Bot supervisor — no persona-specific models
            Some(&*self.web_search_config.load()),
            self.plugin_host.as_ref(),
            self.plugin_registry.as_ref().map(|r| r.as_ref()),
        )
        .await;

        let bot_tools = if let Some(ref wf) = wf_svc {
            let connector_reg = if self.connector_registry.list().is_empty() {
                None
            } else {
                Some(Arc::clone(&self.connector_registry))
            };
            let author_tools = hive_tools::create_workflow_author_tools(
                bot_tools.clone(),
                connector_reg,
                Arc::clone(wf),
                Arc::clone(&self.personas),
                hive_tools::default_event_topics(),
            );
            let mut registry = (*bot_tools).clone();
            for tool in author_tools {
                let _ = registry.register(tool);
            }
            Arc::new(registry)
        } else {
            bot_tools
        };

        let skill_catalog = self.skill_catalog_for_persona("system/general").await;

        let mut guard = self.bot_supervisor.write().await;
        if let Some(sup) = guard.as_ref() {
            return Ok(Arc::clone(sup));
        }

        let wf_svc_for_factory = wf_svc.clone();
        let persona_tool_factory: Arc<dyn hive_agents::PersonaToolFactory> =
            Arc::new(crate::persona_tool_factory::ChatPersonaToolFactory::new(
                Arc::clone(&self.personas),
                self.mcp.clone(),
                self.mcp_catalog.clone(),
                self.event_bus.clone(),
                self.sandbox_config.clone(),
                workspace.to_string_lossy().to_string(),
                self.daemon_addr.clone(),
                Arc::clone(&self.hivemind_home),
                Arc::clone(&self.process_manager),
                Arc::clone(&self.connector_registry),
                self.connector_audit_log.clone(),
                self.connector_service.clone(),
                Arc::clone(&self.scheduler),
                wf_svc_for_factory,
                self.shell_env.clone(),
                Arc::new(hive_contracts::DetectedShells::default()),
                Arc::clone(&self.skills_service),
                Some(Arc::clone(&self.model_router.load())),
                Arc::clone(&self.web_search_config.load()),
                self.plugin_host.clone(),
                self.plugin_registry.clone(),
            ));

        let supervisor = Arc::new(AgentSupervisor::with_executor_and_persona_factory(
            256,
            None,
            Arc::clone(&self.loop_executor),
            Arc::clone(&self.model_router),
            bot_tools,
            Arc::new(Mutex::new(SessionPermissions::new())),
            Arc::clone(&self.personas),
            Some(orchestrator),
            "__bot__".to_string(),
            workspace,
            skill_catalog,
            Some(Arc::new(SessionKnowledgeQueryHandler {
                knowledge_graph_path: Arc::clone(&self.knowledge_graph_path),
            })),
            Some(persona_tool_factory),
            Some("system/general".to_string()),
        ));

        self.spawn_bot_supervisor_bridge(Arc::clone(&supervisor));

        *guard = Some(Arc::clone(&supervisor));
        Ok(supervisor)
    }

    pub fn has_bot_supervisor(&self) -> bool {
        self.bot_supervisor.try_read().map(|g| g.is_some()).unwrap_or(false)
    }

    /// Returns the base directory where per-bot workspaces are created.
    pub fn bot_workspace(&self) -> &std::path::Path {
        &self.bot_workspace
    }

    pub async fn shutdown_bot_supervisor(&self) {
        let mut guard = self.bot_supervisor.write().await;
        if let Some(sup) = guard.take() {
            let _ = sup.kill_all().await;
        }
    }

    fn spawn_bot_supervisor_bridge(&self, supervisor: Arc<AgentSupervisor>) {
        let mut rx = supervisor.subscribe();
        let stream_tx = self.bot_stream_tx.clone();
        let approval_tx = self.approval_tx.clone();
        let sup = Arc::downgrade(&supervisor);
        let bot_svc = self.clone();

        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        // ── File logging (mirrors spawn_supervisor_bridge) ──
                        let bot_id = match &event {
                            SupervisorEvent::AgentSpawned { agent_id, .. }
                            | SupervisorEvent::AgentStatusChanged { agent_id, .. }
                            | SupervisorEvent::AgentTaskAssigned { agent_id, .. }
                            | SupervisorEvent::AgentOutput { agent_id, .. }
                            | SupervisorEvent::AgentCompleted { agent_id, .. } => {
                                Some(agent_id.clone())
                            }
                            _ => None,
                        };
                        if let Some(ref bid) = bot_id {
                            if let Some(logger) = bot_svc.get_or_create_logger(bid).await {
                                logger.handle_event(&SessionEvent::Supervisor(event.clone()));
                                logger.persist_event(&event);
                                logger.persist_session_event(
                                    &SessionEvent::Supervisor(event.clone()),
                                );
                            }
                        }
                        match &event {
                            SupervisorEvent::AgentStatusChanged { agent_id, status } => {
                                if *status == AgentStatus::Done || *status == AgentStatus::Error {
                                    let to_persist = {
                                        let mut configs = bot_svc.bot_configs.write().await;
                                        if let Some(config) = configs.get_mut(agent_id) {
                                            config.active = false;
                                            Some(config.clone())
                                        } else {
                                            None
                                        }
                                    };
                                    if let Some(config) = to_persist {
                                        let _ = bot_svc.persist_bot_config(&config, false).await;
                                    }
                                }
                            }
                            SupervisorEvent::AgentOutput {
                                agent_id,
                                event: hive_contracts::ReasoningEvent::ToolCallCompleted { .. },
                            } => {
                                if let Some(sup) = sup.upgrade() {
                                    if let Some(journal) = sup.get_agent_journal(agent_id) {
                                        bot_svc
                                            .update_persisted_bot_journal(agent_id, &journal)
                                            .await;
                                    }
                                }
                            }
                            _ => {}
                        }

                        if let SupervisorEvent::AgentOutput {
                            ref agent_id,
                            event:
                                hive_contracts::ReasoningEvent::UserInteractionRequired {
                                    ref request_id,
                                    ref tool_id,
                                    ref input,
                                    ref reason,
                                },
                        } = event
                        {
                            let agent_name = sup
                                .upgrade()
                                .map(|s| {
                                    s.get_all_agents()
                                        .iter()
                                        .find(|a| a.agent_id == *agent_id)
                                        .map(|a| a.spec.friendly_name.clone())
                                        .unwrap_or_default()
                                })
                                .unwrap_or_default();
                            tracing::debug!(
                                %agent_id, %request_id, %tool_id,
                                "bot bridge: forwarding approval event"
                            );
                            let _ = approval_tx.send(ApprovalStreamEvent::Added {
                                session_id: "__bot__".to_string(),
                                agent_id: agent_id.clone(),
                                agent_name,
                                request_id: request_id.clone(),
                                tool_id: tool_id.clone(),
                                input: input.clone(),
                                reason: reason.clone(),
                            });
                        }

                        // Forward bot question events so the interactions
                        // stream pushes an updated snapshot immediately.
                        if let SupervisorEvent::AgentOutput {
                            ref agent_id,
                            event:
                                hive_contracts::ReasoningEvent::QuestionAsked { ref request_id, .. },
                        } = event
                        {
                            let agent_name = sup
                                .upgrade()
                                .map(|s| {
                                    s.get_all_agents()
                                        .iter()
                                        .find(|a| a.agent_id == *agent_id)
                                        .map(|a| a.spec.friendly_name.clone())
                                        .unwrap_or_default()
                                })
                                .unwrap_or_default();
                            let _ = approval_tx.send(ApprovalStreamEvent::QuestionAdded {
                                session_id: "__bot__".to_string(),
                                agent_id: agent_id.clone(),
                                agent_name,
                                request_id: request_id.clone(),
                            });
                        }

                        let _ = stream_tx.send(event.into());
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(skipped, "bot bridge lagged — triggering refresh");
                        let _ = approval_tx.send(ApprovalStreamEvent::Refresh);
                    }
                }
            }
        });
    }

    // ── Bot CRUD ───────────────────────────────────────────

    pub async fn launch_bot(&self, mut config: BotConfig) -> Result<BotSummary, ChatServiceError> {
        if config.id.is_empty() {
            let suffix = &Uuid::new_v4().simple().to_string()[..8];
            config.id = format!("bot-{suffix}");
        }
        config.active = true;
        if config.created_at.is_empty() {
            config.created_at = chrono::Utc::now().to_rfc3339();
        }

        let agent_workspace = self.bot_workspace.join(&config.id);
        std::fs::create_dir_all(&agent_workspace).map_err(|e| ChatServiceError::Internal {
            detail: format!("failed to create agent workspace: {e}"),
        })?;

        let supervisor = self.get_or_create_bot_supervisor().await?;
        let spec = config.to_agent_spec();
        let agent_id = config.id.clone();
        let launch_prompt = config.launch_prompt.clone();

        self.persist_bot_config(&config, true).await?;
        self.bot_configs.write().await.insert(agent_id.clone(), config.clone());

        let agent_perms = if config.permission_rules.is_empty() {
            None
        } else {
            Some(Arc::new(Mutex::new(SessionPermissions::with_rules(
                config.permission_rules.clone(),
            ))))
        };
        supervisor
            .spawn_agent(spec, None, None, agent_perms, Some(agent_workspace))
            .await
            .map_err(Self::map_agent_error)?;

        if !launch_prompt.is_empty() {
            supervisor
                .send_to_agent(&agent_id, AgentMessage::Task { content: launch_prompt, from: None })
                .await
                .map_err(Self::map_agent_error)?;
        }

        let status = supervisor.get_agent_status(&agent_id).unwrap_or(AgentStatus::Spawning);

        Ok(BotSummary { config, status, last_error: None, active_model: None, tools: Vec::new() })
    }

    pub async fn launch_workflow_ai_assist(
        &self,
        current_yaml: &str,
        user_prompt: &str,
    ) -> Result<String, ChatServiceError> {
        let preferred_models = {
            let personas = self.personas.lock();
            personas
                .iter()
                .find(|p| p.id == "system/general")
                .and_then(|p| p.preferred_models.clone())
        };

        let authoring_guide =
            hive_workflow_service::hive_workflow::catalog::generate_authoring_guide();
        let yaml_section = if current_yaml.trim().is_empty() {
            "This is a new, empty workflow. Create a complete workflow definition from scratch based on the user's request.".to_string()
        } else {
            format!(
                "Here is the current workflow YAML that the user wants to modify:\n\n```yaml\n{current_yaml}\n```"
            )
        };

        // Pre-fetch connector context so the AI already knows what's configured
        let connector_context = {
            let connectors = self.connector_registry.list();
            if connectors.is_empty() {
                String::new()
            } else {
                let mut ctx = String::from(
                    "## Available Connectors\n\n\
                     The following connectors are already configured. Use their IDs directly in \
                     workflow steps — do NOT ask the user for connection details.\n\n",
                );
                for c in &connectors {
                    let _ = write!(
                        ctx,
                        "- **{}** (ID: `{}`): provider={:?}, status={:?}",
                        c.display_name(),
                        c.id(),
                        c.provider(),
                        c.status()
                    );
                    let caps: Vec<&str> = [
                        c.communication().is_some().then_some("communication"),
                        c.calendar().is_some().then_some("calendar"),
                        c.drive().is_some().then_some("drive"),
                        c.contacts().is_some().then_some("contacts"),
                    ]
                    .into_iter()
                    .flatten()
                    .collect();
                    if !caps.is_empty() {
                        let _ = write!(ctx, " [{}]", caps.join(", "));
                    }
                    ctx.push('\n');
                    // Include channel details for communication-capable connectors
                    if let Some(comm) = c.communication() {
                        if let Ok(channels) = tokio::time::timeout(
                            std::time::Duration::from_secs(5),
                            comm.list_channels(),
                        )
                        .await
                        {
                            if let Ok(channels) = channels {
                                for ch in channels.iter().take(20) {
                                    let _ = write!(ctx, "  - Channel: `{}` ({})", ch.id, ch.name);
                                    if let Some(ref t) = ch.channel_type {
                                        let _ = write!(ctx, " [{}]", t);
                                    }
                                    if let Some(ref g) = ch.group_name {
                                        let _ = write!(ctx, " in {}", g);
                                    }
                                    ctx.push('\n');
                                }
                                if channels.len() > 20 {
                                    let _ = writeln!(
                                        ctx,
                                        "  - ... and {} more channels",
                                        channels.len() - 20
                                    );
                                }
                            }
                        }
                    }
                }
                ctx
            }
        };

        let system_prompt = format!(
            "You are an expert workflow authoring assistant. Your job is to help users create and modify \
             workflow definitions in YAML format. You produce production-quality workflows with proper \
             error handling, typed variables, and clear step naming.\n\n\
             {authoring_guide}\n\n\
             {connector_context}\n\
             ## Current Workflow\n\n\
             {yaml_section}\n\n\
             ## Instructions\n\n\
             1. **Understand the intent**: Parse the user's request to identify the trigger event, \
                core processing logic, output/notification needs, and error handling requirements. \
                Think about the workflow graph structure before writing any YAML.\n\
             2. **Ask targeted questions** (via the `core.ask_user` tool): Only ask when there are genuine \
                ambiguities that affect workflow structure. Don't ask about things you can infer or default \
                sensibly. When calling `core.ask_user`, always provide a `choices` array when possible, and \
                set `allow_freeform` to true. Limit yourself to 1-2 essential questions max.\n\
             3. **Discover available resources**: Use discovery tools strategically:\n\
                - Use `workflow_author.list_available_tools` with a filter to find relevant tools \
                  (don't list all tools — filter by keyword)\n\
                - Use `workflow_author.get_tool_details` to get input/output schemas before using a tool\n\
                - Refer to the **Available Connectors** section above for already-configured connectors — \
                  use their IDs directly without asking the user for connection details. \
                  Call `workflow_author.list_connectors` only if you need to refresh this information.\n\
                - Check `workflow_author.list_personas` if agent steps are needed\n\
                - Check `workflow_author.list_event_topics` if event triggers/gates are needed\n\
             4. **Design the workflow graph**: Plan the step sequence. Consider:\n\
                - Parallel execution where steps are independent\n\
                - Error recovery with `on_error` on all external calls\n\
                - Human checkpoints (`feedback_gate`) for high-stakes actions\n\
                - Proper terminal nodes (`end_workflow`) on every execution path\n\
                - Using `invoke_agent` for reasoning tasks, `call_tool` for deterministic actions\n\
             5. **Generate complete, production-quality YAML**: Include:\n\
                - Descriptive step IDs (e.g., `fetch_customer_data`, not `step_1`)\n\
                - Typed variables with defaults in JSON Schema\n\
                - `on_error` with `retry` on external API calls\n\
                - `timeout_secs` on all `invoke_agent` steps\n\
                - Output mappings on steps whose results are used downstream\n\
                - Both a `schedule` trigger AND a `manual` trigger for scheduled workflows (for testing)\n\
             6. **Submit and summarize**: Call `workflow_author.submit_workflow` with the complete YAML. \
                If validation fails, read the error carefully, fix the issues, and resubmit **once**. \
                After a successful submit, generate 2-3 test cases covering: \
                (a) a happy-path scenario with typical inputs, \
                (b) an edge case (empty input, boundary values), and \
                (c) an error scenario if the workflow has error handling. \
                Then call `workflow_author.submit_tests` with the test cases.\n\
              7. **Run tests**: After submit_tests succeeds, call `workflow_author.run_tests` with the same \
                `definition_name` you used in submit_workflow. Review the results:\n\
                - If all tests pass: STOP and respond with a brief summary of the workflow and test results.\n\
                - If tests fail: analyze the failures, fix the workflow YAML (call submit_workflow again) \
                  or fix the test cases (call submit_tests again), then call run_tests again.\n\
                - Limit yourself to **2 fix-and-rerun cycles** maximum. If tests still fail after 2 attempts, \
                  STOP and present the remaining failures to the user for guidance.\n\n\
             **CRITICAL**: After `workflow_author.submit_workflow` returns `success: true`, generate test cases, \
             call `workflow_author.submit_tests`, then call `workflow_author.run_tests`. \
             If all tests pass, STOP immediately — respond with a brief summary and do nothing else. \
             Do NOT attempt to fix lint warnings by resubmitting — just mention them to the user. \
             Never call `submit_workflow` more than 3 times total.\n\n\
             Important: The YAML you submit must be a complete, valid workflow definition. \
             Always include all required fields (name, triggers, steps). \
             Use template expressions like `{{{{trigger.field}}}}` and `{{{{steps.id.outputs.field}}}}` \
             for dynamic values. Refer to the complete examples in the authoring guide for patterns to follow."
        );

        let agent_id = format!("wf-assist-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);

        let config = BotConfig {
            id: agent_id.clone(),
            friendly_name: "Workflow AI Assist".to_string(),
            description: "Interactive workflow authoring assistant".to_string(),
            avatar: Some("✨".to_string()),
            color: Some("#f59e0b".to_string()),
            model: preferred_models.as_ref().and_then(|v| v.first().cloned()),
            preferred_models,
            loop_strategy: Some(hive_contracts::LoopStrategy::React),
            tool_execution_mode: None,
            system_prompt,
            launch_prompt: user_prompt.to_string(),
            allowed_tools: {
                let mut tools: Vec<String> =
                    hive_tools::WORKFLOW_AUTHOR_TOOL_IDS.iter().map(|s| s.to_string()).collect();
                tools.push("core.ask_user".to_string());
                tools
            },
            data_class: DataClass::Internal,
            role: AgentRole::default(),
            mode: BotMode::IdleAfterTask,
            active: false,
            created_at: String::new(),
            timeout_secs: Some(600),
            permission_rules: vec![],
            tool_limits: Some(hive_contracts::ToolLimitsConfig {
                soft_limit: 20,
                hard_ceiling: 50,
                extension_chunk: 10,
                ..Default::default()
            }),
            persona_id: None,
            shadow_mode: false,
        };

        self.launch_bot(config).await?;
        Ok(agent_id)
    }

    pub async fn continue_workflow_ai_assist(
        &self,
        agent_id: &str,
        current_yaml: &str,
        user_prompt: &str,
    ) -> Result<(), ChatServiceError> {
        let yaml_section = if current_yaml.trim().is_empty() {
            String::new()
        } else {
            format!("## Current Workflow\n\n```yaml\n{current_yaml}\n```\n\n")
        };
        let message = format!("{yaml_section}## User Request\n\n{user_prompt}");
        self.message_bot(agent_id, message).await
    }

    pub async fn list_bots(&self) -> Vec<BotSummary> {
        let configs = self.bot_configs.read().await;
        let supervisor = self.bot_supervisor.read().await;
        let runtime_agents: HashMap<String, AgentSummary> = supervisor
            .as_ref()
            .map(|s| s.get_all_agents().into_iter().map(|a| (a.agent_id.clone(), a)).collect())
            .unwrap_or_default();

        configs
            .values()
            .map(|config| {
                let (status, last_error, active_model, tools) =
                    if let Some(runtime) = runtime_agents.get(&config.id) {
                        (
                            runtime.status.clone(),
                            runtime.last_error.clone(),
                            runtime.active_model.clone(),
                            runtime.tools.clone(),
                        )
                    } else if config.active {
                        (AgentStatus::Spawning, None, None, Vec::new())
                    } else {
                        (AgentStatus::Done, None, None, Vec::new())
                    };
                BotSummary { config: config.clone(), status, last_error, active_model, tools }
            })
            .collect()
    }

    pub async fn message_bot(
        &self,
        agent_id: &str,
        content: String,
    ) -> Result<(), ChatServiceError> {
        let supervisor = self.get_or_create_bot_supervisor().await?;
        supervisor
            .send_to_agent(agent_id, AgentMessage::Task { content, from: Some("user".to_string()) })
            .await
            .map_err(Self::map_agent_error)
    }

    pub async fn deactivate_bot(&self, agent_id: &str) -> Result<(), ChatServiceError> {
        let supervisor = self.get_or_create_bot_supervisor().await?;

        // Snapshot descendants BEFORE killing so we can clean up persisted
        // state even if broadcast events are missed.
        let doomed_ids = supervisor.get_descendant_ids(agent_id);

        supervisor.kill_agent(agent_id).await.map_err(Self::map_agent_error)?;

        // Remove any persisted "session_agent" KG nodes for the bot and its
        // children so pending interactions don't resurface as ghost approvals.
        let graph_path = Arc::clone(&self.knowledge_graph_path);
        let _ = tokio::task::spawn_blocking(move || -> Result<(), ChatServiceError> {
            let graph = open_graph(&graph_path)?;
            for id in &doomed_ids {
                let agent_name = format!("agent-{id}");
                if let Ok(Some(node)) =
                    graph.find_node_by_type_and_name("session_agent", &agent_name)
                {
                    let _ = graph.remove_node(node.id);
                }
            }
            Ok(())
        })
        .await;

        let to_persist = {
            let mut configs = self.bot_configs.write().await;
            if let Some(config) = configs.get_mut(agent_id) {
                config.active = false;
                Some(config.clone())
            } else {
                None
            }
        };
        if let Some(config) = to_persist {
            let _ = self.persist_bot_config(&config, false).await;
        }
        Ok(())
    }

    pub async fn activate_bot(&self, agent_id: &str) -> Result<(), ChatServiceError> {
        let config = {
            let mut configs = self.bot_configs.write().await;
            let config = configs.get_mut(agent_id).ok_or_else(|| {
                ChatServiceError::AgentNotFound { agent_id: agent_id.to_string() }
            })?;
            config.active = true;
            config.clone()
        };
        let _ = self.persist_bot_config(&config, false).await;

        let supervisor = self.get_or_create_bot_supervisor().await?;
        let spec = config.to_agent_spec();
        let agent_perms = if config.permission_rules.is_empty() {
            None
        } else {
            Some(Arc::new(Mutex::new(SessionPermissions::with_rules(
                config.permission_rules.clone(),
            ))))
        };
        let agent_workspace = self.bot_workspace.join(agent_id);
        let _ = std::fs::create_dir_all(&agent_workspace);
        supervisor
            .spawn_agent(spec, None, None, agent_perms, Some(agent_workspace))
            .await
            .map_err(Self::map_agent_error)?;

        if !config.launch_prompt.is_empty() {
            supervisor
                .send_to_agent(
                    agent_id,
                    AgentMessage::Task { content: config.launch_prompt.clone(), from: None },
                )
                .await
                .map_err(Self::map_agent_error)?;
        }
        Ok(())
    }

    pub async fn delete_bot(&self, agent_id: &str) -> Result<(), ChatServiceError> {
        // Remove from in-memory configs BEFORE killing the agent. This
        // prevents the bot supervisor bridge from re-persisting the config
        // when it receives the AgentStatusChanged { Done } event triggered
        // by the Kill signal.
        self.bot_configs.write().await.remove(agent_id);

        let supervisor = self.bot_supervisor.read().await;
        if let Some(sup) = supervisor.as_ref() {
            // Snapshot descendants before kill for KG cleanup.
            let doomed_ids = sup.get_descendant_ids(agent_id);
            let _ = sup.kill_agent(agent_id).await;

            // Remove persisted session_agent nodes for children so pending
            // interactions don't become ghost approvals.
            let graph_path = Arc::clone(&self.knowledge_graph_path);
            let _ = tokio::task::spawn_blocking(move || -> Result<(), ChatServiceError> {
                let graph = open_graph(&graph_path)?;
                for id in &doomed_ids {
                    let agent_name = format!("agent-{id}");
                    if let Ok(Some(node)) =
                        graph.find_node_by_type_and_name("session_agent", &agent_name)
                    {
                        let _ = graph.remove_node(node.id);
                    }
                }
                Ok(())
            })
            .await;
        }
        drop(supervisor);

        self.remove_persisted_bot(agent_id).await?;

        let agent_workspace = self.bot_workspace.join(agent_id);
        if agent_workspace.exists() {
            let _ = std::fs::remove_dir_all(&agent_workspace);
        }

        Ok(())
    }

    pub fn subscribe_bot_events(&self) -> broadcast::Receiver<SessionEvent> {
        self.bot_stream_tx.subscribe()
    }

    pub async fn bot_telemetry(&self) -> Result<hive_agents::TelemetrySnapshot, ChatServiceError> {
        Ok(self.get_or_create_bot_supervisor().await?.telemetry_snapshot())
    }

    pub async fn get_bot_events(
        &self,
        agent_id: &str,
    ) -> Result<Vec<SupervisorEvent>, ChatServiceError> {
        Ok(self.get_or_create_bot_supervisor().await?.get_agent_events(agent_id))
    }

    pub async fn get_bot_events_paged(
        &self,
        agent_id: &str,
        offset: usize,
        limit: usize,
    ) -> Result<(Vec<SupervisorEvent>, usize), ChatServiceError> {
        // Try in-memory first.
        let (events, total) = self
            .get_or_create_bot_supervisor()
            .await?
            .get_agent_events_paged(agent_id, offset, limit);
        if total > 0 {
            return Ok((events, total));
        }
        // Fall back to persisted JSONL files.
        if let Some(logger) = self.get_or_create_logger(agent_id).await {
            return Ok(logger.read_agent_events_paged(agent_id, offset, limit));
        }
        Ok((Vec::new(), 0))
    }

    pub async fn get_bot_permissions(
        &self,
        agent_id: &str,
    ) -> Result<SessionPermissions, ChatServiceError> {
        let sup = self.get_or_create_bot_supervisor().await?;
        Ok(sup.get_agent_permissions(agent_id).unwrap_or_default())
    }

    pub async fn set_bot_permissions(
        &self,
        agent_id: &str,
        permissions: SessionPermissions,
    ) -> Result<(), ChatServiceError> {
        let sup = self.get_or_create_bot_supervisor().await?;
        sup.set_agent_permissions(agent_id, permissions.clone());
        let to_persist = {
            let mut configs = self.bot_configs.write().await;
            if let Some(config) = configs.get_mut(agent_id) {
                config.permission_rules = permissions.rules;
                Some(config.clone())
            } else {
                None
            }
        };
        if let Some(config) = to_persist {
            let _ = self.persist_bot_config(&config, false).await;
        }
        Ok(())
    }

    pub async fn respond_to_bot_interaction(
        &self,
        agent_id: &str,
        response: hive_contracts::UserInteractionResponse,
    ) -> Result<bool, ChatServiceError> {
        let request_id = response.request_id.clone();
        let acknowledged = self
            .get_or_create_bot_supervisor()
            .await?
            .respond_to_agent_interaction(agent_id, response)
            .map_err(Self::map_agent_error)?;
        if acknowledged {
            let _ = self.approval_tx.send(ApprovalStreamEvent::Resolved { request_id });
        }
        Ok(acknowledged)
    }

    pub fn list_bot_workspace_files(
        &self,
        bot_id: &str,
        subdir: Option<&str>,
    ) -> Result<Vec<WorkspaceEntry>, ChatServiceError> {
        let workspace = self.bot_workspace.join(bot_id);
        if !workspace.exists() {
            return Ok(vec![]);
        }
        let target_dir = match subdir {
            Some(rel) => {
                let safe_rel = normalize_workspace_relative_path(rel)?;
                let full = workspace.join(&safe_rel);
                let canonical = full.canonicalize().map_err(|e| ChatServiceError::Internal {
                    detail: format!("directory not found: {e}"),
                })?;
                let canonical_ws = workspace
                    .canonicalize()
                    .map_err(|e| ChatServiceError::Internal { detail: e.to_string() })?;
                if !canonical.starts_with(&canonical_ws) {
                    return Err(ChatServiceError::Internal {
                        detail: "Path traversal not allowed".to_string(),
                    });
                }
                canonical
            }
            None => workspace.clone(),
        };
        Ok(list_workspace_dir(&workspace, &target_dir))
    }

    pub fn read_bot_workspace_file(
        &self,
        bot_id: &str,
        file_path: &str,
    ) -> Result<WorkspaceFileContent, ChatServiceError> {
        let workspace = self.bot_workspace.join(bot_id);
        let full_path = workspace.join(normalize_workspace_relative_path(file_path)?);

        let canonical_workspace = workspace
            .canonicalize()
            .map_err(|error| ChatServiceError::Internal { detail: error.to_string() })?;
        let canonical_file = full_path.canonicalize().map_err(|error| {
            ChatServiceError::Internal { detail: format!("file not found: {error}") }
        })?;
        if !canonical_file.starts_with(&canonical_workspace) {
            return Err(ChatServiceError::Internal {
                detail: "Path traversal not allowed".to_string(),
            });
        }

        read_workspace_file_at(&canonical_file, file_path)
    }

    pub async fn restore_bots(&self) -> Result<(), ChatServiceError> {
        let configs = self.load_bot_configs().await?;
        if configs.is_empty() {
            return Ok(());
        }

        tracing::info!(count = configs.len(), "restoring bots");

        let active_configs: Vec<_> = configs.into_iter().filter(|c| c.active).collect();
        let all_configs = self.load_bot_configs().await?;
        {
            let mut registry = self.bot_configs.write().await;
            for config in &all_configs {
                registry.insert(config.id.clone(), config.clone());
            }
        }

        if active_configs.is_empty() {
            return Ok(());
        }

        let supervisor = self.get_or_create_bot_supervisor().await?;

        for config in active_configs {
            let agent_id = config.id.clone();
            let spec = config.to_agent_spec();

            let agent_perms = if config.permission_rules.is_empty() {
                None
            } else {
                Some(Arc::new(Mutex::new(SessionPermissions::with_rules(
                    config.permission_rules.clone(),
                ))))
            };

            let agent_workspace = self.bot_workspace.join(&agent_id);
            let _ = std::fs::create_dir_all(&agent_workspace);

            match supervisor.spawn_agent(spec, None, None, agent_perms, Some(agent_workspace)).await
            {
                Ok(_) => {
                    if let Some(journal) = self.load_bot_journal(&agent_id).await {
                        if !journal.entries.is_empty() {
                            supervisor.set_agent_journal(&agent_id, journal);
                        }
                    }

                    if !config.launch_prompt.is_empty() {
                        if let Err(e) = supervisor
                            .send_to_agent(
                                &agent_id,
                                AgentMessage::Task {
                                    content: config.launch_prompt.clone(),
                                    from: None,
                                },
                            )
                            .await
                        {
                            tracing::warn!(
                                agent_id,
                                error = %e,
                                "failed to send launch prompt to bot"
                            );
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(agent_id, error = %e, "failed to restore bot");
                }
            }
        }

        Ok(())
    }

    // ── KG persistence ─────────────────────────────────────

    /// Persist a bot config to the knowledge graph.
    ///
    /// When `allow_insert` is `false`, the method only updates an existing KG
    /// node — it will NOT create a new one. This prevents the bot supervisor
    /// bridge (and other update-only callers) from accidentally re-inserting a
    /// config that was already deleted.
    pub(crate) async fn persist_bot_config(
        &self,
        config: &BotConfig,
        allow_insert: bool,
    ) -> Result<(), ChatServiceError> {
        let graph_path = Arc::clone(&self.knowledge_graph_path);
        let agent_id = config.id.clone();
        let content = serde_json::to_string(config).map_err(|e| ChatServiceError::Internal {
            detail: format!("failed to serialize bot config: {e}"),
        })?;
        tokio::task::spawn_blocking(move || {
            let graph = open_graph(&graph_path)?;
            let mut existing = graph
                .list_nodes(Some("bot"), Some(DataClass::Internal), 1000)
                .map_err(|e| ChatServiceError::KnowledgeGraphFailed {
                    operation: "persist_bot_config",
                    detail: e.to_string(),
                })?;
            existing.extend(
                graph.list_nodes(Some("service_agent"), Some(DataClass::Internal), 1000).map_err(
                    |e| ChatServiceError::KnowledgeGraphFailed {
                        operation: "persist_bot_config",
                        detail: e.to_string(),
                    },
                )?,
            );
            let existing_node = existing.iter().find(|n| {
                n.content
                    .as_deref()
                    .and_then(|c| serde_json::from_str::<BotConfig>(c).ok())
                    .map(|cfg| cfg.id == agent_id)
                    .unwrap_or(false)
            });

            if let Some(node) = existing_node {
                graph.update_node_content(node.id, &content).map_err(|e| {
                    ChatServiceError::KnowledgeGraphFailed {
                        operation: "persist_bot_config",
                        detail: e.to_string(),
                    }
                })?;
            } else if allow_insert {
                graph
                    .insert_node(&hive_knowledge::NewNode {
                        name: format!("bot:{agent_id}"),
                        node_type: "bot".to_string(),
                        content: Some(content),
                        data_class: DataClass::Internal,
                    })
                    .map_err(|e| ChatServiceError::KnowledgeGraphFailed {
                        operation: "persist_bot_config",
                        detail: e.to_string(),
                    })?;
            }
            Ok(())
        })
        .await
        .map_err(|e| ChatServiceError::KnowledgeGraphFailed {
            operation: "persist_bot_config",
            detail: e.to_string(),
        })?
    }

    pub(crate) async fn load_bot_configs(&self) -> Result<Vec<BotConfig>, ChatServiceError> {
        let graph_path = Arc::clone(&self.knowledge_graph_path);
        tokio::task::spawn_blocking(move || {
            let graph = open_graph(&graph_path)?;
            let mut nodes = graph
                .list_nodes(Some("bot"), Some(DataClass::Internal), 1000)
                .map_err(|e| ChatServiceError::KnowledgeGraphFailed {
                    operation: "load_bot_configs",
                    detail: e.to_string(),
                })?;
            nodes.extend(
                graph.list_nodes(Some("service_agent"), Some(DataClass::Internal), 1000).map_err(
                    |e| ChatServiceError::KnowledgeGraphFailed {
                        operation: "load_bot_configs",
                        detail: e.to_string(),
                    },
                )?,
            );
            let mut configs = Vec::new();
            for node in nodes {
                if let Some(content) = &node.content {
                    if let Ok(config) = serde_json::from_str::<BotConfig>(content) {
                        configs.push(config);
                    }
                }
            }
            Ok(configs)
        })
        .await
        .map_err(|e| ChatServiceError::KnowledgeGraphFailed {
            operation: "load_bot_configs",
            detail: e.to_string(),
        })?
    }

    pub(crate) async fn remove_persisted_bot(
        &self,
        agent_id: &str,
    ) -> Result<(), ChatServiceError> {
        let graph_path = Arc::clone(&self.knowledge_graph_path);
        let agent_id = agent_id.to_string();
        tokio::task::spawn_blocking(move || {
            let graph = open_graph(&graph_path)?;
            let mut nodes = graph
                .list_nodes(Some("bot"), Some(DataClass::Internal), 1000)
                .map_err(|e| ChatServiceError::KnowledgeGraphFailed {
                    operation: "remove_persisted_bot",
                    detail: e.to_string(),
                })?;
            nodes.extend(
                graph.list_nodes(Some("service_agent"), Some(DataClass::Internal), 1000).map_err(
                    |e| ChatServiceError::KnowledgeGraphFailed {
                        operation: "remove_persisted_bot",
                        detail: e.to_string(),
                    },
                )?,
            );
            for node in nodes {
                if let Some(content) = &node.content {
                    if let Ok(config) = serde_json::from_str::<BotConfig>(content) {
                        if config.id == agent_id {
                            graph.remove_node(node.id).map_err(|e| {
                                ChatServiceError::KnowledgeGraphFailed {
                                    operation: "remove_persisted_bot",
                                    detail: e.to_string(),
                                }
                            })?;
                            break;
                        }
                    }
                }
            }
            Ok(())
        })
        .await
        .map_err(|e| ChatServiceError::KnowledgeGraphFailed {
            operation: "remove_persisted_bot",
            detail: e.to_string(),
        })?
    }

    pub(crate) async fn update_persisted_bot_journal(
        &self,
        agent_id: &str,
        journal: &ConversationJournal,
    ) {
        let graph_path = Arc::clone(&self.knowledge_graph_path);
        let agent_id = agent_id.to_string();
        let journal_json = serde_json::to_string(journal).unwrap_or_default();
        let _ = tokio::task::spawn_blocking(move || -> Result<(), ChatServiceError> {
            let graph = open_graph(&graph_path)?;
            let mut nodes = graph
                .list_nodes(Some("bot"), Some(DataClass::Internal), 1000)
                .map_err(|e| ChatServiceError::KnowledgeGraphFailed {
                    operation: "update_bot_journal",
                    detail: e.to_string(),
                })?;
            nodes.extend(
                graph.list_nodes(Some("service_agent"), Some(DataClass::Internal), 1000).map_err(
                    |e| ChatServiceError::KnowledgeGraphFailed {
                        operation: "update_bot_journal",
                        detail: e.to_string(),
                    },
                )?,
            );
            for node in nodes {
                if let Some(content) = &node.content {
                    if let Ok(mut config) = serde_json::from_str::<serde_json::Value>(content) {
                        if config.get("id").and_then(|v| v.as_str()) == Some(&agent_id) {
                            config["journal"] = serde_json::Value::String(journal_json);
                            let updated = serde_json::to_string(&config).unwrap_or_default();
                            let _ = graph.update_node_content(node.id, &updated);
                            break;
                        }
                    }
                }
            }
            Ok(())
        })
        .await;
    }

    pub(crate) async fn load_bot_journal(&self, agent_id: &str) -> Option<ConversationJournal> {
        let graph_path = Arc::clone(&self.knowledge_graph_path);
        let agent_id = agent_id.to_string();
        tokio::task::spawn_blocking(move || -> Option<ConversationJournal> {
            let graph = open_graph(&graph_path).ok()?;
            let mut nodes = graph.list_nodes(Some("bot"), Some(DataClass::Internal), 1000).ok()?;
            nodes.extend(
                graph.list_nodes(Some("service_agent"), Some(DataClass::Internal), 1000).ok()?,
            );
            for node in nodes {
                if let Some(content) = &node.content {
                    if let Ok(value) = serde_json::from_str::<serde_json::Value>(content) {
                        if value.get("id").and_then(|v| v.as_str()) == Some(&agent_id) {
                            if let Some(journal_str) = value.get("journal").and_then(|v| v.as_str())
                            {
                                return serde_json::from_str(journal_str).ok();
                            }
                        }
                    }
                }
            }
            None
        })
        .await
        .ok()
        .flatten()
    }
}

// ── BotOrchestrator ────────────────────────────────────────

/// Orchestrator for bots — enables agent-to-agent communication within
/// the bot supervisor (list_agents, signal_agent, spawn_agent, etc.).
pub(crate) struct BotOrchestrator {
    bot_service: BotService,
}

impl BotOrchestrator {
    pub(crate) fn new(bot_service: BotService) -> Self {
        Self { bot_service }
    }
}

impl AgentOrchestrator for BotOrchestrator {
    fn spawn_agent(
        &self,
        persona: Persona,
        task: String,
        from: Option<String>,
        friendly_name: Option<String>,
        data_class: hive_classification::DataClass,
        parent_model: Option<hive_model::ModelSelection>,
        keep_alive: bool,
        workspace_path: Option<std::path::PathBuf>,
    ) -> BoxFuture<'_, Result<String, String>> {
        let bot_svc = self.bot_service.clone();
        Box::pin(async move {
            let supervisor =
                bot_svc.get_or_create_bot_supervisor().await.map_err(|error| error.to_string())?;
            let mut spec = agent_spec_from_persona(&persona);
            if let Some(name) = friendly_name {
                spec.friendly_name = name;
            }
            spec.data_class = data_class;
            spec.keep_alive = keep_alive;
            if spec.model.as_ref().is_none_or(|m| m.trim().is_empty()) {
                if let Some(selection) = parent_model {
                    spec.model = Some(format!("{}:{}", selection.provider_id, selection.model));
                }
            }
            let agent_id = supervisor
                .spawn_agent(spec, from.clone(), None, None, workspace_path)
                .await
                .map_err(|error| error.to_string())?;
            supervisor
                .send_to_agent(&agent_id, AgentMessage::Task { content: task, from })
                .await
                .map_err(|error| error.to_string())?;
            Ok(agent_id)
        })
    }

    fn message_agent(
        &self,
        agent_id: String,
        message: String,
        from: String,
    ) -> BoxFuture<'_, Result<(), String>> {
        let bot_svc = self.bot_service.clone();
        Box::pin(async move {
            let supervisor =
                bot_svc.get_or_create_bot_supervisor().await.map_err(|error| error.to_string())?;
            supervisor
                .send_to_agent(&agent_id, AgentMessage::Task { content: message, from: Some(from) })
                .await
                .map_err(|error| error.to_string())
        })
    }

    fn message_session(
        &self,
        _message: String,
        _from_agent_id: String,
    ) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }

    fn list_agents(
        &self,
    ) -> BoxFuture<'_, Result<Vec<(String, String, String, String, Option<String>)>, String>> {
        let bot_svc = self.bot_service.clone();
        Box::pin(async move {
            let supervisor =
                bot_svc.get_or_create_bot_supervisor().await.map_err(|error| error.to_string())?;
            let agents = supervisor
                .get_all_agents()
                .into_iter()
                .map(|a| {
                    (
                        a.agent_id,
                        a.spec.friendly_name,
                        a.spec.description,
                        format!("{:?}", a.status),
                        a.final_result,
                    )
                })
                .collect();
            Ok(agents)
        })
    }

    fn get_agent_result(
        &self,
        agent_id: String,
    ) -> BoxFuture<'_, Result<(String, Option<String>), String>> {
        let bot_svc = self.bot_service.clone();
        Box::pin(async move {
            let supervisor =
                bot_svc.get_or_create_bot_supervisor().await.map_err(|error| error.to_string())?;
            let agents = supervisor.get_all_agents();
            let agent = agents
                .iter()
                .find(|a| a.agent_id == agent_id)
                .ok_or_else(|| format!("agent '{agent_id}' not found"))?;
            Ok((format!("{:?}", agent.status), agent.final_result.clone()))
        })
    }

    fn kill_agent(&self, agent_id: String) -> BoxFuture<'_, Result<(), String>> {
        let bot_svc = self.bot_service.clone();
        Box::pin(async move {
            let supervisor =
                bot_svc.get_or_create_bot_supervisor().await.map_err(|error| error.to_string())?;
            supervisor.kill_agent(&agent_id).await.map_err(|error| error.to_string())
        })
    }

    fn feedback_agent(
        &self,
        agent_id: String,
        message: String,
        from: String,
    ) -> BoxFuture<'_, Result<(), String>> {
        let bot_svc = self.bot_service.clone();
        Box::pin(async move {
            let supervisor =
                bot_svc.get_or_create_bot_supervisor().await.map_err(|error| error.to_string())?;
            supervisor
                .send_to_agent(&agent_id, AgentMessage::Feedback { content: message, from })
                .await
                .map_err(|error| error.to_string())
        })
    }

    fn wait_for_agent(
        &self,
        agent_id: String,
        timeout_secs: Option<u64>,
    ) -> BoxFuture<'_, Result<(String, Option<String>), String>> {
        let bot_svc = self.bot_service.clone();
        let timeout = std::time::Duration::from_secs(timeout_secs.unwrap_or(300));
        Box::pin(async move {
            let supervisor =
                bot_svc.get_or_create_bot_supervisor().await.map_err(|error| error.to_string())?;
            supervisor.wait_for_agent(&agent_id, timeout).await.map_err(|error| error.to_string())
        })
    }

    fn search_bots(
        &self,
        query: String,
    ) -> BoxFuture<'_, Result<Vec<(String, String, String)>, String>> {
        let bot_svc = self.bot_service.clone();
        Box::pin(async move {
            let configs = bot_svc.bot_configs.read().await;
            let keywords: Vec<String> =
                query.split_whitespace().map(|s| s.to_lowercase()).collect();
            let results: Vec<(String, String, String)> = configs
                .values()
                .filter(|cfg| cfg.active)
                .filter(|cfg| {
                    let name_lower = cfg.friendly_name.to_lowercase();
                    let desc_lower = cfg.description.to_lowercase();
                    keywords.iter().any(|kw| name_lower.contains(kw) || desc_lower.contains(kw))
                })
                .map(|cfg| (cfg.id.clone(), cfg.friendly_name.clone(), cfg.description.clone()))
                .collect();
            Ok(results)
        })
    }

    fn get_agent_parent(&self, agent_id: String) -> BoxFuture<'_, Result<Option<String>, String>> {
        let bot_svc = self.bot_service.clone();
        Box::pin(async move {
            let supervisor =
                bot_svc.get_or_create_bot_supervisor().await.map_err(|error| error.to_string())?;
            supervisor
                .get_agent_parent_id(&agent_id)
                .ok_or_else(|| format!("agent '{agent_id}' not found"))
        })
    }
}

// Re-use the SessionKnowledgeQueryHandler from chat.rs
use crate::chat::SessionKnowledgeQueryHandler;
