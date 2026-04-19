use crate::types::AgentSummary;
use std::path::PathBuf;
use std::sync::Arc;

use arc_swap::ArcSwap;
use dashmap::DashMap;
use hive_contracts::{Persona, SessionPermissions, UserInteractionResponse};
use hive_loop::{
    AgentOrchestrator, ConversationJournal, KnowledgeQueryHandler, LoopExecutor,
    UserInteractionGate,
};
use hive_model::ModelRouter;
use hive_tools::ToolRegistry;
use parking_lot::Mutex;
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn, Instrument};

use crate::error::AgentError;
use crate::runner::{AgentExecutionContext, AgentHandle, AgentRunner};
use crate::telemetry::{TelemetrySnapshot, TokenAccumulator};
use crate::types::{AgentMessage, AgentSpec, AgentStatus, ControlSignal, SupervisorEvent};

const AGENT_INBOX_CAPACITY: usize = 64;
const MAX_EVENT_HISTORY: usize = 2000;

/// Lightweight, clonable handle for delivering messages to agents. Obtained
/// from [`AgentSupervisor::send_handle`].
#[derive(Clone)]
pub struct AgentSendHandle {
    agents: Arc<DashMap<String, AgentHandle>>,
    event_tx: broadcast::Sender<SupervisorEvent>,
}

impl AgentSendHandle {
    /// Deliver a message to an agent's inbox, exactly like
    /// [`AgentSupervisor::send_to_agent`].
    pub async fn send_to_agent(&self, agent_id: &str, msg: AgentMessage) -> Result<(), AgentError> {
        let msg_type = match &msg {
            AgentMessage::Task { .. } => "task",
            AgentMessage::Result { .. } => "result",
            AgentMessage::Feedback { .. } => "feedback",
            AgentMessage::Broadcast { .. } => "broadcast",
            AgentMessage::Directive { .. } => "directive",
            AgentMessage::Control(_) => "control",
        };
        let sender = match &msg {
            AgentMessage::Task { from, .. } => from.clone().unwrap_or_else(|| "user".to_string()),
            AgentMessage::Feedback { from, .. } => from.clone(),
            AgentMessage::Broadcast { from, .. } => from.clone(),
            _ => "supervisor".to_string(),
        };

        if let AgentMessage::Task { ref content, .. } = msg {
            if let Some(mut handle) = self.agents.get_mut(agent_id) {
                if handle.original_task.is_none() {
                    handle.original_task = Some(content.clone());
                }
                drop(handle);
                let _ = self.event_tx.send(SupervisorEvent::AgentTaskAssigned {
                    agent_id: agent_id.to_string(),
                    task: content.clone(),
                });
            }
        }

        // Clone inbox_tx and drop DashMap ref before awaiting send.
        let inbox_tx = {
            let handle = self
                .agents
                .get(agent_id)
                .ok_or_else(|| AgentError::AgentNotFound(agent_id.to_string()))?;
            handle.inbox_tx.clone()
        };

        inbox_tx
            .send(msg)
            .await
            .map_err(|e| AgentError::ChannelClosed(format!("{agent_id}: {e}")))?;

        let _ = self.event_tx.send(SupervisorEvent::MessageRouted {
            from: sender,
            to: agent_id.to_string(),
            msg_type: msg_type.to_string(),
        });

        Ok(())
    }
}

pub struct AgentSupervisor {
    agents: Arc<DashMap<String, AgentHandle>>,
    event_history: Arc<DashMap<String, Vec<SupervisorEvent>>>,
    event_tx: broadcast::Sender<SupervisorEvent>,
    execution: Option<AgentExecutionContext>,
    /// Cached persona-specific tool registries + skill catalogs. Keyed by
    /// persona ID. Populated lazily when agents are spawned with a persona
    /// different from the session persona.
    #[allow(clippy::type_complexity)]
    persona_registries:
        Arc<DashMap<String, (Arc<ToolRegistry>, Option<Arc<hive_skills::SkillCatalog>>)>>,
    pub telemetry: Arc<TokenAccumulator>,
    // Keeps the event-listener task alive; dropped when the supervisor is dropped.
    _event_listener: JoinHandle<()>,
}

impl AgentSupervisor {
    pub fn new(event_buffer: usize, budget_limit_usd: Option<f64>) -> Self {
        let (event_tx, _) = broadcast::channel(event_buffer);
        let agents = Arc::new(DashMap::new());
        let event_history = Arc::new(DashMap::new());
        let telemetry = Arc::new(TokenAccumulator::new(budget_limit_usd));
        let _event_listener = Self::spawn_event_listener(
            event_tx.subscribe(),
            Arc::clone(&agents),
            Arc::clone(&event_history),
            Arc::clone(&telemetry),
        );
        Self {
            agents,
            event_history,
            event_tx,
            execution: None,
            persona_registries: Arc::new(DashMap::new()),
            telemetry,
            _event_listener,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_executor(
        event_buffer: usize,
        budget_limit_usd: Option<f64>,
        loop_executor: Arc<LoopExecutor>,
        model_router: Arc<ArcSwap<ModelRouter>>,
        tools: Arc<ToolRegistry>,
        permissions: Arc<Mutex<SessionPermissions>>,
        personas: Arc<Mutex<Vec<Persona>>>,
        agent_orchestrator: Option<Arc<dyn AgentOrchestrator>>,
        session_id: String,
        workspace_path: PathBuf,
        skill_catalog: Option<Arc<hive_skills::SkillCatalog>>,
        knowledge_query_handler: Option<Arc<dyn KnowledgeQueryHandler>>,
    ) -> Self {
        Self::with_executor_and_persona_factory(
            event_buffer,
            budget_limit_usd,
            loop_executor,
            model_router,
            tools,
            permissions,
            personas,
            agent_orchestrator,
            session_id,
            workspace_path,
            skill_catalog,
            knowledge_query_handler,
            None,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_executor_and_persona_factory(
        event_buffer: usize,
        budget_limit_usd: Option<f64>,
        loop_executor: Arc<LoopExecutor>,
        model_router: Arc<ArcSwap<ModelRouter>>,
        tools: Arc<ToolRegistry>,
        permissions: Arc<Mutex<SessionPermissions>>,
        personas: Arc<Mutex<Vec<Persona>>>,
        agent_orchestrator: Option<Arc<dyn AgentOrchestrator>>,
        session_id: String,
        workspace_path: PathBuf,
        skill_catalog: Option<Arc<hive_skills::SkillCatalog>>,
        knowledge_query_handler: Option<Arc<dyn KnowledgeQueryHandler>>,
        persona_tool_factory: Option<Arc<dyn crate::types::PersonaToolFactory>>,
        session_persona_id: Option<String>,
    ) -> Self {
        let (event_tx, _) = broadcast::channel(event_buffer);
        let agents = Arc::new(DashMap::new());
        let event_history = Arc::new(DashMap::new());
        let telemetry = Arc::new(TokenAccumulator::new(budget_limit_usd));
        let _event_listener = Self::spawn_event_listener(
            event_tx.subscribe(),
            Arc::clone(&agents),
            Arc::clone(&event_history),
            Arc::clone(&telemetry),
        );
        Self {
            agents,
            event_history,
            event_tx,
            execution: Some(AgentExecutionContext {
                loop_executor,
                model_router,
                tools,
                permissions,
                personas,
                agent_orchestrator,
                session_id,
                workspace_path,
                skill_catalog,
                knowledge_query_handler,
                persona_tool_factory,
                session_persona_id,
            }),
            persona_registries: Arc::new(DashMap::new()),
            telemetry,
            _event_listener,
        }
    }

    /// Background task that listens for all supervisor events, updates agent
    /// status in the DashMap and appends events to per-agent history buffers.
    fn spawn_event_listener(
        mut rx: broadcast::Receiver<SupervisorEvent>,
        agents: Arc<DashMap<String, AgentHandle>>,
        event_history: Arc<DashMap<String, Vec<SupervisorEvent>>>,
        telemetry: Arc<TokenAccumulator>,
    ) -> JoinHandle<()> {
        tokio::spawn(
            async move {
                tracing::info!("bot supervisor event listener started");
                loop {
                    let event = match rx.recv().await {
                        Ok(event) => event,
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!(skipped = n, "supervisor event listener lagged, some status updates may be missed");
                            continue;
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    };
                    let agent_id = match &event {
                        SupervisorEvent::AgentSpawned { agent_id, .. }
                        | SupervisorEvent::AgentStatusChanged { agent_id, .. }
                        | SupervisorEvent::AgentTaskAssigned { agent_id, .. }
                        | SupervisorEvent::MessageRouted { to: agent_id, .. }
                        | SupervisorEvent::AgentOutput { agent_id, .. }
                        | SupervisorEvent::AgentCompleted { agent_id, .. } => {
                            Some(agent_id.clone())
                        }
                        SupervisorEvent::AllComplete { .. } => None,
                    };

                    // Update agent status when applicable
                    if let SupervisorEvent::AgentStatusChanged { ref agent_id, ref status } = event
                    {
                        if let Some(mut handle) = agents.get_mut(agent_id) {
                            // Clear last_error when transitioning away from Error
                            if *status != AgentStatus::Error {
                                handle.last_error = None;
                            }
                            handle.status = status.clone();
                        }
                    }

                    // Capture error messages from failed agent outputs
                    if let SupervisorEvent::AgentOutput {
                        ref agent_id,
                        event: hive_contracts::ReasoningEvent::Failed { ref error, .. },
                    } = event
                    {
                        if let Some(mut handle) = agents.get_mut(agent_id) {
                            handle.last_error = Some(error.clone());
                        }
                    }

                    // Capture the actual model used from ModelCallStarted events
                    // and record estimated input tokens for telemetry.
                    if let SupervisorEvent::AgentOutput {
                        ref agent_id,
                        event: hive_contracts::ReasoningEvent::ModelCallStarted { ref model, estimated_tokens, .. },
                    } = event
                    {
                        if let Some(mut handle) = agents.get_mut(agent_id) {
                            handle.active_model = Some(model.clone());
                        }
                        if let Some(tokens) = estimated_tokens {
                            telemetry.record_input_tokens(agent_id, model, tokens as u64);
                        }
                    }

                    // Track model call telemetry
                    if let SupervisorEvent::AgentOutput {
                        ref agent_id,
                        event:
                            hive_contracts::ReasoningEvent::ModelCallCompleted {
                                ref model,
                                token_count,
                                ..
                            },
                    } = event
                    {
                        telemetry.record_model_call(agent_id, model, token_count as u64);
                    }

                    // Store the final result on the agent handle when it completes
                    if let SupervisorEvent::AgentCompleted { ref agent_id, ref result } = event {
                        if let Some(mut handle) = agents.get_mut(agent_id) {
                            handle.final_result = Some(result.clone());
                        }
                    }

                    // Track tool call telemetry
                    if let SupervisorEvent::AgentOutput {
                        ref agent_id,
                        event: hive_contracts::ReasoningEvent::ToolCallCompleted { .. },
                    } = event
                    {
                        telemetry.record_tool_call(agent_id);
                    }

                    // Skip TokenDelta events from history — too noisy
                    if matches!(
                        &event,
                        SupervisorEvent::AgentOutput {
                            event: hive_contracts::ReasoningEvent::TokenDelta { .. },
                            ..
                        }
                    ) {
                        continue;
                    }

                    // Append to per-agent history buffer
                    if let Some(id) = agent_id {
                        let mut entry = event_history.entry(id).or_default();
                        if entry.len() >= MAX_EVENT_HISTORY {
                            let drain_count = (entry.len() / 4).max(1);
                            entry.drain(..drain_count);
                        }
                        entry.push(event);
                    }
                }
            }
            .instrument(tracing::info_span!("service", service = "bot-supervisor")),
        )
    }

    /// Get a telemetry snapshot.
    pub fn telemetry_snapshot(&self) -> TelemetrySnapshot {
        self.telemetry.snapshot()
    }

    /// Return a lightweight, clonable send handle that can deliver messages to
    /// agents without holding a reference to the full supervisor. Useful for
    /// building `AgentOrchestrator` implementations that route messages back
    /// into the supervisor.
    pub fn send_handle(&self) -> AgentSendHandle {
        AgentSendHandle { agents: Arc::clone(&self.agents), event_tx: self.event_tx.clone() }
    }

    /// Subscribe to supervisor events.
    pub fn subscribe(&self) -> broadcast::Receiver<SupervisorEvent> {
        self.event_tx.subscribe()
    }

    /// Set or replace the agent orchestrator. Must be called before spawning
    /// agents that need inter-agent messaging.
    pub fn set_agent_orchestrator(&mut self, orch: Option<Arc<dyn AgentOrchestrator>>) {
        if let Some(ref mut exec) = self.execution {
            exec.agent_orchestrator = orch;
        }
    }

    /// Spawn an agent from a spec. Returns the agent's ID.
    ///
    /// `parent_id` is the ID of the agent that spawned this one, or `None` if
    /// spawned directly by the chat session.
    ///
    /// `agent_permissions` overrides the shared supervisor permissions for this
    /// agent. When `None`, the agent inherits the supervisor-level permissions.
    ///
    /// `workspace_override` overrides the supervisor's default workspace path
    /// for this agent. When `None`, the agent inherits the supervisor-level
    /// workspace. Use this to give individual agents their own workspace
    /// (e.g. per-bot directories or workflow workspaces).
    pub async fn spawn_agent(
        &self,
        spec: AgentSpec,
        parent_id: Option<String>,
        session_id: Option<String>,
        agent_permissions: Option<Arc<Mutex<SessionPermissions>>>,
        workspace_override: Option<PathBuf>,
    ) -> Result<String, AgentError> {
        let id = spec.id.clone();

        // Create channel upfront so we can atomically insert a placeholder.
        let (inbox_tx, inbox_rx) = mpsc::channel::<AgentMessage>(AGENT_INBOX_CAPACITY);
        let interaction_gate = Arc::new(UserInteractionGate::new());
        let cancellation_token = CancellationToken::new();

        // Atomically claim the slot. If already taken, return error.
        // Using entry() prevents the TOCTOU race between contains_key and insert.
        {
            use dashmap::mapref::entry::Entry;
            match self.agents.entry(id.clone()) {
                Entry::Occupied(_) => return Err(AgentError::AlreadyExists(id)),
                Entry::Vacant(vacant) => {
                    vacant.insert(AgentHandle {
                        spec: spec.clone(),
                        status: AgentStatus::Spawning,
                        last_error: None,
                        active_model: None,
                        resolved_tools: vec![],
                        interaction_gate: Arc::clone(&interaction_gate),
                        inbox_tx: inbox_tx.clone(),
                        task: None,
                        messages_processed: 0,
                        parent_id: parent_id.clone(),
                        started_at_ms: Some(
                            std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap()
                                .as_millis() as u64,
                        ),
                        original_task: None,
                        session_id: session_id.clone(),
                        conversation_journal: None,
                        permissions: None,
                        final_result: None,
                        cancellation_token: cancellation_token.clone(),
                    });
                }
            }
        }

        // Do async setup outside the DashMap entry. On failure, remove the
        // placeholder so the slot can be reused.
        let setup_result = async {
            // Resolve persona-specific tools + skills when the agent's persona
            // differs from the session persona. Falls back to the session-wide
            // registry when no factory is configured or personas match.
            let (agent_tools, agent_skill_catalog) = self.resolve_persona_tools(&spec).await?;

            // Resolve the actual tool list for this agent
            let resolved_tools = if spec.allowed_tools.iter().any(|t| t == "*") {
                agent_tools.list_definitions().iter().map(|d| d.id.clone()).collect()
            } else {
                spec.allowed_tools
                    .iter()
                    .filter(|t| agent_tools.get(t).is_some())
                    .cloned()
                    .collect()
            };

            let effective_permissions = agent_permissions.unwrap_or_else(|| {
                if let Some(exec) = &self.execution {
                    Arc::new(Mutex::new(exec.permissions.lock().clone()))
                } else {
                    Arc::new(Mutex::new(SessionPermissions::default()))
                }
            });

            let mut runner = if let Some(execution) = &self.execution {
                let effective_workspace =
                    workspace_override.unwrap_or_else(|| execution.workspace_path.clone());
                AgentRunner::with_executor_and_permissions(
                    spec.clone(),
                    inbox_rx,
                    self.event_tx.clone(),
                    Arc::clone(&execution.loop_executor),
                    Arc::clone(&execution.model_router),
                    agent_tools,
                    Arc::clone(&effective_permissions),
                    Arc::clone(&execution.personas),
                    execution.agent_orchestrator.clone(),
                    Arc::clone(&interaction_gate),
                    execution.session_id.clone(),
                    effective_workspace,
                    agent_skill_catalog,
                )
            } else {
                AgentRunner::new(spec.clone(), inbox_rx, self.event_tx.clone())
            };

            let conversation_journal = Arc::new(Mutex::new(ConversationJournal::default()));
            runner.conversation_journal = Some(Arc::clone(&conversation_journal));
            runner.set_cancellation_token(cancellation_token.clone());

            if let Some(execution) = &self.execution {
                runner.knowledge_query_handler = execution.knowledge_query_handler.clone();
            }

            let task = tokio::spawn(runner.run());

            Ok::<_, AgentError>((resolved_tools, effective_permissions, conversation_journal, task))
        }
        .await;

        match setup_result {
            Ok((resolved_tools, effective_permissions, conversation_journal, task)) => {
                // Update the placeholder with the fully-initialized handle.
                if let Some(mut handle) = self.agents.get_mut(&id) {
                    handle.resolved_tools = resolved_tools;
                    handle.task = Some(task);
                    handle.conversation_journal = Some(conversation_journal);
                    handle.permissions = Some(effective_permissions);
                }
            }
            Err(e) => {
                self.agents.remove(&id);
                return Err(e);
            }
        }

        self.emit(SupervisorEvent::AgentSpawned { agent_id: id.clone(), spec, parent_id });

        info!(agent = %id, "agent spawned");
        Ok(id)
    }

    /// Resolve the tool registry and skill catalog for an agent. When the
    /// agent's persona differs from the session persona and a factory is
    /// available, returns a persona-scoped registry (cached for reuse).
    /// Otherwise falls back to the session-wide registry.
    async fn resolve_persona_tools(
        &self,
        spec: &AgentSpec,
    ) -> Result<(Arc<ToolRegistry>, Option<Arc<hive_skills::SkillCatalog>>), AgentError> {
        let Some(execution) = &self.execution else {
            return Ok((Arc::new(ToolRegistry::new()), None));
        };

        // Check whether this agent needs a persona-specific registry.
        let needs_persona_tools = match (&spec.persona_id, &execution.session_persona_id) {
            (Some(agent_persona), Some(session_persona)) => agent_persona != session_persona,
            (Some(_), None) => true,
            _ => false,
        };

        if !needs_persona_tools {
            return Ok((Arc::clone(&execution.tools), execution.skill_catalog.clone()));
        }

        let agent_persona = spec.persona_id.as_deref().unwrap();

        // Check cache first.
        if let Some(cached) = self.persona_registries.get(agent_persona) {
            debug!(persona = agent_persona, "reusing cached persona tool registry");
            return Ok(cached.clone());
        }

        // Build via factory if available.
        if let Some(factory) = &execution.persona_tool_factory {
            let result =
                factory.build_tools_for_persona(agent_persona, &execution.session_id).await?;
            debug!(persona = agent_persona, "built persona-specific tool registry");
            // Use entry API to avoid overwriting a concurrently-built cache entry.
            // If another task raced us, reuse their result (both are equivalent).
            let entry = self.persona_registries.entry(agent_persona.to_string()).or_insert(result);
            return Ok(entry.clone());
        }

        // No factory — fall back to session-wide registry.
        warn!(
            persona = agent_persona,
            "no persona tool factory configured; agent inherits session tools"
        );
        Ok((Arc::clone(&execution.tools), execution.skill_catalog.clone()))
    }

    /// Send a message to a specific agent.
    pub async fn send_to_agent(&self, agent_id: &str, msg: AgentMessage) -> Result<(), AgentError> {
        let msg_type = match &msg {
            AgentMessage::Task { .. } => "task",
            AgentMessage::Result { .. } => "result",
            AgentMessage::Feedback { .. } => "feedback",
            AgentMessage::Broadcast { .. } => "broadcast",
            AgentMessage::Directive { .. } => "directive",
            AgentMessage::Control(_) => "control",
        };

        // Extract the sender identity from the message
        let sender = match &msg {
            AgentMessage::Task { from, .. } => from.clone().unwrap_or_else(|| "user".to_string()),
            AgentMessage::Feedback { from, .. } => from.clone(),
            AgentMessage::Broadcast { from, .. } => from.clone(),
            _ => "supervisor".to_string(),
        };

        // Capture the original task content for restart support and emit event.
        if let AgentMessage::Task { ref content, .. } = msg {
            if let Some(mut handle) = self.agents.get_mut(agent_id) {
                if handle.original_task.is_none() {
                    handle.original_task = Some(content.clone());
                }
                drop(handle);
                // Always emit task-assigned so the UI can show all user messages.
                self.emit(SupervisorEvent::AgentTaskAssigned {
                    agent_id: agent_id.to_string(),
                    task: content.clone(),
                });
            }
        }

        // Clone inbox_tx and drop DashMap ref before awaiting send.
        // Holding a DashMap Ref across .await blocks the entire shard.
        let inbox_tx = {
            let handle = self
                .agents
                .get(agent_id)
                .ok_or_else(|| AgentError::AgentNotFound(agent_id.to_string()))?;
            handle.inbox_tx.clone()
        };

        inbox_tx
            .send(msg)
            .await
            .map_err(|e| AgentError::ChannelClosed(format!("{agent_id}: {e}")))?;

        self.emit(SupervisorEvent::MessageRouted {
            from: sender,
            to: agent_id.to_string(),
            msg_type: msg_type.to_string(),
        });

        debug!(agent = %agent_id, msg_type, "message routed");
        Ok(())
    }

    /// Broadcast a message to all agents.
    pub async fn broadcast(&self, msg: AgentMessage) -> Result<(), AgentError> {
        let mut errors = Vec::new();

        // Snapshot senders to avoid holding DashMap shard locks across await
        let targets: Vec<(String, mpsc::Sender<AgentMessage>)> = self
            .agents
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().inbox_tx.clone()))
            .collect();

        for (id, tx) in targets {
            if let Err(e) = tx.send(msg.clone()).await {
                errors.push(format!("{id}: {e}"));
            } else {
                self.emit(SupervisorEvent::MessageRouted {
                    from: "supervisor".to_string(),
                    to: id,
                    msg_type: "broadcast".to_string(),
                });
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(AgentError::ChannelClosed(errors.join(", ")))
        }
    }

    /// Send a directive (whisper) to a specific agent.
    pub async fn whisper(&self, agent_id: &str, msg: AgentMessage) -> Result<(), AgentError> {
        let content = match &msg {
            AgentMessage::Task { content, .. } => content.clone(),
            AgentMessage::Feedback { content, .. } => content.clone(),
            AgentMessage::Broadcast { content, .. } => content.clone(),
            AgentMessage::Directive { content } => content.clone(),
            AgentMessage::Result { content, .. } => content.clone(),
            AgentMessage::Control(_) => "control".to_string(),
        };

        self.send_to_agent(agent_id, AgentMessage::Directive { content }).await
    }

    /// Pause an agent.
    pub async fn pause_agent(&self, agent_id: &str) -> Result<(), AgentError> {
        self.send_to_agent(agent_id, AgentMessage::Control(ControlSignal::Pause)).await
    }

    /// Resume a paused agent.
    pub async fn resume_agent(&self, agent_id: &str) -> Result<(), AgentError> {
        self.send_to_agent(agent_id, AgentMessage::Control(ControlSignal::Resume)).await
    }

    /// Kill an agent, its descendants, and remove them from the map.
    pub async fn kill_agent(&self, agent_id: &str) -> Result<(), AgentError> {
        // Collect child agents (those with parent_id == agent_id) for recursive kill
        let children: Vec<String> = self
            .agents
            .iter()
            .filter(|entry| entry.value().parent_id.as_deref() == Some(agent_id))
            .map(|entry| entry.key().clone())
            .collect();

        // Kill children first (depth-first cascade)
        for child_id in children {
            if let Err(e) = Box::pin(self.kill_agent(&child_id)).await {
                warn!(agent = %child_id, parent = %agent_id, error = %e, "failed to kill child agent");
            }
        }

        // Close the interaction gate and cancel the token to interrupt
        // in-flight model/tool calls before sending the Kill signal.
        if let Some(handle) = self.agents.get(agent_id) {
            handle.cancellation_token.cancel();
            handle.interaction_gate.close();
        }

        // Remove from map — dropping inbox_tx ensures the runner exits
        if let Some((_, mut handle)) = self.agents.remove(agent_id) {
            if let Err(e) = handle.inbox_tx.send(AgentMessage::Control(ControlSignal::Kill)).await {
                debug!(agent = %agent_id, error = %e, "failed to send kill signal to agent inbox");
            }
            if let Some(task) = handle.task.take() {
                let abort_handle = task.abort_handle();
                if tokio::time::timeout(std::time::Duration::from_secs(5), task).await.is_err() {
                    warn!(agent = %agent_id, "agent task did not shut down in time, aborting");
                    abort_handle.abort();
                }
            }
        } else {
            return Err(AgentError::AgentNotFound(agent_id.to_string()));
        }

        // Clean up per-agent event history to avoid leaking memory
        self.event_history.remove(agent_id);
        self.telemetry.remove_agent(agent_id);

        info!(agent = %agent_id, "agent killed");
        Ok(())
    }

    /// Restart an agent, optionally with a new model. The agent is killed and
    /// re-spawned with the same spec and original task. If `new_model` is
    /// provided the agent's model is changed before re-spawning.
    pub async fn restart_agent(
        &self,
        agent_id: &str,
        new_model: Option<String>,
        new_allowed_tools: Option<Vec<String>>,
    ) -> Result<String, AgentError> {
        let (mut spec, parent_id, original_task, session_id) = {
            let handle = self
                .agents
                .get(agent_id)
                .ok_or_else(|| AgentError::AgentNotFound(agent_id.to_string()))?;
            (
                handle.spec.clone(),
                handle.parent_id.clone(),
                handle.original_task.clone(),
                handle.session_id.clone(),
            )
        };

        let task_content = original_task.ok_or_else(|| {
            AgentError::ExecutionSetup("agent has no recorded task to replay".to_string())
        })?;

        // Kill the old agent.
        self.kill_agent(agent_id).await?;

        // Apply new model if provided.
        if let Some(model) = new_model {
            spec.model = Some(model);
        }

        // Apply new allowed_tools if provided.
        if let Some(tools) = new_allowed_tools {
            spec.allowed_tools = tools;
        }

        // Generate a new unique ID to avoid collision.
        let base_name = spec.id.rsplit_once('-').map(|(base, _)| base).unwrap_or(&spec.id);
        let suffix = &uuid::Uuid::new_v4().simple().to_string()[..8];
        spec.id = format!("{base_name}-{suffix}");

        // Spawn the new agent and send the original task.
        let new_id = self.spawn_agent(spec, parent_id.clone(), session_id, None, None).await?;
        self.send_to_agent(&new_id, AgentMessage::Task { content: task_content, from: parent_id })
            .await?;

        info!(old_agent = %agent_id, new_agent = %new_id, "agent restarted");
        Ok(new_id)
    }

    /// Kill all agents.
    pub async fn kill_all(&self) -> Result<(), AgentError> {
        // Cancel all tokens and close interaction gates FIRST so agents
        // blocked in model/tool calls or approval waits are interrupted.
        for entry in self.agents.iter() {
            entry.value().cancellation_token.cancel();
            entry.value().interaction_gate.close();
        }

        // Snapshot senders to avoid holding DashMap shard locks across await
        let targets: Vec<mpsc::Sender<AgentMessage>> =
            self.agents.iter().map(|entry| entry.value().inbox_tx.clone()).collect();

        // Send kill signal to all agents
        for tx in targets {
            let _ = tx.send(AgentMessage::Control(ControlSignal::Kill)).await;
        }

        // Drain all agents, awaiting their tasks concurrently with a
        // single shared timeout so we never block longer than 5s total.
        let keys: Vec<String> = self.agents.iter().map(|e| e.key().clone()).collect();
        let mut join_set = tokio::task::JoinSet::new();
        for key in &keys {
            if let Some((_, mut handle)) = self.agents.remove(key) {
                if let Some(task) = handle.task.take() {
                    join_set.spawn(task);
                }
            }
        }
        if !join_set.is_empty() {
            let drain_all = async { while join_set.join_next().await.is_some() {} };
            if tokio::time::timeout(std::time::Duration::from_secs(5), drain_all).await.is_err() {
                warn!("some agent tasks did not shut down within 5s");
                join_set.abort_all();
            }
        }

        // Clear all per-agent event history to avoid leaking memory
        self.event_history.clear();

        info!("all agents killed");
        Ok(())
    }

    /// Get the status of an agent.
    pub fn get_agent_status(&self, agent_id: &str) -> Option<AgentStatus> {
        self.agents.get(agent_id).map(|h| h.status.clone())
    }

    /// Return the agent plus all its transitive descendants (depth-first).
    /// Useful for snapshotting which agents will be affected by a kill before
    /// the actual kill removes them from the map.
    pub fn get_descendant_ids(&self, agent_id: &str) -> Vec<String> {
        let mut result = Vec::new();
        let mut stack = vec![agent_id.to_string()];
        while let Some(current) = stack.pop() {
            result.push(current.clone());
            for entry in self.agents.iter() {
                if entry.value().parent_id.as_deref() == Some(&current) {
                    stack.push(entry.key().clone());
                }
            }
        }
        result
    }

    /// Get the parent ID of an agent.
    /// Returns `None` if the agent is not found.
    /// Returns `Some(None)` if the agent exists but has no parent (root-level).
    /// Returns `Some(Some(parent))` if the agent has a parent.
    pub fn get_agent_parent_id(&self, agent_id: &str) -> Option<Option<String>> {
        self.agents.get(agent_id).map(|h| h.parent_id.clone())
    }

    /// Get a snapshot of the agent's conversation journal (for persistence).
    pub fn get_agent_journal(&self, agent_id: &str) -> Option<ConversationJournal> {
        let handle = self.agents.get(agent_id)?;
        let journal = handle.conversation_journal.as_ref()?;
        let snapshot = journal.lock().clone();
        Some(snapshot)
    }

    /// Set the conversation journal on a spawned agent (used during restore to
    /// inject a previously persisted journal before sending the first task).
    pub fn set_agent_journal(&self, agent_id: &str, journal: ConversationJournal) -> bool {
        if let Some(handle) = self.agents.get(agent_id) {
            if let Some(ref j) = handle.conversation_journal {
                let mut guard = j.lock();
                *guard = journal;
                return true;
            }
        }
        false
    }

    /// List all agents with their current state.
    pub fn get_all_agents(&self) -> Vec<AgentSummary> {
        self.agents
            .iter()
            .map(|entry| {
                let h = entry.value();
                AgentSummary {
                    agent_id: entry.key().clone(),
                    spec: h.spec.clone(),
                    status: h.status.clone(),
                    last_error: h.last_error.clone(),
                    active_model: h.active_model.clone(),
                    tools: h.resolved_tools.clone(),
                    parent_id: h.parent_id.clone(),
                    started_at_ms: h.started_at_ms,
                    session_id: h.session_id.clone(),
                    final_result: h.final_result.clone(),
                }
            })
            .collect()
    }

    /// Get the number of active agents.
    pub fn agent_count(&self) -> usize {
        self.agents.len()
    }

    /// Wait for a specific agent to complete. Returns `(status, result)`.
    /// If the agent is already done, returns immediately.
    pub async fn wait_for_agent(
        &self,
        agent_id: &str,
        timeout: std::time::Duration,
    ) -> Result<(String, Option<String>), AgentError> {
        // Subscribe BEFORE checking status to avoid a race where the agent
        // completes between the status check and the subscribe call.
        let mut rx = self.event_tx.subscribe();
        let target = agent_id.to_string();

        // Now check if already completed — if yes, return immediately.
        if let Some(handle) = self.agents.get(agent_id) {
            match handle.status {
                AgentStatus::Done | AgentStatus::Error => {
                    let status = status_string(&handle.status);
                    return Ok((status, handle.final_result.clone()));
                }
                _ => {}
            }
        } else {
            return Err(AgentError::AgentNotFound(agent_id.to_string()));
        }

        // Wait for AgentCompleted event
        match tokio::time::timeout(timeout, async {
            loop {
                match rx.recv().await {
                    Ok(SupervisorEvent::AgentCompleted { agent_id: ref id, ref result })
                        if *id == target =>
                    {
                        // Read final status from handle (may be updated by event listener)
                        let status = self
                            .agents
                            .get(&target)
                            .map(|h| status_string(&h.status))
                            .unwrap_or_else(|| "done".to_string());
                        return Ok((status, Some(result.clone())));
                    }
                    Ok(_) => continue,
                    Err(broadcast::error::RecvError::Closed) => {
                        return Err(AgentError::ChannelClosed(
                            "supervisor event channel closed".into(),
                        ));
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(skipped = n, agent = %target, "lagged while waiting for agent");
                        // Re-check status — the completion event may have been in the skipped batch
                        if let Some(handle) = self.agents.get(&target) {
                            if matches!(handle.status, AgentStatus::Done | AgentStatus::Error) {
                                let status = status_string(&handle.status);
                                return Ok((status, handle.final_result.clone()));
                            }
                        }
                        continue;
                    }
                }
            }
        })
        .await
        {
            Ok(result) => result,
            Err(_) => Ok(("timeout".to_string(), None)),
        }
    }

    /// Respond to a pending user interaction on an agent (e.g. tool approval).
    pub fn respond_to_agent_interaction(
        &self,
        agent_id: &str,
        response: UserInteractionResponse,
    ) -> Result<bool, AgentError> {
        let handle = self
            .agents
            .get(agent_id)
            .ok_or_else(|| AgentError::AgentNotFound(agent_id.to_string()))?;
        Ok(handle.interaction_gate.respond(response))
    }

    /// Get the interaction kind for a pending request on a specific agent.
    pub fn get_agent_pending_kind(
        &self,
        agent_id: &str,
        request_id: &str,
    ) -> Result<Option<hive_contracts::InteractionKind>, AgentError> {
        let handle = self
            .agents
            .get(agent_id)
            .ok_or_else(|| AgentError::AgentNotFound(agent_id.to_string()))?;
        Ok(handle.interaction_gate.get_pending_kind(request_id))
    }

    /// List all pending approval requests across all agents.
    pub fn list_pending_approvals(&self) -> Vec<crate::types::PendingAgentApproval> {
        let mut result = Vec::new();
        for entry in self.agents.iter() {
            let handle = entry.value();
            for (request_id, kind) in handle.interaction_gate.list_pending() {
                if let hive_contracts::InteractionKind::ToolApproval {
                    tool_id,
                    input,
                    reason,
                    ..
                } = kind
                {
                    result.push(crate::types::PendingAgentApproval {
                        agent_id: entry.key().clone(),
                        agent_name: handle.spec.friendly_name.clone(),
                        request_id,
                        tool_id,
                        input,
                        reason,
                    });
                }
            }
        }
        result
    }

    pub fn list_pending_questions(&self) -> Vec<crate::types::PendingAgentQuestion> {
        let mut result = Vec::new();
        for entry in self.agents.iter() {
            let handle = entry.value();
            for (request_id, kind) in handle.interaction_gate.list_pending() {
                if let hive_contracts::InteractionKind::Question {
                    text,
                    choices,
                    allow_freeform,
                    multi_select,
                    message,
                } = kind
                {
                    result.push(crate::types::PendingAgentQuestion {
                        agent_id: entry.key().clone(),
                        agent_name: handle.spec.friendly_name.clone(),
                        request_id,
                        text,
                        choices,
                        allow_freeform,
                        multi_select,
                        message,
                    });
                }
            }
        }
        result
    }

    /// Inject a previously-persisted pending interaction into an agent's gate
    /// so it is immediately visible to `list_pending_questions`.
    /// This is read-only for UI visibility; the entry becomes stale once the
    /// agent re-asks through the normal path.
    pub fn inject_agent_pending_interaction(
        &self,
        agent_id: &str,
        request_id: String,
        kind: hive_contracts::InteractionKind,
    ) -> bool {
        if let Some(handle) = self.agents.get(agent_id) {
            handle.interaction_gate.inject_pending(request_id, kind);
            true
        } else {
            false
        }
    }

    /// Remove all pending interactions from an agent's gate EXCEPT the one
    /// with the given request_id.  Returns the request IDs that were removed.
    pub fn clear_stale_agent_interactions(
        &self,
        agent_id: &str,
        keep_request_id: &str,
    ) -> Vec<String> {
        self.agents
            .get(agent_id)
            .map(|handle| handle.interaction_gate.remove_all_except(keep_request_id))
            .unwrap_or_default()
    }

    /// Get the event history for a specific agent.
    pub fn get_agent_events(&self, agent_id: &str) -> Vec<SupervisorEvent> {
        self.event_history.get(agent_id).map(|events| events.clone()).unwrap_or_default()
    }

    /// Get a page of the event history for a specific agent.
    ///
    /// Returns `(events_slice, total_count)`. Events are ordered oldest-first;
    /// `offset` is counted from the beginning of the buffer.
    pub fn get_agent_events_paged(
        &self,
        agent_id: &str,
        offset: usize,
        limit: usize,
    ) -> (Vec<SupervisorEvent>, usize) {
        match self.event_history.get(agent_id) {
            Some(events) => {
                let total = events.len();
                let start = offset.min(total);
                let end = (start + limit).min(total);
                (events[start..end].to_vec(), total)
            }
            None => (Vec::new(), 0),
        }
    }

    /// Get a clone of the current permission rules.
    pub fn get_permissions(&self) -> Option<SessionPermissions> {
        self.execution.as_ref().map(|e| e.permissions.lock().clone())
    }

    /// Replace all permission rules.
    pub fn set_permissions(&self, permissions: SessionPermissions) {
        if let Some(execution) = &self.execution {
            *execution.permissions.lock() = permissions;
        }
    }

    /// Get per-agent permissions, falling back to supervisor-level permissions.
    pub fn get_agent_permissions(&self, agent_id: &str) -> Option<SessionPermissions> {
        if let Some(handle) = self.agents.get(agent_id) {
            if let Some(ref perms) = handle.permissions {
                return Some(perms.lock().clone());
            }
        }
        self.get_permissions()
    }

    /// Set per-agent permissions. Creates the per-agent mutex if it doesn't exist.
    pub fn set_agent_permissions(&self, agent_id: &str, permissions: SessionPermissions) {
        if let Some(mut handle) = self.agents.get_mut(agent_id) {
            if let Some(ref perms) = handle.permissions {
                *perms.lock() = permissions;
            } else {
                handle.permissions = Some(Arc::new(Mutex::new(permissions)));
            }
        }
    }

    /// Grant a permission rule to a specific agent.
    ///
    /// Adds the rule to the agent's per-agent permissions so future tool calls
    /// matching this rule are auto-approved (or auto-denied) without user interaction.
    pub fn grant_agent_permission(
        &self,
        agent_id: &str,
        rule: hive_contracts::PermissionRule,
    ) -> Result<(), AgentError> {
        let mut handle = self
            .agents
            .get_mut(agent_id)
            .ok_or_else(|| AgentError::AgentNotFound(agent_id.to_string()))?;
        if let Some(ref perms) = handle.permissions {
            perms.lock().add_rule(rule);
        } else {
            let mut sp = SessionPermissions::default();
            sp.add_rule(rule);
            handle.permissions = Some(Arc::new(Mutex::new(sp)));
        }
        Ok(())
    }

    /// Grant a permission rule to ALL agents in this supervisor, plus the
    /// supervisor-level permissions (so future agents inherit it).
    ///
    /// This is the "allow for session" behavior: every current and future agent
    /// in this session will have this rule.
    pub fn grant_all_agents_permission(&self, rule: hive_contracts::PermissionRule) {
        // 1) Update supervisor-level permissions (inherited by new agents).
        if let Some(execution) = &self.execution {
            execution.permissions.lock().add_rule(rule.clone());
        }

        // 2) Update every existing agent's permissions.
        for mut entry in self.agents.iter_mut() {
            let handle = entry.value_mut();
            if let Some(ref perms) = handle.permissions {
                perms.lock().add_rule(rule.clone());
            } else {
                let mut sp = SessionPermissions::default();
                sp.add_rule(rule.clone());
                handle.permissions = Some(Arc::new(Mutex::new(sp)));
            }
        }
    }

    /// Auto-approve any pending tool approval interactions across all agents
    /// that match the given tool pattern and scope.
    ///
    /// Returns the number of interactions that were auto-resolved.
    pub fn auto_approve_matching_pending(
        &self,
        tool_pattern: &str,
        scope: &str,
    ) -> Vec<(String, String)> {
        let mut resolved = Vec::new();

        for entry in self.agents.iter() {
            let handle = entry.value();
            let agent_id = entry.key().clone();

            // Check each pending interaction
            for (request_id, kind) in handle.interaction_gate.list_pending() {
                if let hive_contracts::InteractionKind::ToolApproval {
                    ref tool_id,
                    ref inferred_scope,
                    ..
                } = kind
                {
                    // Match if tool_id matches the pattern and scope matches
                    let scope_matches = match inferred_scope {
                        Some(s) => s == scope || scope == "*",
                        None => scope == "*",
                    };

                    if tool_id == tool_pattern && scope_matches {
                        let response = UserInteractionResponse {
                            request_id: request_id.clone(),
                            payload: hive_contracts::InteractionResponsePayload::ToolApproval {
                                approved: true,
                                allow_session: false,
                                allow_agent: false,
                            },
                        };
                        if handle.interaction_gate.respond(response) {
                            resolved.push((agent_id.clone(), request_id));
                        }
                    }
                }
            }
        }

        resolved
    }

    fn emit(&self, event: SupervisorEvent) {
        let _ = self.event_tx.send(event);
    }
}

/// Serialize an `AgentStatus` to its serde snake_case string (e.g. "done", "error").
fn status_string(status: &AgentStatus) -> String {
    serde_json::to_value(status)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_else(|| format!("{:?}", status).to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hive_contracts::{PermissionRule, ToolApproval};

    /// Helper: create a minimal supervisor without an executor context.
    /// Agents spawned on this supervisor won't run a real loop but their
    /// handles are fully wired, which is sufficient for permission tests.
    fn test_supervisor() -> AgentSupervisor {
        AgentSupervisor::new(64, None)
    }

    fn test_spec(id: &str) -> AgentSpec {
        AgentSpec {
            id: id.to_string(),
            name: id.to_string(),
            friendly_name: id.to_string(),
            description: String::new(),
            role: crate::types::AgentRole::Coder,
            model: None,
            preferred_models: None,
            loop_strategy: None,
            tool_execution_mode: None,
            system_prompt: String::new(),
            allowed_tools: vec!["*".to_string()],
            avatar: None,
            color: None,
            data_class: hive_classification::DataClass::Public,
            keep_alive: false,
            idle_timeout_secs: None,
            tool_limits: None,
            persona_id: None,
            workflow_managed: false,
        }
    }

    fn auto_rule(tool: &str, scope: &str) -> PermissionRule {
        PermissionRule {
            tool_pattern: tool.to_string(),
            scope: scope.to_string(),
            decision: ToolApproval::Auto,
        }
    }

    #[tokio::test]
    async fn grant_agent_permission_visible_to_agent_handle() {
        let sup = test_supervisor();
        let id = sup.spawn_agent(test_spec("a1"), None, None, None, None).await.unwrap();

        // Before granting, no rules exist on the handle
        let perms_before = sup.get_agent_permissions(&id).unwrap();
        assert!(perms_before.rules.is_empty());

        // Grant a permission
        sup.grant_agent_permission(&id, auto_rule("shell.execute", "*")).unwrap();

        // Verify it's visible on the agent handle
        let perms_after = sup.get_agent_permissions(&id).unwrap();
        assert_eq!(perms_after.rules.len(), 1);
        assert_eq!(perms_after.rules[0].tool_pattern, "shell.execute");
        assert_eq!(perms_after.resolve("shell.execute", "*"), Some(ToolApproval::Auto));
    }

    #[tokio::test]
    async fn grant_agent_permission_does_not_affect_other_agents() {
        let sup = test_supervisor();
        let a1 = sup.spawn_agent(test_spec("a1"), None, None, None, None).await.unwrap();
        let a2 = sup.spawn_agent(test_spec("a2"), None, None, None, None).await.unwrap();

        sup.grant_agent_permission(&a1, auto_rule("shell.execute", "*")).unwrap();

        // a2 should NOT have the rule
        let a2_perms = sup.get_agent_permissions(&a2).unwrap();
        assert!(a2_perms.rules.is_empty());
    }

    #[tokio::test]
    async fn grant_all_agents_permission_propagates_to_all() {
        let sup = test_supervisor();
        let a1 = sup.spawn_agent(test_spec("a1"), None, None, None, None).await.unwrap();
        let a2 = sup.spawn_agent(test_spec("a2"), None, None, None, None).await.unwrap();

        sup.grant_all_agents_permission(auto_rule("filesystem.write", "/workspace/**"));

        // Both agents should have the rule
        let a1_perms = sup.get_agent_permissions(&a1).unwrap();
        assert_eq!(a1_perms.rules.len(), 1);
        assert_eq!(
            a1_perms.resolve("filesystem.write", "/workspace/src/main.rs"),
            Some(ToolApproval::Auto)
        );

        let a2_perms = sup.get_agent_permissions(&a2).unwrap();
        assert_eq!(a2_perms.rules.len(), 1);
        assert_eq!(
            a2_perms.resolve("filesystem.write", "/workspace/tests/test.rs"),
            Some(ToolApproval::Auto)
        );
    }

    #[tokio::test]
    async fn grant_all_agents_permission_inherited_by_future_agents() {
        let sup = test_supervisor();
        let a1 = sup.spawn_agent(test_spec("a1"), None, None, None, None).await.unwrap();

        // Grant before a2 exists
        sup.grant_all_agents_permission(auto_rule("shell.execute", "*"));

        // a1 should have the rule
        let a1_perms = sup.get_agent_permissions(&a1).unwrap();
        assert_eq!(a1_perms.rules.len(), 1);

        // NOTE: future agents spawned via `with_executor` would inherit from
        // execution.permissions. Without an executor, new agents start empty.
        // This test verifies the propagation to existing agents.
    }

    #[tokio::test]
    async fn inject_agent_pending_interaction_makes_question_visible() {
        let sup = test_supervisor();
        let spec = test_spec("test-agent");

        let agent_id = sup.spawn_agent(spec, None, None, None, None).await.unwrap();

        let kind = hive_contracts::InteractionKind::Question {
            text: "Pick a number".into(),
            choices: vec!["1".into(), "2".into()],
            allow_freeform: false,
            multi_select: false,
            message: None,
        };

        // Inject the interaction
        let injected = sup.inject_agent_pending_interaction(&agent_id, "q-99".to_string(), kind);
        assert!(injected, "should succeed for existing agent");

        // Should appear in list_pending_questions
        let questions = sup.list_pending_questions();
        assert_eq!(questions.len(), 1);
        assert_eq!(questions[0].request_id, "q-99");
        assert_eq!(questions[0].text, "Pick a number");

        // Respond through the gate
        let response = hive_contracts::UserInteractionResponse {
            request_id: "q-99".to_string(),
            payload: hive_contracts::InteractionResponsePayload::Answer {
                selected_choice: Some(1),
                selected_choices: None,
                text: None,
            },
        };
        let ack = sup.respond_to_agent_interaction(&agent_id, response).unwrap();
        assert!(ack, "response should be acknowledged");

        // Should no longer appear
        assert!(sup.list_pending_questions().is_empty());
    }

    #[tokio::test]
    async fn inject_for_nonexistent_agent_returns_none() {
        let sup = test_supervisor();
        let kind = hive_contracts::InteractionKind::Question {
            text: "?".into(),
            choices: vec![],
            allow_freeform: true,
            multi_select: false,
            message: None,
        };
        let result = sup.inject_agent_pending_interaction("no-such-agent", "q-1".to_string(), kind);
        assert!(!result);
    }

    #[tokio::test]
    async fn kill_all_closes_gates_unblocking_pending_interactions() {
        let sup = test_supervisor();
        let spec = test_spec("blocker");
        let agent_id = sup.spawn_agent(spec, None, None, None, None).await.unwrap();

        // Inject a pending question into the agent's gate
        sup.inject_agent_pending_interaction(
            &agent_id,
            "q-stuck".to_string(),
            hive_contracts::InteractionKind::Question {
                text: "Are you there?".into(),
                choices: vec![],
                allow_freeform: true,
                multi_select: false,
                message: None,
            },
        );

        // Create a real gate request to verify the close unblocks awaiting receivers
        let rx = {
            let handle = sup.agents.get(&agent_id).unwrap();
            handle.interaction_gate.create_request(
                "q-real".to_string(),
                hive_contracts::InteractionKind::Question {
                    text: "real question".into(),
                    choices: vec![],
                    allow_freeform: true,
                    multi_select: false,
                    message: None,
                },
            )
        };

        // kill_all should close gates first, then send Kill signal
        let _ = sup.kill_all().await;

        // The receiver should have resolved with Err (sender dropped by close())
        let result = tokio::time::timeout(std::time::Duration::from_millis(100), rx).await;
        assert!(result.is_ok(), "receiver should not timeout — gate was closed");
        assert!(result.unwrap().is_err(), "should get RecvError when gate is closed");

        // All agents should be gone
        assert!(sup.get_all_agents().is_empty());
    }

    #[tokio::test]
    async fn get_descendant_ids_returns_self_and_children() {
        let sup = test_supervisor();
        let root = sup.spawn_agent(test_spec("root"), None, None, None, None).await.unwrap();
        let child1 =
            sup.spawn_agent(test_spec("child1"), Some(root.clone()), None, None, None).await.unwrap();
        let child2 =
            sup.spawn_agent(test_spec("child2"), Some(root.clone()), None, None, None).await.unwrap();
        let grandchild =
            sup.spawn_agent(test_spec("gc"), Some(child1.clone()), None, None, None).await.unwrap();

        let mut ids = sup.get_descendant_ids(&root);
        ids.sort();
        let mut expected = vec![root.clone(), child1, child2, grandchild];
        expected.sort();
        assert_eq!(ids, expected);

        // Leaf node returns just itself
        let leaf_ids = sup.get_descendant_ids("gc");
        assert_eq!(leaf_ids, vec!["gc".to_string()]);
    }

    #[tokio::test]
    async fn kill_agent_removes_descendants_from_supervisor() {
        let sup = test_supervisor();
        let root = sup.spawn_agent(test_spec("root"), None, None, None, None).await.unwrap();
        let _child =
            sup.spawn_agent(test_spec("child"), Some(root.clone()), None, None, None).await.unwrap();
        let _grandchild =
            sup.spawn_agent(test_spec("gc"), Some("child".to_string()), None, None, None).await.unwrap();

        assert_eq!(sup.agent_count(), 3);
        sup.kill_agent(&root).await.unwrap();
        assert_eq!(sup.agent_count(), 0, "kill_agent should remove target + descendants");
    }
}
