//! Implementations of the workflow service's extension traits
//! (`WorkflowToolExecutor`, `WorkflowAgentRunner`, `WorkflowInteractionGate`,
//! `WorkflowTaskScheduler`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use serde_json::{json, Value};

use hive_agents::{
    generate_friendly_name, generate_random_avatar, AgentMessage, AgentRole,
    SupervisorEvent,
};
use hive_chat::ChatService;
use hive_core::EventBus;
use hive_mcp::{McpCatalogStore, McpService, SessionMcpManager};
use hive_model::sanitize_tool_name;
use hive_scheduler::SchedulerService;
use hive_tools::ToolRegistry;
use hive_workflow::types::{PermissionEntry, ScheduleTaskDef, SignalTarget, WorkflowAttachment};
use hive_workflow_service::{
    AgentInteraction, InterceptedToolCall, WorkflowAgentRunner, WorkflowInteractionGate,
    WorkflowTaskScheduler, WorkflowToolExecutor,
};

// ──────────────────────────────────────────────────────────────────────
// Tool Executor
// ──────────────────────────────────────────────────────────────────────

pub(crate) struct WorkflowToolExecutorImpl {
    registry: parking_lot::RwLock<ToolRegistry>,
    reverse_map: parking_lot::RwLock<HashMap<String, String>>,
    connector_registry: Option<Arc<hive_connectors::ConnectorRegistry>>,
    connector_audit_log: Option<Arc<hive_connectors::ConnectorAuditLog>>,
    #[allow(dead_code)]
    mcp: Arc<McpService>,
    mcp_catalog: McpCatalogStore,
    session_mcp: Arc<SessionMcpManager>,
}

impl WorkflowToolExecutorImpl {
    pub fn new(
        connector_registry: Option<Arc<hive_connectors::ConnectorRegistry>>,
        connector_audit_log: Option<Arc<hive_connectors::ConnectorAuditLog>>,
        mcp: Arc<McpService>,
        mcp_catalog: McpCatalogStore,
        event_bus: EventBus,
    ) -> Self {
        let mut session_mcp = SessionMcpManager::from_configs(
            "__workflow__".to_string(),
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
    fn resolve_tool_id(&self, tool_id: &str) -> Option<String> {
        // Try direct lookup first.
        {
            let reg = self.registry.read();
            if reg.get(tool_id).is_some() {
                return Some(tool_id.to_string());
            }
        }
        // Try reverse map (sanitized → canonical).
        let sanitized = sanitize_tool_name(tool_id);
        let rmap = self.reverse_map.read();
        rmap.get(&sanitized).cloned()
    }
}

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
        let _ = registry.register(Arc::new(hive_tools::ListConnectorsTool::new(
            Arc::clone(cr),
            persona_id.unwrap_or("system/general").to_string(),
        )));
        let _ = registry.register(Arc::new(hive_tools::CommListChannelsTool::new(Arc::clone(cr))));
        let _ = registry.register(Arc::new(hive_tools::CommSendMessageTool::new(Arc::clone(cr))));
        let _ = registry.register(Arc::new(hive_tools::CommReadMessagesTool::new(Arc::clone(cr))));
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
        let _ = registry.register(Arc::new(hive_tools::DriveListFilesTool::new(Arc::clone(cr))));
        let _ = registry.register(Arc::new(hive_tools::DriveReadFileTool::new(Arc::clone(cr))));
        let _ = registry.register(Arc::new(hive_tools::DriveSearchFilesTool::new(Arc::clone(cr))));
        let _ = registry.register(Arc::new(hive_tools::DriveUploadFileTool::new(Arc::clone(cr))));
        let _ = registry.register(Arc::new(hive_tools::DriveShareFileTool::new(Arc::clone(cr))));
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
impl WorkflowToolExecutor for WorkflowToolExecutorImpl {
    async fn execute_tool(
        &self,
        tool_id: &str,
        arguments: Value,
        _permissions: &[PermissionEntry],
    ) -> Result<Value, String> {
        let canonical = self.resolve_tool_id(tool_id).unwrap_or_else(|| tool_id.to_string());
        let tool = {
            let reg = self.registry.read();
            reg.get(&canonical).ok_or_else(|| format!("tool not found: {tool_id}"))?
        };
        let result = tool.execute(arguments).await.map_err(|e| e.to_string())?;
        Ok(result.output)
    }
}

// ──────────────────────────────────────────────────────────────────────
// Agent Runner
// ──────────────────────────────────────────────────────────────────────

pub(crate) struct WorkflowAgentRunnerImpl {
    chat: OnceLock<Arc<ChatService>>,
    entity_graph: Arc<parking_lot::Mutex<Option<Arc<hive_core::EntityGraph>>>>,
    _personas_dir: PathBuf,
}

impl WorkflowAgentRunnerImpl {
    pub fn new(personas_dir: PathBuf) -> Self {
        Self {
            chat: OnceLock::new(),
            entity_graph: Arc::new(parking_lot::Mutex::new(None)),
            _personas_dir: personas_dir,
        }
    }

    pub fn set_chat(&self, chat: Arc<ChatService>) {
        let _ = self.chat.set(chat);
    }

    pub fn set_entity_graph(&self, graph: Arc<hive_core::EntityGraph>) {
        *self.entity_graph.lock() = Some(graph);
    }
}

#[async_trait]
impl WorkflowAgentRunner for WorkflowAgentRunnerImpl {
    async fn spawn_agent(
        &self,
        persona_id: &str,
        task: &str,
        timeout_secs: Option<u64>,
        workspace_path: Option<&str>,
        permissions: &[PermissionEntry],
        attachments: &[WorkflowAttachment],
        attachments_dir: Option<&str>,
        session_id: Option<&str>,
        agent_name: Option<&str>,
        shadow_mode: bool,
    ) -> Result<String, String> {
        let chat = self.chat.get().ok_or("workflow agent runner: ChatService not initialised")?;

        // Resolve persona to get full config (model, loop_strategy, tools, etc.)
        let persona = chat.resolve_persona(Some(persona_id));

        let avatar = if persona.id == "system/general" {
            Some(generate_random_avatar())
        } else {
            persona.avatar.clone()
        };

        // Convert workflow PermissionEntry → agent PermissionRule
        let mut permission_rules: Vec<hive_contracts::PermissionRule> = permissions
            .iter()
            .map(|pe| hive_contracts::PermissionRule {
                tool_pattern: pe.tool_id.clone(),
                scope: pe.resource.clone().unwrap_or_else(|| "*".to_string()),
                decision: match pe.approval {
                    hive_workflow::types::ToolApprovalLevel::Auto => {
                        hive_contracts::ToolApproval::Auto
                    }
                    hive_workflow::types::ToolApprovalLevel::Ask => {
                        hive_contracts::ToolApproval::Ask
                    }
                    hive_workflow::types::ToolApprovalLevel::Deny => {
                        hive_contracts::ToolApproval::Deny
                    }
                },
            })
            .collect();

        // Grant the agent read access to the workflow attachments directory
        if let Some(att_dir) = attachments_dir {
            if !attachments.is_empty() {
                permission_rules.push(hive_contracts::PermissionRule {
                    tool_pattern: "filesystem.*".to_string(),
                    scope: format!("{att_dir}/**"),
                    decision: hive_contracts::ToolApproval::Auto,
                });
            }
        }

        // Build the system prompt, augmenting with attachment info if present
        let mut system_prompt = persona.system_prompt.clone();
        if !attachments.is_empty() {
            if let Some(att_dir) = attachments_dir {
                system_prompt.push_str("\n\nYou have access to the following reference files:\n");
                for att in attachments {
                    let path = format!("{}/{}_{}", att_dir, att.id, att.filename);
                    system_prompt.push_str(&format!("- `{}`: {}\n", path, att.description));
                }
                system_prompt
                    .push_str("\nYou can read these files using the filesystem.read tool.\n");
            }
        }

        // When a session_id is provided, register the agent on the per-session
        // supervisor so its events (questions, approvals, status) flow through
        // the session's SSE stream — critical for chat-based workflows.
        if let Some(sid) = session_id {
            let suffix = &uuid::Uuid::new_v4().simple().to_string()[..8];
            let agent_id = format!("bot-{suffix}");

            let agent_perms = if permission_rules.is_empty() {
                None
            } else {
                Some(Arc::new(parking_lot::Mutex::new(
                    hive_contracts::SessionPermissions::with_rules(permission_rules),
                )))
            };

            let display_name =
                agent_name.map(|s| s.to_string()).unwrap_or_else(generate_friendly_name);

            let spec = hive_agents::AgentSpec {
                id: agent_id.clone(),
                name: display_name.clone(),
                friendly_name: display_name,
                description: persona.description.clone(),
                role: AgentRole::Custom(persona.id.clone()),
                model: persona.preferred_models.as_ref().and_then(|v| v.first().cloned()),
                preferred_models: persona.preferred_models.clone(),
                loop_strategy: Some(persona.loop_strategy.clone()),
                tool_execution_mode: Some(persona.tool_execution_mode),
                system_prompt,
                allowed_tools: persona.allowed_tools.clone(),
                avatar,
                color: persona.color.clone(),
                data_class: hive_classification::DataClass::Public,
                keep_alive: true,
                idle_timeout_secs: None,
                tool_limits: None,
                persona_id: Some(persona.id.clone()),
                workflow_managed: true,
                shadow_mode,
            };

            let supervisor = chat
                .get_or_create_supervisor(sid)
                .await
                .map_err(|e| format!("get session supervisor: {e}"))?;

            let ws_override = workspace_path.map(std::path::PathBuf::from);

            supervisor
                .spawn_agent(spec, None, Some(sid.to_string()), agent_perms, ws_override)
                .await
                .map_err(|e| format!("spawn agent on session supervisor: {e}"))?;

            // NOTE: Entity graph registration for workflow agents is handled by
            // the caller (ServiceStepExecutor) which registers agent → workflow.
            // Do NOT register agent → session here — it would be overwritten
            // for async agents but would stick for sync agents, breaking badge
            // propagation to workflow entities.

            if !task.is_empty() {
                supervisor
                    .send_to_agent(
                        &agent_id,
                        AgentMessage::Task { content: task.to_string(), from: None },
                    )
                    .await
                    .map_err(|e| format!("send task to agent: {e}"))?;
            }

            return Ok(agent_id);
        }

        // Fallback: no session_id → use the bot supervisor (background workflows)
        //
        // When a workflow workspace is provided, spawn directly on the bot
        // supervisor with the workspace override so the agent operates in the
        // workflow's workspace rather than a per-bot directory.
        if let Some(wp) = workspace_path {
            let supervisor = chat
                .get_or_create_bot_supervisor()
                .await
                .map_err(|e| format!("get bot supervisor: {e}"))?;

            let suffix = &uuid::Uuid::new_v4().simple().to_string()[..8];
            let agent_id = format!("bot-{suffix}");

            let agent_perms = if permission_rules.is_empty() {
                None
            } else {
                Some(Arc::new(parking_lot::Mutex::new(
                    hive_contracts::SessionPermissions::with_rules(permission_rules),
                )))
            };

            let display_name =
                agent_name.map(|s| s.to_string()).unwrap_or_else(generate_friendly_name);

            let spec = hive_agents::AgentSpec {
                id: agent_id.clone(),
                name: display_name.clone(),
                friendly_name: display_name,
                description: persona.description.clone(),
                role: AgentRole::Custom(persona.id.clone()),
                model: persona.preferred_models.as_ref().and_then(|v| v.first().cloned()),
                preferred_models: persona.preferred_models.clone(),
                loop_strategy: Some(persona.loop_strategy.clone()),
                tool_execution_mode: Some(persona.tool_execution_mode),
                system_prompt,
                allowed_tools: persona.allowed_tools.clone(),
                avatar,
                color: persona.color.clone(),
                data_class: hive_classification::DataClass::Public,
                keep_alive: true,
                idle_timeout_secs: None,
                tool_limits: None,
                persona_id: Some(persona.id.clone()),
                workflow_managed: true,
                shadow_mode,
            };

            let ws_override = std::path::PathBuf::from(wp);
            supervisor
                .spawn_agent(spec, None, None, agent_perms, Some(ws_override))
                .await
                .map_err(|e| format!("spawn agent on bot supervisor: {e}"))?;

            if !task.is_empty() {
                supervisor
                    .send_to_agent(
                        &agent_id,
                        AgentMessage::Task { content: task.to_string(), from: None },
                    )
                    .await
                    .map_err(|e| format!("send task to agent: {e}"))?;
            }

            return Ok(agent_id);
        }

        // No workspace and no session → launch as a standalone bot with its
        // own per-bot workspace directory.
        // Use the bot supervisor directly (like the workspace path) so we
        // can pass shadow_mode correctly. BotConfig.to_agent_spec() does
        // not support shadow_mode.
        let supervisor = chat
            .get_or_create_bot_supervisor()
            .await
            .map_err(|e| format!("get bot supervisor: {e}"))?;

        let suffix = &uuid::Uuid::new_v4().simple().to_string()[..8];
        let agent_id = format!("bot-{suffix}");

        let agent_perms = if permission_rules.is_empty() {
            None
        } else {
            Some(Arc::new(parking_lot::Mutex::new(
                hive_contracts::SessionPermissions::with_rules(permission_rules),
            )))
        };

        let display_name =
            agent_name.map(|s| s.to_string()).unwrap_or_else(generate_friendly_name);

        let spec = hive_agents::AgentSpec {
            id: agent_id.clone(),
            name: display_name.clone(),
            friendly_name: display_name,
            description: persona.description.clone(),
            role: AgentRole::Custom(persona.id.clone()),
            model: persona.preferred_models.as_ref().and_then(|v| v.first().cloned()),
            preferred_models: persona.preferred_models.clone(),
            loop_strategy: Some(persona.loop_strategy.clone()),
            tool_execution_mode: Some(persona.tool_execution_mode),
            system_prompt,
            allowed_tools: persona.allowed_tools.clone(),
            avatar,
            color: persona.color.clone(),
            data_class: hive_classification::DataClass::Public,
            keep_alive: false,
            idle_timeout_secs: None,
            tool_limits: None,
            persona_id: Some(persona.id.clone()),
            workflow_managed: true,
            shadow_mode,
        };

        // Create a per-agent workspace under the bots directory.
        let agent_workspace = chat.bot_workspace().join(&agent_id);
        let _ = std::fs::create_dir_all(&agent_workspace);

        supervisor
            .spawn_agent(spec, None, None, agent_perms, Some(agent_workspace))
            .await
            .map_err(|e| format!("spawn agent on bot supervisor: {e}"))?;

        if !task.is_empty() {
            supervisor
                .send_to_agent(
                    &agent_id,
                    AgentMessage::Task { content: task.to_string(), from: None },
                )
                .await
                .map_err(|e| format!("send task to agent: {e}"))?;
        }

        Ok(agent_id)
    }

    async fn wait_for_agent(
        &self,
        agent_id: &str,
        timeout_secs: Option<u64>,
        session_id: Option<&str>,
    ) -> Result<Value, String> {
        let chat = self.chat.get().ok_or("workflow agent runner: ChatService not initialised")?;

        let supervisor = if let Some(sid) = session_id {
            chat.get_or_create_supervisor(sid)
                .await
                .map_err(|e| format!("get session supervisor: {e}"))?
        } else {
            chat.get_or_create_bot_supervisor()
                .await
                .map_err(|e| format!("get bot supervisor: {e}"))?
        };
        let mut rx = supervisor.subscribe();

        // Check if the agent has already completed before we subscribed
        if let Some(status) = supervisor.get_agent_status(agent_id) {
            if status == hive_agents::AgentStatus::Done || status == hive_agents::AgentStatus::Error
            {
                return Ok(json!({
                    "agent_id": agent_id,
                    "result": null,
                    "status": "completed",
                }));
            }
        }

        let deadline = timeout_secs
            .map(|secs| tokio::time::Instant::now() + tokio::time::Duration::from_secs(secs));

        loop {
            let event = if let Some(dl) = deadline {
                match tokio::time::timeout_at(dl, rx.recv()).await {
                    Ok(ev) => ev,
                    Err(_) => {
                        return Err(format!("agent timed out after {}s", timeout_secs.unwrap()));
                    }
                }
            } else {
                rx.recv().await
            };
            match event {
                Ok(SupervisorEvent::AgentCompleted { agent_id: ref completed_id, ref result })
                    if *completed_id == agent_id =>
                {
                    return Ok(json!({
                        "agent_id": agent_id,
                        "result": result,
                        "status": "completed",
                    }));
                }
                Ok(_) => continue,
                Err(_) => return Err("supervisor event channel closed".to_string()),
            }
        }
    }

    async fn spawn_and_wait_agent(
        &self,
        persona_id: &str,
        task: &str,
        timeout_secs: Option<u64>,
        workspace_path: Option<&str>,
        permissions: &[PermissionEntry],
        attachments: &[WorkflowAttachment],
        attachments_dir: Option<&str>,
        session_id: Option<&str>,
        on_spawned: Option<Box<dyn FnOnce(String) + Send + Sync>>,
        agent_name: Option<&str>,
        shadow_mode: bool,
        auto_respond: bool,
    ) -> Result<(String, Value, Vec<InterceptedToolCall>, Vec<AgentInteraction>), String> {
        let chat = self.chat.get().ok_or("workflow agent runner: ChatService not initialised")?;

        // Subscribe to supervisor events BEFORE spawning the agent
        // to avoid missing AgentCompleted if the agent finishes quickly.
        let supervisor = if let Some(sid) = session_id {
            chat.get_or_create_supervisor(sid)
                .await
                .map_err(|e| format!("get session supervisor: {e}"))?
        } else {
            chat.get_or_create_bot_supervisor()
                .await
                .map_err(|e| format!("get bot supervisor: {e}"))?
        };
        let mut rx = supervisor.subscribe();

        // Now spawn the agent
        let agent_id = self
            .spawn_agent(
                persona_id,
                task,
                timeout_secs,
                workspace_path,
                permissions,
                attachments,
                attachments_dir,
                session_id,
                agent_name,
                shadow_mode,
            )
            .await?;

        // Notify the caller of the agent_id so they can persist the mapping
        // before we start waiting (which may block for a long time).
        if let Some(cb) = on_spawned {
            cb(agent_id.clone());
        }

        // Wait for completion using the pre-subscribed receiver.
        // In test/auto_respond mode, apply a safety-net deadline even if
        // the step definition didn't specify a timeout.
        let effective_timeout = timeout_secs.or_else(|| if auto_respond { Some(120) } else { None });
        let deadline = effective_timeout
            .map(|secs| tokio::time::Instant::now() + tokio::time::Duration::from_secs(secs));

        let mut intercepted_calls: Vec<InterceptedToolCall> = Vec::new();
        let mut agent_interactions: Vec<AgentInteraction> = Vec::new();

        tracing::info!(
            %agent_id,
            auto_respond,
            effective_timeout_secs = ?effective_timeout,
            "spawn_and_wait_agent: entering event loop"
        );

        loop {
            let event = if let Some(dl) = deadline {
                match tokio::time::timeout_at(dl, rx.recv()).await {
                    Ok(ev) => ev,
                    Err(_) => {
                        return Err(format!(
                            "agent timed out after {}s",
                            effective_timeout.unwrap()
                        ));
                    }
                }
            } else {
                rx.recv().await
            };
            match event {
                Ok(SupervisorEvent::AgentCompleted { agent_id: ref completed_id, ref result })
                    if *completed_id == agent_id =>
                {
                    tracing::info!(
                        %agent_id,
                        "spawn_and_wait_agent: agent completed"
                    );
                    let val = json!({
                        "agent_id": &agent_id,
                        "result": result,
                        "status": "completed",
                    });
                    return Ok((agent_id, val, intercepted_calls, agent_interactions));
                }
                // Capture shadow-mode tool interceptions from the agent
                Ok(SupervisorEvent::AgentOutput {
                    agent_id: ref aid,
                    event: hive_contracts::ReasoningEvent::ToolCallIntercepted {
                        ref tool_id,
                        ref input,
                    },
                }) if *aid == agent_id => {
                    intercepted_calls.push(InterceptedToolCall {
                        tool_id: tool_id.clone(),
                        input: input.clone(),
                    });
                    continue;
                }
                // Auto-respond to agent questions (ask_user) when in test mode.
                Ok(SupervisorEvent::AgentOutput {
                    agent_id: ref aid,
                    event: hive_contracts::ReasoningEvent::QuestionAsked {
                        ref request_id,
                        ref text,
                        ref choices,
                        ref allow_freeform,
                        ..
                    },
                }) if *aid == agent_id && auto_respond => {
                    let auto_answer = if choices.is_empty() {
                        "proceed".to_string()
                    } else {
                        choices[0].clone()
                    };
                    tracing::info!(
                        agent_id = %aid,
                        request_id = %request_id,
                        answer = %auto_answer,
                        "auto-responding to agent question in test mode"
                    );
                    agent_interactions.push(AgentInteraction {
                        kind: "ask_user".to_string(),
                        details: json!({
                            "question": text,
                            "choices": choices,
                            "allow_freeform": allow_freeform,
                            "auto_response": auto_answer,
                        }),
                    });
                    let payload = if choices.is_empty() {
                        hive_contracts::InteractionResponsePayload::Answer {
                            selected_choice: None,
                            selected_choices: None,
                            text: Some("proceed".to_string()),
                        }
                    } else {
                        hive_contracts::InteractionResponsePayload::Answer {
                            selected_choice: Some(0),
                            selected_choices: None,
                            text: None,
                        }
                    };
                    let response = hive_contracts::UserInteractionResponse {
                        request_id: request_id.clone(),
                        payload,
                    };
                    if let Err(e) = supervisor.respond_to_agent_interaction(aid, response) {
                        tracing::warn!(
                            agent_id = %aid,
                            "failed to auto-respond to question: {e}"
                        );
                    }
                    continue;
                }
                // Auto-approve tool approvals when in test mode.
                Ok(SupervisorEvent::AgentOutput {
                    agent_id: ref aid,
                    event: hive_contracts::ReasoningEvent::UserInteractionRequired {
                        ref request_id,
                        ref tool_id,
                        ref input,
                        ref reason,
                    },
                }) if *aid == agent_id && auto_respond => {
                    tracing::info!(
                        agent_id = %aid,
                        request_id = %request_id,
                        tool_id = %tool_id,
                        "auto-approving tool in test mode"
                    );
                    agent_interactions.push(AgentInteraction {
                        kind: "tool_approval".to_string(),
                        details: json!({
                            "tool_id": tool_id,
                            "input": input,
                            "reason": reason,
                            "auto_approved": true,
                        }),
                    });
                    let response = hive_contracts::UserInteractionResponse {
                        request_id: request_id.clone(),
                        payload: hive_contracts::InteractionResponsePayload::ToolApproval {
                            approved: true,
                            allow_session: false,
                            allow_agent: false,
                        },
                    };
                    if let Err(e) = supervisor.respond_to_agent_interaction(aid, response) {
                        tracing::warn!(
                            agent_id = %aid,
                            "failed to auto-approve tool: {e}"
                        );
                    }
                    continue;
                }
                Ok(ref other) => {
                    tracing::trace!(
                        %agent_id,
                        event_type = std::any::type_name_of_val(other),
                        "spawn_and_wait_agent: ignoring unmatched event"
                    );
                    continue;
                }
                Err(ref e) => {
                    tracing::error!(
                        %agent_id,
                        error = %e,
                        "spawn_and_wait_agent: supervisor event channel error"
                    );
                    return Err(format!("supervisor event channel error: {e}"));
                }
            }
        }
    }

    async fn signal_agent(&self, target: &SignalTarget, content: &str) -> Result<Value, String> {
        let chat = self.chat.get().ok_or("workflow agent runner: ChatService not initialised")?;

        match target {
            SignalTarget::Session { session_id } => {
                use hive_chat::SendMessageRequest;
                chat.enqueue_message(
                    session_id,
                    SendMessageRequest {
                        content: content.to_string(),
                        scan_decision: None,
                        preferred_models: None,
                        data_class_override: None,
                        agent_id: None,
                        role: Default::default(),
                        canvas_position: None,
                        excluded_tools: None,
                        excluded_skills: None,
                        attachments: vec![],
                        skip_preempt: Some(true),
                    },
                )
                .await
                .map_err(|e| format!("send to session: {e}"))?;
                Ok(json!({ "sent": true, "target": "session", "session_id": session_id }))
            }
            SignalTarget::Agent { agent_id } => {
                // Find the session containing this agent and route through
                // the session's supervisor.
                let sessions = chat.list_sessions().await;
                for session in &sessions {
                    if let Ok(supervisor) = chat.get_or_create_supervisor(&session.id).await {
                        let agents = supervisor.get_all_agents();
                        if agents.iter().any(|a| a.agent_id == *agent_id) {
                            let handle = supervisor.send_handle();
                            handle
                                .send_to_agent(
                                    agent_id,
                                    AgentMessage::Task {
                                        content: content.to_string(),
                                        from: Some("workflow".to_string()),
                                    },
                                )
                                .await
                                .map_err(|e| format!("send to agent: {e}"))?;
                            return Ok(json!({
                                "sent": true,
                                "target": "agent",
                                "agent_id": agent_id,
                                "session_id": session.id,
                            }));
                        }
                    }
                }
                Err(format!("agent not found in any session: {agent_id}"))
            }
        }
    }

    async fn inject_session_notification(
        &self,
        session_id: &str,
        source_name: &str,
        message: &str,
    ) -> Result<(), String> {
        let chat = self.chat.get().ok_or("workflow agent runner: ChatService not initialised")?;
        chat.append_notification(session_id, source_name, message).await;
        Ok(())
    }

    async fn inject_session_question(
        &self,
        session_id: &str,
        request_id: &str,
        prompt: &str,
        choices: &[String],
        allow_freeform: bool,
        workflow_instance_id: i64,
        workflow_step_id: &str,
        workflow_name: &str,
    ) -> Result<(), String> {
        let chat = self.chat.get().ok_or("workflow agent runner: ChatService not initialised")?;
        chat.insert_question_message(
            session_id,
            &format!("workflow:{workflow_instance_id}"),
            &format!("Workflow: {workflow_name}"),
            request_id,
            prompt,
            choices,
            allow_freeform,
            false, // multi_select
            None,  // message_content
            Some(workflow_instance_id),
            Some(workflow_step_id),
        )
        .await;
        Ok(())
    }

    async fn kill_agent(&self, session_id: &str, agent_id: &str) -> Result<(), String> {
        let chat = self.chat.get().ok_or("workflow agent runner: ChatService not initialised")?;
        // Trigger-launched workflows use a sentinel parent_session_id (e.g.
        // "trigger-manager") that is not a real chat session. Their agents
        // live on the bot supervisor instead.
        let supervisor = if session_id.starts_with("trigger-") || session_id == "manual" {
            chat.get_or_create_bot_supervisor()
                .await
                .map_err(|e| format!("failed to get bot supervisor: {e}"))?
        } else {
            chat.get_or_create_supervisor(session_id)
                .await
                .map_err(|e| format!("failed to get supervisor: {e}"))?
        };
        supervisor.kill_agent(agent_id).await.map_err(|e| format!("failed to kill agent: {e}"))
    }

    async fn mark_session_question_answered(
        &self,
        session_id: &str,
        request_id: &str,
        answer_text: &str,
    ) -> Result<(), String> {
        let chat = self.chat.get().ok_or("workflow agent runner: ChatService not initialised")?;
        chat.mark_question_message_answered(session_id, request_id, answer_text).await;
        Ok(())
    }
}

// ──────────────────────────────────────────────────────────────────────
// Interaction Gate
// ──────────────────────────────────────────────────────────────────────

pub(crate) struct WorkflowInteractionGateImpl {
    event_bus: hive_core::EventBus,
}

impl WorkflowInteractionGateImpl {
    pub fn new(event_bus: hive_core::EventBus) -> Self {
        Self { event_bus }
    }
}

#[async_trait]
impl WorkflowInteractionGate for WorkflowInteractionGateImpl {
    async fn create_feedback_request(
        &self,
        instance_id: i64,
        step_id: &str,
        prompt: &str,
        choices: Option<&[String]>,
        allow_freeform: bool,
    ) -> Result<String, String> {
        let request_id = uuid::Uuid::new_v4().to_string();

        let payload = json!({
            "request_id": request_id,
            "instance_id": instance_id,
            "step_id": step_id,
            "kind": {
                "type": "question",
                "text": prompt,
                "choices": choices.unwrap_or(&[]),
                "allowFreeform": allow_freeform,
            }
        });

        let _ = self.event_bus.publish("workflow.interaction.requested", "hive-workflow", payload);

        Ok(request_id)
    }
}

// ──────────────────────────────────────────────────────────────────────
// Task Scheduler
// ──────────────────────────────────────────────────────────────────────

pub(crate) struct WorkflowTaskSchedulerImpl {
    scheduler: Arc<SchedulerService>,
}

impl WorkflowTaskSchedulerImpl {
    pub fn new(scheduler: Arc<SchedulerService>) -> Self {
        Self { scheduler }
    }
}

#[async_trait]
impl WorkflowTaskScheduler for WorkflowTaskSchedulerImpl {
    async fn schedule_task(
        &self,
        schedule_def: &ScheduleTaskDef,
        parent_session_id: Option<&str>,
        parent_agent_id: Option<&str>,
    ) -> Result<String, String> {
        use hive_contracts::{CreateTaskRequest, TaskAction, TaskSchedule};

        // Parse the cron expression from the workflow definition.
        let task_schedule = if schedule_def.schedule.is_empty() {
            TaskSchedule::Once
        } else {
            TaskSchedule::Cron { expression: schedule_def.schedule.clone() }
        };

        // The action payload from the workflow is freeform JSON.
        // Try to deserialize it as a TaskAction; fall back to EmitEvent.
        let action: TaskAction = serde_json::from_value(schedule_def.action.clone())
            .unwrap_or_else(|_| TaskAction::EmitEvent {
                topic: format!("workflow.scheduled.{}", schedule_def.name),
                payload: schedule_def.action.clone(),
            });

        let request = CreateTaskRequest {
            name: schedule_def.name.clone(),
            description: Some("Scheduled by workflow step".to_string()),
            schedule: task_schedule,
            action,
            owner_session_id: parent_session_id.map(String::from),
            owner_agent_id: parent_agent_id.map(String::from),
            max_retries: None,
            retry_delay_ms: None,
        };

        let task =
            self.scheduler.create_task(request).map_err(|e| format!("schedule task: {e}"))?;

        Ok(task.id)
    }
}

// ──────────────────────────────────────────────────────────────────────
// Prompt Template Renderer
// ──────────────────────────────────────────────────────────────────────

pub(crate) struct WorkflowPromptRendererImpl {
    personas_dir: PathBuf,
}

impl WorkflowPromptRendererImpl {
    pub fn new(personas_dir: PathBuf) -> Self {
        Self { personas_dir }
    }
}

#[async_trait]
impl hive_workflow_service::WorkflowPromptRenderer for WorkflowPromptRendererImpl {
    async fn render_prompt_template(
        &self,
        persona_id: &str,
        prompt_id: &str,
        parameters: Value,
    ) -> Result<String, String> {
        use hive_core::{find_prompt_template, load_personas, render_prompt_template};

        let personas = load_personas(&self.personas_dir)
            .map_err(|e| format!("failed to load personas: {e}"))?;
        let persona = personas
            .iter()
            .find(|p| p.id == persona_id)
            .ok_or_else(|| format!("persona '{}' not found", persona_id))?;
        let template = find_prompt_template(&persona.prompts, prompt_id).ok_or_else(|| {
            format!("prompt template '{}' not found on persona '{}'", prompt_id, persona_id)
        })?;
        render_prompt_template(template, &parameters).map_err(|e| format!("render failed: {e}"))
    }
}
