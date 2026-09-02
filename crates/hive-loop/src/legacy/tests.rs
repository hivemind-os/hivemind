use super::*;
use hive_classification::{ChannelClass, DataClass};
use hive_contracts::{
    CodeActConfig, InteractionKind, InteractionResponsePayload, LoopStrategy as ConfigLoopStrategy,
    PermissionRule, Persona, SessionPermissions, ToolAnnotations, ToolApproval, ToolDefinition,
    ToolExecutionMode, ToolLimitsConfig, UserInteractionResponse, WorkspaceClassification,
};
use hive_model::{
    Capability, CompletionRequest, CompletionResponse, ModelProvider, ModelRouter, ModelSelection,
    ProviderDescriptor,
};
use hive_tools::{
    CalculatorTool, FileSystemListTool, FileSystemReadTool, KillAgentTool, ListAgentsTool,
    ListPersonasTool, SignalAgentTool, SpawnAgentTool, Tool, ToolRegistry, ToolResult,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct TestProvider {
    descriptor: ProviderDescriptor,
    responses: Arc<Mutex<VecDeque<String>>>,
    prompts: Arc<Mutex<Vec<String>>>,
}

impl TestProvider {
    fn new(responses: Vec<String>) -> (Self, Arc<Mutex<Vec<String>>>) {
        let prompts = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                descriptor: ProviderDescriptor {
                    id: "test".to_string(),
                    name: None,
                    kind: hive_model::ProviderKind::Mock,
                    models: vec!["test-model".to_string()],
                    model_capabilities: BTreeMap::from([(
                        "test-model".to_string(),
                        BTreeSet::from([Capability::Chat]),
                    )]),
                    model_limits: std::collections::BTreeMap::new(),
                    priority: 10,
                    available: true,
                },
                responses: Arc::new(Mutex::new(VecDeque::from(responses))),
                prompts: Arc::clone(&prompts),
            },
            prompts,
        )
    }
}

#[derive(Default)]
struct MockAgentOrchestrator {
    next_id: AtomicUsize,
    spawned: Mutex<Vec<(String, String, Option<String>)>>,
    messages: Mutex<Vec<(String, String, String)>>,
}

impl AgentOrchestrator for MockAgentOrchestrator {
    fn spawn_agent(
        &self,
        persona: Persona,
        task: String,
        from: Option<String>,
        _friendly_name: Option<String>,
        _data_class: hive_classification::DataClass,
        _parent_model: Option<hive_model::ModelSelection>,
        _keep_alive: bool,
        _workspace_path: Option<PathBuf>,
    ) -> BoxFuture<'_, Result<String, String>> {
        self.spawned.lock().unwrap().push((persona.id.clone(), task, from));
        let agent_id =
            format!("{}-{}", persona.id, self.next_id.fetch_add(1, Ordering::SeqCst) + 1);
        Box::pin(async move { Ok(agent_id) })
    }

    fn message_agent(
        &self,
        agent_id: String,
        message: String,
        from: String,
    ) -> BoxFuture<'_, Result<(), String>> {
        self.messages.lock().unwrap().push((agent_id, message, from));
        Box::pin(async move { Ok(()) })
    }

    fn list_agents(
        &self,
    ) -> BoxFuture<'_, Result<Vec<(String, String, String, String, Option<String>)>, String>> {
        let spawned = self.spawned.lock().unwrap();
        let agents: Vec<_> = spawned
            .iter()
            .map(|(id, _, _)| (id.clone(), id.clone(), String::new(), "Running".to_string(), None))
            .collect();
        Box::pin(async move { Ok(agents) })
    }

    fn get_agent_result(
        &self,
        _agent_id: String,
    ) -> BoxFuture<'_, Result<(String, Option<String>), String>> {
        Box::pin(async move { Ok(("Done".to_string(), None)) })
    }

    fn kill_agent(&self, _agent_id: String) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move { Ok(()) })
    }

    fn message_session(
        &self,
        _message: String,
        _from_agent_id: String,
    ) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move { Ok(()) })
    }

    fn feedback_agent(
        &self,
        agent_id: String,
        message: String,
        from: String,
    ) -> BoxFuture<'_, Result<(), String>> {
        self.messages.lock().unwrap().push((agent_id, message, from));
        Box::pin(async move { Ok(()) })
    }

    fn get_agent_parent(&self, agent_id: String) -> BoxFuture<'_, Result<Option<String>, String>> {
        // The test context sets current_agent_id = "system/general" with
        // parent_agent_id = "parent-1". Return matching relationships so
        // check_agent_family access control passes.
        let parent = match agent_id.as_str() {
            "system/general" | "system/planner-1" => Some("parent-1".to_string()),
            _ => None,
        };
        Box::pin(async move { Ok(parent) })
    }
}

impl ModelProvider for TestProvider {
    fn descriptor(&self) -> &ProviderDescriptor {
        &self.descriptor
    }

    fn complete(
        &self,
        request: &CompletionRequest,
        selection: &ModelSelection,
    ) -> Result<CompletionResponse, anyhow::Error> {
        self.prompts.lock().unwrap().push(request.prompt.clone());
        let mut responses = self.responses.lock().unwrap();
        let response = responses.pop_front().ok_or_else(|| anyhow::anyhow!("no response"))?;
        Ok(CompletionResponse {
            provider_id: self.descriptor.id.clone(),
            model: selection.model.clone(),
            content: response,
            tool_calls: vec![],
            usage: None,

            finish_reason: None,
        })
    }
}

#[tokio::test]
async fn execute_tool_call_intercepts_agent_orchestration_tools() {
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(SpawnAgentTool::default())).expect("register spawn tool");
    registry.register(Arc::new(SignalAgentTool::default())).expect("register signal tool");
    registry.register(Arc::new(ListAgentsTool::default())).expect("register list tool");
    registry.register(Arc::new(ListPersonasTool::default())).expect("register list personas tool");
    registry.register(Arc::new(KillAgentTool::default())).expect("register kill tool");

    let orchestrator = Arc::new(MockAgentOrchestrator::default());
    let context = LoopContext {
        conversation: ConversationContext {
            session_id: "session-agents".to_string(),
            message_id: "msg-agents".to_string(),
            prompt: "coordinate work".to_string(),
            prompt_content_parts: vec![],
            history: vec![],
            conversation_journal: None,
            initial_tool_iterations: 0,
        },
        routing: RoutingConfig {
            required_capabilities: BTreeSet::new(),
            preferred_models: None,
            loop_strategy: None,
            routing_decision: None,
        },
        security: SecurityContext {
            data_class: DataClass::Internal,
            permissions: Arc::new(parking_lot::Mutex::new(SessionPermissions::new())),
            workspace_classification: None,
            effective_data_class: Arc::new(AtomicU8::new(DataClass::Internal.to_i64() as u8)),
            connector_service: None,
            shadow_mode: false,
        },
        tools_ctx: ToolsContext {
            tools: Arc::new(registry),
            skill_catalog: None,
            knowledge_query_handler: None,
            tool_execution_mode: ToolExecutionMode::default(),
        },
        agent: AgentContext {
            persona: Some(Persona::default_persona()),
            agent_orchestrator: Some(orchestrator.clone()),
            personas: vec![Persona {
                id: "system/planner".to_string(),
                name: "Planner".to_string(),
                description: "Plans execution.".to_string(),
                system_prompt: "Plan the work.".to_string(),
                loop_strategy: ConfigLoopStrategy::React,
                preferred_models: None,
                allowed_tools: vec!["filesystem.read".to_string()],
                mcp_servers: Vec::new(),
                avatar: None,
                color: None,
                tool_execution_mode: ToolExecutionMode::default(),
                context_map_strategy: hive_contracts::ContextMapStrategy::default(),
                secondary_models: None,
                archived: false,
                bundled: false,
                mcp_sampling: false,
                mcp_sampling_requires_approval: true,
                mcp_sampling_max_tokens: None,
                prompts: Default::default(),
            }],
            current_agent_id: Some("system/general".to_string()),
            parent_agent_id: Some("parent-1".to_string()),
            workspace_path: None,
            keep_alive: false,
            session_messaged: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        },
        tool_limits: ToolLimitsConfig::default(),
        code_act_config: CodeActConfig::default(),
        session_registry: None,
        preempt_signal: None,
        cancellation_token: None,
    };

    let spawn_result = execute_tool_call(
        &context,
        ToolCall {
            tool_id: "core.spawn_agent".to_string(),
            input: json!({ "agent_name": "Planner", "task": "Break down the task" }),
        },
        &[],
        None,
        None,
        None,
    )
    .await
    .expect("spawn tool result");
    assert_eq!(spawn_result.output["agent_id"], json!("system/planner-1"));

    let message_result = execute_tool_call(
        &context,
        ToolCall {
            tool_id: "core.signal_agent".to_string(),
            input: json!({ "agent_id": "parent", "content": "Done" }),
        },
        &[],
        None,
        None,
        None,
    )
    .await
    .expect("message tool result");
    assert_eq!(message_result.output["agent_id"], json!("parent-1"));
    assert_eq!(message_result.output["delivered"], json!(true));

    let spawned = orchestrator.spawned.lock().unwrap();
    assert_eq!(
        spawned.as_slice(),
        [(
            "system/planner".to_string(),
            "Break down the task".to_string(),
            Some("system/general".to_string())
        )]
    );
    drop(spawned);

    let messages = orchestrator.messages.lock().unwrap();
    assert_eq!(
        messages.as_slice(),
        [("parent-1".to_string(), "Done".to_string(), "system/general".to_string())]
    );
}

#[tokio::test]
async fn react_executes_tool_call_and_responds() {
    let responses = vec![
        "<tool_call>{\"tool\":\"math.calculate\",\"input\":{\"expression\":\"1+1\"}}</tool_call>"
            .to_string(),
        "done".to_string(),
    ];
    let (provider, prompts) = TestProvider::new(responses);
    let mut router = ModelRouter::new();
    router.register_provider(provider);
    let router = Arc::new(router);

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(CalculatorTool::default())).expect("register calculator tool");
    let tools = Arc::new(registry);

    let executor = LoopExecutor::new(Arc::new(ReActStrategy));
    let context = LoopContext {
        conversation: ConversationContext {
            session_id: "session-1".to_string(),
            message_id: "msg-1".to_string(),
            prompt: "Say hello".to_string(),
            prompt_content_parts: vec![],
            history: vec![],
            conversation_journal: None,
            initial_tool_iterations: 0,
        },
        routing: RoutingConfig {
            required_capabilities: BTreeSet::new(),
            preferred_models: None,
            loop_strategy: None,
            routing_decision: None,
        },
        security: SecurityContext {
            data_class: DataClass::Internal,
            permissions: Arc::new(parking_lot::Mutex::new(SessionPermissions::new())),
            workspace_classification: None,
            effective_data_class: Arc::new(AtomicU8::new(DataClass::Internal.to_i64() as u8)),
            connector_service: None,
            shadow_mode: false,
        },
        tools_ctx: ToolsContext {
            tools,
            skill_catalog: None,
            knowledge_query_handler: None,
            tool_execution_mode: ToolExecutionMode::default(),
        },
        agent: AgentContext {
            persona: None,
            agent_orchestrator: None,
            personas: Vec::new(),
            current_agent_id: None,
            parent_agent_id: None,
            workspace_path: None,
            keep_alive: false,
            session_messaged: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        },
        tool_limits: ToolLimitsConfig::default(),
        code_act_config: CodeActConfig::default(),
        session_registry: None,
        preempt_signal: None,
        cancellation_token: None,
    };

    let result = executor.run(context, router).await.expect("loop result");
    assert_eq!(result.content, "done");

    let recorded = prompts.lock().unwrap();
    assert_eq!(recorded.len(), 2);
    assert!(recorded[1].contains("<tool_result>"));
}

#[tokio::test]
async fn sequential_returns_response_without_tool_calls() {
    let responses = vec!["Hello, world!".to_string()];
    let (provider, prompts) = TestProvider::new(responses);
    let mut router = ModelRouter::new();
    router.register_provider(provider);
    let router = Arc::new(router);

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(CalculatorTool::default())).expect("register calculator tool");
    let tools = Arc::new(registry);

    let executor = LoopExecutor::new(Arc::new(SequentialStrategy));
    let context = LoopContext {
        conversation: ConversationContext {
            session_id: "session-seq".to_string(),
            message_id: "msg-seq".to_string(),
            prompt: "What is 1+1?".to_string(),
            prompt_content_parts: vec![],
            history: vec![],
            conversation_journal: None,
            initial_tool_iterations: 0,
        },
        routing: RoutingConfig {
            required_capabilities: BTreeSet::new(),
            preferred_models: None,
            loop_strategy: None,
            routing_decision: None,
        },
        security: SecurityContext {
            data_class: DataClass::Internal,
            permissions: Arc::new(parking_lot::Mutex::new(SessionPermissions::new())),
            workspace_classification: None,
            effective_data_class: Arc::new(AtomicU8::new(DataClass::Internal.to_i64() as u8)),
            connector_service: None,
            shadow_mode: false,
        },
        tools_ctx: ToolsContext {
            tools,
            skill_catalog: None,
            knowledge_query_handler: None,
            tool_execution_mode: ToolExecutionMode::default(),
        },
        agent: AgentContext {
            persona: None,
            agent_orchestrator: None,
            personas: Vec::new(),
            current_agent_id: None,
            parent_agent_id: None,
            workspace_path: None,
            keep_alive: false,
            session_messaged: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        },
        tool_limits: ToolLimitsConfig::default(),
        code_act_config: CodeActConfig::default(),
        session_registry: None,
        preempt_signal: None,
        cancellation_token: None,
    };

    let result = executor.run(context, router).await.expect("loop result");
    assert_eq!(result.content, "Hello, world!");

    // Sequential should only call model once and never invoke tools
    let recorded = prompts.lock().unwrap();
    assert_eq!(recorded.len(), 1);
}

#[tokio::test]
async fn sequential_ignores_tool_call_in_response() {
    // Even if model returns a tool_call block, Sequential ignores it
    let responses =
        vec!["<tool_call>{\"tool\":\"core.echo\",\"input\":{\"value\":\"hi\"}}</tool_call>"
            .to_string()];
    let (provider, prompts) = TestProvider::new(responses);
    let mut router = ModelRouter::new();
    router.register_provider(provider);
    let router = Arc::new(router);

    let _tools = Arc::new(ToolRegistry::new());
    let executor = LoopExecutor::new(Arc::new(SequentialStrategy));
    let context = LoopContext {
        conversation: ConversationContext {
            session_id: "session-seq2".to_string(),
            message_id: "msg-seq2".to_string(),
            prompt: "echo hi".to_string(),
            prompt_content_parts: vec![],
            history: vec![],
            conversation_journal: None,
            initial_tool_iterations: 0,
        },
        routing: RoutingConfig {
            required_capabilities: BTreeSet::new(),
            preferred_models: None,
            loop_strategy: None,
            routing_decision: None,
        },
        security: SecurityContext {
            data_class: DataClass::Internal,
            permissions: Arc::new(parking_lot::Mutex::new(SessionPermissions::new())),
            workspace_classification: None,
            effective_data_class: Arc::new(AtomicU8::new(DataClass::Internal.to_i64() as u8)),
            connector_service: None,
            shadow_mode: false,
        },
        tools_ctx: ToolsContext {
            tools: Arc::new(ToolRegistry::new()),
            skill_catalog: None,
            knowledge_query_handler: None,
            tool_execution_mode: ToolExecutionMode::default(),
        },
        agent: AgentContext {
            persona: None,
            agent_orchestrator: None,
            personas: Vec::new(),
            current_agent_id: None,
            parent_agent_id: None,
            workspace_path: None,
            keep_alive: false,
            session_messaged: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        },
        tool_limits: ToolLimitsConfig::default(),
        code_act_config: CodeActConfig::default(),
        session_registry: None,
        preempt_signal: None,
        cancellation_token: None,
    };

    let result = executor.run(context, router).await.expect("loop result");
    // The raw content is returned as-is, no tool execution
    assert!(result.content.contains("tool_call"));
    let recorded = prompts.lock().unwrap();
    assert_eq!(recorded.len(), 1);
}

#[tokio::test]
async fn loop_context_strategy_overrides_executor_default() {
    let responses = vec!["Hello from sequential".to_string()];
    let (provider, prompts) = TestProvider::new(responses);
    let mut router = ModelRouter::new();
    router.register_provider(provider);
    let router = Arc::new(router);

    let executor = LoopExecutor::new(Arc::new(ReActStrategy));
    let context = LoopContext {
        conversation: ConversationContext {
            session_id: "session-override".to_string(),
            message_id: "msg-override".to_string(),
            prompt: "Ignore tools".to_string(),
            prompt_content_parts: vec![],
            history: vec![],
            conversation_journal: None,
            initial_tool_iterations: 0,
        },
        routing: RoutingConfig {
            required_capabilities: BTreeSet::new(),
            preferred_models: None,
            loop_strategy: Some(ConfigLoopStrategy::Sequential),
            routing_decision: None,
        },
        security: SecurityContext {
            data_class: DataClass::Internal,
            permissions: Arc::new(parking_lot::Mutex::new(SessionPermissions::new())),
            workspace_classification: None,
            effective_data_class: Arc::new(AtomicU8::new(DataClass::Internal.to_i64() as u8)),
            connector_service: None,
            shadow_mode: false,
        },
        tools_ctx: ToolsContext {
            tools: Arc::new(ToolRegistry::new()),
            skill_catalog: None,
            knowledge_query_handler: None,
            tool_execution_mode: ToolExecutionMode::default(),
        },
        agent: AgentContext {
            persona: Some(Persona::default_persona()),
            agent_orchestrator: None,
            personas: Vec::new(),
            current_agent_id: None,
            parent_agent_id: None,
            workspace_path: None,
            keep_alive: false,
            session_messaged: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        },
        tool_limits: ToolLimitsConfig::default(),
        code_act_config: CodeActConfig::default(),
        session_registry: None,
        preempt_signal: None,
        cancellation_token: None,
    };

    let result = executor.run(context, router).await.expect("loop result");
    assert_eq!(result.content, "Hello from sequential");
    assert_eq!(prompts.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn plan_then_execute_with_tool_calls() {
    let responses = vec![
        // Phase 1: plan
        "1. Calculate 1+1\n2. Summarize".to_string(),
        // Phase 2 step 1: tool call
        "<tool_call>{\"tool\":\"math.calculate\",\"input\":{\"expression\":\"1+1\"}}</tool_call>"
            .to_string(),
        // Phase 2 step 1 continued: final answer for step
        "calculated 2".to_string(),
        // Phase 2 step 2: final answer (no tool)
        "summary complete".to_string(),
    ];
    let (provider, prompts) = TestProvider::new(responses);
    let mut router = ModelRouter::new();
    router.register_provider(provider);
    let router = Arc::new(router);

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(CalculatorTool::default())).expect("register calculator tool");
    let tools = Arc::new(registry);

    let executor = LoopExecutor::new(Arc::new(PlanThenExecuteStrategy));
    let context = LoopContext {
        conversation: ConversationContext {
            session_id: "session-plan".to_string(),
            message_id: "msg-plan".to_string(),
            prompt: "Calculate 1+1 then summarize".to_string(),
            prompt_content_parts: vec![],
            history: vec![],
            conversation_journal: None,
            initial_tool_iterations: 0,
        },
        routing: RoutingConfig {
            required_capabilities: BTreeSet::new(),
            preferred_models: None,
            loop_strategy: None,
            routing_decision: None,
        },
        security: SecurityContext {
            data_class: DataClass::Internal,
            permissions: Arc::new(parking_lot::Mutex::new(SessionPermissions::new())),
            workspace_classification: None,
            effective_data_class: Arc::new(AtomicU8::new(DataClass::Internal.to_i64() as u8)),
            connector_service: None,
            shadow_mode: false,
        },
        tools_ctx: ToolsContext {
            tools,
            skill_catalog: None,
            knowledge_query_handler: None,
            tool_execution_mode: ToolExecutionMode::default(),
        },
        agent: AgentContext {
            persona: None,
            agent_orchestrator: None,
            personas: Vec::new(),
            current_agent_id: None,
            parent_agent_id: None,
            workspace_path: None,
            keep_alive: false,
            session_messaged: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        },
        tool_limits: ToolLimitsConfig::default(),
        code_act_config: CodeActConfig::default(),
        session_registry: None,
        preempt_signal: None,
        cancellation_token: None,
    };

    let result = executor.run(context, router).await.expect("loop result");
    assert_eq!(result.content, "summary complete");

    let recorded = prompts.lock().unwrap();
    // 1 plan call + 2 model calls for step 1 (tool call + answer) + 1 for step 2 = 4
    assert_eq!(recorded.len(), 4);
    // Second call should contain the step prompt
    assert!(recorded[1].contains("Current step"));
}

#[tokio::test]
async fn plan_then_execute_no_plan_returns_response() {
    // If the model doesn't output a parseable plan, return the response as-is
    let responses = vec!["Just a plain answer without numbered steps".to_string()];
    let (provider, _prompts) = TestProvider::new(responses);
    let mut router = ModelRouter::new();
    router.register_provider(provider);
    let router = Arc::new(router);

    let _tools = Arc::new(ToolRegistry::new());
    let executor = LoopExecutor::new(Arc::new(PlanThenExecuteStrategy));
    let context = LoopContext {
        conversation: ConversationContext {
            session_id: "session-plan2".to_string(),
            message_id: "msg-plan2".to_string(),
            prompt: "Do something".to_string(),
            prompt_content_parts: vec![],
            history: vec![],
            conversation_journal: None,
            initial_tool_iterations: 0,
        },
        routing: RoutingConfig {
            required_capabilities: BTreeSet::new(),
            preferred_models: None,
            loop_strategy: None,
            routing_decision: None,
        },
        security: SecurityContext {
            data_class: DataClass::Internal,
            permissions: Arc::new(parking_lot::Mutex::new(SessionPermissions::new())),
            workspace_classification: None,
            effective_data_class: Arc::new(AtomicU8::new(DataClass::Internal.to_i64() as u8)),
            connector_service: None,
            shadow_mode: false,
        },
        tools_ctx: ToolsContext {
            tools: Arc::new(ToolRegistry::new()),
            skill_catalog: None,
            knowledge_query_handler: None,
            tool_execution_mode: ToolExecutionMode::default(),
        },
        agent: AgentContext {
            persona: None,
            agent_orchestrator: None,
            personas: Vec::new(),
            current_agent_id: None,
            parent_agent_id: None,
            workspace_path: None,
            keep_alive: false,
            session_messaged: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        },
        tool_limits: ToolLimitsConfig::default(),
        code_act_config: CodeActConfig::default(),
        session_registry: None,
        preempt_signal: None,
        cancellation_token: None,
    };

    let result = executor.run(context, router).await.expect("loop result");
    assert_eq!(result.content, "Just a plain answer without numbered steps");
}

#[tokio::test]
async fn execute_tool_call_applies_workspace_classification_to_file_reads() {
    let workspace_root =
        std::env::temp_dir().join(format!("hive-loop-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&workspace_root).expect("create temp workspace");
    std::fs::write(workspace_root.join("secret.txt"), "classified").expect("write temp file");

    let mut registry = ToolRegistry::new();
    registry
        .register(Arc::new(FileSystemReadTool::new(workspace_root.clone())))
        .expect("register file read tool");

    let mut workspace_classification = WorkspaceClassification::new(DataClass::Public);
    workspace_classification.set_override("secret.txt", DataClass::Restricted);

    let context = LoopContext {
        conversation: ConversationContext {
            session_id: "session-file-read".to_string(),
            message_id: "msg-file-read".to_string(),
            prompt: "Read the secret file".to_string(),
            prompt_content_parts: vec![],
            history: vec![],
            conversation_journal: None,
            initial_tool_iterations: 0,
        },
        routing: RoutingConfig {
            required_capabilities: BTreeSet::new(),
            preferred_models: None,
            loop_strategy: None,
            routing_decision: None,
        },
        security: SecurityContext {
            data_class: DataClass::Internal,
            permissions: Arc::new(parking_lot::Mutex::new(SessionPermissions::new())),
            workspace_classification: Some(Arc::new(workspace_classification)),
            effective_data_class: Arc::new(AtomicU8::new(DataClass::Internal.to_i64() as u8)),
            connector_service: None,
            shadow_mode: false,
        },
        tools_ctx: ToolsContext {
            tools: Arc::new(registry),
            skill_catalog: None,
            knowledge_query_handler: None,
            tool_execution_mode: ToolExecutionMode::default(),
        },
        agent: AgentContext {
            persona: None,
            agent_orchestrator: None,
            personas: Vec::new(),
            current_agent_id: None,
            parent_agent_id: None,
            workspace_path: None,
            keep_alive: false,
            session_messaged: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        },
        tool_limits: ToolLimitsConfig::default(),
        code_act_config: CodeActConfig::default(),
        session_registry: None,
        preempt_signal: None,
        cancellation_token: None,
    };

    // The DataClassificationMiddleware resolves workspace classification
    // in its after_tool_result hook.
    let classification_mw: Arc<dyn LoopMiddleware> =
        Arc::new(crate::classification_middleware::DataClassificationMiddleware::new(None));

    let result = execute_tool_call(
        &context,
        ToolCall { tool_id: "filesystem.read".to_string(), input: json!({ "path": "secret.txt" }) },
        &[classification_mw],
        None,
        None,
        None,
    )
    .await
    .expect("tool call succeeds");

    assert_eq!(result.data_class, DataClass::Restricted);

    std::fs::remove_dir_all(&workspace_root).expect("cleanup temp workspace");
}

/// Integration test for the classification escalation flow.
///
/// Scenario: workspace has two files — one Public, one Internal.
/// Reading the public file should NOT escalate effective_data_class
/// beyond Public.  Reading the internal file SHOULD escalate to Internal.
/// Intermediate tool calls (filesystem.list) must not taint the session.
#[tokio::test]
async fn effective_data_class_only_escalates_from_classified_file_reads() {
    let workspace_root =
        std::env::temp_dir().join(format!("hive-loop-class-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&workspace_root).expect("create temp workspace");
    std::fs::write(workspace_root.join("public.txt"), "hello world").expect("write public");
    std::fs::write(workspace_root.join("internal.txt"), "secret stuff").expect("write internal");

    let mut registry = ToolRegistry::new();
    registry
        .register(Arc::new(FileSystemReadTool::new(workspace_root.clone())))
        .expect("register read");
    registry
        .register(Arc::new(FileSystemListTool::new(workspace_root.clone())))
        .expect("register list");

    let mut wc = WorkspaceClassification::new(DataClass::Internal);
    wc.set_override("public.txt", DataClass::Public);
    wc.set_override("internal.txt", DataClass::Internal);

    let effective_dc = Arc::new(AtomicU8::new(DataClass::Public.to_i64() as u8));

    let context = LoopContext {
        conversation: ConversationContext {
            session_id: "session-class-test".to_string(),
            message_id: "msg-class-test".to_string(),
            prompt: String::new(),
            prompt_content_parts: vec![],
            history: vec![],
            conversation_journal: None,
            initial_tool_iterations: 0,
        },
        routing: RoutingConfig {
            required_capabilities: BTreeSet::new(),
            preferred_models: None,
            loop_strategy: None,
            routing_decision: None,
        },
        security: SecurityContext {
            data_class: DataClass::Public,
            permissions: Arc::new(parking_lot::Mutex::new(SessionPermissions::new())),
            workspace_classification: Some(Arc::new(wc)),
            effective_data_class: effective_dc.clone(),
            connector_service: None,
            shadow_mode: false,
        },
        tools_ctx: ToolsContext {
            tools: Arc::new(registry),
            skill_catalog: None,
            knowledge_query_handler: None,
            tool_execution_mode: ToolExecutionMode::default(),
        },
        agent: AgentContext {
            persona: None,
            agent_orchestrator: None,
            personas: Vec::new(),
            current_agent_id: None,
            parent_agent_id: None,
            workspace_path: None,
            keep_alive: false,
            session_messaged: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        },
        tool_limits: ToolLimitsConfig::default(),
        code_act_config: CodeActConfig::default(),
        session_registry: None,
        preempt_signal: None,
        cancellation_token: None,
    };

    // Classification middleware is needed for after_tool_result resolution.
    let classification_mw: Arc<dyn LoopMiddleware> =
        Arc::new(crate::classification_middleware::DataClassificationMiddleware::new(None));
    let mw = &[classification_mw];

    // Step 1: filesystem.list — should NOT escalate effective_data_class.
    // (filesystem.list hardcodes DataClass::Internal but classification is
    // not resolved for directory listings without a specific file match.)
    let outcome = run_single_tool_call(
        &ToolCall { tool_id: "filesystem.list".to_string(), input: json!({ "path": "." }) },
        &context,
        mw,
        None,
        None,
        None,
    )
    .await;
    assert!(!outcome.is_error, "filesystem.list should succeed: {}", outcome.output);
    assert_eq!(
        context.effective_data_class(),
        DataClass::Public,
        "filesystem.list must NOT escalate effective_data_class"
    );

    // Step 2: Read the PUBLIC file — should resolve to Public, no escalation.
    let outcome = run_single_tool_call(
        &ToolCall {
            tool_id: "filesystem.read".to_string(),
            input: json!({ "path": "public.txt" }),
        },
        &context,
        mw,
        None,
        None,
        None,
    )
    .await;
    assert!(!outcome.is_error, "read public.txt should succeed: {}", outcome.output);
    assert_eq!(
        context.effective_data_class(),
        DataClass::Public,
        "reading a Public file must keep effective_data_class at Public"
    );

    // Step 3: Read the INTERNAL file — should escalate to Internal.
    let outcome = run_single_tool_call(
        &ToolCall {
            tool_id: "filesystem.read".to_string(),
            input: json!({ "path": "internal.txt" }),
        },
        &context,
        mw,
        None,
        None,
        None,
    )
    .await;
    assert!(!outcome.is_error, "read internal.txt should succeed: {}", outcome.output);
    assert_eq!(
        context.effective_data_class(),
        DataClass::Internal,
        "reading an Internal file must escalate effective_data_class to Internal"
    );

    std::fs::remove_dir_all(&workspace_root).expect("cleanup temp workspace");
}

/// Full end-to-end scenario: workspace with Public and Internal files,
/// read only the public file, then send via a Public connector.
/// The send MUST NOT be blocked by classification.
#[tokio::test]
async fn send_public_file_through_public_connector_is_not_blocked() {
    use hive_connectors::ConnectorServiceHandle;

    // Mock connector service that returns Public output-class
    struct MockConnectorSvc;
    impl ConnectorServiceHandle for MockConnectorSvc {
        fn resolve_output_class(&self, _cid: &str, _dest: &str) -> Option<DataClass> {
            Some(DataClass::Public)
        }
        fn resolve_destination_approval(
            &self,
            _cid: &str,
            _dest: &str,
        ) -> Option<hive_contracts::ToolApproval> {
            None
        }
    }

    // Minimal send tool that just succeeds (we don't care about actual send)
    struct FakeSendTool(ToolDefinition);
    impl FakeSendTool {
        fn new() -> Self {
            Self(ToolDefinition {
                id: "comm.send_external_message".to_string(),
                name: "Send".to_string(),
                description: "mock".to_string(),
                input_schema: json!({"type": "object"}),
                output_schema: None,
                channel_class: ChannelClass::Public,
                side_effects: true,
                approval: ToolApproval::Auto,
                annotations: ToolAnnotations {
                    title: "Send".to_string(),
                    read_only_hint: None,
                    destructive_hint: None,
                    idempotent_hint: None,
                    open_world_hint: None,
                },
            })
        }
    }
    impl Tool for FakeSendTool {
        fn definition(&self) -> &ToolDefinition {
            &self.0
        }
        fn execute(
            &self,
            _input: Value,
        ) -> hive_tools::BoxFuture<'_, Result<ToolResult, hive_tools::ToolError>> {
            Box::pin(async {
                Ok(ToolResult { output: json!({"status": "sent"}), data_class: DataClass::Public })
            })
        }
    }

    let workspace_root =
        std::env::temp_dir().join(format!("hive-loop-e2e-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&workspace_root).expect("create workspace");
    std::fs::write(workspace_root.join("public.txt"), "hello world").expect("write public");
    std::fs::write(workspace_root.join("internal.txt"), "secret stuff").expect("write internal");

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(FileSystemReadTool::new(workspace_root.clone()))).unwrap();
    registry.register(Arc::new(FileSystemListTool::new(workspace_root.clone()))).unwrap();
    registry.register(Arc::new(FakeSendTool::new())).unwrap();

    // Workspace default is Internal; public.txt overridden to Public
    let mut wc = WorkspaceClassification::new(DataClass::Internal);
    wc.set_override("public.txt", DataClass::Public);
    wc.set_override("internal.txt", DataClass::Internal);

    let effective_dc = Arc::new(AtomicU8::new(DataClass::Public.to_i64() as u8));

    let context = LoopContext {
        conversation: ConversationContext {
            session_id: "session-e2e-send".to_string(),
            message_id: "msg-e2e-send".to_string(),
            prompt: String::new(),
            prompt_content_parts: vec![],
            history: vec![],
            conversation_journal: None,
            initial_tool_iterations: 0,
        },
        routing: RoutingConfig {
            required_capabilities: BTreeSet::new(),
            preferred_models: None,
            loop_strategy: None,
            routing_decision: None,
        },
        security: SecurityContext {
            data_class: DataClass::Public,
            permissions: Arc::new(parking_lot::Mutex::new(SessionPermissions::new())),
            workspace_classification: Some(Arc::new(wc)),
            effective_data_class: effective_dc.clone(),
            connector_service: Some(Arc::new(MockConnectorSvc)),
            shadow_mode: false,
        },
        tools_ctx: ToolsContext {
            tools: Arc::new(registry),
            skill_catalog: None,
            knowledge_query_handler: None,
            tool_execution_mode: ToolExecutionMode::default(),
        },
        agent: AgentContext {
            persona: None,
            agent_orchestrator: None,
            personas: Vec::new(),
            current_agent_id: None,
            parent_agent_id: None,
            workspace_path: None,
            keep_alive: false,
            session_messaged: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        },
        tool_limits: ToolLimitsConfig::default(),
        code_act_config: CodeActConfig::default(),
        session_registry: None,
        preempt_signal: None,
        cancellation_token: None,
    };

    let classification_mw: Arc<dyn LoopMiddleware> =
        Arc::new(crate::classification_middleware::DataClassificationMiddleware::new(Some(
            Arc::new(MockConnectorSvc),
        )));
    let mw: &[Arc<dyn LoopMiddleware>] = &[classification_mw];

    // 1. Agent lists directory (should not taint session)
    let outcome = run_single_tool_call(
        &ToolCall { tool_id: "filesystem.list".to_string(), input: json!({"path": "."}) },
        &context,
        mw,
        None,
        None,
        None,
    )
    .await;
    assert!(!outcome.is_error, "list failed: {}", outcome.output);
    assert_eq!(
        context.effective_data_class(),
        DataClass::Public,
        "filesystem.list must not escalate"
    );

    // 2. Agent reads public.txt
    let outcome = run_single_tool_call(
        &ToolCall { tool_id: "filesystem.read".to_string(), input: json!({"path": "public.txt"}) },
        &context,
        mw,
        None,
        None,
        None,
    )
    .await;
    assert!(!outcome.is_error, "read public.txt failed: {}", outcome.output);
    assert_eq!(
        context.effective_data_class(),
        DataClass::Public,
        "public file read must not escalate"
    );

    // 3. Agent sends via comm.send_external_message through Public connector.
    //    This MUST succeed (not be blocked by classification).
    let send_result = execute_tool_call(
        &context,
        ToolCall {
            tool_id: "comm.send_external_message".to_string(),
            input: json!({
                "connector_id": "test-connector",
                "to": "user@example.com",
                "body": "hello world"
            }),
        },
        mw,
        None,
        None,
        None,
    )
    .await
    .expect("send must NOT be blocked — session only has Public data");

    assert_eq!(send_result.output["status"], "sent");

    // 4. Now read the internal file — effective should escalate
    let outcome = run_single_tool_call(
        &ToolCall {
            tool_id: "filesystem.read".to_string(),
            input: json!({"path": "internal.txt"}),
        },
        &context,
        mw,
        None,
        None,
        None,
    )
    .await;
    assert!(!outcome.is_error);
    assert_eq!(
        context.effective_data_class(),
        DataClass::Internal,
        "internal file read must escalate"
    );

    // 5. Now sending through the Public connector SHOULD be blocked
    let send_result_2 = execute_tool_call(
        &context,
        ToolCall {
            tool_id: "comm.send_external_message".to_string(),
            input: json!({
                "connector_id": "test-connector",
                "to": "user@example.com",
                "body": "secret stuff"
            }),
        },
        mw,
        None,
        None,
        None,
    )
    .await;

    assert!(
        send_result_2.is_err(),
        "sending Internal data through Public connector must be blocked"
    );

    std::fs::remove_dir_all(&workspace_root).expect("cleanup");
}

/// When workspace classification uses the default (Public), reading any
/// file must NOT escalate effective_data_class above Public — regardless
/// of the hardcoded data_class on the tool result.
#[tokio::test]
async fn default_workspace_classification_does_not_taint_session() {
    use hive_connectors::ConnectorServiceHandle;

    struct MockConnectorSvc;
    impl ConnectorServiceHandle for MockConnectorSvc {
        fn resolve_output_class(&self, _cid: &str, _dest: &str) -> Option<DataClass> {
            Some(DataClass::Internal)
        }
        fn resolve_destination_approval(
            &self,
            _cid: &str,
            _dest: &str,
        ) -> Option<hive_contracts::ToolApproval> {
            None
        }
    }

    struct FakeSendTool(ToolDefinition);
    impl FakeSendTool {
        fn new() -> Self {
            Self(ToolDefinition {
                id: "comm.send_external_message".to_string(),
                name: "Send".to_string(),
                description: "mock".to_string(),
                input_schema: json!({"type": "object"}),
                output_schema: None,
                channel_class: ChannelClass::Internal,
                side_effects: true,
                approval: ToolApproval::Auto,
                annotations: ToolAnnotations {
                    title: "Send".to_string(),
                    read_only_hint: None,
                    destructive_hint: None,
                    idempotent_hint: None,
                    open_world_hint: None,
                },
            })
        }
    }
    impl Tool for FakeSendTool {
        fn definition(&self) -> &ToolDefinition {
            &self.0
        }
        fn execute(
            &self,
            _input: Value,
        ) -> hive_tools::BoxFuture<'_, Result<ToolResult, hive_tools::ToolError>> {
            Box::pin(async {
                Ok(ToolResult {
                    output: json!({"status": "sent"}),
                    data_class: DataClass::Internal,
                })
            })
        }
    }

    let workspace_root =
        std::env::temp_dir().join(format!("hive-loop-default-class-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&workspace_root).expect("create workspace");
    std::fs::write(workspace_root.join("readme.txt"), "public content").expect("write file");

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(FileSystemReadTool::new(workspace_root.clone()))).unwrap();
    registry.register(Arc::new(FakeSendTool::new())).unwrap();

    // Use DEFAULT workspace classification (no overrides at all).
    let wc = WorkspaceClassification::default();
    assert_eq!(wc.default, DataClass::Internal, "default must be Internal");

    let effective_dc = Arc::new(AtomicU8::new(DataClass::Internal.to_i64() as u8));
    let context = LoopContext {
        conversation: ConversationContext {
            session_id: "session-default-class".to_string(),
            message_id: "msg-default-class".to_string(),
            prompt: String::new(),
            prompt_content_parts: vec![],
            history: vec![],
            conversation_journal: None,
            initial_tool_iterations: 0,
        },
        routing: RoutingConfig {
            required_capabilities: BTreeSet::new(),
            preferred_models: None,
            loop_strategy: None,
            routing_decision: None,
        },
        security: SecurityContext {
            data_class: DataClass::Public,
            permissions: Arc::new(parking_lot::Mutex::new(SessionPermissions::new())),
            workspace_classification: Some(Arc::new(wc)),
            effective_data_class: effective_dc.clone(),
            connector_service: Some(Arc::new(MockConnectorSvc)),
            shadow_mode: false,
        },
        tools_ctx: ToolsContext {
            tools: Arc::new(registry),
            skill_catalog: None,
            knowledge_query_handler: None,
            tool_execution_mode: ToolExecutionMode::default(),
        },
        agent: AgentContext {
            persona: None,
            agent_orchestrator: None,
            personas: Vec::new(),
            current_agent_id: None,
            parent_agent_id: None,
            workspace_path: None,
            keep_alive: false,
            session_messaged: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        },
        tool_limits: ToolLimitsConfig::default(),
        code_act_config: CodeActConfig::default(),
        session_registry: None,
        preempt_signal: None,
        cancellation_token: None,
    };

    let classification_mw: Arc<dyn LoopMiddleware> =
        Arc::new(crate::classification_middleware::DataClassificationMiddleware::new(Some(
            Arc::new(MockConnectorSvc),
        )));
    let mw: &[Arc<dyn LoopMiddleware>] = &[classification_mw];

    // Read a file with no override — should resolve to workspace default (Internal)
    let outcome = run_single_tool_call(
        &ToolCall { tool_id: "filesystem.read".to_string(), input: json!({"path": "readme.txt"}) },
        &context,
        mw,
        None,
        None,
        None,
    )
    .await;
    assert!(!outcome.is_error, "read failed: {}", outcome.output);
    assert_eq!(
        context.effective_data_class(),
        DataClass::Internal,
        "reading file with default Internal classification must NOT escalate beyond Internal"
    );

    // Send through Internal connector — must succeed
    let send_result = execute_tool_call(
        &context,
        ToolCall {
            tool_id: "comm.send_external_message".to_string(),
            input: json!({
                "connector_id": "test-connector",
                "to": "user@example.com",
                "body": "public content"
            }),
        },
        mw,
        None,
        None,
        None,
    )
    .await
    .expect("send must succeed — session only has Internal data");

    assert_eq!(send_result.output["status"], "sent");

    std::fs::remove_dir_all(&workspace_root).expect("cleanup");
}

#[test]
fn strategy_kind_build_returns_correct_type() {
    // Verify that build() returns a strategy that can be used with LoopExecutor
    let react = StrategyKind::ReAct.build();
    let sequential = StrategyKind::Sequential.build();
    let plan = StrategyKind::PlanThenExecute.build();

    // Each should produce a valid Arc<dyn LoopStrategy>
    let _executor_react = LoopExecutor::new(react);
    let _executor_seq = LoopExecutor::new(sequential);
    let _executor_plan = LoopExecutor::new(plan);
}

#[test]
fn strategy_kind_equality() {
    assert_eq!(StrategyKind::ReAct, StrategyKind::ReAct);
    assert_eq!(StrategyKind::Sequential, StrategyKind::Sequential);
    assert_eq!(StrategyKind::PlanThenExecute, StrategyKind::PlanThenExecute);
    assert_ne!(StrategyKind::ReAct, StrategyKind::Sequential);
}

#[test]
fn parse_plan_numbered_list() {
    let plan = "1. First step\n2. Second step\n3. Third step";
    let steps = PlanThenExecuteStrategy::parse_plan(plan);
    assert_eq!(steps, vec!["First step", "Second step", "Third step"]);
}

#[test]
fn parse_plan_dash_list() {
    let plan = "- Step A\n- Step B";
    let steps = PlanThenExecuteStrategy::parse_plan(plan);
    assert_eq!(steps, vec!["Step A", "Step B"]);
}

#[test]
fn parse_plan_parses_all_steps() {
    let lines: Vec<String> = (1..=15).map(|i| format!("{i}. Step {i}")).collect();
    let plan = lines.join("\n");
    let steps = PlanThenExecuteStrategy::parse_plan(&plan);
    // parse_plan returns all parsed steps; truncation to MAX_PLAN_STEPS happens in run()
    assert_eq!(steps.len(), 15);
}

fn make_batch_context(mode: ToolExecutionMode) -> LoopContext {
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(CalculatorTool::default())).expect("register calculator");
    LoopContext {
        conversation: ConversationContext {
            session_id: "session-batch".to_string(),
            message_id: "msg-batch".to_string(),
            prompt: String::new(),
            prompt_content_parts: vec![],
            history: vec![],
            conversation_journal: None,
            initial_tool_iterations: 0,
        },
        routing: RoutingConfig {
            required_capabilities: BTreeSet::new(),
            preferred_models: None,
            loop_strategy: None,
            routing_decision: None,
        },
        security: SecurityContext {
            data_class: DataClass::Internal,
            permissions: Arc::new(parking_lot::Mutex::new(SessionPermissions::new())),
            workspace_classification: None,
            effective_data_class: Arc::new(AtomicU8::new(DataClass::Internal.to_i64() as u8)),
            connector_service: None,
            shadow_mode: false,
        },
        tools_ctx: ToolsContext {
            tools: Arc::new(registry),
            skill_catalog: None,
            knowledge_query_handler: None,
            tool_execution_mode: mode,
        },
        agent: AgentContext {
            persona: None,
            agent_orchestrator: None,
            personas: Vec::new(),
            current_agent_id: None,
            parent_agent_id: None,
            workspace_path: None,
            keep_alive: false,
            session_messaged: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        },
        tool_limits: ToolLimitsConfig::default(),
        code_act_config: CodeActConfig::default(),
        session_registry: None,
        preempt_signal: None,
        cancellation_token: None,
    }
}

fn good_call() -> ToolCall {
    ToolCall { tool_id: "math.calculate".to_string(), input: json!({"expression": "1+1"}) }
}

fn bad_call() -> ToolCall {
    ToolCall { tool_id: "nonexistent.tool".to_string(), input: json!({}) }
}

#[tokio::test]
async fn sequential_partial_stops_at_first_error() {
    let ctx = make_batch_context(ToolExecutionMode::SequentialPartial);
    let calls = vec![good_call(), bad_call(), good_call()];
    let (result, journal) = execute_tool_batch(&calls, &ctx, &[], None, None, None).await;

    // Should have 2 results: the first success and the error (third call skipped)
    assert_eq!(journal.len(), 2);
    assert!(!journal[0].output.contains("ERROR"));
    assert!(journal[1].output.contains("ERROR"));
    assert!(result.contains("math.calculate"));
    assert!(result.contains("nonexistent.tool"));
    // The third good_call should NOT appear
    assert_eq!(result.matches("math.calculate").count(), 1);
}

#[tokio::test]
async fn sequential_full_continues_past_errors() {
    let ctx = make_batch_context(ToolExecutionMode::SequentialFull);
    let calls = vec![good_call(), bad_call(), good_call()];
    let (result, journal) = execute_tool_batch(&calls, &ctx, &[], None, None, None).await;

    // Should have all 3 results
    assert_eq!(journal.len(), 3);
    assert!(!journal[0].output.contains("ERROR"));
    assert!(journal[1].output.contains("ERROR"));
    assert!(!journal[2].output.contains("ERROR"));
    // math.calculate should appear twice
    assert_eq!(result.matches("math.calculate").count(), 2);
}

#[tokio::test]
async fn parallel_executes_all_including_errors() {
    let ctx = make_batch_context(ToolExecutionMode::Parallel);
    let calls = vec![good_call(), bad_call(), good_call()];
    let (result, journal) = execute_tool_batch(&calls, &ctx, &[], None, None, None).await;

    // Should have all 3 results
    assert_eq!(journal.len(), 3);
    assert!(!journal[0].output.contains("ERROR"));
    assert!(journal[1].output.contains("ERROR"));
    assert!(!journal[2].output.contains("ERROR"));
    // math.calculate should appear twice
    assert_eq!(result.matches("math.calculate").count(), 2);
}

#[tokio::test]
async fn sequential_partial_succeeds_when_no_errors() {
    let ctx = make_batch_context(ToolExecutionMode::SequentialPartial);
    let calls = vec![good_call(), good_call()];
    let (_, journal) = execute_tool_batch(&calls, &ctx, &[], None, None, None).await;

    assert_eq!(journal.len(), 2);
    assert!(journal.iter().all(|j| !j.output.contains("ERROR")));
}

/// A deny rule with bare email pattern `*@domain.com` must block
/// `comm.send_external_message` to `user@domain.com` even without
/// the `comm:` prefix in the scope.
#[tokio::test]
async fn bare_email_deny_rule_blocks_comm_send() {
    use hive_connectors::ConnectorServiceHandle;

    struct MockConnectorSvc;
    impl ConnectorServiceHandle for MockConnectorSvc {
        fn resolve_output_class(&self, _cid: &str, _dest: &str) -> Option<DataClass> {
            Some(DataClass::Public)
        }
        fn resolve_destination_approval(
            &self,
            _cid: &str,
            _dest: &str,
        ) -> Option<hive_contracts::ToolApproval> {
            None
        }
    }

    struct FakeSendTool(ToolDefinition);
    impl FakeSendTool {
        fn new() -> Self {
            Self(ToolDefinition {
                id: "comm.send_external_message".to_string(),
                name: "Send".to_string(),
                description: "mock".to_string(),
                input_schema: json!({"type": "object"}),
                output_schema: None,
                channel_class: ChannelClass::Public,
                side_effects: true,
                approval: ToolApproval::Auto,
                annotations: ToolAnnotations {
                    title: "Send".to_string(),
                    read_only_hint: None,
                    destructive_hint: None,
                    idempotent_hint: None,
                    open_world_hint: None,
                },
            })
        }
    }
    impl Tool for FakeSendTool {
        fn definition(&self) -> &ToolDefinition {
            &self.0
        }
        fn execute(
            &self,
            _input: Value,
        ) -> hive_tools::BoxFuture<'_, Result<ToolResult, hive_tools::ToolError>> {
            Box::pin(async {
                Ok(ToolResult { output: json!({"status": "sent"}), data_class: DataClass::Public })
            })
        }
    }

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(FakeSendTool::new())).unwrap();

    // Session permissions with a bare email deny rule (no `comm:` prefix)
    let mut perms = SessionPermissions::new();
    perms.add_rule(PermissionRule {
        tool_pattern: "comm.send_external_message".to_string(),
        scope: "*@blocked.com".to_string(),
        decision: ToolApproval::Deny,
    });

    let effective_dc = Arc::new(AtomicU8::new(DataClass::Public.to_i64() as u8));

    let context = LoopContext {
        conversation: ConversationContext {
            session_id: "session-deny-test".to_string(),
            message_id: "msg-deny-test".to_string(),
            prompt: String::new(),
            prompt_content_parts: vec![],
            history: vec![],
            conversation_journal: None,
            initial_tool_iterations: 0,
        },
        routing: RoutingConfig {
            required_capabilities: BTreeSet::new(),
            preferred_models: None,
            loop_strategy: None,
            routing_decision: None,
        },
        security: SecurityContext {
            data_class: DataClass::Public,
            permissions: Arc::new(parking_lot::Mutex::new(perms)),
            workspace_classification: None,
            effective_data_class: effective_dc,
            connector_service: Some(Arc::new(MockConnectorSvc)),
            shadow_mode: false,
        },
        tools_ctx: ToolsContext {
            tools: Arc::new(registry),
            skill_catalog: None,
            knowledge_query_handler: None,
            tool_execution_mode: ToolExecutionMode::default(),
        },
        agent: AgentContext {
            persona: None,
            agent_orchestrator: None,
            personas: Vec::new(),
            current_agent_id: None,
            parent_agent_id: None,
            workspace_path: None,
            keep_alive: false,
            session_messaged: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        },
        tool_limits: ToolLimitsConfig::default(),
        code_act_config: CodeActConfig::default(),
        session_registry: None,
        preempt_signal: None,
        cancellation_token: None,
    };

    let classification_mw: Arc<dyn LoopMiddleware> =
        Arc::new(crate::classification_middleware::DataClassificationMiddleware::new(Some(
            Arc::new(MockConnectorSvc),
        )));
    let mw: &[Arc<dyn LoopMiddleware>] = &[classification_mw];

    // 1. Sending to user@blocked.com must be DENIED
    let result = execute_tool_call(
        &context,
        ToolCall {
            tool_id: "comm.send_external_message".to_string(),
            input: json!({
                "connector_id": "test-connector",
                "to": "user@blocked.com",
                "body": "hello"
            }),
        },
        mw,
        None,
        None,
        None,
    )
    .await;

    assert!(result.is_err(), "send to user@blocked.com must be denied by bare email rule");
    let err = result.unwrap_err();
    assert!(matches!(err, LoopError::ToolDenied { .. }), "expected ToolDenied, got: {err:?}");

    // 2. Sending to user@allowed.com must SUCCEED (no matching deny rule)
    let result = execute_tool_call(
        &context,
        ToolCall {
            tool_id: "comm.send_external_message".to_string(),
            input: json!({
                "connector_id": "test-connector",
                "to": "user@allowed.com",
                "body": "hello"
            }),
        },
        mw,
        None,
        None,
        None,
    )
    .await;

    assert!(result.is_ok(), "send to user@allowed.com must not be blocked: {result:?}");
}

/// Deny rule with fully-qualified `comm:*:*@domain.com` scope must also work.
#[tokio::test]
async fn qualified_comm_deny_rule_blocks_comm_send() {
    use hive_connectors::ConnectorServiceHandle;

    struct MockConnectorSvc;
    impl ConnectorServiceHandle for MockConnectorSvc {
        fn resolve_output_class(&self, _cid: &str, _dest: &str) -> Option<DataClass> {
            Some(DataClass::Public)
        }
        fn resolve_destination_approval(
            &self,
            _cid: &str,
            _dest: &str,
        ) -> Option<hive_contracts::ToolApproval> {
            None
        }
    }

    struct FakeSendTool(ToolDefinition);
    impl FakeSendTool {
        fn new() -> Self {
            Self(ToolDefinition {
                id: "comm.send_external_message".to_string(),
                name: "Send".to_string(),
                description: "mock".to_string(),
                input_schema: json!({"type": "object"}),
                output_schema: None,
                channel_class: ChannelClass::Public,
                side_effects: true,
                approval: ToolApproval::Auto,
                annotations: ToolAnnotations {
                    title: "Send".to_string(),
                    read_only_hint: None,
                    destructive_hint: None,
                    idempotent_hint: None,
                    open_world_hint: None,
                },
            })
        }
    }
    impl Tool for FakeSendTool {
        fn definition(&self) -> &ToolDefinition {
            &self.0
        }
        fn execute(
            &self,
            _input: Value,
        ) -> hive_tools::BoxFuture<'_, Result<ToolResult, hive_tools::ToolError>> {
            Box::pin(async {
                Ok(ToolResult { output: json!({"status": "sent"}), data_class: DataClass::Public })
            })
        }
    }

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(FakeSendTool::new())).unwrap();

    // Fully-qualified deny rule
    let mut perms = SessionPermissions::new();
    perms.add_rule(PermissionRule {
        tool_pattern: "comm.*".to_string(),
        scope: "comm:*:*@blocked.com".to_string(),
        decision: ToolApproval::Deny,
    });

    let effective_dc = Arc::new(AtomicU8::new(DataClass::Public.to_i64() as u8));

    let context = LoopContext {
        conversation: ConversationContext {
            session_id: "session-deny-qualified".to_string(),
            message_id: "msg-deny-qualified".to_string(),
            prompt: String::new(),
            prompt_content_parts: vec![],
            history: vec![],
            conversation_journal: None,
            initial_tool_iterations: 0,
        },
        routing: RoutingConfig {
            required_capabilities: BTreeSet::new(),
            preferred_models: None,
            loop_strategy: None,
            routing_decision: None,
        },
        security: SecurityContext {
            data_class: DataClass::Public,
            permissions: Arc::new(parking_lot::Mutex::new(perms)),
            workspace_classification: None,
            effective_data_class: effective_dc,
            connector_service: Some(Arc::new(MockConnectorSvc)),
            shadow_mode: false,
        },
        tools_ctx: ToolsContext {
            tools: Arc::new(registry),
            skill_catalog: None,
            knowledge_query_handler: None,
            tool_execution_mode: ToolExecutionMode::default(),
        },
        agent: AgentContext {
            persona: None,
            agent_orchestrator: None,
            personas: Vec::new(),
            current_agent_id: None,
            parent_agent_id: None,
            workspace_path: None,
            keep_alive: false,
            session_messaged: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        },
        tool_limits: ToolLimitsConfig::default(),
        code_act_config: CodeActConfig::default(),
        session_registry: None,
        preempt_signal: None,
        cancellation_token: None,
    };

    let classification_mw: Arc<dyn LoopMiddleware> =
        Arc::new(crate::classification_middleware::DataClassificationMiddleware::new(Some(
            Arc::new(MockConnectorSvc),
        )));
    let mw: &[Arc<dyn LoopMiddleware>] = &[classification_mw];

    let result = execute_tool_call(
        &context,
        ToolCall {
            tool_id: "comm.send_external_message".to_string(),
            input: json!({
                "connector_id": "test-connector",
                "to": "boss@blocked.com",
                "body": "hello"
            }),
        },
        mw,
        None,
        None,
        None,
    )
    .await;

    assert!(result.is_err(), "qualified comm deny rule must block send");
    assert!(matches!(result.unwrap_err(), LoopError::ToolDenied { .. }));
}

// ── Preemption tests ────────────────────────────────────────────────

#[test]
fn build_preemption_summary_formats_tool_calls() {
    let mut journal = ConversationJournal::default();
    journal.record(JournalEntry {
        phase: JournalPhase::ToolCycle,
        turn: 1,
        tool_calls: vec![
            JournalToolCall {
                tool_id: "file.read".to_string(),
                input: r#"{"path":"src/main.rs"}"#.to_string(),
                output: "fn main() {}".to_string(),
                tool_call_id: None,
                is_error: false,
            },
            JournalToolCall {
                tool_id: "search".to_string(),
                input: r#"{"query":"auth"}"#.to_string(),
                output: "3 results found".to_string(),
                tool_call_id: None,
                is_error: false,
            },
        ],
        assistant_content: None,
    });

    let summary = super::build_preemption_summary(&journal);
    assert!(summary.contains("[Turn paused to process a new message]"));
    assert!(summary.contains("1. Called `file.read`"));
    assert!(summary.contains("2. Called `search`"));
    assert!(summary.contains("fn main() {}"));
}

#[test]
fn build_preemption_summary_truncates_long_output() {
    let mut journal = ConversationJournal::default();
    let long_output = "x".repeat(300);
    journal.record(JournalEntry {
        phase: JournalPhase::ToolCycle,
        turn: 1,
        tool_calls: vec![JournalToolCall {
            tool_id: "file.read".to_string(),
            input: "{}".to_string(),
            output: long_output,
            tool_call_id: None,
            is_error: false,
        }],
        assistant_content: None,
    });

    let summary = super::build_preemption_summary(&journal);
    // Output should be truncated to ~200 chars + ellipsis
    assert!(summary.contains("…"));
    assert!(summary.len() < 400);
}

#[test]
fn build_preemption_summary_empty_journal() {
    let journal = ConversationJournal::default();
    let summary = super::build_preemption_summary(&journal);
    assert!(summary.contains("(no tool calls completed)"));
}

#[tokio::test]
async fn check_preempt_returns_none_when_signal_not_set() {
    let signal = Arc::new(AtomicBool::new(false));
    let decision = RoutingDecision {
        selected: ModelSelection { provider_id: "p".into(), model: "m".into() },
        fallback_chain: vec![],
        reason: String::new(),
        effective_context_window: None,
        effective_max_output_tokens: None,
    };

    let result = super::check_preempt(&Some(signal), &None, &decision, "p", "m", None).await;
    assert!(result.is_none());
}

#[tokio::test]
async fn check_preempt_returns_none_when_no_signal() {
    let decision = RoutingDecision {
        selected: ModelSelection { provider_id: "p".into(), model: "m".into() },
        fallback_chain: vec![],
        reason: String::new(),
        effective_context_window: None,
        effective_max_output_tokens: None,
    };

    let result = super::check_preempt(&None, &None, &decision, "p", "m", None).await;
    assert!(result.is_none());
}

#[tokio::test]
async fn check_preempt_returns_result_when_signal_set() {
    let signal = Arc::new(AtomicBool::new(true));
    let decision = RoutingDecision {
        selected: ModelSelection { provider_id: "test".into(), model: "test-model".into() },
        fallback_chain: vec![],
        reason: String::new(),
        effective_context_window: None,
        effective_max_output_tokens: None,
    };

    let result =
        super::check_preempt(&Some(signal), &None, &decision, "test", "test-model", None).await;
    assert!(result.is_some());
    let r = result.unwrap();
    assert!(r.preempted);
    assert!(r.content.contains("[Turn paused"));
}

#[tokio::test]
async fn check_preempt_emits_event() {
    let signal = Arc::new(AtomicBool::new(true));
    let decision = RoutingDecision {
        selected: ModelSelection { provider_id: "p".into(), model: "m".into() },
        fallback_chain: vec![],
        reason: String::new(),
        effective_context_window: None,
        effective_max_output_tokens: None,
    };
    let (tx, mut rx) = tokio::sync::mpsc::channel::<LoopEvent>(10);

    let _ = super::check_preempt(&Some(signal), &None, &decision, "p", "m", Some(&tx)).await;

    let event = rx.try_recv().expect("should have emitted an event");
    assert!(matches!(event, LoopEvent::Preempted));
}

#[tokio::test]
async fn react_preempts_after_tool_batch_when_signal_set() {
    // Set up: model returns a tool call, then (if not preempted) "done".
    // Signal is set before the loop starts, so it should preempt after
    // the first tool batch instead of calling the model a second time.
    let responses = vec![
        "<tool_call>{\"tool\":\"math.calculate\",\"input\":{\"expression\":\"1+1\"}}</tool_call>"
            .to_string(),
        "done".to_string(),
    ];
    let (provider, prompts) = TestProvider::new(responses);
    let mut router = ModelRouter::new();
    router.register_provider(provider);
    let router = Arc::new(router);

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(CalculatorTool::default())).expect("register");
    let tools = Arc::new(registry);

    let signal = Arc::new(AtomicBool::new(true));

    let executor = LoopExecutor::new(Arc::new(ReActStrategy));
    let context = LoopContext {
        conversation: ConversationContext {
            session_id: "session-preempt".to_string(),
            message_id: "msg-preempt".to_string(),
            prompt: "Calculate 1+1".to_string(),
            prompt_content_parts: vec![],
            history: vec![],
            conversation_journal: Some(Arc::new(parking_lot::Mutex::new(
                ConversationJournal::default(),
            ))),
            initial_tool_iterations: 0,
        },
        routing: RoutingConfig {
            required_capabilities: BTreeSet::new(),
            preferred_models: None,
            loop_strategy: None,
            routing_decision: None,
        },
        security: SecurityContext {
            data_class: DataClass::Internal,
            permissions: Arc::new(parking_lot::Mutex::new(SessionPermissions::new())),
            workspace_classification: None,
            effective_data_class: Arc::new(AtomicU8::new(DataClass::Internal.to_i64() as u8)),
            connector_service: None,
            shadow_mode: false,
        },
        tools_ctx: ToolsContext {
            tools,
            skill_catalog: None,
            knowledge_query_handler: None,
            tool_execution_mode: ToolExecutionMode::default(),
        },
        agent: AgentContext {
            persona: None,
            agent_orchestrator: None,
            personas: Vec::new(),
            current_agent_id: None,
            parent_agent_id: None,
            workspace_path: None,
            keep_alive: false,
            session_messaged: Arc::new(AtomicBool::new(false)),
        },
        tool_limits: ToolLimitsConfig::default(),
        code_act_config: CodeActConfig::default(),
        session_registry: None,
        preempt_signal: Some(signal),
        cancellation_token: None,
    };

    let result = executor.run(context, router).await.expect("loop result");

    // Should have been preempted after the first tool batch
    assert!(result.preempted, "result should be preempted");
    assert!(result.content.contains("[Turn paused"));
    assert!(result.content.contains("math.calculate"));

    // Model should have been called only once (the tool-call response),
    // not a second time (the "done" response was never consumed).
    let recorded = prompts.lock().unwrap();
    assert_eq!(recorded.len(), 1, "model should only be called once before preemption");
}

#[tokio::test]
async fn react_completes_normally_without_signal() {
    // Same setup as above but without preempt signal — should run to completion.
    let responses = vec![
        "<tool_call>{\"tool\":\"math.calculate\",\"input\":{\"expression\":\"1+1\"}}</tool_call>"
            .to_string(),
        "done".to_string(),
    ];
    let (provider, prompts) = TestProvider::new(responses);
    let mut router = ModelRouter::new();
    router.register_provider(provider);
    let router = Arc::new(router);

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(CalculatorTool::default())).expect("register");

    let executor = LoopExecutor::new(Arc::new(ReActStrategy));
    let context = LoopContext {
        conversation: ConversationContext {
            session_id: "session-normal".to_string(),
            message_id: "msg-normal".to_string(),
            prompt: "Calculate 1+1".to_string(),
            prompt_content_parts: vec![],
            history: vec![],
            conversation_journal: None,
            initial_tool_iterations: 0,
        },
        routing: RoutingConfig {
            required_capabilities: BTreeSet::new(),
            preferred_models: None,
            loop_strategy: None,
            routing_decision: None,
        },
        security: SecurityContext {
            data_class: DataClass::Internal,
            permissions: Arc::new(parking_lot::Mutex::new(SessionPermissions::new())),
            workspace_classification: None,
            effective_data_class: Arc::new(AtomicU8::new(DataClass::Internal.to_i64() as u8)),
            connector_service: None,
            shadow_mode: false,
        },
        tools_ctx: ToolsContext {
            tools: Arc::new(registry),
            skill_catalog: None,
            knowledge_query_handler: None,
            tool_execution_mode: ToolExecutionMode::default(),
        },
        agent: AgentContext {
            persona: None,
            agent_orchestrator: None,
            personas: Vec::new(),
            current_agent_id: None,
            parent_agent_id: None,
            workspace_path: None,
            keep_alive: false,
            session_messaged: Arc::new(AtomicBool::new(false)),
        },
        tool_limits: ToolLimitsConfig::default(),
        code_act_config: CodeActConfig::default(),
        session_registry: None,
        preempt_signal: None,
        cancellation_token: None,
    };

    let result = executor.run(context, router).await.expect("loop result");
    assert!(!result.preempted, "should not be preempted");
    assert_eq!(result.content, "done");

    let recorded = prompts.lock().unwrap();
    assert_eq!(recorded.len(), 2, "model should be called twice");
}

#[tokio::test]
async fn react_no_preempt_when_no_tools_called() {
    // Model returns plain text (no tool calls). Even with signal set,
    // there is no tool batch checkpoint, so it should complete normally.
    let responses = vec!["Just a text response".to_string()];
    let (provider, _prompts) = TestProvider::new(responses);
    let mut router = ModelRouter::new();
    router.register_provider(provider);
    let router = Arc::new(router);

    let signal = Arc::new(AtomicBool::new(true));

    let executor = LoopExecutor::new(Arc::new(ReActStrategy));
    let context = LoopContext {
        conversation: ConversationContext {
            session_id: "session-notools".to_string(),
            message_id: "msg-notools".to_string(),
            prompt: "Hello".to_string(),
            prompt_content_parts: vec![],
            history: vec![],
            conversation_journal: None,
            initial_tool_iterations: 0,
        },
        routing: RoutingConfig {
            required_capabilities: BTreeSet::new(),
            preferred_models: None,
            loop_strategy: None,
            routing_decision: None,
        },
        security: SecurityContext {
            data_class: DataClass::Internal,
            permissions: Arc::new(parking_lot::Mutex::new(SessionPermissions::new())),
            workspace_classification: None,
            effective_data_class: Arc::new(AtomicU8::new(DataClass::Internal.to_i64() as u8)),
            connector_service: None,
            shadow_mode: false,
        },
        tools_ctx: ToolsContext {
            tools: Arc::new(ToolRegistry::new()),
            skill_catalog: None,
            knowledge_query_handler: None,
            tool_execution_mode: ToolExecutionMode::default(),
        },
        agent: AgentContext {
            persona: None,
            agent_orchestrator: None,
            personas: Vec::new(),
            current_agent_id: None,
            parent_agent_id: None,
            workspace_path: None,
            keep_alive: false,
            session_messaged: Arc::new(AtomicBool::new(false)),
        },
        tool_limits: ToolLimitsConfig::default(),
        code_act_config: CodeActConfig::default(),
        session_registry: None,
        preempt_signal: Some(signal),
        cancellation_token: None,
    };

    let result = executor.run(context, router).await.expect("loop result");
    // No tool batch → no checkpoint → no preemption
    assert!(!result.preempted);
    assert_eq!(result.content, "Just a text response");
}

// ── Budget exemption tests ──────────────────────────────────────

#[test]
fn test_budget_exempt_agent_tools() {
    assert!(is_budget_exempt("core.list_agents"));
    assert!(is_budget_exempt("core.get_agent_result"));
    assert!(is_budget_exempt("core.wait_for_agent"));
}

#[test]
fn test_budget_exempt_process_tools() {
    assert!(is_budget_exempt("process.status"));
    assert!(is_budget_exempt("process.list"));
}

#[test]
fn test_budget_not_exempt_regular_tools() {
    assert!(!is_budget_exempt("core.spawn_agent"));
    assert!(!is_budget_exempt("core.signal_agent"));
    assert!(!is_budget_exempt("core.kill_agent"));
    assert!(!is_budget_exempt("shell.exec"));
    assert!(!is_budget_exempt("fs.read"));
    assert!(!is_budget_exempt("process.start"));
    assert!(!is_budget_exempt("process.kill"));
    assert!(!is_budget_exempt("process.write"));
}

// ── Access control tests ───────────────────────────────────────────────

/// Orchestrator mock that tracks parent relationships for access control tests.
struct AccessControlOrchestrator {
    /// Map of agent_id → parent_id.
    parents: std::collections::HashMap<String, Option<String>>,
    messages: Mutex<Vec<(String, String, String)>>,
    kills: Mutex<Vec<String>>,
}

impl AccessControlOrchestrator {
    fn new(parents: Vec<(&str, Option<&str>)>) -> Self {
        Self {
            parents: parents
                .into_iter()
                .map(|(id, parent)| (id.to_string(), parent.map(|s| s.to_string())))
                .collect(),
            messages: Mutex::new(Vec::new()),
            kills: Mutex::new(Vec::new()),
        }
    }
}

impl AgentOrchestrator for AccessControlOrchestrator {
    fn spawn_agent(
        &self,
        _: Persona,
        _: String,
        _: Option<String>,
        _: Option<String>,
        _: hive_classification::DataClass,
        _: Option<hive_model::ModelSelection>,
        _: bool,
        _: Option<PathBuf>,
    ) -> BoxFuture<'_, Result<String, String>> {
        Box::pin(async { Ok("new-id".to_string()) })
    }
    fn message_agent(
        &self,
        agent_id: String,
        message: String,
        from: String,
    ) -> BoxFuture<'_, Result<(), String>> {
        self.messages.lock().unwrap().push((agent_id, message, from));
        Box::pin(async { Ok(()) })
    }
    fn message_session(&self, _: String, _: String) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }
    fn feedback_agent(&self, _: String, _: String, _: String) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }
    fn list_agents(
        &self,
    ) -> BoxFuture<'_, Result<Vec<(String, String, String, String, Option<String>)>, String>> {
        Box::pin(async { Ok(vec![]) })
    }
    fn get_agent_result(
        &self,
        _: String,
    ) -> BoxFuture<'_, Result<(String, Option<String>), String>> {
        Box::pin(async { Ok(("Done".to_string(), None)) })
    }
    fn kill_agent(&self, agent_id: String) -> BoxFuture<'_, Result<(), String>> {
        self.kills.lock().unwrap().push(agent_id);
        Box::pin(async { Ok(()) })
    }
    fn get_agent_parent(&self, agent_id: String) -> BoxFuture<'_, Result<Option<String>, String>> {
        let parent = self.parents.get(&agent_id).cloned();
        Box::pin(async move { parent.ok_or_else(|| format!("agent '{agent_id}' not found")) })
    }
}

fn make_access_control_context(
    orchestrator: Arc<dyn AgentOrchestrator>,
    caller_id: Option<&str>,
    parent_id: Option<&str>,
) -> LoopContext {
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(SignalAgentTool::default())).expect("register signal tool");
    registry.register(Arc::new(KillAgentTool::default())).expect("register kill tool");
    LoopContext {
        conversation: ConversationContext {
            session_id: "session-acl".to_string(),
            message_id: "msg-acl".to_string(),
            prompt: "test access".to_string(),
            prompt_content_parts: vec![],
            history: vec![],
            conversation_journal: None,
            initial_tool_iterations: 0,
        },
        routing: RoutingConfig {
            required_capabilities: BTreeSet::new(),
            preferred_models: None,
            loop_strategy: None,
            routing_decision: None,
        },
        security: SecurityContext {
            data_class: DataClass::Internal,
            permissions: Arc::new(parking_lot::Mutex::new(SessionPermissions::new())),
            workspace_classification: None,
            effective_data_class: Arc::new(AtomicU8::new(DataClass::Internal.to_i64() as u8)),
            connector_service: None,
            shadow_mode: false,
        },
        tools_ctx: ToolsContext {
            tools: Arc::new(registry),
            skill_catalog: None,
            knowledge_query_handler: None,
            tool_execution_mode: ToolExecutionMode::default(),
        },
        agent: AgentContext {
            persona: Some(Persona::default_persona()),
            agent_orchestrator: Some(orchestrator),
            personas: Vec::new(),
            current_agent_id: caller_id.map(|s| s.to_string()),
            parent_agent_id: parent_id.map(|s| s.to_string()),
            workspace_path: None,
            keep_alive: false,
            session_messaged: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        },
        tool_limits: ToolLimitsConfig::default(),
        code_act_config: CodeActConfig::default(),
        session_registry: None,
        preempt_signal: None,
        cancellation_token: None,
    }
}

#[tokio::test]
async fn signal_agent_allows_parent_to_child() {
    // parent-agent spawned child-agent
    let orch = Arc::new(AccessControlOrchestrator::new(vec![
        ("parent-agent", None),
        ("child-agent", Some("parent-agent")),
    ]));
    let ctx = make_access_control_context(orch.clone(), Some("parent-agent"), None);
    let result = execute_tool_call(
        &ctx,
        ToolCall {
            tool_id: "core.signal_agent".to_string(),
            input: json!({"agent_id": "child-agent", "content": "hello"}),
        },
        &[],
        None,
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(result.output["delivered"], json!(true));
}

#[tokio::test]
async fn signal_agent_allows_child_to_parent() {
    let orch = Arc::new(AccessControlOrchestrator::new(vec![
        ("parent-agent", None),
        ("child-agent", Some("parent-agent")),
    ]));
    // child signals parent — caller_parent matches target_id
    let ctx = make_access_control_context(orch.clone(), Some("child-agent"), Some("parent-agent"));
    let result = execute_tool_call(
        &ctx,
        ToolCall {
            tool_id: "core.signal_agent".to_string(),
            input: json!({"agent_id": "parent-agent", "content": "done"}),
        },
        &[],
        None,
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(result.output["delivered"], json!(true));
}

#[tokio::test]
async fn signal_agent_allows_siblings() {
    // Both agents spawned by same parent
    let orch = Arc::new(AccessControlOrchestrator::new(vec![
        ("sibling-a", Some("root")),
        ("sibling-b", Some("root")),
    ]));
    let ctx = make_access_control_context(orch.clone(), Some("sibling-a"), Some("root"));
    let result = execute_tool_call(
        &ctx,
        ToolCall {
            tool_id: "core.signal_agent".to_string(),
            input: json!({"agent_id": "sibling-b", "content": "hey"}),
        },
        &[],
        None,
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(result.output["delivered"], json!(true));
}

#[tokio::test]
async fn signal_agent_allows_root_siblings() {
    // Both root-level agents (no parent — spawned by session)
    let orch = Arc::new(AccessControlOrchestrator::new(vec![("root-a", None), ("root-b", None)]));
    let ctx = make_access_control_context(orch.clone(), Some("root-a"), None);
    let result = execute_tool_call(
        &ctx,
        ToolCall {
            tool_id: "core.signal_agent".to_string(),
            input: json!({"agent_id": "root-b", "content": "hi"}),
        },
        &[],
        None,
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(result.output["delivered"], json!(true));
}

#[tokio::test]
async fn signal_agent_denies_unrelated_agents() {
    // agent-a and agent-x have different parents, not related
    let orch = Arc::new(AccessControlOrchestrator::new(vec![
        ("agent-a", Some("parent-1")),
        ("agent-x", Some("parent-2")),
    ]));
    let ctx = make_access_control_context(orch.clone(), Some("agent-a"), Some("parent-1"));
    let result = execute_tool_call(
        &ctx,
        ToolCall {
            tool_id: "core.signal_agent".to_string(),
            input: json!({"agent_id": "agent-x", "content": "nope"}),
        },
        &[],
        None,
        None,
        None,
    )
    .await
    .unwrap();
    assert!(result.output["error"].as_str().unwrap().contains("Access denied"));
}

#[tokio::test]
async fn signal_agent_allows_bot_prefix() {
    // Bot targets always allowed for messaging, even without family relationship
    let orch = Arc::new(AccessControlOrchestrator::new(vec![]));
    let ctx = make_access_control_context(orch.clone(), Some("agent-a"), Some("parent-1"));
    let result = execute_tool_call(
        &ctx,
        ToolCall {
            tool_id: "core.signal_agent".to_string(),
            input: json!({"agent_id": "bot:my-bot", "content": "work"}),
        },
        &[],
        None,
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(result.output["delivered"], json!(true));
}

#[tokio::test]
async fn signal_agent_allows_session_level_caller() {
    // Session-level callers (no current_agent_id) are always allowed
    let orch = Arc::new(AccessControlOrchestrator::new(vec![("any-agent", Some("someone"))]));
    let ctx = make_access_control_context(orch.clone(), None, None);
    let result = execute_tool_call(
        &ctx,
        ToolCall {
            tool_id: "core.signal_agent".to_string(),
            input: json!({"agent_id": "any-agent", "content": "hi"}),
        },
        &[],
        None,
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(result.output["delivered"], json!(true));
}

#[tokio::test]
async fn kill_agent_allows_parent_to_kill_child() {
    let orch = Arc::new(AccessControlOrchestrator::new(vec![
        ("parent-agent", None),
        ("child-agent", Some("parent-agent")),
    ]));
    let ctx = make_access_control_context(orch.clone(), Some("parent-agent"), None);
    let result = execute_tool_call(
        &ctx,
        ToolCall {
            tool_id: "core.kill_agent".to_string(),
            input: json!({"agent_id": "child-agent"}),
        },
        &[],
        None,
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(result.output["killed"], json!(true));
    assert_eq!(orch.kills.lock().unwrap().as_slice(), ["child-agent"]);
}

#[tokio::test]
async fn kill_agent_denies_child_killing_parent() {
    let orch = Arc::new(AccessControlOrchestrator::new(vec![
        ("parent-agent", None),
        ("child-agent", Some("parent-agent")),
    ]));
    let ctx = make_access_control_context(orch.clone(), Some("child-agent"), Some("parent-agent"));
    let result = execute_tool_call(
        &ctx,
        ToolCall {
            tool_id: "core.kill_agent".to_string(),
            input: json!({"agent_id": "parent-agent"}),
        },
        &[],
        None,
        None,
        None,
    )
    .await
    .unwrap();
    assert!(result.output["error"].as_str().unwrap().contains("Access denied"));
    assert!(orch.kills.lock().unwrap().is_empty());
}

#[tokio::test]
async fn kill_agent_denies_sibling_kill() {
    let orch = Arc::new(AccessControlOrchestrator::new(vec![
        ("sibling-a", Some("root")),
        ("sibling-b", Some("root")),
    ]));
    let ctx = make_access_control_context(orch.clone(), Some("sibling-a"), Some("root"));
    let result = execute_tool_call(
        &ctx,
        ToolCall {
            tool_id: "core.kill_agent".to_string(),
            input: json!({"agent_id": "sibling-b"}),
        },
        &[],
        None,
        None,
        None,
    )
    .await
    .unwrap();
    assert!(result.output["error"].as_str().unwrap().contains("Access denied"));
}

#[tokio::test]
async fn kill_agent_denies_bot_prefix() {
    let orch = Arc::new(AccessControlOrchestrator::new(vec![]));
    let ctx = make_access_control_context(orch.clone(), Some("agent-a"), Some("root"));
    let result = execute_tool_call(
        &ctx,
        ToolCall {
            tool_id: "core.kill_agent".to_string(),
            input: json!({"agent_id": "bot:my-bot"}),
        },
        &[],
        None,
        None,
        None,
    )
    .await
    .unwrap();
    assert!(result.output["error"].as_str().unwrap().contains("Access denied"));
}

#[tokio::test]
async fn kill_agent_allows_session_level_caller() {
    let orch = Arc::new(AccessControlOrchestrator::new(vec![("any-agent", Some("root"))]));
    let ctx = make_access_control_context(orch.clone(), None, None);
    let result = execute_tool_call(
        &ctx,
        ToolCall {
            tool_id: "core.kill_agent".to_string(),
            input: json!({"agent_id": "any-agent"}),
        },
        &[],
        None,
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(result.output["killed"], json!(true));
}

#[tokio::test]
async fn inject_pending_makes_question_visible() {
    let gate = UserInteractionGate::new();
    let kind = InteractionKind::Question {
        text: "What color?".into(),
        choices: vec!["red".into(), "blue".into()],
        allow_freeform: true,
        multi_select: false,
        message: None,
    };
    gate.inject_pending("q-1".to_string(), kind.clone());

    // Should appear in list_pending
    let pending = gate.list_pending();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].0, "q-1");

    // Responding should remove it from the gate
    let response = UserInteractionResponse {
        request_id: "q-1".to_string(),
        payload: InteractionResponsePayload::Answer {
            selected_choice: Some(0),
            selected_choices: None,
            text: None,
        },
    };
    assert!(gate.respond(response));
    assert!(gate.list_pending().is_empty());
}

#[tokio::test]
async fn remove_all_except_clears_stale_injected_entries() {
    let gate = UserInteractionGate::new();
    let kind = InteractionKind::Question {
        text: "old question".into(),
        choices: vec![],
        allow_freeform: true,
        multi_select: false,
        message: None,
    };
    // Inject an old entry (simulating daemon restart injection)
    gate.inject_pending("old-q".to_string(), kind.clone());

    // Agent re-asks through the normal path → new entry
    let _rx = gate.create_request(
        "new-q".to_string(),
        InteractionKind::Question {
            text: "new question".into(),
            choices: vec![],
            allow_freeform: true,
            multi_select: false,
            message: None,
        },
    );

    assert_eq!(gate.list_pending().len(), 2);

    // Clean up stale entries, keeping the new one
    let removed = gate.remove_all_except("new-q");
    assert_eq!(removed, vec!["old-q"]);

    // Only the new entry should remain
    let pending = gate.list_pending();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].0, "new-q");
}

#[tokio::test]
async fn close_gate_unblocks_pending_question() {
    let gate = Arc::new(UserInteractionGate::new());
    let rx = gate.create_request(
        "q-block".to_string(),
        InteractionKind::Question {
            text: "blocking question".into(),
            choices: vec![],
            allow_freeform: true,
            multi_select: false,
            message: None,
        },
    );

    // Spawn a task that waits on the receiver
    let gate_clone = Arc::clone(&gate);
    let handle = tokio::spawn(async move {
        // Close the gate after a short delay to unblock the receiver
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        gate_clone.close();
    });

    // rx.await should resolve with Err(RecvError) once the gate is closed
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), rx).await;
    assert!(result.is_ok(), "should not timeout — gate.close() should unblock");
    assert!(result.unwrap().is_err(), "should receive RecvError when sender is dropped");

    // Gate should be empty
    assert!(gate.list_pending().is_empty());
    handle.await.unwrap();
}

#[tokio::test]
async fn close_gate_unblocks_pending_approval() {
    let gate = Arc::new(UserInteractionGate::new());
    let rx = gate.create_request(
        "approve-block".to_string(),
        InteractionKind::ToolApproval {
            tool_id: "shell.execute".into(),
            input: r#"{"command": "ls"}"#.into(),
            reason: "needs approval".into(),
            inferred_scope: None,
        },
    );

    let gate_clone = Arc::clone(&gate);
    let handle = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        gate_clone.close();
    });

    let result = tokio::time::timeout(std::time::Duration::from_secs(2), rx).await;
    assert!(result.is_ok(), "should not timeout");
    assert!(result.unwrap().is_err(), "should receive RecvError");
    assert!(gate.list_pending().is_empty());
    handle.await.unwrap();
}

#[tokio::test]
async fn execute_tool_call_cancelled_by_token() {
    // A tool that sleeps forever — cancellation should interrupt it
    struct SlowTool {
        def: ToolDefinition,
    }
    impl Tool for SlowTool {
        fn definition(&self) -> &ToolDefinition {
            &self.def
        }
        fn execute(
            &self,
            _input: serde_json::Value,
        ) -> hive_tools::BoxFuture<'_, Result<hive_tools::ToolResult, hive_tools::ToolError>>
        {
            Box::pin(async {
                // Sleep indefinitely — only cancellation can stop us
                tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
                Ok(hive_tools::ToolResult {
                    output: json!({"done": true}),
                    data_class: DataClass::Internal,
                })
            })
        }
    }

    let mut registry = ToolRegistry::new();
    registry
        .register(Arc::new(SlowTool {
            def: ToolDefinition {
                id: "test.slow".to_string(),
                name: "slow_tool".to_string(),
                description: "a slow tool".to_string(),
                input_schema: json!({"type": "object"}),
                output_schema: None,
                channel_class: ChannelClass::Internal,
                side_effects: false,
                approval: ToolApproval::Auto,
                annotations: ToolAnnotations {
                    title: "slow".to_string(),
                    read_only_hint: Some(true),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(true),
                    open_world_hint: Some(false),
                },
            },
        }))
        .unwrap();

    let token = tokio_util::sync::CancellationToken::new();
    let context = LoopContext {
        conversation: ConversationContext {
            session_id: "cancel-test".to_string(),
            message_id: "msg-cancel".to_string(),
            prompt: "test".to_string(),
            prompt_content_parts: vec![],
            history: vec![],
            conversation_journal: None,
            initial_tool_iterations: 0,
        },
        routing: RoutingConfig {
            required_capabilities: BTreeSet::new(),
            preferred_models: None,
            loop_strategy: None,
            routing_decision: None,
        },
        security: SecurityContext {
            data_class: DataClass::Internal,
            permissions: Arc::new(parking_lot::Mutex::new(SessionPermissions::new())),
            workspace_classification: None,
            effective_data_class: Arc::new(AtomicU8::new(DataClass::Internal.to_i64() as u8)),
            connector_service: None,
            shadow_mode: false,
        },
        tools_ctx: ToolsContext {
            tools: Arc::new(registry),
            skill_catalog: None,
            knowledge_query_handler: None,
            tool_execution_mode: ToolExecutionMode::default(),
        },
        agent: AgentContext {
            persona: None,
            agent_orchestrator: None,
            personas: vec![],
            current_agent_id: None,
            parent_agent_id: None,
            workspace_path: None,
            keep_alive: false,
            session_messaged: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        },
        tool_limits: ToolLimitsConfig::default(),
        code_act_config: CodeActConfig::default(),
        session_registry: None,
        preempt_signal: None,
        cancellation_token: Some(token.clone()),
    };

    // Cancel after a short delay
    let token_clone = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        token_clone.cancel();
    });

    let result = execute_tool_call(
        &context,
        ToolCall { tool_id: "test.slow".to_string(), input: json!({}) },
        &[],
        None,
        None,
        None,
    )
    .await;

    assert!(result.is_err());
    match result.unwrap_err() {
        LoopError::Cancelled => {} // expected
        other => panic!("expected LoopError::Cancelled, got: {other:?}"),
    }
}

#[tokio::test]
async fn execute_tool_call_without_token_runs_normally() {
    // Verify tool execution works normally when no cancellation token is set
    let context = LoopContext {
        conversation: ConversationContext {
            session_id: "no-cancel-test".to_string(),
            message_id: "msg-no-cancel".to_string(),
            prompt: "test".to_string(),
            prompt_content_parts: vec![],
            history: vec![],
            conversation_journal: None,
            initial_tool_iterations: 0,
        },
        routing: RoutingConfig {
            required_capabilities: BTreeSet::new(),
            preferred_models: None,
            loop_strategy: None,
            routing_decision: None,
        },
        security: SecurityContext {
            data_class: DataClass::Internal,
            permissions: Arc::new(parking_lot::Mutex::new(SessionPermissions::new())),
            workspace_classification: None,
            effective_data_class: Arc::new(AtomicU8::new(DataClass::Internal.to_i64() as u8)),
            connector_service: None,
            shadow_mode: false,
        },
        tools_ctx: ToolsContext {
            tools: Arc::new({
                let mut r = ToolRegistry::new();
                r.register(Arc::new(CalculatorTool::default())).unwrap();
                r
            }),
            skill_catalog: None,
            knowledge_query_handler: None,
            tool_execution_mode: ToolExecutionMode::default(),
        },
        agent: AgentContext {
            persona: None,
            agent_orchestrator: None,
            personas: vec![],
            current_agent_id: None,
            parent_agent_id: None,
            workspace_path: None,
            keep_alive: false,
            session_messaged: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        },
        tool_limits: ToolLimitsConfig::default(),
        code_act_config: CodeActConfig::default(),
        session_registry: None,
        preempt_signal: None,
        cancellation_token: None,
    };

    let result = execute_tool_call(
        &context,
        ToolCall { tool_id: "math.calculate".to_string(), input: json!({"expression": "2 + 2"}) },
        &[],
        None,
        None,
        None,
    )
    .await;

    assert!(result.is_ok(), "tool should execute normally without cancellation token");
}
