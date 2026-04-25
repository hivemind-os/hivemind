mod traits;
mod triggers;

pub use traits::*;
pub use triggers::TriggerManager;
pub use triggers::{ActiveEventGateSnapshot, ActiveTriggerSnapshot, ActiveTriggersResponse};

// Re-export core types for consumers
pub use hive_workflow;

use hive_core::EventBus;
use hive_workflow::*;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, Instrument};

/// Returns true for sentinel parent_session_id values that do NOT correspond
/// to a real chat session (e.g. trigger-launched or test-runner workflows).
fn is_synthetic_session_id(id: &str) -> bool {
    id.starts_with("trigger-") || id == "test-runner"
}

/// A pending workflow feedback gate request, surfaced to the parent session.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkflowFeedbackItem {
    pub instance_id: i64,
    pub step_id: String,
    pub definition_name: String,
    pub prompt: String,
    pub choices: Vec<String>,
    pub allow_freeform: bool,
    pub parent_session_id: String,
}

// ---------------------------------------------------------------------------
// EventBus-backed WorkflowEventEmitter
// ---------------------------------------------------------------------------

/// Publishes workflow events to the hivemind event bus.
pub struct EventBusEmitter {
    bus: EventBus,
}

impl EventBusEmitter {
    pub fn new(bus: EventBus) -> Self {
        Self { bus }
    }
}

#[async_trait::async_trait]
impl WorkflowEventEmitter for EventBusEmitter {
    async fn emit(&self, event: WorkflowEvent) {
        let (topic, payload) = match &event {
            WorkflowEvent::InstanceCreated {
                instance_id,
                definition_name,
                parent_session_id,
                mode,
                execution_mode,
            } => (
                "workflow.instance.created",
                json!({
                    "instance_id": instance_id,
                    "definition_name": definition_name,
                    "parent_session_id": parent_session_id,
                    "mode": mode,
                    "execution_mode": execution_mode,
                }),
            ),
            WorkflowEvent::InstanceStarted { instance_id } => {
                ("workflow.instance.started", json!({ "instance_id": instance_id }))
            }
            WorkflowEvent::InstancePaused { instance_id } => {
                ("workflow.instance.paused", json!({ "instance_id": instance_id }))
            }
            WorkflowEvent::InstanceResumed { instance_id } => {
                ("workflow.instance.resumed", json!({ "instance_id": instance_id }))
            }
            WorkflowEvent::InstanceCompleted { instance_id, output, result_message } => (
                "workflow.instance.completed",
                json!({ "instance_id": instance_id, "output": output, "result_message": result_message }),
            ),
            WorkflowEvent::InstanceFailed { instance_id, error } => {
                ("workflow.instance.failed", json!({ "instance_id": instance_id, "error": error }))
            }
            WorkflowEvent::InstanceKilled { instance_id } => {
                ("workflow.instance.killed", json!({ "instance_id": instance_id }))
            }
            WorkflowEvent::StepStarted { instance_id, step_id } => {
                ("workflow.step.started", json!({ "instance_id": instance_id, "step_id": step_id }))
            }
            WorkflowEvent::StepCompleted { instance_id, step_id, outputs } => (
                "workflow.step.completed",
                json!({ "instance_id": instance_id, "step_id": step_id, "outputs": outputs }),
            ),
            WorkflowEvent::StepFailed { instance_id, step_id, error } => (
                "workflow.step.failed",
                json!({ "instance_id": instance_id, "step_id": step_id, "error": error }),
            ),
            WorkflowEvent::StepWaiting { instance_id, step_id, waiting_type } => (
                "workflow.step.waiting",
                json!({ "instance_id": instance_id, "step_id": step_id, "waiting_type": waiting_type }),
            ),
            WorkflowEvent::InteractionRequested { instance_id, step_id, prompt, choices } => (
                "workflow.interaction.requested",
                json!({
                    "instance_id": instance_id,
                    "step_id": step_id,
                    "prompt": prompt,
                    "choices": choices,
                }),
            ),
            WorkflowEvent::InteractionResponded { instance_id, step_id, .. } => (
                "workflow.interaction.responded",
                json!({ "instance_id": instance_id, "step_id": step_id }),
            ),
            WorkflowEvent::EventGateResolved { instance_id, step_id } => (
                "workflow.event_gate.resolved",
                json!({ "instance_id": instance_id, "step_id": step_id }),
            ),
            WorkflowEvent::TestCaseStarted { definition_name, test_name, index, total } => (
                "workflow.test.case_started",
                json!({ "definition_name": definition_name, "test_name": test_name, "index": index, "total": total }),
            ),
            WorkflowEvent::TestCaseCompleted { definition_name, test_name, passed, duration_ms, index, total } => (
                "workflow.test.case_completed",
                json!({ "definition_name": definition_name, "test_name": test_name, "passed": passed, "duration_ms": duration_ms, "index": index, "total": total }),
            ),
            WorkflowEvent::TestRunCompleted { definition_name, total, passed, failed } => (
                "workflow.test.run_completed",
                json!({ "definition_name": definition_name, "total": total, "passed": passed, "failed": failed }),
            ),
        };
        if let Err(e) = self.bus.publish(topic, "hive-workflow", payload) {
            tracing::warn!("failed to publish workflow event: {e}");
        }
    }
}

/// Wraps an inner emitter and also injects chat-mode workflow results into
/// the parent session's history as notification messages so the main agent
/// has context about completed workflows.
struct ChatInjectingEmitter {
    inner: Arc<dyn WorkflowEventEmitter>,
    agent_runner: Arc<Mutex<Option<Arc<dyn WorkflowAgentRunner>>>>,
    store: Arc<dyn WorkflowPersistence>,
    entity_graph: Arc<parking_lot::Mutex<Option<Arc<hive_core::EntityGraph>>>>,
}

#[async_trait::async_trait]
impl WorkflowEventEmitter for ChatInjectingEmitter {
    async fn emit(&self, event: WorkflowEvent) {
        // Delegate to the inner emitter first
        self.inner.emit(event.clone()).await;

        // On completion/failure/kill of a chat-mode workflow, inject notification
        // into session and kill any child agents owned by the workflow.
        match &event {
            WorkflowEvent::InstanceCompleted { instance_id, result_message, .. } => {
                self.try_inject_result(*instance_id, result_message.as_deref()).await;
                self.cleanup_instance_agents(*instance_id).await;
            }
            WorkflowEvent::InstanceFailed { instance_id, error } => {
                self.try_inject_result(*instance_id, Some(&format!("Workflow failed: {error}")))
                    .await;
                self.cleanup_instance_agents(*instance_id).await;
            }
            WorkflowEvent::InstanceKilled { instance_id } => {
                // The kill() path already does cascade cleanup, but belt-and-suspenders
                // to catch any agents missed by the explicit kill cascade.
                self.cleanup_instance_agents(*instance_id).await;
            }
            WorkflowEvent::InteractionResponded {
                instance_id,
                request_id: Some(request_id),
                response_text,
                ..
            } => {
                let answer = response_text.as_deref().unwrap_or("(answered)");
                self.try_mark_question_answered(*instance_id, request_id, answer).await;
            }
            _ => {}
        }
    }
}

impl ChatInjectingEmitter {
    async fn try_inject_result(&self, instance_id: i64, message: Option<&str>) {
        let message = match message {
            Some(m) if !m.is_empty() => m,
            _ => return,
        };
        // Look up the instance to get session_id and mode
        let instance = match self.store.get_instance(instance_id) {
            Ok(Some(inst)) => inst,
            _ => return,
        };
        // Only inject for chat-mode workflows
        if instance.definition.mode != types::WorkflowMode::Chat {
            return;
        }
        let session_id = &instance.parent_session_id;
        let def_name = &instance.definition.name;
        let runner = self.agent_runner.lock().await.clone();
        if let Some(runner) = runner {
            let content = format!("[Workflow result from: {def_name}]\n{message}");
            if let Err(e) = runner.inject_session_notification(session_id, def_name, &content).await
            {
                tracing::warn!("failed to inject workflow notification into session: {e}");
            }
        }
    }

    /// Mark the chat question message as answered when a feedback gate is resolved.
    async fn try_mark_question_answered(&self, instance_id: i64, request_id: &str, answer: &str) {
        let instance = match self.store.get_instance(instance_id) {
            Ok(Some(inst)) => inst,
            _ => return,
        };
        if instance.definition.mode != types::WorkflowMode::Chat {
            return;
        }
        let session_id = &instance.parent_session_id;
        let runner = self.agent_runner.lock().await.clone();
        if let Some(runner) = runner {
            if let Err(e) =
                runner.mark_session_question_answered(session_id, request_id, answer).await
            {
                tracing::warn!("failed to mark workflow question as answered: {e}");
            }
        }
    }

    /// Kill all child agents owned by a completed/failed/killed workflow instance.
    async fn cleanup_instance_agents(&self, instance_id: i64) {
        let instance = match self.store.get_instance(instance_id) {
            Ok(Some(inst)) => inst,
            _ => return,
        };
        let runner = match self.agent_runner.lock().await.clone() {
            Some(r) => r,
            None => return,
        };
        let session_id = &instance.parent_session_id;

        // Collect agent IDs from step states
        let mut agent_ids: Vec<String> =
            instance.step_states.values().filter_map(|s| s.child_agent_id.clone()).collect();

        // Also collect from entity graph (catches orphaned agents not tracked in step state)
        if let Some(graph) = self.entity_graph.lock().as_ref() {
            let wf_ref = hive_core::workflow_ref(&instance_id.to_string());
            let descendants = graph.descendants(&wf_ref);
            for node in &descendants {
                if let Some((hive_core::EntityType::Agent, id)) =
                    hive_core::parse_entity_ref(&node.entity_id)
                {
                    if !agent_ids.contains(&id.to_string()) {
                        agent_ids.push(id.to_string());
                    }
                }
            }
        }

        if agent_ids.is_empty() {
            return;
        }

        tracing::info!(
            instance_id = %instance_id,
            agent_count = agent_ids.len(),
            "cleaning up child agents for terminal workflow"
        );

        for agent_id in &agent_ids {
            if let Err(e) = runner.kill_agent(session_id, agent_id).await {
                tracing::debug!(
                    agent_id = %agent_id,
                    instance_id = %instance_id,
                    error = %e,
                    "cleanup: failed to kill workflow agent (may already be gone)"
                );
            } else {
                tracing::info!(
                    agent_id = %agent_id,
                    instance_id = %instance_id,
                    "cleanup: killed child agent of completed workflow"
                );
            }
        }

        // Remove the workflow and its descendants from the entity graph
        if let Some(graph) = self.entity_graph.lock().as_ref() {
            graph.remove(&hive_core::workflow_ref(&instance_id.to_string()));
        }
    }
}

/// High-level workflow service managing definitions, instances, and lifecycle.
/// This is the main entry point for the workflow engine, used by the API layer.
pub struct WorkflowService {
    store: Arc<dyn WorkflowPersistence>,
    engine: Arc<WorkflowEngine>,
    attachment_store: AttachmentStore,
    // Shared with ServiceStepExecutor so set_* propagates to the executor.
    tool_executor: Arc<Mutex<Option<Arc<dyn WorkflowToolExecutor>>>>,
    agent_runner: Arc<Mutex<Option<Arc<dyn WorkflowAgentRunner>>>>,
    interaction_gate: Arc<Mutex<Option<Arc<dyn WorkflowInteractionGate>>>>,
    task_scheduler: Arc<Mutex<Option<Arc<dyn WorkflowTaskScheduler>>>>,
    event_gate_registrar: Arc<Mutex<Option<Arc<dyn WorkflowEventGateRegistrar>>>>,
    prompt_renderer: Arc<Mutex<Option<Arc<dyn WorkflowPromptRenderer>>>>,
    entity_graph: Arc<parking_lot::Mutex<Option<Arc<hive_core::EntityGraph>>>>,
    /// Base directory for auto-created workflow workspaces (e.g. `~/.hivemind/workflows`).
    /// When set, trigger-launched workflows that have no explicit workspace will
    /// get `<base>/<instance_id>/workspace` created automatically.
    workspaces_base_dir: Option<PathBuf>,
}

impl WorkflowService {
    /// Create a new workflow service with SQLite-backed persistence.
    pub fn new(
        db_path: impl AsRef<Path>,
        event_bus: Option<EventBus>,
    ) -> Result<Self, WorkflowError> {
        // Derive the attachments base directory from the db_path's parent
        let attachments_base_dir = db_path
            .as_ref()
            .parent()
            .map(|p| p.join("workflow-attachments"))
            .unwrap_or_else(|| PathBuf::from("workflow-attachments"));

        let store: Arc<dyn WorkflowPersistence> = Arc::new(WorkflowStore::new(db_path)?);
        let tool_executor: Arc<Mutex<Option<Arc<dyn WorkflowToolExecutor>>>> =
            Arc::new(Mutex::new(None));
        let agent_runner: Arc<Mutex<Option<Arc<dyn WorkflowAgentRunner>>>> =
            Arc::new(Mutex::new(None));
        let interaction_gate: Arc<Mutex<Option<Arc<dyn WorkflowInteractionGate>>>> =
            Arc::new(Mutex::new(None));
        let task_scheduler: Arc<Mutex<Option<Arc<dyn WorkflowTaskScheduler>>>> =
            Arc::new(Mutex::new(None));
        let event_gate_registrar: Arc<Mutex<Option<Arc<dyn WorkflowEventGateRegistrar>>>> =
            Arc::new(Mutex::new(None));
        let prompt_renderer: Arc<Mutex<Option<Arc<dyn WorkflowPromptRenderer>>>> =
            Arc::new(Mutex::new(None));
        let entity_graph: Arc<parking_lot::Mutex<Option<Arc<hive_core::EntityGraph>>>> =
            Arc::new(parking_lot::Mutex::new(None));

        let executor = Arc::new(ServiceStepExecutor {
            tool_executor: Arc::clone(&tool_executor),
            agent_runner: Arc::clone(&agent_runner),
            interaction_gate: Arc::clone(&interaction_gate),
            task_scheduler: Arc::clone(&task_scheduler),
            event_gate_registrar: Arc::clone(&event_gate_registrar),
            prompt_renderer: Arc::clone(&prompt_renderer),
            entity_graph: Arc::clone(&entity_graph),
            store: Arc::clone(&store),
            engine: std::sync::Mutex::new(None),
        });
        let inner_emitter: Arc<dyn WorkflowEventEmitter> = match event_bus {
            Some(bus) => Arc::new(EventBusEmitter::new(bus)),
            None => Arc::new(NullEventEmitter),
        };
        let emitter: Arc<dyn WorkflowEventEmitter> = Arc::new(ChatInjectingEmitter {
            inner: inner_emitter,
            agent_runner: Arc::clone(&agent_runner),
            store: Arc::clone(&store),
            entity_graph: Arc::clone(&entity_graph),
        });
        let mut engine = WorkflowEngine::new(
            store.clone(),
            Arc::clone(&executor) as Arc<dyn StepExecutor>,
            emitter,
        );
        engine.set_attachments_base_dir(attachments_base_dir.clone());
        let engine = Arc::new(engine);
        *executor.engine.lock().unwrap() = Some(Arc::clone(&engine));

        Ok(Self {
            store,
            engine,
            attachment_store: AttachmentStore::new(attachments_base_dir),
            tool_executor,
            agent_runner,
            interaction_gate,
            task_scheduler,
            event_gate_registrar,
            prompt_renderer,
            entity_graph,
            workspaces_base_dir: None,
        })
    }

    /// Create a new workflow service with in-memory storage (for testing).
    pub fn in_memory() -> Result<Self, WorkflowError> {
        let store: Arc<dyn WorkflowPersistence> = Arc::new(WorkflowStore::in_memory()?);
        let tool_executor: Arc<Mutex<Option<Arc<dyn WorkflowToolExecutor>>>> =
            Arc::new(Mutex::new(None));
        let agent_runner: Arc<Mutex<Option<Arc<dyn WorkflowAgentRunner>>>> =
            Arc::new(Mutex::new(None));
        let interaction_gate: Arc<Mutex<Option<Arc<dyn WorkflowInteractionGate>>>> =
            Arc::new(Mutex::new(None));
        let task_scheduler: Arc<Mutex<Option<Arc<dyn WorkflowTaskScheduler>>>> =
            Arc::new(Mutex::new(None));
        let event_gate_registrar: Arc<Mutex<Option<Arc<dyn WorkflowEventGateRegistrar>>>> =
            Arc::new(Mutex::new(None));
        let prompt_renderer: Arc<Mutex<Option<Arc<dyn WorkflowPromptRenderer>>>> =
            Arc::new(Mutex::new(None));
        let entity_graph: Arc<parking_lot::Mutex<Option<Arc<hive_core::EntityGraph>>>> =
            Arc::new(parking_lot::Mutex::new(None));

        let executor = Arc::new(ServiceStepExecutor {
            tool_executor: Arc::clone(&tool_executor),
            agent_runner: Arc::clone(&agent_runner),
            interaction_gate: Arc::clone(&interaction_gate),
            task_scheduler: Arc::clone(&task_scheduler),
            event_gate_registrar: Arc::clone(&event_gate_registrar),
            prompt_renderer: Arc::clone(&prompt_renderer),
            entity_graph: Arc::clone(&entity_graph),
            store: Arc::clone(&store),
            engine: std::sync::Mutex::new(None),
        });
        let emitter = Arc::new(NullEventEmitter);
        let engine = Arc::new(WorkflowEngine::new(
            store.clone(),
            Arc::clone(&executor) as Arc<dyn StepExecutor>,
            emitter,
        ));
        *executor.engine.lock().unwrap() = Some(Arc::clone(&engine));

        let tmp_att_dir = std::env::temp_dir().join("hive-wf-attachments-test");
        Ok(Self {
            store,
            engine,
            attachment_store: AttachmentStore::new(tmp_att_dir),
            tool_executor,
            agent_runner,
            interaction_gate,
            task_scheduler,
            event_gate_registrar,
            prompt_renderer,
            entity_graph,
            workspaces_base_dir: None,
        })
    }

    /// Create with full dependency injection (event emitter, step executor).
    pub fn with_deps(
        store: Arc<dyn WorkflowPersistence>,
        step_executor: Arc<dyn StepExecutor>,
        event_emitter: Arc<dyn WorkflowEventEmitter>,
    ) -> Self {
        let engine = Arc::new(WorkflowEngine::new(store.clone(), step_executor, event_emitter));
        let tmp_att_dir = std::env::temp_dir().join("hive-wf-attachments");
        Self {
            store,
            engine,
            attachment_store: AttachmentStore::new(tmp_att_dir),
            tool_executor: Arc::new(Mutex::new(None)),
            agent_runner: Arc::new(Mutex::new(None)),
            interaction_gate: Arc::new(Mutex::new(None)),
            task_scheduler: Arc::new(Mutex::new(None)),
            event_gate_registrar: Arc::new(Mutex::new(None)),
            prompt_renderer: Arc::new(Mutex::new(None)),
            entity_graph: Arc::new(parking_lot::Mutex::new(None)),
            workspaces_base_dir: None,
        }
    }

    // -- Extension trait setters (called by hive-api after construction) --

    /// Get a reference to the underlying store.
    pub fn store(&self) -> &Arc<dyn WorkflowPersistence> {
        &self.store
    }

    /// Set the base directory for auto-created workflow workspaces.
    pub fn set_workspaces_base_dir(&mut self, dir: PathBuf) {
        self.workspaces_base_dir = Some(dir);
    }

    pub async fn set_tool_executor(&self, executor: Arc<dyn WorkflowToolExecutor>) {
        *self.tool_executor.lock().await = Some(executor);
    }

    pub async fn set_agent_runner(&self, runner: Arc<dyn WorkflowAgentRunner>) {
        *self.agent_runner.lock().await = Some(runner);
    }

    pub async fn set_interaction_gate(&self, gate: Arc<dyn WorkflowInteractionGate>) {
        *self.interaction_gate.lock().await = Some(gate);
    }

    pub async fn set_task_scheduler(&self, scheduler: Arc<dyn WorkflowTaskScheduler>) {
        *self.task_scheduler.lock().await = Some(scheduler);
    }

    pub async fn set_event_gate_registrar(&self, registrar: Arc<dyn WorkflowEventGateRegistrar>) {
        *self.event_gate_registrar.lock().await = Some(registrar);
    }

    pub async fn set_prompt_renderer(&self, renderer: Arc<dyn WorkflowPromptRenderer>) {
        *self.prompt_renderer.lock().await = Some(renderer);
    }

    pub fn set_entity_graph(&self, graph: Arc<hive_core::EntityGraph>) {
        *self.entity_graph.lock() = Some(graph);
    }

    /// Helper to register an entity if the graph is available.
    fn register_entity(
        &self,
        entity_id: &str,
        entity_type: hive_core::EntityType,
        parent_ref: Option<&str>,
        label: &str,
    ) {
        if let Some(graph) = self.entity_graph.lock().as_ref() {
            graph.register(entity_id, entity_type, parent_ref, label);
        }
    }

    // -----------------------------------------------------------------------
    // Definition CRUD
    // -----------------------------------------------------------------------

    /// Parse and save a workflow definition from YAML source.
    pub async fn save_definition(
        &self,
        yaml_source: &str,
    ) -> Result<WorkflowDefinition, WorkflowError> {
        let _span = tracing::info_span!("service", service = "workflows").entered();
        let mut def: WorkflowDefinition = serde_yaml::from_str(yaml_source)?;
        validate_definition(&def)?;

        // Pin the definition id to the stable DB external_id for existing
        // definitions.  The YAML may omit the `id` field, causing
        // serde(default) to generate a fresh UUID on every save.  The DB
        // preserves the original external_id via ON CONFLICT, but we need the
        // returned struct (and persisted JSON) to carry the correct id so that
        // callers (e.g. trigger registration) can match on it.
        let existing = self.store.get_latest_definition(&def.name)?;
        if let Some((existing_def, _yaml)) = &existing {
            def.id = existing_def.id.clone();
        } else {
            // New definition — validate the name.
            WorkflowDefinition::validate_name(&def.name)
                .map_err(|reason| WorkflowError::ValidationError { reason })?;
        }

        // Minimize lock hold: acquire, write, release immediately.
        {
            let store = &self.store;
            store.save_definition(yaml_source, &def)?;
        }
        info!("Saved workflow definition: {} v{}", def.name, def.version);
        Ok(def)
    }

    /// List all workflow definitions.
    pub async fn list_definitions(&self) -> Result<Vec<WorkflowDefinitionSummary>, WorkflowError> {
        let store = &self.store;
        store.list_definitions()
    }

    /// Get a specific definition by name and version.
    pub async fn get_definition(
        &self,
        name: &str,
        version: &str,
    ) -> Result<(WorkflowDefinition, String), WorkflowError> {
        let store = &self.store;
        store.get_definition(name, version)?.ok_or_else(|| WorkflowError::DefinitionNotFound {
            name: name.to_string(),
            version: version.to_string(),
        })
    }

    /// Get the latest version of a definition.
    pub async fn get_latest_definition(
        &self,
        name: &str,
    ) -> Result<(WorkflowDefinition, String), WorkflowError> {
        let store = &self.store;
        store.get_latest_definition(name)?.ok_or_else(|| WorkflowError::DefinitionNotFound {
            name: name.to_string(),
            version: "latest".to_string(),
        })
    }

    /// Get a definition by its immutable ID.
    pub async fn get_definition_by_id(
        &self,
        id: &str,
    ) -> Result<(WorkflowDefinition, String), WorkflowError> {
        let store = &self.store;
        store.get_definition_by_id(id)?.ok_or_else(|| WorkflowError::DefinitionNotFound {
            name: id.to_string(),
            version: "any".to_string(),
        })
    }

    /// Delete a definition.
    pub async fn delete_definition(
        &self,
        name: &str,
        version: &str,
    ) -> Result<bool, WorkflowError> {
        // Look up the definition to get the ID before deleting, so we can
        // clean up attachment files.
        if let Ok(Some((def, _yaml))) = self.store.get_definition(name, version) {
            let _ = self.attachment_store.delete_version(&def.id, version);
        }
        let store = &self.store;
        store.delete_definition(name, version)
    }

    /// Copy an existing workflow definition under a new namespaced name.
    pub async fn copy_definition(
        &self,
        source_name: &str,
        source_version: Option<&str>,
        new_name: &str,
    ) -> Result<WorkflowDefinition, WorkflowError> {
        // 1. Validate the new name.
        WorkflowDefinition::validate_name(new_name)
            .map_err(|reason| WorkflowError::ValidationError { reason })?;

        // 2. Load the source definition.
        let (source_def, _source_yaml) = match source_version {
            Some(v) => self.get_definition(source_name, v).await?,
            None => self.get_latest_definition(source_name).await?,
        };

        // 3. Build the new definition.
        let mut new_def = source_def.clone();
        new_def.id = generate_workflow_id();
        new_def.name = new_name.to_string();
        new_def.version = "1.0".to_string();
        new_def.bundled = false;
        new_def.archived = false;

        // 4. Persist via the extended save (no factory hash).
        let new_yaml = serde_yaml::to_string(&new_def)?;
        self.store.save_definition_ext(&new_yaml, &new_def, None)?;

        // 5. Copy attachment files from the source to the new workflow.
        let src_dir = self.attachment_store.version_dir(&source_def.id, &source_def.version);
        if src_dir.exists() {
            for entry in std::fs::read_dir(&src_dir).map_err(|e| {
                WorkflowError::Other(format!(
                    "failed to read attachment directory {}: {e}",
                    src_dir.display()
                ))
            })? {
                let entry = entry.map_err(|e| WorkflowError::Other(e.to_string()))?;
                if entry.file_type().map_err(|e| WorkflowError::Other(e.to_string()))?.is_file() {
                    let data = std::fs::read(entry.path()).map_err(|e| {
                        WorkflowError::Other(format!(
                            "failed to read attachment {}: {e}",
                            entry.path().display()
                        ))
                    })?;
                    let filename = entry.file_name().to_string_lossy().to_string();
                    let dst_dir = self.attachment_store.version_dir(&new_def.id, &new_def.version);
                    std::fs::create_dir_all(&dst_dir).map_err(|e| {
                        WorkflowError::Other(format!(
                            "failed to create attachment directory {}: {e}",
                            dst_dir.display()
                        ))
                    })?;
                    let dst_path = dst_dir.join(&filename);
                    std::fs::write(&dst_path, &data).map_err(|e| {
                        WorkflowError::Other(format!(
                            "failed to write attachment {}: {e}",
                            dst_path.display()
                        ))
                    })?;
                }
            }
        }

        info!("Copied workflow '{}' -> '{}' v{}", source_name, new_def.name, new_def.version);
        Ok(new_def)
    }

    // -----------------------------------------------------------------------
    // Bundled (factory-shipped) workflows
    // -----------------------------------------------------------------------

    /// Seed bundled workflows into the database.
    ///
    /// For each bundled workflow YAML:
    /// - If no definition with that name exists → insert with `bundled = true`
    ///   and record the factory YAML hash.
    /// - If a definition exists and its stored YAML matches the factory hash
    ///   (user has not modified it) → auto-update to the new factory YAML.
    /// - If the stored YAML has been modified by the user → leave it alone.
    ///
    /// Returns the number of definitions written.
    pub async fn seed_bundled_workflows(&self) -> Result<usize, WorkflowError> {
        let _span = tracing::info_span!("service", service = "workflows").entered();
        use sha2::{Digest, Sha256};

        let bundled = hive_core::bundled_workflow_yamls();
        let mut written = 0;

        for &(name, factory_yaml) in bundled {
            let factory_hash = format!("{:x}", Sha256::digest(factory_yaml.as_bytes()));

            // Parse the factory YAML to get the version.
            let factory_def: WorkflowDefinition = serde_yaml::from_str(factory_yaml)?;
            let version = &factory_def.version;

            match self.store.get_definition_meta(name, version)? {
                Some((stored_yaml, stored_hash, was_archived)) => {
                    let stored_yaml_hash = format!("{:x}", Sha256::digest(stored_yaml.as_bytes()));
                    let old_factory_hash = stored_hash.unwrap_or_default();

                    if stored_yaml_hash == old_factory_hash || stored_yaml_hash == factory_hash {
                        // User has NOT modified — auto-update if factory changed.
                        if stored_yaml_hash != factory_hash {
                            let mut def: WorkflowDefinition = serde_yaml::from_str(factory_yaml)?;
                            validate_definition(&def)?;
                            def.bundled = true;
                            def.archived = was_archived;
                            self.store.save_definition_ext(
                                factory_yaml,
                                &def,
                                Some(&factory_hash),
                            )?;
                            written += 1;
                            info!(
                                workflow = name,
                                "auto-updated bundled workflow to new factory version"
                            );
                        }
                        // else: already up-to-date
                    }
                    // else: user has modified — leave alone
                }
                None => {
                    // First run — insert.
                    let mut def: WorkflowDefinition = serde_yaml::from_str(factory_yaml)?;
                    validate_definition(&def)?;
                    def.bundled = true;
                    self.store.save_definition_ext(factory_yaml, &def, Some(&factory_hash))?;
                    written += 1;
                    info!(workflow = name, "seeded bundled workflow");
                }
            }
        }

        Ok(written)
    }

    /// Reset a bundled workflow to its factory YAML.
    ///
    /// Returns the freshly saved definition, or `None` if the name is not a
    /// bundled workflow.
    pub async fn reset_bundled_workflow(
        &self,
        name: &str,
    ) -> Result<Option<WorkflowDefinition>, WorkflowError> {
        let _span = tracing::info_span!("service", service = "workflows").entered();
        use sha2::{Digest, Sha256};

        let factory_yaml = match hive_core::bundled_workflow_yaml(name) {
            Some(y) => y,
            None => return Ok(None),
        };

        let mut def: WorkflowDefinition = serde_yaml::from_str(factory_yaml)?;
        validate_definition(&def)?;
        def.bundled = true;
        def.archived = false;

        let factory_hash = format!("{:x}", Sha256::digest(factory_yaml.as_bytes()));
        self.store.save_definition_ext(factory_yaml, &def, Some(&factory_hash))?;
        info!(workflow = name, "reset bundled workflow to factory defaults");
        Ok(Some(def))
    }

    /// Set the `archived` flag on a workflow definition.
    pub async fn archive_definition(
        &self,
        name: &str,
        version: &str,
        archived: bool,
    ) -> Result<bool, WorkflowError> {
        self.store.set_archived(name, version, archived)
    }

    /// Set the `triggers_paused` flag on a workflow definition.
    pub async fn set_triggers_paused(
        &self,
        name: &str,
        version: &str,
        paused: bool,
    ) -> Result<bool, WorkflowError> {
        self.store.set_triggers_paused(name, version, paused)
    }

    // -----------------------------------------------------------------------
    // Attachment management
    // -----------------------------------------------------------------------

    /// Upload an attachment file for a workflow definition.
    pub fn upload_attachment(
        &self,
        workflow_id: &str,
        version: &str,
        attachment_id: &str,
        filename: &str,
        data: &[u8],
    ) -> Result<std::path::PathBuf, WorkflowError> {
        self.attachment_store.store(workflow_id, version, attachment_id, filename, data)
    }

    /// Delete an attachment file for a workflow definition.
    pub fn delete_attachment(
        &self,
        workflow_id: &str,
        version: &str,
        attachment_id: &str,
        filename: &str,
    ) -> Result<(), WorkflowError> {
        self.attachment_store.delete(workflow_id, version, attachment_id, filename)
    }

    /// Get the attachments directory for a workflow definition.
    pub fn attachments_dir(&self, workflow_id: &str, version: &str) -> std::path::PathBuf {
        self.attachment_store.version_dir(workflow_id, version)
    }

    /// Copy all attachment files from one version to another.
    pub fn copy_attachments(
        &self,
        workflow_id: &str,
        from_version: &str,
        to_version: &str,
    ) -> Result<(), WorkflowError> {
        self.attachment_store.copy_version(workflow_id, from_version, to_version)
    }

    // -----------------------------------------------------------------------
    // Instance lifecycle
    // -----------------------------------------------------------------------

    /// Launch a new workflow instance.
    #[allow(clippy::too_many_arguments)]
    pub async fn launch(
        &self,
        name: &str,
        version: Option<&str>,
        inputs: Value,
        parent_session_id: &str,
        parent_agent_id: Option<&str>,
        permission_overrides: Option<Vec<PermissionEntry>>,
        trigger_step_id: Option<&str>,
        workspace_path: Option<&str>,
        execution_mode: ExecutionMode,
    ) -> Result<i64, WorkflowError> {
        let svc_span = tracing::info_span!("service", service = "workflows");
        let (def, _yaml) = if let Some(v) = version {
            self.get_definition(name, v).await?
        } else {
            self.get_latest_definition(name).await?
        };

        let permissions = permission_overrides.unwrap_or_default();

        svc_span.in_scope(|| {
            info!(
                "Launching workflow: {} v{} for session {}",
                def.name, def.version, parent_session_id
            );
        });

        // Resolve the effective workspace: use the caller's workspace if
        // provided, otherwise auto-create one under workspaces_base_dir.
        // Note: we use a temp placeholder for auto-created dirs since
        // the actual instance_id is assigned by the store.
        let effective_workspace = match workspace_path {
            Some(wp) => Some(wp.to_string()),
            None => {
                if let Some(base) = &self.workspaces_base_dir {
                    let ws_dir = base.join(uuid::Uuid::new_v4().to_string()).join("workspace");
                    std::fs::create_dir_all(&ws_dir).map_err(|e| {
                        WorkflowError::Other(format!(
                            "failed to create workflow workspace at {}: {e}",
                            ws_dir.display()
                        ))
                    })?;
                    Some(ws_dir.to_string_lossy().into_owned())
                } else {
                    None
                }
            }
        };

        let id = self
            .engine
            .launch_background(
                def.clone(),
                inputs,
                parent_session_id.to_string(),
                parent_agent_id.map(String::from),
                permissions,
                trigger_step_id.map(String::from),
                effective_workspace,
                execution_mode,
            )
            .await?;

        // Register in entity ownership graph
        let parent_ref = parent_agent_id.map(hive_core::agent_ref).or_else(|| {
            if parent_session_id != "trigger-manager" && parent_session_id != "manual" {
                Some(hive_core::session_ref(parent_session_id))
            } else {
                None
            }
        });
        self.register_entity(
            &hive_core::workflow_ref(&id.to_string()),
            hive_core::EntityType::Workflow,
            parent_ref.as_deref(),
            &def.name,
        );

        Ok(id)
    }

    /// Pause a running workflow instance.
    pub async fn pause(&self, instance_id: i64) -> Result<(), WorkflowError> {
        self.engine.pause(instance_id).await
    }

    /// Resume a paused workflow instance.
    pub async fn resume(&self, instance_id: i64) -> Result<(), WorkflowError> {
        self.engine.resume(instance_id).await
    }

    /// Kill a running workflow instance and cascade to child workflows and agents.
    pub async fn kill(&self, instance_id: i64) -> Result<(), WorkflowError> {
        let svc_span = tracing::info_span!("service", service = "workflows");

        // Signal the engine's killed flag EARLY so any in-flight run_loop
        // iteration notices the kill at its next check and exits promptly,
        // rather than waiting for engine.kill() to acquire the instance lock.
        self.engine.mark_killed(instance_id).await;

        // Clean up any active event gate subscriptions for this instance.
        // Clone the Arc out of the mutex before awaiting to avoid holding
        // the lock across an async call.
        let registrar = self.event_gate_registrar.lock().await.clone();
        if let Some(registrar) = registrar {
            registrar.unregister_instance_gates(instance_id).await;
        }

        // Cascade: kill child workflows and agents from step states.
        // Clone the agent runner Arc once before the loop to avoid
        // re-acquiring the lock on every iteration.
        let agent_runner = self.agent_runner.lock().await.clone();
        if let Ok(Some(instance)) = self.store.get_instance(instance_id) {
            let session_id = &instance.parent_session_id;
            for state in instance.step_states.values() {
                // Kill child workflows recursively
                if let Some(ref child_wf_id) = state.child_workflow_id {
                    if let Err(e) = Box::pin(self.kill(*child_wf_id)).await {
                        svc_span.in_scope(|| {
                            tracing::warn!(
                                child_workflow = %child_wf_id,
                                parent_workflow = %instance_id,
                                error = %e,
                                "failed to cascade-kill child workflow"
                            )
                        });
                    }
                }
                // Kill child agents
                if let Some(ref child_agent_id) = state.child_agent_id {
                    if let Some(ref runner) = agent_runner {
                        if let Err(e) = runner.kill_agent(session_id, child_agent_id).await {
                            svc_span.in_scope(|| {
                                tracing::warn!(
                                    child_agent = %child_agent_id,
                                    parent_workflow = %instance_id,
                                    error = %e,
                                    "failed to cascade-kill child agent"
                                )
                            });
                        }
                    }
                }
            }
        }

        // Belt-and-suspenders: also kill any orphaned descendants found
        // in the entity graph that weren't tracked in step state.
        self.cascade_kill_descendants(instance_id).await;

        // Remove the workflow and all descendants from the ownership graph
        if let Some(graph) = self.entity_graph.lock().as_ref() {
            graph.remove(&hive_core::workflow_ref(&instance_id.to_string()));
        }

        self.engine.kill(instance_id).await
    }

    /// Use the entity graph to discover and kill all descendant agents
    /// and sub-workflows of a workflow instance. This catches children
    /// that may not be tracked in step state (e.g. orphaned by crashes).
    async fn cascade_kill_descendants(&self, instance_id: i64) {
        let svc_span = tracing::info_span!("service", service = "workflows");
        let descendants = {
            let graph = self.entity_graph.lock();
            match graph.as_ref() {
                Some(g) => g.descendants(&hive_core::workflow_ref(&instance_id.to_string())),
                None => return,
            }
        };

        if descendants.is_empty() {
            return;
        }

        // Resolve parent_session_id for agent kills
        let session_id = self
            .store
            .get_instance(instance_id)
            .ok()
            .flatten()
            .map(|inst| inst.parent_session_id.clone());

        // Clone the agent runner Arc once before the loop to avoid
        // holding the lock across async calls.
        let agent_runner = self.agent_runner.lock().await.clone();

        // Process in reverse (leaves first) so children die before parents
        for node in descendants.iter().rev() {
            let Some((entity_type, id)) = hive_core::parse_entity_ref(&node.entity_id) else {
                continue;
            };
            match entity_type {
                hive_core::EntityType::Workflow => {
                    // Kill the child workflow engine state
                    let Ok(child_id) = id.parse::<i64>() else { continue };
                    if let Err(e) = self.engine.kill(child_id).await {
                        svc_span.in_scope(|| {
                            tracing::warn!(
                                child_workflow = %id,
                                parent_workflow = %instance_id,
                                error = %e,
                                "graph cascade: failed to kill child workflow"
                            )
                        });
                    }
                }
                hive_core::EntityType::Agent => {
                    if let Some(ref sid) = session_id {
                        if let Some(ref runner) = agent_runner {
                            if let Err(e) = runner.kill_agent(sid, id).await {
                                svc_span.in_scope(|| {
                                    tracing::warn!(
                                        child_agent = %id,
                                        parent_workflow = %instance_id,
                                        error = %e,
                                        "graph cascade: failed to kill child agent"
                                    )
                                });
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// Kill and delete all workflow instances belonging to a session.
    /// Called during session deletion to prevent orphaned workflows.
    pub async fn cleanup_session_workflows(&self, session_id: &str) {
        let svc_span = tracing::info_span!("service", service = "workflows");
        let filter = types::InstanceFilter {
            parent_session_id: Some(session_id.to_string()),
            ..Default::default()
        };
        let instances = match self.store.list_instances(&filter) {
            Ok(result) => result.items,
            Err(e) => {
                svc_span.in_scope(|| tracing::warn!(session_id, error = %e, "failed to list workflows for session cleanup"));
                return;
            }
        };

        for inst in &instances {
            let is_active = matches!(
                inst.status,
                types::WorkflowStatus::Running
                    | types::WorkflowStatus::Paused
                    | types::WorkflowStatus::Pending
                    | types::WorkflowStatus::WaitingOnInput
                    | types::WorkflowStatus::WaitingOnEvent
            );
            if is_active {
                if let Err(e) = self.kill(inst.id).await {
                    svc_span.in_scope(|| {
                        tracing::warn!(
                            instance_id = %inst.id,
                            session_id,
                            error = %e,
                            "failed to kill workflow during session cleanup"
                        )
                    });
                }
            }
            // Delete the instance from the store (step states cascade via FK)
            if let Err(e) = self.store.delete_instance(inst.id) {
                svc_span.in_scope(|| {
                    tracing::warn!(
                        instance_id = %inst.id,
                        session_id,
                        error = %e,
                        "failed to delete workflow instance during session cleanup"
                    )
                });
            }
        }

        if !instances.is_empty() {
            svc_span.in_scope(|| {
                tracing::info!(
                    session_id,
                    count = instances.len(),
                    "cleaned up workflow instances for deleted session"
                )
            });
        }
    }

    /// Recover orphaned workflow instances after a daemon restart.
    ///
    /// Delegates to [`WorkflowEngine::recover_instances`], which scans the
    /// store for instances that were running or waiting when the previous
    /// process exited and resumes them.
    pub async fn recover(&self) -> Result<usize, WorkflowError> {
        let handles = self.engine.recover_instances().await?;
        let count = handles.len();

        if !handles.is_empty() {
            tokio::spawn(
                async move {
                    for (i, handle) in handles.into_iter().enumerate() {
                        if let Err(e) = handle.await {
                            tracing::error!("workflow recovery task {i} failed: {e}");
                        }
                    }
                }
                .instrument(tracing::info_span!("service", service = "workflows")),
            );
        }

        Ok(count)
    }

    /// Backfill the entity graph with all workflow instances currently in the store.
    pub fn backfill_entity_graph(&self, graph: &hive_core::EntityGraph) {
        let _span = tracing::info_span!("service", service = "workflows").entered();
        let filter = InstanceFilter::default();
        let result = match self.store.list_instances(&filter) {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("entity graph workflow backfill failed: {e}");
                return;
            }
        };
        let mut count = 0usize;
        for inst in &result.items {
            let parent_ref = if is_synthetic_session_id(&inst.parent_session_id)
                || inst.parent_session_id == "manual"
            {
                None
            } else {
                Some(hive_core::session_ref(&inst.parent_session_id))
            };
            graph.register(
                &hive_core::workflow_ref(&inst.id.to_string()),
                hive_core::EntityType::Workflow,
                parent_ref.as_deref(),
                &inst.definition_name,
            );
            count += 1;
        }
        tracing::info!("entity graph workflow backfill: {count} instances");
    }

    /// Get the full state of a workflow instance.
    pub async fn get_instance(&self, instance_id: i64) -> Result<WorkflowInstance, WorkflowError> {
        let store = &self.store;
        store.get_instance(instance_id)?.ok_or(WorkflowError::InstanceNotFound { id: instance_id })
    }

    /// List workflow instances with optional filtering.
    pub async fn list_instances(
        &self,
        filter: &InstanceFilter,
    ) -> Result<InstanceListResult, WorkflowError> {
        let store = &self.store;
        store.list_instances(filter)
    }

    /// Respond to a feedback gate on a specific step.
    pub async fn respond_to_gate(
        &self,
        instance_id: i64,
        step_id: &str,
        response: Value,
    ) -> Result<(), WorkflowError> {
        self.engine.respond_to_gate(instance_id, step_id, response).await
    }

    /// Respond to an event gate on a specific step with event data.
    pub async fn respond_to_event(
        &self,
        instance_id: i64,
        step_id: &str,
        event_data: Value,
    ) -> Result<(), WorkflowError> {
        self.engine.respond_to_event(instance_id, step_id, event_data).await
    }

    /// List intercepted actions for a shadow-mode instance.
    pub async fn list_intercepted_actions(
        &self,
        instance_id: i64,
        limit: usize,
        offset: usize,
    ) -> Result<InterceptedActionPage, WorkflowError> {
        self.store.list_intercepted_actions(instance_id, limit, offset)
    }

    /// Get a summary of intercepted actions for a shadow-mode instance.
    pub async fn get_shadow_summary(
        &self,
        instance_id: i64,
    ) -> Result<ShadowSummary, WorkflowError> {
        self.store.get_shadow_summary(instance_id)
    }

    /// Run workflow unit tests defined on the workflow definition.
    /// Optionally filter to specific test names.
    pub async fn run_tests(
        &self,
        definition_name: &str,
        version: Option<&str>,
        test_names: Option<&[String]>,
        auto_respond: bool,
        cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    ) -> Result<(Vec<hive_workflow::TestResult>, usize), WorkflowError> {
        let (def, _yaml) = if let Some(v) = version {
            self.get_definition(definition_name, v).await?
        } else {
            self.get_latest_definition(definition_name).await?
        };
        if def.tests.is_empty() {
            return Ok((vec![], 0));
        }
        let engine = &self.engine;

        // Collect the test cases to run (respecting optional filter).
        let cases: Vec<&hive_workflow::WorkflowTestCase> = def
            .tests
            .iter()
            .filter(|tc| {
                test_names.map_or(true, |filter| filter.iter().any(|n| n == &tc.name))
            })
            .collect();
        let total = cases.len();

        let mut results = Vec::new();
        for (idx, tc) in cases.iter().enumerate() {
            // Check cancellation before starting next test.
            if let Some(ref flag) = cancel {
                if flag.load(std::sync::atomic::Ordering::Relaxed) {
                    tracing::info!(definition_name, test_name = %tc.name, "test run cancelled before test {}/{}", idx + 1, total);
                    break;
                }
            }

            // Emit "test started" progress event.
            engine
                .emit_event(hive_workflow::WorkflowEvent::TestCaseStarted {
                    definition_name: definition_name.to_string(),
                    test_name: tc.name.clone(),
                    index: idx,
                    total,
                })
                .await;

            let result = {
                // Create a per-test workspace so agents spawned during the
                // test have a workspace_path (and thus use the AgentSpec path
                // that correctly propagates shadow_mode).
                let ws = self.workspaces_base_dir.as_ref().map(|base| {
                    let dir = base.join(format!("test-{}", uuid::Uuid::new_v4())).join("workspace");
                    let _ = std::fs::create_dir_all(&dir);
                    dir.to_string_lossy().into_owned()
                });
                hive_workflow::run_test_case(engine, &def, tc, auto_respond, ws).await?
            };

            // Emit "test completed" progress event.
            engine
                .emit_event(hive_workflow::WorkflowEvent::TestCaseCompleted {
                    definition_name: definition_name.to_string(),
                    test_name: tc.name.clone(),
                    passed: result.passed,
                    duration_ms: result.duration_ms,
                    index: idx,
                    total,
                })
                .await;

            results.push(result);
        }

        // Emit "all done" event.
        let passed = results.iter().filter(|r| r.passed).count();
        let cancelled = results.len() < total;
        engine
            .emit_event(hive_workflow::WorkflowEvent::TestRunCompleted {
                definition_name: definition_name.to_string(),
                total,
                passed,
                failed: results.iter().filter(|r| !r.passed).count(),
            })
            .await;

        if cancelled {
            tracing::info!(definition_name, completed = results.len(), total, "test run cancelled");
        }

        Ok((results, total))
    }

    /// List pending workflow feedback requests for a given parent session.
    /// Returns structured items containing instance_id, step_id, prompt, choices, etc.
    pub async fn list_waiting_feedback_for_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<WorkflowFeedbackItem>, WorkflowError> {
        let store = &self.store;
        let raw = store.list_waiting_feedback_for_session(session_id)?;
        let mut items = Vec::new();
        for (
            instance_id,
            step_id,
            def_json,
            interaction_prompt,
            interaction_choices,
            interaction_allow_freeform,
        ) in raw
        {
            if let Ok(def) = serde_json::from_str::<hive_workflow::WorkflowDefinition>(&def_json) {
                if let Some(step_def) = def.steps.iter().find(|s| s.id == step_id) {
                    if let hive_workflow::StepType::Task {
                        task:
                            hive_workflow::TaskDef::FeedbackGate { prompt, choices, allow_freeform },
                    } = &step_def.step_type
                    {
                        // Use resolved values from step state when available,
                        // falling back to definition template for backward compat.
                        let resolved_prompt = interaction_prompt.unwrap_or_else(|| prompt.clone());
                        let resolved_choices = interaction_choices
                            .and_then(|c| serde_json::from_str::<Vec<String>>(&c).ok())
                            .unwrap_or_else(|| choices.clone().unwrap_or_default());
                        let resolved_allow_freeform =
                            interaction_allow_freeform.unwrap_or(*allow_freeform);

                        items.push(WorkflowFeedbackItem {
                            instance_id,
                            step_id: step_id.clone(),
                            definition_name: def.name.clone(),
                            prompt: resolved_prompt,
                            choices: resolved_choices,
                            allow_freeform: resolved_allow_freeform,
                            parent_session_id: session_id.to_string(),
                        });
                    }
                }
            }
        }
        Ok(items)
    }

    /// List ALL pending feedback gates across all workflow instances.
    pub async fn list_all_waiting_feedback(
        &self,
    ) -> Result<Vec<WorkflowFeedbackItem>, WorkflowError> {
        let store = &self.store;
        let raw = store.list_all_waiting_feedback()?;
        let mut items = Vec::new();
        for (
            instance_id,
            step_id,
            def_json,
            interaction_prompt,
            interaction_choices,
            interaction_allow_freeform,
            parent_session_id,
        ) in raw
        {
            if let Ok(def) = serde_json::from_str::<hive_workflow::WorkflowDefinition>(&def_json) {
                if let Some(step_def) = def.steps.iter().find(|s| s.id == step_id) {
                    if let hive_workflow::StepType::Task {
                        task:
                            hive_workflow::TaskDef::FeedbackGate { prompt, choices, allow_freeform },
                    } = &step_def.step_type
                    {
                        let resolved_prompt = interaction_prompt.unwrap_or_else(|| prompt.clone());
                        let resolved_choices = interaction_choices
                            .and_then(|c| serde_json::from_str::<Vec<String>>(&c).ok())
                            .unwrap_or_else(|| choices.clone().unwrap_or_default());
                        let resolved_allow_freeform =
                            interaction_allow_freeform.unwrap_or(*allow_freeform);

                        items.push(WorkflowFeedbackItem {
                            instance_id,
                            step_id: step_id.clone(),
                            definition_name: def.name.clone(),
                            prompt: resolved_prompt,
                            choices: resolved_choices,
                            allow_freeform: resolved_allow_freeform,
                            parent_session_id: parent_session_id.clone(),
                        });
                    }
                }
            }
        }
        Ok(items)
    }

    /// Return child_agent_ids grouped by instance_id for active workflows.
    pub async fn list_child_agent_ids(
        &self,
    ) -> Result<std::collections::HashMap<i64, Vec<String>>, WorkflowError> {
        self.store.list_child_agent_ids()
    }

    /// Update permissions on an active workflow instance.
    /// Propagates the change to any child workflow instances launched by this
    /// workflow (recursively).
    pub async fn update_permissions(
        &self,
        instance_id: i64,
        permissions: Vec<PermissionEntry>,
    ) -> Result<(), WorkflowError> {
        let store = &self.store;
        let mut instance = store
            .get_instance(instance_id)?
            .ok_or(WorkflowError::InstanceNotFound { id: instance_id })?;
        instance.permissions = permissions.clone();
        instance.updated_at_ms = now_ms();
        store.update_instance(&instance)?;

        // Propagate to child workflows.
        let child_ids: Vec<i64> =
            instance.step_states.values().filter_map(|s| s.child_workflow_id).collect();

        for child_id in child_ids {
            Box::pin(self.update_permissions(child_id, permissions.clone())).await?;
        }

        Ok(())
    }

    /// Delete a workflow instance, cascading to all descendant entities.
    ///
    /// Uses the entity ownership graph to discover child agents and
    /// sub-workflows, kills any that are still active, removes them
    /// from the graph, and then deletes the instance from the store.
    pub async fn delete_instance(&self, instance_id: i64) -> Result<bool, WorkflowError> {
        let instance = match self.store.get_instance(instance_id)? {
            Some(instance) => instance,
            None => return Ok(false),
        };

        let is_active = matches!(
            instance.status,
            types::WorkflowStatus::Running
                | types::WorkflowStatus::Paused
                | types::WorkflowStatus::Pending
                | types::WorkflowStatus::WaitingOnInput
                | types::WorkflowStatus::WaitingOnEvent
        );
        if is_active {
            self.kill(instance_id).await?;
        }

        // Use the entity graph to cascade-kill any live descendants
        self.cascade_kill_descendants(instance_id).await;

        // Remove the entity (and all descendants) from the ownership graph
        if let Some(graph) = self.entity_graph.lock().as_ref() {
            graph.remove(&hive_core::workflow_ref(&instance_id.to_string()));
        }

        let store = &self.store;
        store.delete_instance(instance_id)
    }

    /// Archive or unarchive a workflow instance.
    pub async fn archive_instance(
        &self,
        instance_id: i64,
        archived: bool,
    ) -> Result<bool, WorkflowError> {
        self.store.set_instance_archived(instance_id, archived)
    }
}

// ---------------------------------------------------------------------------
// ServiceStepExecutor — bridges to the extension traits
// ---------------------------------------------------------------------------

struct ServiceStepExecutor {
    tool_executor: Arc<Mutex<Option<Arc<dyn WorkflowToolExecutor>>>>,
    agent_runner: Arc<Mutex<Option<Arc<dyn WorkflowAgentRunner>>>>,
    interaction_gate: Arc<Mutex<Option<Arc<dyn WorkflowInteractionGate>>>>,
    task_scheduler: Arc<Mutex<Option<Arc<dyn WorkflowTaskScheduler>>>>,
    event_gate_registrar: Arc<Mutex<Option<Arc<dyn WorkflowEventGateRegistrar>>>>,
    prompt_renderer: Arc<Mutex<Option<Arc<dyn WorkflowPromptRenderer>>>>,
    entity_graph: Arc<parking_lot::Mutex<Option<Arc<hive_core::EntityGraph>>>>,
    store: Arc<dyn WorkflowPersistence>,
    engine: std::sync::Mutex<Option<Arc<WorkflowEngine>>>,
}

#[async_trait::async_trait]
impl StepExecutor for ServiceStepExecutor {
    async fn call_tool(
        &self,
        tool_id: &str,
        arguments: Value,
        ctx: &ExecutionContext,
    ) -> Result<Value, String> {
        let executor = self.tool_executor.lock().await.clone();
        match executor {
            Some(e) => e.execute_tool(tool_id, arguments, &ctx.permissions).await,
            None => Err("Tool executor not configured".to_string()),
        }
    }

    async fn invoke_agent(
        &self,
        persona_id: &str,
        task: &str,
        async_exec: bool,
        timeout_secs: Option<u64>,
        step_permissions: &[PermissionEntry],
        agent_name: Option<&str>,
        existing_agent_id: Option<&str>,
        ctx: &ExecutionContext,
    ) -> Result<Value, String> {
        let runner = self.agent_runner.lock().await.clone();
        // Use per-step permissions if provided, otherwise fall back to context
        let effective_permissions =
            if step_permissions.is_empty() { &ctx.permissions } else { step_permissions };
        let shadow = ctx.execution_mode == hive_workflow::types::ExecutionMode::Shadow;
        match runner {
            Some(r) => {
                // Recovery path: if we have an existing agent from a previous
                // daemon run, try to signal + wait instead of spawning new.
                if let Some(agent_id) = existing_agent_id {
                    tracing::info!(
                        agent_id = %agent_id,
                        step_id = %ctx.step_id,
                        "resuming existing agent instead of spawning new"
                    );
                    // Signal the (already-restored) agent with the task so
                    // it starts processing. If the agent is dead this will
                    // error and we fall through to a fresh spawn.
                    match r
                        .signal_agent(&SignalTarget::Agent { agent_id: agent_id.to_string() }, task)
                        .await
                    {
                        Ok(_) => {
                            let session_id = if is_synthetic_session_id(&ctx.parent_session_id) {
                                None
                            } else {
                                Some(ctx.parent_session_id.as_str())
                            };
                            return r.wait_for_agent(agent_id, timeout_secs, session_id).await;
                        }
                        Err(e) => {
                            tracing::warn!(
                                agent_id = %agent_id,
                                error = %e,
                                "failed to signal existing agent, falling through to fresh spawn"
                            );
                            // Fall through to normal spawn path below
                        }
                    }
                }

                // Only pass session_id to the agent runner when there is a real
                // chat session backing this workflow.  Trigger-launched workflows
                // use a sentinel parent_session_id that is not a real session.
                // The test runner also uses a synthetic "test-runner" id.
                let session_id = if is_synthetic_session_id(&ctx.parent_session_id) {
                    None
                } else {
                    Some(ctx.parent_session_id.as_str())
                };
                if async_exec {
                    // Async agents: spawn and return immediately.
                    let agent_id = r
                        .spawn_agent(
                            persona_id,
                            task,
                            timeout_secs,
                            ctx.workspace_path.as_deref(),
                            effective_permissions,
                            &ctx.selected_attachments,
                            ctx.attachments_dir.as_deref(),
                            session_id,
                            agent_name,
                            shadow,
                        )
                        .await?;

                    // Persist child_agent_id
                    if let Err(e) =
                        self.store.set_child_agent_id(ctx.instance_id, &ctx.step_id, &agent_id)
                    {
                        tracing::warn!(
                            instance_id = %ctx.instance_id,
                            step_id = %ctx.step_id,
                            agent_id = %agent_id,
                            "failed to persist child_agent_id early: {e}"
                        );
                    }

                    // Register agent in entity graph (parent = workflow instance)
                    if let Some(graph) = self.entity_graph.lock().as_ref() {
                        graph.register(
                            &hive_core::agent_ref(&agent_id),
                            hive_core::EntityType::Agent,
                            Some(&hive_core::workflow_ref(&ctx.instance_id.to_string())),
                            &format!("wf-agent-{}", &agent_id[..8.min(agent_id.len())]),
                        );
                    }

                    Ok(serde_json::json!({
                        "agent_id": agent_id,
                        "status": "spawned",
                    }))
                } else {
                    // Sync agents: spawn, persist child_agent_id immediately
                    // (so enrichment queries can map agent→workflow while running),
                    // then wait for completion.
                    // The on_spawned callback persists the mapping right after
                    // spawn but before the blocking wait.
                    let store = self.store.clone();
                    let cb_instance_id = ctx.instance_id;
                    let cb_step_id = ctx.step_id.clone();
                    let cb_entity_graph = self.entity_graph.lock().clone();
                    let on_spawned: Box<dyn FnOnce(String) + Send + Sync> =
                        Box::new(move |agent_id: String| {
                            if let Err(e) =
                                store.set_child_agent_id(cb_instance_id, &cb_step_id, &agent_id)
                            {
                                tracing::warn!(
                                    instance_id = %cb_instance_id,
                                    step_id = %cb_step_id,
                                    agent_id = %agent_id,
                                    "failed to persist child_agent_id early: {e}"
                                );
                            }
                            // Register agent in entity graph (parent = workflow instance)
                            if let Some(ref graph) = cb_entity_graph {
                                graph.register(
                                    &hive_core::agent_ref(&agent_id),
                                    hive_core::EntityType::Agent,
                                    Some(&hive_core::workflow_ref(&cb_instance_id.to_string())),
                                    &format!("wf-agent-{}", &agent_id[..8.min(agent_id.len())]),
                                );
                            }
                        });

                    let (_agent_id, result, intercepted_calls) = r
                        .spawn_and_wait_agent(
                            persona_id,
                            task,
                            timeout_secs,
                            ctx.workspace_path.as_deref(),
                            effective_permissions,
                            &ctx.selected_attachments,
                            ctx.attachments_dir.as_deref(),
                            session_id,
                            Some(on_spawned),
                            agent_name,
                            shadow,
                            ctx.auto_respond_interactions,
                        )
                        .await?;

                    // Persist intercepted tool calls from shadow-mode agents
                    for ic in intercepted_calls {
                        let action = hive_workflow::InterceptedAction {
                            id: 0,
                            instance_id: ctx.instance_id,
                            step_id: ctx.step_id.clone(),
                            kind: "tool_call".to_string(),
                            timestamp_ms: std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_millis() as u64,
                            details: serde_json::json!({
                                "tool_id": ic.tool_id,
                                "arguments": ic.input,
                            }),
                        };
                        if let Err(e) = self.store.save_intercepted_action(&action) {
                            tracing::warn!(
                                instance_id = %ctx.instance_id,
                                step_id = %ctx.step_id,
                                tool_id = %ic.tool_id,
                                "failed to persist intercepted tool call: {e}"
                            );
                        }
                    }

                    Ok(result)
                }
            }
            None => Err("Agent runner not configured".to_string()),
        }
    }

    async fn signal_agent(
        &self,
        target: &SignalTarget,
        content: &str,
        _ctx: &ExecutionContext,
    ) -> Result<Value, String> {
        let runner = self.agent_runner.lock().await.clone();
        match runner {
            Some(r) => r.signal_agent(target, content).await,
            None => Err("Agent runner not configured".to_string()),
        }
    }

    async fn wait_for_agent(
        &self,
        agent_id: &str,
        timeout_secs: Option<u64>,
        ctx: &ExecutionContext,
    ) -> Result<Value, String> {
        let runner = self.agent_runner.lock().await.clone();
        match runner {
            Some(r) => {
                let session_id = if is_synthetic_session_id(&ctx.parent_session_id) {
                    None
                } else {
                    Some(ctx.parent_session_id.as_str())
                };
                r.wait_for_agent(agent_id, timeout_secs, session_id).await
            }
            None => Err("Agent runner not configured".to_string()),
        }
    }

    async fn create_feedback_request(
        &self,
        instance_id: i64,
        step_id: &str,
        prompt: &str,
        choices: Option<&[String]>,
        allow_freeform: bool,
        ctx: &ExecutionContext,
    ) -> Result<String, String> {
        let gate = self.interaction_gate.lock().await.clone();
        let request_id = match gate {
            Some(g) => {
                g.create_feedback_request(instance_id, step_id, prompt, choices, allow_freeform)
                    .await?
            }
            None => return Err("Interaction gate not configured".to_string()),
        };

        // Inject a question message into the parent chat session so the
        // feedback gate appears inline in the chat timeline.
        // Only inject for chat-mode workflows (same guard as ChatInjectingEmitter).
        let runner = self.agent_runner.lock().await.clone();
        if let Some(runner) = runner {
            let instance = self.store.get_instance(instance_id).ok().flatten();
            let is_chat_mode = instance
                .as_ref()
                .map(|inst| inst.definition.mode == types::WorkflowMode::Chat)
                .unwrap_or(false);
            if is_chat_mode {
                let workflow_name =
                    instance.as_ref().map(|inst| inst.definition.name.as_str()).unwrap_or("");
                if let Err(e) = runner
                    .inject_session_question(
                        &ctx.parent_session_id,
                        &request_id,
                        prompt,
                        choices.unwrap_or(&[]),
                        allow_freeform,
                        instance_id,
                        step_id,
                        workflow_name,
                    )
                    .await
                {
                    tracing::warn!(
                        instance_id,
                        step_id,
                        "failed to inject workflow feedback question into session: {e}"
                    );
                }
            }
        }

        Ok(request_id)
    }

    async fn launch_workflow(
        &self,
        workflow_name: &str,
        inputs: Value,
        ctx: &ExecutionContext,
    ) -> Result<i64, String> {
        // Resolve the latest definition from the store.
        let store = &self.store;
        let (def, _yaml) = store
            .get_latest_definition(workflow_name)
            .map_err(|e| format!("load definition: {e}"))?
            .ok_or_else(|| format!("workflow definition '{workflow_name}' not found"))?;

        let engine = {
            let guard = self.engine.lock().map_err(|e| format!("engine lock: {e}"))?;
            guard.as_ref().ok_or_else(|| "workflow engine not configured".to_string())?.clone()
        };

        let instance_id: i64 = engine
            .launch_background(
                def.clone(),
                inputs,
                ctx.parent_session_id.clone(),
                ctx.parent_agent_id.clone(),
                ctx.permissions.clone(),
                None,
                ctx.workspace_path.clone(),
                ctx.execution_mode,
            )
            .await
            .map_err(|e| format!("launch child workflow: {e}"))?;

        // Register sub-workflow in entity graph (parent = parent workflow)
        if let Some(graph) = self.entity_graph.lock().as_ref() {
            graph.register(
                &hive_core::workflow_ref(&instance_id.to_string()),
                hive_core::EntityType::Workflow,
                Some(&hive_core::workflow_ref(&ctx.instance_id.to_string())),
                &def.name,
            );
        }

        Ok(instance_id)
    }

    async fn schedule_task(
        &self,
        schedule: &ScheduleTaskDef,
        ctx: &ExecutionContext,
    ) -> Result<String, String> {
        let scheduler = self.task_scheduler.lock().await.clone();
        match scheduler {
            Some(s) => {
                let parent_sid = if is_synthetic_session_id(&ctx.parent_session_id) {
                    None
                } else {
                    Some(ctx.parent_session_id.as_str())
                };
                s.schedule_task(schedule, parent_sid, ctx.parent_agent_id.as_deref()).await
            }
            None => Err("Task scheduler not configured".to_string()),
        }
    }

    async fn register_event_gate(
        &self,
        instance_id: i64,
        step_id: &str,
        topic: &str,
        filter: Option<&str>,
        timeout_secs: Option<u64>,
        _ctx: &ExecutionContext,
    ) -> Result<String, String> {
        let registrar = self.event_gate_registrar.lock().await.clone();
        match registrar {
            Some(r) => {
                r.register_event_gate(instance_id, step_id, topic, filter, timeout_secs).await
            }
            None => Err("Event gate registrar not configured".to_string()),
        }
    }

    async fn render_prompt_template(
        &self,
        persona_id: &str,
        prompt_id: &str,
        parameters: Value,
        _ctx: &ExecutionContext,
    ) -> Result<String, String> {
        let renderer = self.prompt_renderer.lock().await.clone();
        match renderer {
            Some(r) => r.render_prompt_template(persona_id, prompt_id, parameters).await,
            None => Err("Prompt renderer not configured".to_string()),
        }
    }

    async fn on_instance_stopped(&self, instance_id: i64) -> Result<(), String> {
        let registrar = self.event_gate_registrar.lock().await.clone();
        if let Some(registrar) = registrar {
            registrar.unregister_instance_gates(instance_id).await;
        }
        Ok(())
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use hive_workflow::executor::{
        ExecutionContext, StepExecutor, WorkflowEngine, WorkflowEventEmitter,
    };
    use hive_workflow::store::WorkflowStore;
    use hive_workflow::types::{
        PermissionEntry, ScheduleTaskDef, SignalTarget, WorkflowEvent, WorkflowStatus,
    };
    use hive_workflow::{validate_definition, WorkflowDefinition, WorkflowPersistence};
    use serde_json::Value;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    #[test]
    fn test_all_bundled_workflows_parse_and_validate() {
        let bundled = hive_core::bundled_workflow_yamls();
        for &(name, yaml) in bundled {
            let def: WorkflowDefinition = serde_yaml::from_str(yaml)
                .unwrap_or_else(|e| panic!("Failed to parse bundled workflow '{}': {}", name, e));
            validate_definition(&def)
                .unwrap_or_else(|e| panic!("Bundled workflow '{}' failed validation: {}", name, e));
        }
    }

    struct MockExecutor;

    #[async_trait]
    impl StepExecutor for MockExecutor {
        async fn call_tool(
            &self,
            _: &str,
            _: Value,
            _: &ExecutionContext,
        ) -> Result<Value, String> {
            Ok(serde_json::json!({"status": "ok"}))
        }
        async fn invoke_agent(
            &self,
            _: &str,
            _: &str,
            _: bool,
            _: Option<u64>,
            _: &[PermissionEntry],
            _: Option<&str>,
            _: Option<&str>,
            _: &ExecutionContext,
        ) -> Result<Value, String> {
            Ok(serde_json::json!({"result": "agent completed"}))
        }
        async fn signal_agent(
            &self,
            _: &SignalTarget,
            _: &str,
            _: &ExecutionContext,
        ) -> Result<Value, String> {
            Ok(Value::Null)
        }
        async fn wait_for_agent(
            &self,
            _: &str,
            _: Option<u64>,
            _: &ExecutionContext,
        ) -> Result<Value, String> {
            Ok(serde_json::json!({"result": "agent completed"}))
        }
        async fn create_feedback_request(
            &self,
            _: i64,
            _: &str,
            _: &str,
            _: Option<&[String]>,
            _: bool,
            _: &ExecutionContext,
        ) -> Result<String, String> {
            Ok("mock-request-id".to_string())
        }
        async fn register_event_gate(
            &self,
            _: i64,
            _: &str,
            _: &str,
            _: Option<&str>,
            _: Option<u64>,
            _: &ExecutionContext,
        ) -> Result<String, String> {
            Ok("mock-sub-id".to_string())
        }
        async fn launch_workflow(
            &self,
            _: &str,
            _: Value,
            _: &ExecutionContext,
        ) -> Result<i64, String> {
            Ok(1001)
        }
        async fn schedule_task(
            &self,
            _: &ScheduleTaskDef,
            _: &ExecutionContext,
        ) -> Result<String, String> {
            Ok("task-001".to_string())
        }
        async fn render_prompt_template(
            &self,
            _persona_id: &str,
            _prompt_id: &str,
            _params: Value,
            _ctx: &ExecutionContext,
        ) -> Result<String, String> {
            Ok("Rendered prompt template content".to_string())
        }
    }

    struct CollectingEmitter {
        events: Mutex<Vec<WorkflowEvent>>,
    }

    impl CollectingEmitter {
        fn new() -> Self {
            Self { events: Mutex::new(Vec::new()) }
        }
    }

    #[async_trait]
    impl WorkflowEventEmitter for CollectingEmitter {
        async fn emit(&self, event: WorkflowEvent) {
            self.events.lock().await.push(event);
        }
    }

    #[tokio::test]
    async fn test_software_feature_workflow_reaches_plan_step() {
        let yaml = hive_core::bundled_workflow_yamls()
            .iter()
            .find(|(name, _)| *name == "system/software/major-feature")
            .expect("software/major-feature workflow not found")
            .1;
        let def: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();

        let store = Arc::new(WorkflowStore::in_memory().unwrap());
        let executor = Arc::new(MockExecutor);
        let emitter = Arc::new(CollectingEmitter::new());
        let engine = WorkflowEngine::new(store.clone(), executor, emitter.clone());

        let inputs = serde_json::json!({
            "feature_description": "Add user authentication",
            "write_spec": false,
            "build_poc": false,
            "update_docs": false,
        });

        let instance_id = engine
            .launch(def, inputs, "test-session".into(), None, vec![], Some("start".to_string()))
            .await
            .unwrap();

        let instance = store.get_instance(instance_id).unwrap().unwrap();

        // Print all step states for debugging
        let mut steps: Vec<(&String, &hive_workflow::types::StepState)> =
            instance.step_states.iter().collect();
        steps.sort_by_key(|(id, _)| (*id).clone());
        for (id, state) in &steps {
            eprintln!(
                "  {} -> {:?}{}",
                id,
                state.status,
                state.error.as_ref().map(|e| format!(" ERROR: {}", e)).unwrap_or_default()
            );
        }

        eprintln!("Workflow status: {:?}", instance.status);

        // The workflow should NOT be completed — it should be waiting on input
        // (feedback gate) or at least have executed the plan step
        assert_ne!(
            instance.status,
            WorkflowStatus::Completed,
            "Workflow should not complete immediately — it should be waiting on the plan step or feedback gate"
        );
    }
}
