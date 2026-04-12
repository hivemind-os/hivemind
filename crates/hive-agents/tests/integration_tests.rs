use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use parking_lot::Mutex;

use arc_swap::ArcSwap;
use hive_agents::{
    AgentError, AgentMessage, AgentRole, AgentSpec, AgentStatus, AgentSupervisor, FlowEdge,
    FlowType, SupervisorEvent, TopologyDef,
};
use hive_contracts::SessionPermissions;
use hive_loop::{LoopExecutor, ReActStrategy};
use hive_model::{Capability, EchoProvider, ModelRouter, ProviderDescriptor, ProviderKind};
use hive_tools::ToolRegistry;
use tempfile::tempdir;
use tokio::sync::broadcast;
use tokio::time::{timeout, Duration};

fn make_spec(id: &str, name: &str, role: AgentRole) -> AgentSpec {
    AgentSpec {
        id: id.to_string(),
        name: name.to_string(),
        friendly_name: format!("{}_{}", id, name.to_lowercase().replace(' ', "_")),
        description: String::new(),
        role,
        model: None,
        preferred_models: None,
        loop_strategy: None,
        tool_execution_mode: None,
        system_prompt: format!("You are {name}"),
        allowed_tools: vec![],
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

/// Drain events from a broadcast receiveruntil the channel is empty or we've
/// collected `max` events. Uses a short timeout per recv to avoid hanging.
async fn collect_events(
    rx: &mut broadcast::Receiver<SupervisorEvent>,
    max: usize,
) -> Vec<SupervisorEvent> {
    let mut events = Vec::new();
    for _ in 0..max {
        match timeout(Duration::from_millis(200), rx.recv()).await {
            Ok(Ok(ev)) => events.push(ev),
            _ => break,
        }
    }
    events
}

// ── 1. Spawn agent ──────────────────────────────────────────────────────────

#[tokio::test]
async fn test_spawn_agent() {
    let sup = AgentSupervisor::new(128, None);
    let spec = make_spec("a1", "Alice", AgentRole::Coder);

    let id = sup.spawn_agent(spec, None, None, None, None).await.unwrap();
    assert_eq!(id, "a1");
    assert_eq!(sup.agent_count(), 1);

    let agents = sup.get_all_agents();
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].agent_id, "a1");

    sup.kill_all().await.unwrap();
}

// ── 2. Send message ─────────────────────────────────────────────────────────

#[tokio::test]
async fn test_send_message() {
    let sup = AgentSupervisor::new(128, None);
    let mut rx = sup.subscribe();
    let spec = make_spec("a1", "Alice", AgentRole::Coder);
    sup.spawn_agent(spec, None, None, None, None).await.unwrap();

    sup.send_to_agent(
        "a1",
        AgentMessage::Task {
            content: "Write hello world".to_string(),
            from: Some("user".to_string()),
        },
    )
    .await
    .unwrap();

    // Give the runner a moment to process
    tokio::time::sleep(Duration::from_millis(100)).await;

    let events = collect_events(&mut rx, 50).await;

    let has_output = events.iter().any(|e| matches!(e, SupervisorEvent::AgentOutput { .. }));
    let has_completed = events.iter().any(|e| matches!(e, SupervisorEvent::AgentCompleted { .. }));
    assert!(has_output, "expected AgentOutput event");
    assert!(has_completed, "expected AgentCompleted event");

    sup.kill_all().await.unwrap();
}

// ── 3. Broadcast ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_broadcast() {
    let sup = AgentSupervisor::new(256, None);
    let mut rx = sup.subscribe();

    for i in 0..3 {
        let spec = make_spec(&format!("a{i}"), &format!("Agent{i}"), AgentRole::Researcher);
        sup.spawn_agent(spec, None, None, None, None).await.unwrap();
    }

    sup.broadcast(AgentMessage::Broadcast {
        content: "Hello everyone".to_string(),
        from: "supervisor".to_string(),
    })
    .await
    .unwrap();

    tokio::time::sleep(Duration::from_millis(200)).await;

    let events = collect_events(&mut rx, 100).await;

    let routed: Vec<_> =
        events.iter().filter(|e| matches!(e, SupervisorEvent::MessageRouted { .. })).collect();
    assert!(routed.len() >= 3, "expected at least 3 MessageRouted events, got {}", routed.len());

    sup.kill_all().await.unwrap();
}

// ── 4. Kill agent ────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_kill_agent() {
    let sup = AgentSupervisor::new(128, None);
    let spec = make_spec("a1", "Alice", AgentRole::Coder);
    sup.spawn_agent(spec, None, None, None, None).await.unwrap();
    assert_eq!(sup.agent_count(), 1);

    sup.kill_agent("a1").await.unwrap();

    assert_eq!(sup.agent_count(), 0);
    assert!(sup.get_agent_status("a1").is_none());
}

// ── 5. Pause / Resume ────────────────────────────────────────────────────────

#[tokio::test]
async fn test_pause_resume() {
    let sup = AgentSupervisor::new(128, None);
    let mut rx = sup.subscribe();
    let spec = make_spec("a1", "Alice", AgentRole::Coder);
    sup.spawn_agent(spec, None, None, None, None).await.unwrap();

    sup.pause_agent("a1").await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let events = collect_events(&mut rx, 50).await;
    let has_paused = events.iter().any(|e| {
        matches!(e, SupervisorEvent::AgentStatusChanged { status: AgentStatus::Paused, .. })
    });
    assert!(has_paused, "expected Paused status event");

    sup.resume_agent("a1").await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let events = collect_events(&mut rx, 50).await;
    let has_waiting = events.iter().any(|e| {
        matches!(e, SupervisorEvent::AgentStatusChanged { status: AgentStatus::Waiting, .. })
    });
    assert!(has_waiting, "expected Waiting status event after resume");

    sup.kill_all().await.unwrap();
}

// ── 6. Pipeline topology ─────────────────────────────────────────────────────

#[tokio::test]
async fn test_pipeline_topology() {
    let sup = AgentSupervisor::new(256, None);
    let mut rx = sup.subscribe();

    let topology = TopologyDef {
        agents: vec![
            make_spec("planner", "Planner", AgentRole::Planner),
            make_spec("coder", "Coder", AgentRole::Coder),
            make_spec("reviewer", "Reviewer", AgentRole::Reviewer),
        ],
        flows: vec![
            FlowEdge {
                from: "planner".to_string(),
                to: vec!["coder".to_string()],
                flow_type: FlowType::Pipeline,
            },
            FlowEdge {
                from: "coder".to_string(),
                to: vec!["reviewer".to_string()],
                flow_type: FlowType::Pipeline,
            },
        ],
    };

    sup.run_topology(topology, "Build a web app".to_string()).await.unwrap();

    tokio::time::sleep(Duration::from_millis(200)).await;

    assert_eq!(sup.agent_count(), 3);

    let events = collect_events(&mut rx, 100).await;

    let spawned_count =
        events.iter().filter(|e| matches!(e, SupervisorEvent::AgentSpawned { .. })).count();
    assert_eq!(spawned_count, 3);

    let routed_to_planner = events.iter().any(|e| {
        matches!(
            e,
            SupervisorEvent::MessageRouted { to, .. } if to == "planner"
        )
    });
    assert!(routed_to_planner, "expected initial task routed to planner");

    sup.kill_all().await.unwrap();
}

// ── 7. Fan-out topology ──────────────────────────────────────────────────────

#[tokio::test]
async fn test_fanout_topology() {
    let sup = AgentSupervisor::new(256, None);

    let topology = TopologyDef {
        agents: vec![
            make_spec("planner", "Planner", AgentRole::Planner),
            make_spec("w1", "Worker1", AgentRole::Researcher),
            make_spec("w2", "Worker2", AgentRole::Researcher),
            make_spec("w3", "Worker3", AgentRole::Researcher),
        ],
        flows: vec![FlowEdge {
            from: "planner".to_string(),
            to: vec!["w1".to_string(), "w2".to_string(), "w3".to_string()],
            flow_type: FlowType::FanOut,
        }],
    };

    sup.run_topology(topology, "Research topic".to_string()).await.unwrap();

    assert_eq!(sup.agent_count(), 4);

    sup.kill_all().await.unwrap();
}

// ── 8. Feedback topology ─────────────────────────────────────────────────────

#[tokio::test]
async fn test_feedback_topology() {
    let sup = AgentSupervisor::new(256, None);
    let mut rx = sup.subscribe();

    let topology = TopologyDef {
        agents: vec![
            make_spec("coder", "Coder", AgentRole::Coder),
            make_spec("reviewer", "Reviewer", AgentRole::Reviewer),
        ],
        flows: vec![
            FlowEdge {
                from: "coder".to_string(),
                to: vec!["reviewer".to_string()],
                flow_type: FlowType::Feedback,
            },
            FlowEdge {
                from: "reviewer".to_string(),
                to: vec!["coder".to_string()],
                flow_type: FlowType::Feedback,
            },
        ],
    };

    sup.run_topology(topology, "Write function".to_string()).await.unwrap();

    tokio::time::sleep(Duration::from_millis(200)).await;

    assert_eq!(sup.agent_count(), 2);

    let events = collect_events(&mut rx, 100).await;
    let spawned_count =
        events.iter().filter(|e| matches!(e, SupervisorEvent::AgentSpawned { .. })).count();
    assert_eq!(spawned_count, 2);

    sup.kill_all().await.unwrap();
}

// ── 9. Concurrent agents ────────────────────────────────────────────────────

#[tokio::test]
async fn test_concurrent_agents() {
    let sup = AgentSupervisor::new(512, None);

    for i in 0..5 {
        let spec = make_spec(&format!("w{i}"), &format!("Worker{i}"), AgentRole::Analyst);
        sup.spawn_agent(spec, None, None, None, None).await.unwrap();
    }

    for i in 0..5 {
        let id = format!("w{i}");
        let msg = AgentMessage::Task {
            content: format!("Analyze dataset {i}"),
            from: Some("supervisor".to_string()),
        };
        sup.send_to_agent(&id, msg).await.unwrap();
    }

    tokio::time::sleep(Duration::from_millis(300)).await;

    // All 5 agents should still be alive (no deadlocks)
    assert_eq!(sup.agent_count(), 5);

    sup.kill_all().await.unwrap();
}

// ── 10. Agent not found ──────────────────────────────────────────────────────

#[tokio::test]
async fn test_agent_not_found() {
    let sup = AgentSupervisor::new(128, None);

    let result = sup
        .send_to_agent(
            "nonexistent",
            AgentMessage::Task { content: "test".to_string(), from: None },
        )
        .await;

    assert!(result.is_err());
    match result.unwrap_err() {
        AgentError::AgentNotFound(id) => assert_eq!(id, "nonexistent"),
        other => panic!("expected AgentNotFound, got {other:?}"),
    }
}

// ── 11. Supervisor events ────────────────────────────────────────────────────

#[tokio::test]
async fn test_supervisor_events() {
    let sup = AgentSupervisor::new(128, None);
    let mut rx = sup.subscribe();

    let spec = make_spec("a1", "Alice", AgentRole::Writer);
    sup.spawn_agent(spec, None, None, None, None).await.unwrap();

    sup.send_to_agent(
        "a1",
        AgentMessage::Task { content: "Write a story".to_string(), from: Some("user".to_string()) },
    )
    .await
    .unwrap();

    tokio::time::sleep(Duration::from_millis(150)).await;

    let events = collect_events(&mut rx, 50).await;

    let event_types: Vec<String> = events
        .iter()
        .map(|e| match e {
            SupervisorEvent::AgentSpawned { .. } => "spawned".to_string(),
            SupervisorEvent::AgentStatusChanged { status, .. } => format!("status:{status:?}"),
            SupervisorEvent::AgentTaskAssigned { .. } => "task_assigned".to_string(),
            SupervisorEvent::MessageRouted { .. } => "routed".to_string(),
            SupervisorEvent::AgentOutput { .. } => "output".to_string(),
            SupervisorEvent::AgentCompleted { .. } => "completed".to_string(),
            SupervisorEvent::AllComplete { .. } => "all_complete".to_string(),
        })
        .collect();

    assert!(event_types.contains(&"spawned".to_string()), "missing spawned event");
    assert!(event_types.contains(&"routed".to_string()), "missing routed event");
    assert!(event_types.contains(&"output".to_string()), "missing output event");
    assert!(event_types.contains(&"completed".to_string()), "missing completed event");

    sup.kill_all().await.unwrap();
}

// ── 12. Kill all ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_kill_all() {
    let sup = AgentSupervisor::new(128, None);

    for i in 0..4 {
        let spec = make_spec(&format!("a{i}"), &format!("Agent{i}"), AgentRole::Coder);
        sup.spawn_agent(spec, None, None, None, None).await.unwrap();
    }
    assert_eq!(sup.agent_count(), 4);

    sup.kill_all().await.unwrap();
    assert_eq!(sup.agent_count(), 0);
}

// ── 13. Duplicate agent ──────────────────────────────────────────────────────

#[tokio::test]
async fn test_duplicate_agent_rejected() {
    let sup = AgentSupervisor::new(128, None);
    let spec = make_spec("a1", "Alice", AgentRole::Coder);

    sup.spawn_agent(spec.clone(), None, None, None, None).await.unwrap();
    let result = sup.spawn_agent(spec, None, None, None, None).await;

    assert!(result.is_err());
    match result.unwrap_err() {
        AgentError::AlreadyExists(id) => assert_eq!(id, "a1"),
        other => panic!("expected AlreadyExists, got {other:?}"),
    }

    sup.kill_all().await.unwrap();
}

// ── 14. Topology validation ─────────────────────────────────────────────────

#[tokio::test]
async fn test_topology_validation() {
    // Empty topology
    let empty = TopologyDef { agents: vec![], flows: vec![] };
    assert!(AgentSupervisor::validate_topology(&empty).is_err());

    // Unknown agent reference in flow
    let bad_ref = TopologyDef {
        agents: vec![make_spec("a1", "Alice", AgentRole::Coder)],
        flows: vec![FlowEdge {
            from: "a1".to_string(),
            to: vec!["nonexistent".to_string()],
            flow_type: FlowType::Pipeline,
        }],
    };
    assert!(AgentSupervisor::validate_topology(&bad_ref).is_err());

    // Pipeline with multiple targets
    let multi_pipeline = TopologyDef {
        agents: vec![
            make_spec("a1", "Alice", AgentRole::Coder),
            make_spec("a2", "Bob", AgentRole::Reviewer),
            make_spec("a3", "Carol", AgentRole::Writer),
        ],
        flows: vec![FlowEdge {
            from: "a1".to_string(),
            to: vec!["a2".to_string(), "a3".to_string()],
            flow_type: FlowType::Pipeline,
        }],
    };
    assert!(AgentSupervisor::validate_topology(&multi_pipeline).is_err());

    // Valid topology
    let valid = TopologyDef {
        agents: vec![
            make_spec("a1", "Alice", AgentRole::Coder),
            make_spec("a2", "Bob", AgentRole::Reviewer),
        ],
        flows: vec![FlowEdge {
            from: "a1".to_string(),
            to: vec!["a2".to_string()],
            flow_type: FlowType::Pipeline,
        }],
    };
    assert!(AgentSupervisor::validate_topology(&valid).is_ok());
}

// ── 15. Whisper (directive) ──────────────────────────────────────────────────

#[tokio::test]
async fn test_whisper() {
    let sup = AgentSupervisor::new(128, None);
    let mut rx = sup.subscribe();
    let spec = make_spec("a1", "Alice", AgentRole::Coder);
    sup.spawn_agent(spec, None, None, None, None).await.unwrap();

    sup.whisper(
        "a1",
        AgentMessage::Task {
            content: "Secret instruction".to_string(),
            from: Some("supervisor".to_string()),
        },
    )
    .await
    .unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;

    let events = collect_events(&mut rx, 50).await;
    let has_output = events.iter().any(|e| matches!(e, SupervisorEvent::AgentOutput { .. }));
    assert!(has_output, "expected AgentOutput from whisper");

    sup.kill_all().await.unwrap();
}

// ── 16. Loop executor integration ─────────────────────────────────────────────
#[tokio::test]
async fn test_spawned_agent_uses_loop_executor_when_configured() {
    let workspace = tempdir().unwrap();
    let mut router = ModelRouter::new();
    router.register_provider(EchoProvider::new(
        ProviderDescriptor {
            id: "mock".to_string(),
            name: Some("Mock".to_string()),
            kind: ProviderKind::Mock,
            models: vec!["test-model".to_string()],
            model_capabilities: BTreeMap::from([(
                "test-model".to_string(),
                BTreeSet::from([Capability::Chat]),
            )]),
            priority: 10,
            available: true,
        },
        "agent loop",
    ));

    let sup = AgentSupervisor::with_executor(
        128,
        None,
        Arc::new(LoopExecutor::new(Arc::new(ReActStrategy))),
        Arc::new(ArcSwap::from_pointee(router)),
        Arc::new(ToolRegistry::new()),
        Arc::new(Mutex::new(SessionPermissions::default())),
        Arc::new(Mutex::new(Vec::new())),
        None,
        "session-123".to_string(),
        workspace.path().to_path_buf(),
        None,
        None,
    );
    let mut rx = sup.subscribe();

    let mut spec = make_spec("a1", "Alice", AgentRole::Coder);
    spec.model = Some("mock:test-model".to_string());
    sup.spawn_agent(spec, None, None, None, None).await.unwrap();

    sup.send_to_agent(
        "a1",
        AgentMessage::Task {
            content: "Write hello world".to_string(),
            from: Some("user".to_string()),
        },
    )
    .await
    .unwrap();

    tokio::time::sleep(Duration::from_millis(200)).await;

    let events = collect_events(&mut rx, 100).await;
    let completed = events.iter().find_map(|event| match event {
        SupervisorEvent::AgentCompleted { result, .. } => Some(result.clone()),
        _ => None,
    });

    assert!(workspace.path().exists());
    let completed = completed.expect("expected completion result");
    assert!(completed.contains("agent loop responded"), "unexpected completion: {completed}");

    sup.kill_all().await.unwrap();
}

// ── Persona tool isolation ─────────────────────────────────────────────────

/// A mock PersonaToolFactory that returns a distinct ToolRegistry per persona.
struct MockPersonaToolFactory {
    persona_tools: std::collections::HashMap<String, Vec<String>>,
    call_count: std::sync::Arc<parking_lot::Mutex<std::collections::HashMap<String, usize>>>,
}

impl MockPersonaToolFactory {
    fn new(persona_tools: std::collections::HashMap<String, Vec<String>>) -> Self {
        Self {
            persona_tools,
            call_count: std::sync::Arc::new(parking_lot::Mutex::new(
                std::collections::HashMap::new(),
            )),
        }
    }

    fn call_count_for(&self, persona_id: &str) -> usize {
        self.call_count.lock().get(persona_id).copied().unwrap_or(0)
    }
}

#[async_trait::async_trait]
impl hive_agents::PersonaToolFactory for MockPersonaToolFactory {
    async fn build_tools_for_persona(
        &self,
        persona_id: &str,
        _session_id: &str,
    ) -> Result<(Arc<ToolRegistry>, Option<Arc<hive_skills::SkillCatalog>>), AgentError> {
        *self.call_count.lock().entry(persona_id.to_string()).or_insert(0) += 1;

        let mut registry = ToolRegistry::new();
        if let Some(tool_ids) = self.persona_tools.get(persona_id) {
            for id in tool_ids {
                let _ = registry.register(Arc::new(TestToolSimple::new(id)));
            }
        }
        Ok((Arc::new(registry), None))
    }
}

/// Minimal tool for persona isolation tests.
struct TestToolSimple {
    definition: hive_tools::ToolDefinition,
}

impl TestToolSimple {
    fn new(id: &str) -> Self {
        Self {
            definition: hive_tools::ToolDefinition {
                id: id.to_string(),
                name: id.to_string(),
                description: format!("test tool {id}"),
                input_schema: serde_json::json!({ "type": "object" }),
                output_schema: None,
                channel_class: hive_classification::ChannelClass::Internal,
                side_effects: false,
                approval: hive_contracts::ToolApproval::Auto,
                annotations: hive_contracts::ToolAnnotations {
                    title: id.to_string(),
                    read_only_hint: None,
                    destructive_hint: None,
                    idempotent_hint: None,
                    open_world_hint: None,
                },
            },
        }
    }
}

impl hive_tools::Tool for TestToolSimple {
    fn definition(&self) -> &hive_tools::ToolDefinition {
        &self.definition
    }

    fn execute(
        &self,
        _input: serde_json::Value,
    ) -> hive_tools::BoxFuture<'_, Result<hive_tools::ToolResult, hive_tools::ToolError>> {
        Box::pin(async {
            Ok(hive_tools::ToolResult {
                output: serde_json::json!("ok"),
                data_class: hive_classification::DataClass::Public,
            })
        })
    }
}

#[tokio::test]
async fn test_spawn_agent_same_persona_uses_session_registry() {
    let mut session_registry = ToolRegistry::new();
    session_registry.register(Arc::new(TestToolSimple::new("session.tool"))).unwrap();

    let factory = Arc::new(MockPersonaToolFactory::new(std::collections::HashMap::new()));

    let sup = AgentSupervisor::with_executor_and_persona_factory(
        128,
        None,
        Arc::new(LoopExecutor::new(Arc::new(ReActStrategy))),
        Arc::new(ArcSwap::from_pointee(ModelRouter::new())),
        Arc::new(session_registry),
        Arc::new(Mutex::new(SessionPermissions::default())),
        Arc::new(Mutex::new(Vec::new())),
        None,
        "session-1".to_string(),
        std::path::PathBuf::from("/tmp/test"),
        None,
        None,
        Some(factory.clone() as Arc<dyn hive_agents::PersonaToolFactory>),
        Some("persona-a".to_string()),
    );

    let mut spec = make_spec("agent-1", "Agent", AgentRole::Coder);
    spec.persona_id = Some("persona-a".to_string());
    spec.allowed_tools = vec!["*".to_string()];

    sup.spawn_agent(spec, None, None, None, None).await.unwrap();

    // Factory should NOT have been called — same persona as session.
    assert_eq!(factory.call_count_for("persona-a"), 0);

    let agents = sup.get_all_agents();
    assert!(agents[0].tools.contains(&"session.tool".to_string()));

    sup.kill_all().await.unwrap();
}

#[tokio::test]
async fn test_spawn_agent_different_persona_calls_factory() {
    let mut session_registry = ToolRegistry::new();
    session_registry.register(Arc::new(TestToolSimple::new("session.tool"))).unwrap();

    let mut persona_tools = std::collections::HashMap::new();
    persona_tools.insert("persona-b".to_string(), vec!["persona-b.tool".to_string()]);

    let factory = Arc::new(MockPersonaToolFactory::new(persona_tools));

    let sup = AgentSupervisor::with_executor_and_persona_factory(
        128,
        None,
        Arc::new(LoopExecutor::new(Arc::new(ReActStrategy))),
        Arc::new(ArcSwap::from_pointee(ModelRouter::new())),
        Arc::new(session_registry),
        Arc::new(Mutex::new(SessionPermissions::default())),
        Arc::new(Mutex::new(Vec::new())),
        None,
        "session-1".to_string(),
        std::path::PathBuf::from("/tmp/test"),
        None,
        None,
        Some(factory.clone() as Arc<dyn hive_agents::PersonaToolFactory>),
        Some("persona-a".to_string()),
    );

    let mut spec = make_spec("agent-b", "AgentB", AgentRole::Coder);
    spec.persona_id = Some("persona-b".to_string());
    spec.allowed_tools = vec!["*".to_string()];

    sup.spawn_agent(spec, None, None, None, None).await.unwrap();

    // Factory should have been called for persona-b.
    assert_eq!(factory.call_count_for("persona-b"), 1);

    let agents = sup.get_all_agents();
    let resolved = &agents[0].tools;
    assert!(
        resolved.contains(&"persona-b.tool".to_string()),
        "expected persona-b.tool, got {resolved:?}"
    );
    assert!(!resolved.contains(&"session.tool".to_string()), "should NOT contain session.tool");

    sup.kill_all().await.unwrap();
}

#[tokio::test]
async fn test_persona_registry_is_cached() {
    let mut persona_tools = std::collections::HashMap::new();
    persona_tools.insert("persona-c".to_string(), vec!["persona-c.tool".to_string()]);

    let factory = Arc::new(MockPersonaToolFactory::new(persona_tools));

    let sup = AgentSupervisor::with_executor_and_persona_factory(
        128,
        None,
        Arc::new(LoopExecutor::new(Arc::new(ReActStrategy))),
        Arc::new(ArcSwap::from_pointee(ModelRouter::new())),
        Arc::new(ToolRegistry::new()),
        Arc::new(Mutex::new(SessionPermissions::default())),
        Arc::new(Mutex::new(Vec::new())),
        None,
        "session-1".to_string(),
        std::path::PathBuf::from("/tmp/test"),
        None,
        None,
        Some(factory.clone() as Arc<dyn hive_agents::PersonaToolFactory>),
        Some("persona-a".to_string()),
    );

    let mut spec1 = make_spec("agent-c1", "AgentC1", AgentRole::Coder);
    spec1.persona_id = Some("persona-c".to_string());
    spec1.allowed_tools = vec!["*".to_string()];
    sup.spawn_agent(spec1, None, None, None, None).await.unwrap();

    let mut spec2 = make_spec("agent-c2", "AgentC2", AgentRole::Coder);
    spec2.persona_id = Some("persona-c".to_string());
    spec2.allowed_tools = vec!["*".to_string()];
    sup.spawn_agent(spec2, None, None, None, None).await.unwrap();

    // Factory should have been called only once — second spawn uses cache.
    assert_eq!(factory.call_count_for("persona-c"), 1);
    assert_eq!(sup.agent_count(), 2);

    sup.kill_all().await.unwrap();
}

#[tokio::test]
async fn test_allowed_tools_filters_persona_registry() {
    let mut persona_tools = std::collections::HashMap::new();
    persona_tools.insert(
        "persona-d".to_string(),
        vec!["tool.alpha".to_string(), "tool.beta".to_string(), "tool.gamma".to_string()],
    );

    let factory = Arc::new(MockPersonaToolFactory::new(persona_tools));

    let sup = AgentSupervisor::with_executor_and_persona_factory(
        128,
        None,
        Arc::new(LoopExecutor::new(Arc::new(ReActStrategy))),
        Arc::new(ArcSwap::from_pointee(ModelRouter::new())),
        Arc::new(ToolRegistry::new()),
        Arc::new(Mutex::new(SessionPermissions::default())),
        Arc::new(Mutex::new(Vec::new())),
        None,
        "session-1".to_string(),
        std::path::PathBuf::from("/tmp/test"),
        None,
        None,
        Some(factory.clone() as Arc<dyn hive_agents::PersonaToolFactory>),
        Some("persona-a".to_string()),
    );

    let mut spec = make_spec("agent-d", "AgentD", AgentRole::Coder);
    spec.persona_id = Some("persona-d".to_string());
    spec.allowed_tools = vec!["tool.alpha".to_string(), "tool.gamma".to_string()];

    sup.spawn_agent(spec, None, None, None, None).await.unwrap();

    let agents = sup.get_all_agents();
    let resolved = &agents[0].tools;
    assert!(resolved.contains(&"tool.alpha".to_string()));
    assert!(resolved.contains(&"tool.gamma".to_string()));
    assert!(!resolved.contains(&"tool.beta".to_string()), "tool.beta should be filtered out");

    sup.kill_all().await.unwrap();
}

#[tokio::test]
async fn test_spawn_agent_no_persona_uses_session_registry() {
    let mut session_registry = ToolRegistry::new();
    session_registry.register(Arc::new(TestToolSimple::new("session.tool"))).unwrap();

    let mut persona_tools = std::collections::HashMap::new();
    persona_tools.insert("other".to_string(), vec!["other.tool".to_string()]);

    let factory = Arc::new(MockPersonaToolFactory::new(persona_tools));

    let sup = AgentSupervisor::with_executor_and_persona_factory(
        128,
        None,
        Arc::new(LoopExecutor::new(Arc::new(ReActStrategy))),
        Arc::new(ArcSwap::from_pointee(ModelRouter::new())),
        Arc::new(session_registry),
        Arc::new(Mutex::new(SessionPermissions::default())),
        Arc::new(Mutex::new(Vec::new())),
        None,
        "session-1".to_string(),
        std::path::PathBuf::from("/tmp/test"),
        None,
        None,
        Some(factory.clone() as Arc<dyn hive_agents::PersonaToolFactory>),
        Some("persona-a".to_string()),
    );

    let mut spec = make_spec("agent-none", "AgentNone", AgentRole::Coder);
    spec.persona_id = None;
    spec.allowed_tools = vec!["*".to_string()];

    sup.spawn_agent(spec, None, None, None, None).await.unwrap();

    assert_eq!(factory.call_count_for("other"), 0);

    let agents = sup.get_all_agents();
    assert!(agents[0].tools.contains(&"session.tool".to_string()));

    sup.kill_all().await.unwrap();
}
