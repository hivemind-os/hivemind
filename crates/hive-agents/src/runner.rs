use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8};
use std::sync::Arc;

use arc_swap::ArcSwap;
use hive_contracts::{InteractionKind, Persona, ReasoningEvent, SessionPermissions};
use hive_loop::{
    AgentContext, AgentOrchestrator, ConversationContext, ConversationJournal,
    KnowledgeQueryHandler, LoopContext, LoopError, LoopEvent, LoopExecutor, RoutingConfig,
    SecurityContext, ToolsContext, UserInteractionGate,
};
use hive_model::{Capability, CompletionMessage, ModelRouter};
use hive_tools::ToolRegistry;
use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};
use uuid::Uuid;

use crate::types::{AgentMessage, AgentSpec, AgentStatus, ControlSignal, SupervisorEvent};

/// Rich error from `execute_task` that preserves structured info from
/// `LoopError::ModelExecution` so callers can populate `ReasoningEvent::Failed`
/// with `error_code`, `http_status`, `provider_id`, and `model`.
struct TaskError {
    message: String,
    error_code: Option<String>,
    http_status: Option<u16>,
    provider_id: Option<String>,
    model: Option<String>,
    cancelled: bool,
}

impl From<LoopError> for TaskError {
    fn from(err: LoopError) -> Self {
        match err {
            LoopError::Cancelled => Self {
                message: "cancelled".to_string(),
                error_code: None,
                http_status: None,
                provider_id: None,
                model: None,
                cancelled: true,
            },
            LoopError::ModelExecution { message, error_code, http_status, provider_id, model } => {
                Self {
                    message: format!("model execution failed: {message}"),
                    error_code,
                    http_status,
                    provider_id,
                    model,
                    cancelled: false,
                }
            }
            other => Self {
                message: other.to_string(),
                error_code: None,
                http_status: None,
                provider_id: None,
                model: None,
                cancelled: false,
            },
        }
    }
}

#[derive(Clone)]
pub struct AgentExecutionContext {
    pub loop_executor: Arc<LoopExecutor>,
    pub model_router: Arc<ArcSwap<ModelRouter>>,
    pub tools: Arc<ToolRegistry>,
    pub permissions: Arc<Mutex<SessionPermissions>>,
    pub personas: Arc<Mutex<Vec<Persona>>>,
    pub agent_orchestrator: Option<Arc<dyn AgentOrchestrator>>,
    pub session_id: String,
    pub workspace_path: PathBuf,
    pub skill_catalog: Option<Arc<hive_skills::SkillCatalog>>,
    pub knowledge_query_handler: Option<Arc<dyn KnowledgeQueryHandler>>,
    /// Factory for building persona-specific tool registries. When a child
    /// agent's persona differs from the session persona, the supervisor uses
    /// this to produce an isolated tool set.
    pub persona_tool_factory: Option<Arc<dyn crate::types::PersonaToolFactory>>,
    /// The persona ID of the session that owns this execution context. Used to
    /// detect when a child agent needs a different tool registry.
    pub session_persona_id: Option<String>,
}

/// Handle held by the supervisor for each spawned agent.
pub struct AgentHandle {
    pub spec: AgentSpec,
    pub status: AgentStatus,
    pub last_error: Option<String>,
    pub active_model: Option<String>,
    pub resolved_tools: Vec<String>,
    pub interaction_gate: Arc<UserInteractionGate>,
    pub inbox_tx: mpsc::Sender<AgentMessage>,
    pub task: Option<JoinHandle<()>>,
    pub messages_processed: u64,
    /// The ID of the parent agent, or None if spawned by the chat session.
    pub parent_id: Option<String>,
    /// Unix epoch milliseconds when this agent was spawned.
    pub started_at_ms: Option<u64>,
    /// The original task content for restart support.
    pub original_task: Option<String>,
    /// The top-level chat session this agent belongs to, if any.
    pub session_id: Option<String>,
    /// Shared journal for recording tool cycles (used for mid-task resume).
    pub conversation_journal: Option<Arc<Mutex<ConversationJournal>>>,
    /// Per-agent permissions (if set, overrides supervisor-level permissions).
    pub permissions: Option<Arc<Mutex<SessionPermissions>>>,
    /// The final result produced by the agent when its task completed.
    pub final_result: Option<String>,
    /// Token cancelled when the agent is killed, enabling cooperative
    /// cancellation of in-flight model and tool calls.
    pub cancellation_token: CancellationToken,
}

/// Runs a single agent as a Tokio task, processing messages from its inbox.
///
/// When execution context is configured, task messages are routed through the
/// real loop executor. Otherwise the historical placeholder behavior is kept so
/// lightweight supervisor tests can continue to run without a model stack.
pub struct AgentRunner {
    pub spec: AgentSpec,
    inbox_rx: mpsc::Receiver<AgentMessage>,
    event_tx: broadcast::Sender<SupervisorEvent>,
    loop_executor: Option<Arc<LoopExecutor>>,
    model_router: Option<Arc<ArcSwap<ModelRouter>>>,
    tools: Option<Arc<ToolRegistry>>,
    permissions: Option<Arc<Mutex<SessionPermissions>>>,
    personas: Option<Arc<Mutex<Vec<Persona>>>>,
    agent_orchestrator: Option<Arc<dyn AgentOrchestrator>>,
    interaction_gate: Arc<UserInteractionGate>,
    session_id: String,
    workspace_path: Option<PathBuf>,
    skill_catalog: Option<Arc<hive_skills::SkillCatalog>>,
    pub conversation_journal: Option<Arc<Mutex<ConversationJournal>>>,
    /// Handler for knowledge.query tool calls.
    pub knowledge_query_handler: Option<Arc<dyn KnowledgeQueryHandler>>,
    /// Accumulated multi-turn conversation history for keep_alive agents.
    conversation_history: Vec<CompletionMessage>,
    /// Token cancelled externally to interrupt in-flight work.
    cancellation_token: CancellationToken,
}

impl AgentRunner {
    pub fn new(
        spec: AgentSpec,
        inbox_rx: mpsc::Receiver<AgentMessage>,
        event_tx: broadcast::Sender<SupervisorEvent>,
    ) -> Self {
        Self {
            spec,
            inbox_rx,
            event_tx,
            loop_executor: None,
            model_router: None,
            tools: None,
            permissions: None,
            personas: None,
            agent_orchestrator: None,
            interaction_gate: Arc::new(UserInteractionGate::new()),
            session_id: String::new(),
            workspace_path: None,
            skill_catalog: None,
            conversation_journal: None,
            knowledge_query_handler: None,
            conversation_history: Vec::new(),
            cancellation_token: CancellationToken::new(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_executor(
        spec: AgentSpec,
        inbox_rx: mpsc::Receiver<AgentMessage>,
        event_tx: broadcast::Sender<SupervisorEvent>,
        loop_executor: Arc<LoopExecutor>,
        model_router: Arc<ArcSwap<ModelRouter>>,
        tools: Arc<ToolRegistry>,
        session_id: String,
        workspace_path: PathBuf,
    ) -> Self {
        Self::with_executor_and_permissions(
            spec,
            inbox_rx,
            event_tx,
            loop_executor,
            model_router,
            tools,
            Arc::new(Mutex::new(SessionPermissions::default())),
            Arc::new(Mutex::new(Vec::new())),
            None,
            Arc::new(UserInteractionGate::new()),
            session_id,
            workspace_path,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_executor_and_permissions(
        spec: AgentSpec,
        inbox_rx: mpsc::Receiver<AgentMessage>,
        event_tx: broadcast::Sender<SupervisorEvent>,
        loop_executor: Arc<LoopExecutor>,
        model_router: Arc<ArcSwap<ModelRouter>>,
        tools: Arc<ToolRegistry>,
        permissions: Arc<Mutex<SessionPermissions>>,
        personas: Arc<Mutex<Vec<Persona>>>,
        agent_orchestrator: Option<Arc<dyn AgentOrchestrator>>,
        interaction_gate: Arc<UserInteractionGate>,
        session_id: String,
        workspace_path: PathBuf,
        skill_catalog: Option<Arc<hive_skills::SkillCatalog>>,
    ) -> Self {
        Self {
            spec,
            inbox_rx,
            event_tx,
            loop_executor: Some(loop_executor),
            model_router: Some(model_router),
            tools: Some(tools),
            permissions: Some(permissions),
            personas: Some(personas),
            agent_orchestrator,
            interaction_gate,
            session_id,
            workspace_path: Some(workspace_path),
            skill_catalog,
            conversation_journal: None,
            knowledge_query_handler: None,
            conversation_history: Vec::new(),
            cancellation_token: CancellationToken::new(),
        }
    }

    /// Set the cancellation token for this runner. Called by the supervisor
    /// after construction so the token is shared with the `AgentHandle`.
    pub fn set_cancellation_token(&mut self, token: CancellationToken) {
        self.cancellation_token = token;
    }

    pub async fn run(mut self) {
        let agent_id = self.spec.id.clone();
        let mut paused = false;
        let mut messages_processed: u64 = 0;

        self.emit(SupervisorEvent::AgentStatusChanged {
            agent_id: agent_id.clone(),
            status: AgentStatus::Waiting,
        });

        let idle_timeout = self.spec.idle_timeout_secs.map(std::time::Duration::from_secs);

        loop {
            let msg = if let Some(timeout_dur) = idle_timeout {
                match tokio::time::timeout(timeout_dur, self.inbox_rx.recv()).await {
                    Ok(Some(msg)) => msg,
                    Ok(None) => break, // channel closed
                    Err(_) => {
                        // Idle timeout elapsed
                        debug!(agent = %agent_id, timeout_secs = ?self.spec.idle_timeout_secs, "idle timeout elapsed, terminating");
                        self.emit(SupervisorEvent::AgentStatusChanged {
                            agent_id: agent_id.clone(),
                            status: AgentStatus::Done,
                        });
                        self.emit(SupervisorEvent::AgentCompleted {
                            agent_id: agent_id.clone(),
                            result: "Idle timeout".to_string(),
                        });
                        return;
                    }
                }
            } else {
                match self.inbox_rx.recv().await {
                    Some(msg) => msg,
                    None => break,
                }
            };

            match msg {
                AgentMessage::Control(ControlSignal::Kill) => {
                    debug!(agent = %agent_id, "received kill signal");
                    self.emit(SupervisorEvent::AgentStatusChanged {
                        agent_id: agent_id.clone(),
                        status: AgentStatus::Done,
                    });
                    self.emit(SupervisorEvent::AgentCompleted {
                        agent_id: agent_id.clone(),
                        result: "Killed".to_string(),
                    });
                    return;
                }
                AgentMessage::Control(ControlSignal::Pause) => {
                    paused = true;
                    self.emit(SupervisorEvent::AgentStatusChanged {
                        agent_id: agent_id.clone(),
                        status: AgentStatus::Paused,
                    });
                    continue;
                }
                AgentMessage::Control(ControlSignal::Resume) => {
                    paused = false;
                    self.emit(SupervisorEvent::AgentStatusChanged {
                        agent_id: agent_id.clone(),
                        status: AgentStatus::Waiting,
                    });
                    continue;
                }
                _ if paused => {
                    debug!(agent = %agent_id, "message dropped — agent is paused");
                    continue;
                }
                AgentMessage::Task { ref content, ref from } => {
                    self.emit(SupervisorEvent::AgentStatusChanged {
                        agent_id: agent_id.clone(),
                        status: AgentStatus::Active,
                    });

                    match self.execute_task(&agent_id, content, from.clone()).await {
                        Ok(result) => {
                            messages_processed += 1;
                            // Route result back as feedback (non-executing) to prevent loops
                            if let Some(ref sender) = from {
                                if sender != "user" {
                                    if let Some(ref orch) = self.agent_orchestrator {
                                        let _ = orch
                                            .feedback_agent(
                                                sender.clone(),
                                                result.clone(),
                                                agent_id.clone(),
                                            )
                                            .await;
                                    }
                                }
                            }
                            self.emit(SupervisorEvent::AgentCompleted {
                                agent_id: agent_id.clone(),
                                result,
                            });
                            if self.spec.keep_alive {
                                self.emit(SupervisorEvent::AgentStatusChanged {
                                    agent_id: agent_id.clone(),
                                    status: AgentStatus::Waiting,
                                });
                            } else {
                                self.emit(SupervisorEvent::AgentStatusChanged {
                                    agent_id: agent_id.clone(),
                                    status: AgentStatus::Done,
                                });
                                return;
                            }
                        }
                        Err(error) if error.cancelled => {
                            messages_processed += 1;
                            debug!(agent = %agent_id, "task cancelled");
                            // Don't emit Failed — the Kill handler above (or
                            // the next inbox read) will emit Done/Completed.
                            // Just break out so the runner can process the Kill
                            // signal from the inbox.
                            self.emit(SupervisorEvent::AgentStatusChanged {
                                agent_id: agent_id.clone(),
                                status: AgentStatus::Done,
                            });
                            self.emit(SupervisorEvent::AgentCompleted {
                                agent_id: agent_id.clone(),
                                result: "Killed".to_string(),
                            });
                            return;
                        }
                        Err(error) => {
                            messages_processed += 1;
                            self.emit(SupervisorEvent::AgentOutput {
                                agent_id: agent_id.clone(),
                                event: ReasoningEvent::Failed {
                                    error: error.message.clone(),
                                    error_code: error.error_code,
                                    http_status: error.http_status,
                                    provider_id: error.provider_id,
                                    model: error.model,
                                },
                            });
                            self.emit(SupervisorEvent::AgentStatusChanged {
                                agent_id: agent_id.clone(),
                                status: AgentStatus::Error,
                            });
                            self.emit(SupervisorEvent::AgentCompleted {
                                agent_id: agent_id.clone(),
                                result: error.message,
                            });
                            if !self.spec.keep_alive {
                                return;
                            }
                        }
                    }
                }
                AgentMessage::Broadcast { ref content, ref from } => {
                    self.emit(SupervisorEvent::AgentStatusChanged {
                        agent_id: agent_id.clone(),
                        status: AgentStatus::Active,
                    });

                    let result = format!(
                        "[{}] broadcast from {}: {}",
                        self.spec.name,
                        from,
                        &preview(content, 80)
                    );

                    self.emit(SupervisorEvent::AgentOutput {
                        agent_id: agent_id.clone(),
                        event: ReasoningEvent::Completed { result },
                    });

                    messages_processed += 1;

                    self.emit(SupervisorEvent::AgentStatusChanged {
                        agent_id: agent_id.clone(),
                        status: AgentStatus::Waiting,
                    });
                }
                AgentMessage::Feedback { ref content, ref from } => {
                    self.emit(SupervisorEvent::AgentStatusChanged {
                        agent_id: agent_id.clone(),
                        status: AgentStatus::Active,
                    });

                    // Execute the feedback as a task so the agent can process
                    // the response, but do NOT auto-reply (prevents loops).
                    match self.execute_task(&agent_id, content, Some(from.clone())).await {
                        Ok(result) => {
                            messages_processed += 1;
                            self.emit(SupervisorEvent::AgentCompleted {
                                agent_id: agent_id.clone(),
                                result,
                            });
                        }
                        Err(e) => {
                            warn!(agent = %agent_id, error = %e.message, "feedback task failed");
                            self.emit(SupervisorEvent::AgentOutput {
                                agent_id: agent_id.clone(),
                                event: ReasoningEvent::Completed {
                                    result: format!(
                                        "[{}] feedback processing error: {}",
                                        self.spec.name, e.message
                                    ),
                                },
                            });
                        }
                    }

                    if self.spec.keep_alive {
                        self.emit(SupervisorEvent::AgentStatusChanged {
                            agent_id: agent_id.clone(),
                            status: AgentStatus::Waiting,
                        });
                    } else {
                        self.emit(SupervisorEvent::AgentStatusChanged {
                            agent_id: agent_id.clone(),
                            status: AgentStatus::Done,
                        });
                        break;
                    }
                }
                AgentMessage::Directive { ref content } => {
                    self.emit(SupervisorEvent::AgentStatusChanged {
                        agent_id: agent_id.clone(),
                        status: AgentStatus::Active,
                    });

                    let result =
                        format!("[{}] directive: {}", self.spec.name, &preview(content, 100));

                    self.emit(SupervisorEvent::AgentOutput {
                        agent_id: agent_id.clone(),
                        event: ReasoningEvent::Completed { result },
                    });

                    messages_processed += 1;

                    self.emit(SupervisorEvent::AgentStatusChanged {
                        agent_id: agent_id.clone(),
                        status: AgentStatus::Waiting,
                    });
                }
                AgentMessage::Result { .. } => {
                    messages_processed += 1;
                }
            }
        }

        debug!(agent = %agent_id, messages_processed, "inbox closed");
        self.emit(SupervisorEvent::AgentStatusChanged {
            agent_id: agent_id.clone(),
            status: AgentStatus::Done,
        });
        self.emit(SupervisorEvent::AgentCompleted { agent_id, result: "Inbox closed".to_string() });
    }

    async fn execute_task(
        &mut self,
        agent_id: &str,
        content: &str,
        parent_agent_id: Option<String>,
    ) -> Result<String, TaskError> {
        let Some(executor) = self.loop_executor.as_ref() else {
            let result = self.placeholder_task_result(content);
            self.emit(SupervisorEvent::AgentOutput {
                agent_id: agent_id.to_string(),
                event: ReasoningEvent::Completed { result: result.clone() },
            });
            return Ok(result);
        };

        let Some(model_router) = self.model_router.as_ref().map(|router| router.load_full()) else {
            let result = self.placeholder_task_result(content);
            self.emit(SupervisorEvent::AgentOutput {
                agent_id: agent_id.to_string(),
                event: ReasoningEvent::Completed { result: result.clone() },
            });
            return Ok(result);
        };

        if let Some(agent_workspace) = self.agent_workspace_path() {
            std::fs::create_dir_all(&agent_workspace).map_err(|error| TaskError {
                message: format!(
                    "failed to prepare workspace for agent {} at {}: {}",
                    self.spec.id,
                    agent_workspace.display(),
                    error
                ),
                error_code: None,
                http_status: None,
                provider_id: None,
                model: None,
                cancelled: false,
            })?;
        }

        let tools = self.filter_tools();
        let loop_context =
            self.build_loop_context(content, tools, model_router.as_ref(), parent_agent_id);
        let (loop_event_tx, mut loop_event_rx) = mpsc::channel(4096);
        let supervisor_tx = self.event_tx.clone();
        let agent_id = agent_id.to_string();
        let prompt_preview = preview(content, 120);

        let forward_handle = tokio::spawn(async move {
            while let Some(event) = loop_event_rx.recv().await {
                let reasoning_event = convert_loop_event(event, &prompt_preview, &agent_id);
                let _ = supervisor_tx.send(SupervisorEvent::AgentOutput {
                    agent_id: agent_id.clone(),
                    event: reasoning_event,
                });
            }
        });

        let cancellation_token = self.cancellation_token.clone();
        let result = tokio::select! {
            biased;
            _ = cancellation_token.cancelled() => {
                Err(TaskError {
                    message: "cancelled".to_string(),
                    error_code: None,
                    http_status: None,
                    provider_id: None,
                    model: None,
                    cancelled: true,
                })
            }
            result = executor.run_with_events(
                loop_context,
                model_router,
                loop_event_tx,
                Some(Arc::clone(&self.interaction_gate)),
            ) => {
                result.map(|r| r.content).map_err(TaskError::from)
            }
        };

        let _ = forward_handle.await;

        // For keep_alive agents, accumulate conversation history
        if self.spec.keep_alive {
            self.conversation_history.push(CompletionMessage {
                role: "user".to_string(),
                content: content.to_string(),
                content_parts: vec![],
            });
            if let Ok(ref response) = result {
                self.conversation_history.push(CompletionMessage {
                    role: "assistant".to_string(),
                    content: response.clone(),
                    content_parts: vec![],
                });
            }
        }

        result
    }

    fn build_loop_context(
        &self,
        content: &str,
        tools: Arc<ToolRegistry>,
        _model_router: &ModelRouter,
        parent_agent_id: Option<String>,
    ) -> LoopContext {
        let required_capabilities = BTreeSet::from([Capability::Chat]);

        let mut system_parts = Vec::new();
        if !self.spec.system_prompt.trim().is_empty() {
            system_parts.push(self.spec.system_prompt.clone());
        }
        // Inject parent context so the agent knows how to communicate back
        let reply_target = if let Some(ref parent_id) = parent_agent_id {
            format!(
                "You were spawned by agent '{}'. Your own agent ID is '{}'. \
                 You can signal your parent using the core.signal_agent \
                 tool with agent_id='{}'.",
                parent_id, self.spec.id, parent_id
            )
        } else {
            format!(
                "You were spawned by the chat session. Your own agent ID is '{}'. \
                 You can signal the chat session using the \
                 core.signal_agent tool with agent_id='session'.",
                self.spec.id
            )
        };

        let lifecycle_hint = if self.spec.keep_alive {
            "You are a bot running as a background service. There is no interactive \
             chat thread — the user cannot see your output directly. You MUST use the \
             core.ask_user tool whenever you need to communicate with or ask something \
             of the user. After completing a task, you will remain active and may \
             receive additional instructions. Stay ready."
        } else {
            "You are a one-shot agent. There is no interactive chat thread — the user \
             cannot see your output directly. You MUST use the core.ask_user tool \
             whenever you need to communicate with or ask something of the user. \
             Complete your task, send your results back, and you will be automatically \
             terminated."
        };

        system_parts.push(format!("{reply_target} {lifecycle_hint}"));

        let mut history = if system_parts.is_empty() {
            Vec::new()
        } else {
            vec![CompletionMessage {
                role: "system".to_string(),
                content: system_parts.join("\n\n"),
                content_parts: vec![],
            }]
        };

        // For keep_alive agents, include accumulated conversation history
        if self.spec.keep_alive && !self.conversation_history.is_empty() {
            history.extend(self.conversation_history.clone());
        }
        let mut personas = self.personas.as_ref().map(|p| p.lock().clone()).unwrap_or_default();
        if !personas.iter().any(|p| p.id == "system/general") {
            personas.insert(0, Persona::default_persona());
        }

        // If we have a journal with entries, reconstruct the prompt to resume mid-task
        let (prompt, initial_tool_iterations) = if let Some(ref journal) = self.conversation_journal
        {
            let j = journal.lock();
            if !j.entries.is_empty() {
                let reconstructed = j.reconstruct_react_prompt(content);
                let iterations = j.tool_iteration_count();
                (reconstructed, iterations)
            } else {
                (content.to_string(), 0)
            }
        } else {
            (content.to_string(), 0)
        };

        // Create a fresh journal or use the existing one for continued recording
        let conversation_journal = Some(
            self.conversation_journal
                .clone()
                .unwrap_or_else(|| Arc::new(Mutex::new(ConversationJournal::default()))),
        );

        LoopContext {
            conversation: ConversationContext {
                session_id: self.session_id.clone(),
                message_id: Uuid::new_v4().to_string(),
                prompt,
                prompt_content_parts: vec![],
                history,
                conversation_journal,
                initial_tool_iterations,
            },
            routing: RoutingConfig {
                required_capabilities,
                preferred_models: self.resolve_preferred_models(),
                loop_strategy: self.spec.loop_strategy.clone(),
                routing_decision: None,
            },
            security: SecurityContext {
                data_class: self.spec.data_class,
                permissions: self
                    .permissions
                    .clone()
                    .unwrap_or_else(|| Arc::new(Mutex::new(SessionPermissions::default()))),
                workspace_classification: None,
                effective_data_class: Arc::new(AtomicU8::new(self.spec.data_class.to_i64() as u8)),
                connector_service: None,
                shadow_mode: self.spec.shadow_mode,
            },
            tools_ctx: ToolsContext {
                tools,
                skill_catalog: self.skill_catalog.clone(),
                knowledge_query_handler: self.knowledge_query_handler.clone(),
                tool_execution_mode: self.spec.tool_execution_mode.unwrap_or_default(),
            },
            agent: AgentContext {
                persona: None,
                agent_orchestrator: self.agent_orchestrator.clone(),
                personas,
                current_agent_id: Some(self.spec.id.clone()),
                parent_agent_id,
                workspace_path: self.workspace_path.clone(),
                keep_alive: self.spec.keep_alive,
                session_messaged: Arc::new(AtomicBool::new(false)),
            },
            tool_limits: self.spec.tool_limits.clone().unwrap_or_default(),
            preempt_signal: None,
            cancellation_token: Some(self.cancellation_token.clone()),
        }
    }

    /// Convert the agent's single model spec into a preferred_models pattern
    /// list for the model router.  The model string is treated as a glob
    /// pattern (e.g. `gpt-5.*`) and may include a `provider:` prefix.
    fn resolve_preferred_models(&self) -> Option<Vec<String>> {
        // Prefer the full list when available; fall back to the single model.
        if let Some(ref models) = self.spec.preferred_models {
            if !models.is_empty() {
                return Some(models.clone());
            }
        }
        let raw = self.spec.model.as_deref()?.trim();
        if raw.is_empty() {
            return None;
        }
        Some(vec![raw.to_string()])
    }

    fn filter_tools(&self) -> Arc<ToolRegistry> {
        let Some(base_tools) = self.tools.as_ref() else {
            return Arc::new(ToolRegistry::new());
        };

        let all_tools_allowed = self.spec.allowed_tools.iter().any(|tool_id| tool_id == "*");

        // If everything is allowed, return as-is
        if all_tools_allowed {
            return Arc::clone(base_tools);
        }

        let mut filtered = ToolRegistry::new();

        // Explicit tool allowlist
        for tool_id in &self.spec.allowed_tools {
            let Some(tool) = base_tools.get(tool_id) else {
                debug!(agent = %self.spec.id, tool_id, "agent requested unknown tool");
                continue;
            };
            if let Err(error) = filtered.register(tool) {
                debug!(agent = %self.spec.id, tool_id, %error, "failed to register filtered tool");
            }
        }

        Arc::new(filtered)
    }

    fn placeholder_task_result(&self, content: &str) -> String {
        format!("[{}] processed: {}", self.spec.name, &preview(content, 100))
    }

    fn agent_workspace_path(&self) -> Option<PathBuf> {
        self.workspace_path.clone()
    }

    fn emit(&self, event: SupervisorEvent) {
        let _ = self.event_tx.send(event);
    }
}

fn convert_loop_event(event: LoopEvent, prompt_preview: &str, agent_id: &str) -> ReasoningEvent {
    match event {
        LoopEvent::ModelLoading { provider_id, model, tool_result_counts, estimated_tokens } => ReasoningEvent::ModelCallStarted {
            model: format!("{provider_id}:{model}"),
            prompt_preview: prompt_preview.to_string(),
            tool_result_counts,
            estimated_tokens,
        },
        LoopEvent::Token { delta } => ReasoningEvent::TokenDelta { token: delta },
        LoopEvent::ModelDone { content, provider_id, model } => {
            ReasoningEvent::ModelCallCompleted {
                token_count: content.split_whitespace().count() as u32,
                model: format!("{provider_id}:{model}"),
                content,
            }
        }
        LoopEvent::ToolCallStart { tool_id, input } => {
            ReasoningEvent::ToolCallStarted { tool_id, input: parse_json_or_text(&input) }
        }
        LoopEvent::ToolCallResult { tool_id, output, is_error } => {
            ReasoningEvent::ToolCallCompleted {
                tool_id,
                output: parse_json_or_text(&output),
                is_error,
            }
        }
        LoopEvent::UserInteractionRequired { request_id, kind } => match kind {
            InteractionKind::ToolApproval { tool_id, input, reason, .. } => {
                ReasoningEvent::UserInteractionRequired { request_id, tool_id, input, reason }
            }
            InteractionKind::Question { text, choices, allow_freeform, multi_select, message } => {
                ReasoningEvent::QuestionAsked {
                    request_id,
                    agent_id: agent_id.to_string(),
                    text,
                    choices,
                    allow_freeform,
                    multi_select,
                    message,
                }
            }
            InteractionKind::AppToolCall { tool_name, .. } => {
                ReasoningEvent::ToolCallStarted {
                    tool_id: format!("app.{tool_name}"),
                    input: serde_json::json!({}),
                }
            }
        },
        LoopEvent::Done { content, .. } => ReasoningEvent::Completed { result: content },
        LoopEvent::Error { message, error_code, http_status, provider_id, model } => ReasoningEvent::Failed {
            error: message,
            error_code,
            http_status,
            provider_id,
            model,
        },
        LoopEvent::ModelRetry { provider_id, model, attempt, max_attempts, error_kind, http_status, backoff_ms } => {
            ReasoningEvent::ModelRetry {
                provider_id,
                model,
                attempt,
                max_attempts,
                error_kind,
                http_status,
                backoff_ms,
            }
        }
        LoopEvent::AgentSessionMessage { from_agent_id, content } => {
            ReasoningEvent::ToolCallCompleted {
                tool_id: format!("core.message_session(from={from_agent_id})"),
                output: json!({ "from": from_agent_id, "message": content }),
                is_error: false,
            }
        }
        LoopEvent::ModelFallback { from_provider, from_model, to_provider, to_model } => {
            ReasoningEvent::PathAbandoned {
                reason: format!(
                    "Model {from_provider}:{from_model} unavailable, fell back to {to_provider}:{to_model}"
                ),
            }
        }
        LoopEvent::BudgetExtended { new_budget, extensions_granted } => {
            ReasoningEvent::PathAbandoned {
                reason: format!(
                    "Tool-call budget extended to {new_budget} (extension #{extensions_granted})"
                ),
            }
        }
        LoopEvent::StallWarning { tool_name, repeated_count } => {
            ReasoningEvent::PathAbandoned {
                reason: format!(
                    "Stall warning: `{tool_name}` called {repeated_count} times with identical arguments"
                ),
            }
        }
        LoopEvent::Preempted => {
            ReasoningEvent::PathAbandoned {
                reason: "Turn preempted: a new user message is waiting".to_string(),
            }
        }
        LoopEvent::ToolCallArgDelta { index, call_id, tool_name, arguments_so_far } => {
            ReasoningEvent::ToolCallArgDelta {
                index,
                call_id,
                tool_name,
                arguments_so_far,
            }
        }
    }
}

fn parse_json_or_text(value: &str) -> Value {
    serde_json::from_str(value).unwrap_or_else(|_| json!({ "text": value }))
}

fn preview(content: &str, max_chars: usize) -> String {
    let trimmed = content.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    format!("{}…", trimmed.chars().take(max_chars).collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::AgentRole;
    use hive_classification::{ChannelClass, DataClass};
    use hive_contracts::{ToolAnnotations, ToolApproval, ToolDefinition};

    use hive_tools::{Tool, ToolResult};
    use serde_json::json;

    struct TestTool {
        definition: ToolDefinition,
    }

    impl TestTool {
        fn new(id: &str) -> Self {
            Self {
                definition: ToolDefinition {
                    id: id.to_string(),
                    name: id.to_string(),
                    description: format!("tool {id}"),
                    input_schema: json!({ "type": "object" }),
                    output_schema: None,
                    channel_class: ChannelClass::Internal,
                    side_effects: false,
                    approval: ToolApproval::Auto,
                    annotations: ToolAnnotations {
                        title: id.to_string(),
                        read_only_hint: Some(true),
                        destructive_hint: Some(false),
                        idempotent_hint: Some(true),
                        open_world_hint: Some(false),
                    },
                },
            }
        }
    }

    impl Tool for TestTool {
        fn definition(&self) -> &ToolDefinition {
            &self.definition
        }

        fn execute(
            &self,
            _input: Value,
        ) -> hive_tools::BoxFuture<'_, Result<ToolResult, hive_tools::ToolError>> {
            Box::pin(async {
                Ok(ToolResult { output: json!({ "ok": true }), data_class: DataClass::Internal })
            })
        }
    }

    fn make_runner(spec: AgentSpec, tools: Arc<ToolRegistry>) -> AgentRunner {
        let (_inbox_tx, inbox_rx) = mpsc::channel(8);
        let (event_tx, _) = broadcast::channel(8);
        AgentRunner::with_executor(
            spec,
            inbox_rx,
            event_tx,
            Arc::new(LoopExecutor::new(Arc::new(hive_loop::ReActStrategy))),
            Arc::new(ArcSwap::from_pointee(ModelRouter::new())),
            tools,
            "session-1".to_string(),
            PathBuf::from("/tmp/hive-agent-tests"),
        )
    }

    #[test]
    fn filter_tools_respects_allowed_tools() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(TestTool::new("allowed.tool"))).unwrap();
        registry.register(Arc::new(TestTool::new("blocked.tool"))).unwrap();

        let runner = make_runner(
            AgentSpec {
                id: "agent-1".to_string(),
                name: "Agent 1".to_string(),
                friendly_name: "test_agent".to_string(),
                description: String::new(),
                role: AgentRole::Researcher,
                model: None,
                preferred_models: None,
                loop_strategy: None,
                tool_execution_mode: None,
                system_prompt: "You are testing".to_string(),
                allowed_tools: vec!["allowed.tool".to_string()],
                avatar: None,
                color: None,
                data_class: hive_classification::DataClass::Public,
                keep_alive: false,
                idle_timeout_secs: None,
                tool_limits: None,
                persona_id: None,
                workflow_managed: false,
                shadow_mode: false,
            },
            Arc::new(registry),
        );

        let filtered = runner.filter_tools();
        let definitions = filtered.list_definitions();
        assert_eq!(definitions.len(), 1);
        assert_eq!(definitions[0].id, "allowed.tool");
    }

    #[test]
    fn filter_tools_wildcard_mcp_servers_preserves_mcp_tools() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(TestTool::new("filesystem.read"))).unwrap();
        registry.register(Arc::new(TestTool::new("mcp.my-server.get_data"))).unwrap();
        registry.register(Arc::new(TestTool::new("mcp.other-server.do_thing"))).unwrap();

        let runner = make_runner(
            AgentSpec {
                id: "agent-1".to_string(),
                name: "Agent 1".to_string(),
                friendly_name: "test_agent".to_string(),
                description: String::new(),
                role: AgentRole::Researcher,
                model: None,
                preferred_models: None,
                loop_strategy: None,
                tool_execution_mode: None,
                system_prompt: "You are testing".to_string(),
                allowed_tools: vec!["*".to_string()],
                avatar: None,
                color: None,
                data_class: hive_classification::DataClass::Public,
                keep_alive: false,
                idle_timeout_secs: None,
                tool_limits: None,
                persona_id: None,
                workflow_managed: false,
                shadow_mode: false,
            },
            Arc::new(registry),
        );

        let filtered = runner.filter_tools();
        let definitions = filtered.list_definitions();
        let ids: Vec<&str> = definitions.iter().map(|d| d.id.as_str()).collect();
        assert!(ids.contains(&"filesystem.read"), "built-in tools should be kept");
        assert!(ids.contains(&"mcp.my-server.get_data"), "MCP tools should be kept with wildcard");
        assert!(
            ids.contains(&"mcp.other-server.do_thing"),
            "MCP tools should be kept with wildcard"
        );
        assert_eq!(definitions.len(), 3);
    }

    #[test]
    fn resolve_preferred_models_from_agent_spec() {
        let runner = make_runner(
            AgentSpec {
                id: "agent-1".to_string(),
                name: "Agent 1".to_string(),
                friendly_name: "test_agent".to_string(),
                description: String::new(),
                role: AgentRole::Researcher,
                model: Some("test-model".to_string()),
                preferred_models: None,
                loop_strategy: None,
                tool_execution_mode: None,
                system_prompt: "You are testing".to_string(),
                allowed_tools: vec![],
                avatar: None,
                color: None,
                data_class: hive_classification::DataClass::Public,
                keep_alive: false,
                idle_timeout_secs: None,
                tool_limits: None,
                persona_id: None,
                workflow_managed: false,
                shadow_mode: false,
            },
            Arc::new(ToolRegistry::new()),
        );

        let models = runner.resolve_preferred_models().expect("preferred models");
        assert_eq!(models, vec!["test-model".to_string()]);
    }

    #[test]
    fn resolve_preferred_models_uses_full_list_over_single_model() {
        let runner = make_runner(
            AgentSpec {
                id: "agent-1".to_string(),
                name: "Agent 1".to_string(),
                friendly_name: "test_agent".to_string(),
                description: String::new(),
                role: AgentRole::Researcher,
                model: Some("fallback-model".to_string()),
                preferred_models: Some(vec!["gpt-5.*".to_string(), "claude-*".to_string()]),
                loop_strategy: None,
                tool_execution_mode: None,
                system_prompt: "You are testing".to_string(),
                allowed_tools: vec![],
                avatar: None,
                color: None,
                data_class: hive_classification::DataClass::Public,
                keep_alive: false,
                idle_timeout_secs: None,
                tool_limits: None,
                persona_id: None,
                workflow_managed: false,
                shadow_mode: false,
            },
            Arc::new(ToolRegistry::new()),
        );

        let models = runner.resolve_preferred_models().expect("preferred models");
        assert_eq!(models, vec!["gpt-5.*".to_string(), "claude-*".to_string()]);
    }

    #[test]
    fn resolve_preferred_models_falls_back_to_single_model_when_list_empty() {
        let runner = make_runner(
            AgentSpec {
                id: "agent-1".to_string(),
                name: "Agent 1".to_string(),
                friendly_name: "test_agent".to_string(),
                description: String::new(),
                role: AgentRole::Researcher,
                model: Some("fallback-model".to_string()),
                preferred_models: Some(vec![]),
                loop_strategy: None,
                tool_execution_mode: None,
                system_prompt: "You are testing".to_string(),
                allowed_tools: vec![],
                avatar: None,
                color: None,
                data_class: hive_classification::DataClass::Public,
                keep_alive: false,
                idle_timeout_secs: None,
                tool_limits: None,
                persona_id: None,
                workflow_managed: false,
                shadow_mode: false,
            },
            Arc::new(ToolRegistry::new()),
        );

        let models = runner.resolve_preferred_models().expect("preferred models");
        assert_eq!(models, vec!["fallback-model".to_string()]);
    }

    #[test]
    fn bot_config_to_agent_spec_preserves_preferred_models() {
        use crate::{BotConfig, BotMode};

        let config = BotConfig {
            id: "bot-123".to_string(),
            friendly_name: "Test Bot".to_string(),
            description: "A test bot".to_string(),
            avatar: Some("🤖".to_string()),
            color: Some("#ff0000".to_string()),
            model: Some("gpt-5.*".to_string()),
            preferred_models: Some(vec![
                "gpt-5.*".to_string(),
                "claude-*".to_string(),
                "local:llama-*".to_string(),
            ]),
            loop_strategy: None,
            tool_execution_mode: None,
            system_prompt: "You are a test bot".to_string(),
            launch_prompt: "Do the thing".to_string(),
            allowed_tools: vec!["*".to_string()],
            data_class: hive_classification::DataClass::Public,
            role: AgentRole::Custom("test-persona".to_string()),
            mode: BotMode::OneShot,
            active: true,
            created_at: String::new(),
            timeout_secs: Some(60),
            permission_rules: vec![],
            tool_limits: None,
            persona_id: None,
        };

        let spec = config.to_agent_spec();
        assert_eq!(spec.model, Some("gpt-5.*".to_string()));
        assert_eq!(
            spec.preferred_models,
            Some(vec!["gpt-5.*".to_string(), "claude-*".to_string(), "local:llama-*".to_string(),])
        );
        assert!(!spec.keep_alive, "OneShot should set keep_alive=false");
    }

    #[tokio::test]
    async fn set_cancellation_token_replaces_default() {
        let (_inbox_tx, inbox_rx) = mpsc::channel(8);
        let (event_tx, _) = broadcast::channel(8);
        let mut runner = AgentRunner::new(
            AgentSpec {
                id: "agent-1".to_string(),
                name: "Agent 1".to_string(),
                friendly_name: "test_agent".to_string(),
                description: String::new(),
                role: AgentRole::Researcher,
                model: None,
                preferred_models: None,
                loop_strategy: None,
                tool_execution_mode: None,
                system_prompt: String::new(),
                allowed_tools: vec![],
                avatar: None,
                color: None,
                data_class: hive_classification::DataClass::Public,
                keep_alive: false,
                idle_timeout_secs: None,
                tool_limits: None,
                persona_id: None,
                workflow_managed: false,
                shadow_mode: false,
            },
            inbox_rx,
            event_tx,
        );

        // Default token should not be cancelled
        assert!(!runner.cancellation_token.is_cancelled());

        // Replace with a pre-cancelled token
        let external_token = CancellationToken::new();
        external_token.cancel();
        runner.set_cancellation_token(external_token.clone());

        assert!(runner.cancellation_token.is_cancelled());
    }

    #[tokio::test]
    async fn runner_kill_signal_emits_done_status() {
        let (inbox_tx, inbox_rx) = mpsc::channel(8);
        let (event_tx, mut event_rx) = broadcast::channel(32);
        let runner = AgentRunner::new(
            AgentSpec {
                id: "test-runner".to_string(),
                name: "Test Runner".to_string(),
                friendly_name: "test_runner".to_string(),
                description: String::new(),
                role: AgentRole::Researcher,
                model: None,
                preferred_models: None,
                loop_strategy: None,
                tool_execution_mode: None,
                system_prompt: String::new(),
                allowed_tools: vec![],
                avatar: None,
                color: None,
                data_class: hive_classification::DataClass::Public,
                keep_alive: false,
                idle_timeout_secs: None,
                tool_limits: None,
                persona_id: None,
                workflow_managed: false,
                shadow_mode: false,
            },
            inbox_rx,
            event_tx,
        );

        let handle = tokio::spawn(runner.run());

        // Send Kill signal
        inbox_tx.send(AgentMessage::Control(ControlSignal::Kill)).await.unwrap();

        // Runner should complete
        tokio::time::timeout(std::time::Duration::from_secs(2), handle)
            .await
            .expect("runner should exit promptly on Kill")
            .unwrap();

        // Should have received Waiting (initial) and Done status events
        let mut got_done = false;
        while let Ok(event) = event_rx.try_recv() {
            if let SupervisorEvent::AgentStatusChanged { status: AgentStatus::Done, .. } = event {
                got_done = true;
            }
        }
        assert!(got_done, "should emit Done status on Kill");
    }

    #[tokio::test]
    async fn runner_pre_cancelled_token_exits_on_next_task() {
        // Without a real executor, execute_task returns a placeholder result
        // immediately (token select is never reached). The runner will emit
        // AgentCompleted with the placeholder and exit (keep_alive=false).
        // This verifies the runner doesn't hang when the token is pre-cancelled.
        let (inbox_tx, inbox_rx) = mpsc::channel(8);
        let (event_tx, mut event_rx) = broadcast::channel(32);

        let token = CancellationToken::new();
        token.cancel();

        let mut runner = AgentRunner::new(
            AgentSpec {
                id: "precancel-agent".to_string(),
                name: "Pre Cancel".to_string(),
                friendly_name: "precancel".to_string(),
                description: String::new(),
                role: AgentRole::Researcher,
                model: None,
                preferred_models: None,
                loop_strategy: None,
                tool_execution_mode: None,
                system_prompt: String::new(),
                allowed_tools: vec![],
                avatar: None,
                color: None,
                data_class: hive_classification::DataClass::Public,
                keep_alive: false,
                idle_timeout_secs: None,
                tool_limits: None,
                persona_id: None,
                workflow_managed: false,
                shadow_mode: false,
            },
            inbox_rx,
            event_tx,
        );
        runner.set_cancellation_token(token);

        let handle = tokio::spawn(runner.run());

        // Send a task — without executor, returns placeholder immediately
        inbox_tx
            .send(AgentMessage::Task { content: "do something".to_string(), from: None })
            .await
            .unwrap();

        // Runner should complete quickly (no executor → placeholder → Done)
        tokio::time::timeout(std::time::Duration::from_secs(2), handle)
            .await
            .expect("runner should exit promptly")
            .unwrap();

        // Check for AgentCompleted event (placeholder result, not "Killed" since no executor)
        let mut got_completed = false;
        while let Ok(event) = event_rx.try_recv() {
            if let SupervisorEvent::AgentCompleted { .. } = event {
                got_completed = true;
            }
        }
        assert!(got_completed, "should produce AgentCompleted event");
    }
}
