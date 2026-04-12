//! Full-stack integration tests for email-triggered workflows that invoke agents.
//!
//! Unlike shallow tests that mock at the `StepExecutor` boundary, these tests
//! wire up the **real** agent pipeline:
//!
//!   WorkflowEngine → FullStackExecutor → AgentSupervisor → AgentRunner
//!     → LoopExecutor → ScriptProvider (mock LLM) → MockTool (mock tools)
//!
//! Only the LLM calls and tool calls are mocked.  Everything in between —
//! routing, preferred_models resolution, context compaction thresholds,
//! permission propagation, template resolution — runs for real.

use std::collections::BTreeMap;
use std::sync::Arc;

use arc_swap::ArcSwap;
use parking_lot::Mutex as ParkingMutex;
use serde_json::{json, Value};

use hive_agents::types::{AgentMessage, AgentSpec, SupervisorEvent};
use hive_agents::AgentSupervisor;
use hive_classification::{ChannelClass, DataClass};
use hive_contracts::{
    PermissionRule, Persona, SessionPermissions, ToolAnnotations, ToolApproval, ToolDefinition,
    ToolExecutionMode,
};
use hive_loop::{LoopExecutor, ReActStrategy};
use hive_model::{
    Capability, CompletionChunk, CompletionRequest, CompletionResponse, CompletionStream,
    FinishReason, ModelProvider, ModelRouter, ModelSelection, ProviderDescriptor, ProviderKind,
    ToolCallResponse,
};
use hive_tools::{Tool, ToolError, ToolRegistry, ToolResult};
use hive_workflow::executor::{ExecutionContext, NullEventEmitter, StepExecutor, WorkflowEngine};
use hive_workflow::store::{WorkflowPersistence, WorkflowStore};
use hive_workflow::types::*;

// ===========================================================================
// ScriptProvider — queue-based mock ModelProvider
// ===========================================================================

struct ScriptProvider {
    responses: ParkingMutex<Vec<CompletionResponse>>,
    recorded_requests: Arc<ParkingMutex<Vec<CompletionRequest>>>,
    descriptor: ProviderDescriptor,
}

impl ScriptProvider {
    fn new(model: &str, responses: Vec<CompletionResponse>) -> Self {
        Self {
            responses: ParkingMutex::new(responses),
            recorded_requests: Arc::new(ParkingMutex::new(Vec::new())),
            descriptor: ProviderDescriptor {
                id: "test-provider".to_string(),
                name: Some("Test Provider".to_string()),
                kind: ProviderKind::Mock,
                models: vec![model.to_string()],
                model_capabilities: BTreeMap::from([(
                    model.to_string(),
                    [Capability::Chat, Capability::ToolUse].into_iter().collect(),
                )]),
                priority: 100,
                available: true,
            },
        }
    }

    fn recorder(&self) -> Arc<ParkingMutex<Vec<CompletionRequest>>> {
        Arc::clone(&self.recorded_requests)
    }

    fn text(content: &str) -> CompletionResponse {
        CompletionResponse {
            provider_id: "test-provider".to_string(),
            model: "test-model".to_string(),
            content: content.to_string(),
            tool_calls: vec![],
        }
    }

    fn tool_call(name: &str, args: Value) -> CompletionResponse {
        CompletionResponse {
            provider_id: "test-provider".to_string(),
            model: "test-model".to_string(),
            content: String::new(),
            tool_calls: vec![ToolCallResponse {
                id: format!("call-{name}"),
                name: name.to_string(),
                arguments: args,
            }],
        }
    }
}

impl ModelProvider for ScriptProvider {
    fn descriptor(&self) -> &ProviderDescriptor {
        &self.descriptor
    }

    fn complete(
        &self,
        request: &CompletionRequest,
        _selection: &ModelSelection,
    ) -> anyhow::Result<CompletionResponse> {
        self.recorded_requests.lock().push(request.clone());
        let mut responses = self.responses.lock();
        if responses.is_empty() {
            Ok(Self::text("done"))
        } else {
            Ok(responses.remove(0))
        }
    }

    fn complete_stream(
        &self,
        request: &CompletionRequest,
        selection: &ModelSelection,
    ) -> anyhow::Result<CompletionStream> {
        let response = self.complete(request, selection)?;
        let chunk = CompletionChunk {
            delta: response.content,
            finish_reason: Some(FinishReason::Stop),
            tool_calls: response.tool_calls,
        };
        Ok(Box::pin(tokio_stream::once(Ok(chunk))))
    }
}

// ===========================================================================
// MockTool — configurable tool mock that records inputs
// ===========================================================================

struct MockTool {
    definition: ToolDefinition,
    response: ParkingMutex<Option<Value>>,
    recorded_inputs: Arc<ParkingMutex<Vec<Value>>>,
}

impl MockTool {
    fn new(id: &str) -> Self {
        Self {
            definition: ToolDefinition {
                id: id.to_string(),
                name: id.to_string(),
                description: format!("Mock tool {id}"),
                input_schema: json!({"type": "object"}),
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
            response: ParkingMutex::new(None),
            recorded_inputs: Arc::new(ParkingMutex::new(Vec::new())),
        }
    }

    fn with_response(self, value: Value) -> Self {
        *self.response.lock() = Some(value);
        self
    }

    fn recorded_inputs(&self) -> Vec<Value> {
        self.recorded_inputs.lock().clone()
    }

    fn inputs_handle(&self) -> Arc<ParkingMutex<Vec<Value>>> {
        Arc::clone(&self.recorded_inputs)
    }
}

impl Tool for MockTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(
        &self,
        input: Value,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ToolResult, ToolError>> + Send + '_>,
    > {
        self.recorded_inputs.lock().push(input.clone());
        let response = self.response.lock().clone();
        Box::pin(async move {
            let output = response.unwrap_or(json!({"ok": true}));
            Ok(ToolResult { output, data_class: DataClass::Internal })
        })
    }
}

// ===========================================================================
// FullStackExecutor — bridges workflow steps to the real agent pipeline
// ===========================================================================

/// A `StepExecutor` that runs real agents through the full loop pipeline.
/// Only the LLM and tools are mocked.
struct FullStackExecutor {
    tools: Arc<ToolRegistry>,
    model_router: Arc<ArcSwap<ModelRouter>>,
    loop_executor: Arc<LoopExecutor>,
    /// LLM requests recorded by the ScriptProvider (for assertions).
    llm_recorder: Arc<ParkingMutex<Vec<CompletionRequest>>>,
}

impl FullStackExecutor {
    fn new(
        tools: Arc<ToolRegistry>,
        model_router: Arc<ArcSwap<ModelRouter>>,
        loop_executor: Arc<LoopExecutor>,
        llm_recorder: Arc<ParkingMutex<Vec<CompletionRequest>>>,
    ) -> Self {
        Self { tools, model_router, loop_executor, llm_recorder }
    }
}

#[async_trait::async_trait]
impl StepExecutor for FullStackExecutor {
    async fn call_tool(
        &self,
        tool_id: &str,
        arguments: Value,
        _ctx: &ExecutionContext,
    ) -> Result<Value, String> {
        let tool = self
            .tools
            .get(tool_id)
            .ok_or_else(|| format!("tool {tool_id} not found in registry"))?;
        let result = tool.execute(arguments).await.map_err(|e| format!("tool error: {e}"))?;
        Ok(result.output)
    }

    async fn invoke_agent(
        &self,
        _persona_id: &str,
        task: &str,
        _async_exec: bool,
        _timeout_secs: Option<u64>,
        _step_permissions: &[PermissionEntry],
        _agent_name: Option<&str>,
        _: Option<&str>,
        ctx: &ExecutionContext,
    ) -> Result<Value, String> {
        // Convert workflow permissions → agent permissions
        let mut session_perms = SessionPermissions::new();
        for pe in &ctx.permissions {
            session_perms.add_rule(PermissionRule {
                tool_pattern: pe.tool_id.clone(),
                scope: pe.resource.clone().unwrap_or_else(|| "*".to_string()),
                decision: match pe.approval {
                    ToolApprovalLevel::Auto => ToolApproval::Auto,
                    ToolApprovalLevel::Ask => ToolApproval::Ask,
                    ToolApprovalLevel::Deny => ToolApproval::Deny,
                },
            });
        }
        // Ensure all tools are auto-approved for tests
        if !session_perms.rules.iter().any(|r| r.tool_pattern == "*") {
            session_perms.add_rule(PermissionRule {
                tool_pattern: "*".to_string(),
                scope: "*".to_string(),
                decision: ToolApproval::Auto,
            });
        }

        let workspace = std::env::temp_dir().join("hive-test-agent-workspace");
        let _ = std::fs::create_dir_all(&workspace);

        // Build a real AgentSupervisor with the mock LLM + tools
        let supervisor = AgentSupervisor::with_executor(
            64,
            None,
            Arc::clone(&self.loop_executor),
            Arc::clone(&self.model_router),
            Arc::clone(&self.tools),
            Arc::new(ParkingMutex::new(session_perms)),
            Arc::new(ParkingMutex::new(vec![Persona::default_persona()])),
            None, // no agent orchestrator
            "wf-test-session".to_string(),
            workspace,
            None, // no skill catalog
            None, // no knowledge query handler
        );

        let spec = AgentSpec {
            id: format!("agent-{}", uuid::Uuid::new_v4().simple()),
            name: "wf-agent".to_string(),
            friendly_name: "Workflow Agent".to_string(),
            description: String::new(),
            role: Default::default(),
            model: Some("test-model".to_string()),
            preferred_models: Some(vec!["test-model".to_string()]),
            loop_strategy: None,
            tool_execution_mode: Some(ToolExecutionMode::default()),
            system_prompt: String::new(),
            allowed_tools: vec!["*".to_string()],
            avatar: None,
            color: None,
            data_class: DataClass::Internal,
            keep_alive: false,
            idle_timeout_secs: None,
            tool_limits: None,
            persona_id: None,
            workflow_managed: false,
        };

        let agent_id = spec.id.clone();
        let mut rx = supervisor.subscribe();

        supervisor
            .spawn_agent(spec, None, None, None, None)
            .await
            .map_err(|e| format!("spawn failed: {e}"))?;

        supervisor
            .send_to_agent(&agent_id, AgentMessage::Task { content: task.to_string(), from: None })
            .await
            .map_err(|e| format!("send task failed: {e}"))?;

        // Wait for agent completion
        let timeout = tokio::time::Duration::from_secs(10);
        let deadline = tokio::time::Instant::now() + timeout;

        loop {
            match tokio::time::timeout_at(deadline, rx.recv()).await {
                Ok(Ok(SupervisorEvent::AgentCompleted { agent_id: ref cid, ref result }))
                    if *cid == agent_id =>
                {
                    return Ok(json!({
                        "result": result,
                        "status": "completed",
                        "agent_id": agent_id,
                    }));
                }
                Ok(Ok(_)) => continue,
                Ok(Err(_)) => return Err("supervisor channel closed".to_string()),
                Err(_) => return Err(format!("agent timed out after {timeout:?}")),
            }
        }
    }

    async fn signal_agent(
        &self,
        _target: &SignalTarget,
        _content: &str,
        _ctx: &ExecutionContext,
    ) -> Result<Value, String> {
        Ok(json!({ "sent": true }))
    }

    async fn wait_for_agent(
        &self,
        _: &str,
        _: Option<u64>,
        _: &ExecutionContext,
    ) -> Result<Value, String> {
        Ok(Value::Null)
    }

    async fn create_feedback_request(
        &self,
        _instance_id: i64,
        step_id: &str,
        _prompt: &str,
        _choices: Option<&[String]>,
        _allow_freeform: bool,
        _ctx: &ExecutionContext,
    ) -> Result<String, String> {
        Ok(format!("feedback-{step_id}"))
    }

    async fn register_event_gate(
        &self,
        _instance_id: i64,
        _step_id: &str,
        _topic: &str,
        _filter: Option<&str>,
        _timeout_secs: Option<u64>,
        _ctx: &ExecutionContext,
    ) -> Result<String, String> {
        Ok("event-gate-sub-id".to_string())
    }

    async fn launch_workflow(
        &self,
        _workflow_name: &str,
        _inputs: Value,
        _ctx: &ExecutionContext,
    ) -> Result<i64, String> {
        Ok(9999)
    }

    async fn schedule_task(
        &self,
        schedule: &ScheduleTaskDef,
        _ctx: &ExecutionContext,
    ) -> Result<String, String> {
        Ok(format!("task-{}", schedule.name))
    }

    async fn render_prompt_template(
        &self,
        _persona_id: &str,
        _prompt_id: &str,
        _parameters: Value,
        _ctx: &ExecutionContext,
    ) -> Result<String, String> {
        Err("render_prompt_template not implemented in test executor".to_string())
    }
}

// ===========================================================================
// Helpers
// ===========================================================================

struct TestHarness {
    engine: Arc<WorkflowEngine>,
    store: Arc<WorkflowStore>,
    llm_recorder: Arc<ParkingMutex<Vec<CompletionRequest>>>,
    comm_send_inputs: Arc<ParkingMutex<Vec<Value>>>,
}

/// Build a full-stack test harness with the given LLM script and tools.
fn make_harness(llm_responses: Vec<CompletionResponse>) -> TestHarness {
    let provider = ScriptProvider::new("test-model", llm_responses);
    let llm_recorder = provider.recorder();

    let mut router = ModelRouter::new();
    router.register_provider(provider);
    let model_router = Arc::new(ArcSwap::from_pointee(router));

    let loop_executor = Arc::new(LoopExecutor::new(Arc::new(ReActStrategy)));

    // Register mock tools
    let mut tools = ToolRegistry::new();

    let comm_send = MockTool::new("comm.send_external_message").with_response(json!({
        "message_id": "sent-msg-456",
        "status": "sent",
        "data_class": "public"
    }));
    let comm_send_inputs = comm_send.inputs_handle();
    let comm_send = Arc::new(comm_send);
    tools.register(comm_send as Arc<dyn Tool>).unwrap();

    // core.ask_user placeholder (intercepted by the loop, never executed)
    tools.register(Arc::new(MockTool::new("core.ask_user")) as Arc<dyn Tool>).unwrap();

    let tools = Arc::new(tools);

    let exec = Arc::new(FullStackExecutor::new(
        Arc::clone(&tools),
        Arc::clone(&model_router),
        loop_executor,
        Arc::clone(&llm_recorder),
    ));

    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let emitter = Arc::new(NullEventEmitter);
    let engine = Arc::new(WorkflowEngine::new(store.clone(), exec, emitter));

    TestHarness { engine, store, llm_recorder, comm_send_inputs }
}

async fn save_and_launch(
    engine: &WorkflowEngine,
    store: &WorkflowStore,
    yaml: &str,
    inputs: Value,
) -> i64 {
    let def: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();
    store.save_definition(yaml, &def).unwrap();
    engine.launch(def, inputs, "test-session".into(), None, vec![], None).await.unwrap()
}

async fn save_and_launch_with_permissions(
    engine: &WorkflowEngine,
    store: &WorkflowStore,
    yaml: &str,
    inputs: Value,
    permissions: Vec<PermissionEntry>,
) -> i64 {
    let def: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();
    store.save_definition(yaml, &def).unwrap();
    engine.launch(def, inputs, "test-session".into(), None, permissions, None).await.unwrap()
}

fn email_event_payload() -> Value {
    json!({
        "connector_id": "conn-microsoft-001",
        "from": "alice@example.com",
        "to": "support@ourcompany.com",
        "subject": "Help with billing issue",
        "body": "Hi, I was charged twice for my subscription last month. Order #12345. Please refund the duplicate charge. Thanks, Alice",
        "message_id": "msg-abc-123",
        "metadata": {
            "channel_id": "inbox",
            "thread_id": "thread-99",
            "timestamp": "2026-03-22T10:30:00Z"
        }
    })
}

async fn tick() {
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
}

// ===========================================================================
// Test 1: Email trigger → invoke agent (real loop) that calls comm.send_external_message
//
// The workflow receives an email, resolves template expressions into the
// agent's task, then the agent runs through the REAL LoopExecutor with a
// scripted LLM that emits a comm.send_external_message tool call.  We verify:
//   - The LLM prompt contains the resolved email fields
//   - The comm.send_external_message MockTool received the correct arguments
//   - The workflow completed successfully
// ===========================================================================

#[tokio::test]
async fn test_email_trigger_agent_calls_comm_send_via_real_loop() {
    // Script the mock LLM:
    //   1. First call → tool_call to comm.send_external_message
    //   2. Second call → text "reply sent" (agent completes)
    let harness = make_harness(vec![
        ScriptProvider::tool_call(
            "comm.send_external_message",
            json!({
                "connector_id": "conn-microsoft-001",
                "to": "alice@example.com",
                "subject": "Re: Help with billing issue",
                "body": "Dear Alice, we have processed your refund."
            }),
        ),
        ScriptProvider::text("I have sent the reply to Alice."),
    ]);

    let yaml = r#"
name: email-auto-reply
version: "1.0"
steps:
  - id: receive
    type: trigger
    trigger:
      type: manual
      inputs:
      - name: connector_id
        type: string
      - name: from
        type: string
      - name: subject
        type: string
      - name: body
        type: string
    outputs:
      connector_id: "{{trigger.connector_id}}"
      sender: "{{trigger.from}}"
      subject: "{{trigger.subject}}"
      body: "{{trigger.body}}"
    next: [reply_agent]

  - id: reply_agent
    type: task
    task:
      kind: invoke_agent
      persona_id: email-responder
      task: "You received an email from {{steps.receive.outputs.sender}} about '{{steps.receive.outputs.subject}}'. Body: {{steps.receive.outputs.body}}. Reply using comm.send_external_message with connector_id={{steps.receive.outputs.connector_id}} to={{steps.receive.outputs.sender}}."
      async: false
      timeout_secs: 120
    outputs:
      agent_result: "{{result.result}}"
    next: [done]

  - id: done
    type: control_flow
    control:
      kind: end_workflow
output:
  agent_result: "{{steps.reply_agent.outputs.agent_result}}"
"#;

    let id = save_and_launch(&harness.engine, &harness.store, yaml, email_event_payload()).await;
    tick().await;

    // Verify workflow completed
    let inst = harness.store.get_instance(id).unwrap().unwrap();
    assert_eq!(
        inst.status,
        WorkflowStatus::Completed,
        "workflow should complete, got {:?}: {:?}",
        inst.status,
        inst.error
    );

    // Verify the LLM received a prompt with all email fields resolved
    let requests = harness.llm_recorder.lock();
    assert!(!requests.is_empty(), "LLM should have received at least one request");
    let first_prompt = &requests[0].prompt;
    assert!(
        first_prompt.contains("alice@example.com"),
        "LLM prompt should contain sender: {first_prompt}"
    );
    assert!(
        first_prompt.contains("Help with billing issue"),
        "LLM prompt should contain subject: {first_prompt}"
    );
    assert!(
        first_prompt.contains("charged twice"),
        "LLM prompt should contain body: {first_prompt}"
    );
    assert!(
        first_prompt.contains("conn-microsoft-001"),
        "LLM prompt should contain connector_id: {first_prompt}"
    );
    drop(requests);

    // Verify the mock comm.send_external_message tool received the correct args
    let inputs = harness.comm_send_inputs.lock();
    assert_eq!(inputs.len(), 1, "comm.send_external_message should be called once");
    assert_eq!(inputs[0]["connector_id"], "conn-microsoft-001");
    assert_eq!(inputs[0]["to"], "alice@example.com");
    assert_eq!(inputs[0]["subject"], "Re: Help with billing issue");
    assert!(inputs[0]["body"].as_str().unwrap().contains("refund"));
}

// ===========================================================================
// Test 2: Agent reply via comm tool as a workflow call_tool step
//
// The agent drafts a reply (LLM returns text), then the WORKFLOW itself
// calls comm.send_external_message as a separate call_tool step using the agent's
// output.  This tests the full data flow: trigger → agent (real loop) →
// output captured → template resolution → tool execution.
// ===========================================================================

#[tokio::test]
async fn test_email_workflow_agent_then_workflow_tool_step() {
    // Script: agent just returns text (no tool calls)
    let harness = make_harness(vec![ScriptProvider::text(
        "Dear Alice, we have processed your refund for Order #12345.",
    )]);

    let yaml = r#"
name: email-agent-then-tool
version: "1.0"
steps:
  - id: receive
    type: trigger
    trigger:
      type: manual
      inputs:
      - name: connector_id
        type: string
      - name: from
        type: string
      - name: subject
        type: string
      - name: body
        type: string
    outputs:
      connector_id: "{{trigger.connector_id}}"
      sender: "{{trigger.from}}"
      subject: "{{trigger.subject}}"
      body: "{{trigger.body}}"
    next: [draft_reply]

  - id: draft_reply
    type: task
    task:
      kind: invoke_agent
      persona_id: support-agent
      task: "Draft a professional reply to this email from {{steps.receive.outputs.sender}} about {{steps.receive.outputs.subject}}: {{steps.receive.outputs.body}}"
      async: false
    outputs:
      reply_text: "{{result.result}}"
    next: [send_reply]

  - id: send_reply
    type: task
    task:
      kind: call_tool
      tool_id: comm.send_external_message
      arguments:
        connector_id: "{{steps.receive.outputs.connector_id}}"
        to: "{{steps.receive.outputs.sender}}"
        subject: "Re: {{steps.receive.outputs.subject}}"
        body: "{{steps.draft_reply.outputs.reply_text}}"
    outputs:
      send_status: "{{result.status}}"
      sent_message_id: "{{result.message_id}}"
    next: [done]

  - id: done
    type: control_flow
    control:
      kind: end_workflow
output:
  reply_text: "{{steps.draft_reply.outputs.reply_text}}"
  send_status: "{{steps.send_reply.outputs.send_status}}"
  sent_message_id: "{{steps.send_reply.outputs.sent_message_id}}"
"#;

    let id = save_and_launch(&harness.engine, &harness.store, yaml, email_event_payload()).await;
    tick().await;

    let inst = harness.store.get_instance(id).unwrap().unwrap();
    assert_eq!(
        inst.status,
        WorkflowStatus::Completed,
        "workflow should complete, got {:?}: {:?}",
        inst.status,
        inst.error
    );

    // Verify the workflow's call_tool step invoked comm.send_external_message
    let inputs = harness.comm_send_inputs.lock();
    assert_eq!(inputs.len(), 1, "comm.send_external_message should be called once");
    assert_eq!(inputs[0]["connector_id"], "conn-microsoft-001");
    assert_eq!(inputs[0]["to"], "alice@example.com");
    assert_eq!(inputs[0]["subject"], "Re: Help with billing issue");
    // The body should be the agent's output (the LLM text)
    assert!(
        inputs[0]["body"].as_str().unwrap().contains("refund for Order #12345"),
        "body should contain agent's drafted reply: {:?}",
        inputs[0]["body"]
    );
    drop(inputs);

    // Verify workflow output captured both agent and tool results
    let output = inst.output.unwrap();
    assert_eq!(output["send_status"], "sent");
    assert_eq!(output["sent_message_id"], "sent-msg-456");
    assert!(output["reply_text"].as_str().unwrap().contains("refund for Order #12345"));
}

// ===========================================================================
// Test 3: Email trigger → feedback gate → agent → comm send
//
// The workflow pauses at a feedback gate before invoking the agent.
// After approval, the agent runs through the real loop and calls the tool.
// ===========================================================================

#[tokio::test]
async fn test_email_workflow_with_feedback_gate_before_agent() {
    // Script for the agent AFTER feedback approval:
    //   1. tool_call to comm.send_external_message
    //   2. text completion
    let harness = make_harness(vec![
        ScriptProvider::tool_call(
            "comm.send_external_message",
            json!({
                "connector_id": "conn-microsoft-001",
                "to": "alice@example.com",
                "body": "Your refund has been processed."
            }),
        ),
        ScriptProvider::text("Reply sent successfully."),
    ]);

    let yaml = r#"
name: email-with-confirmation
version: "1.0"
steps:
  - id: receive
    type: trigger
    trigger:
      type: manual
      inputs:
      - name: connector_id
        type: string
      - name: from
        type: string
      - name: subject
        type: string
      - name: body
        type: string
    outputs:
      connector_id: "{{trigger.connector_id}}"
      sender: "{{trigger.from}}"
      subject: "{{trigger.subject}}"
      body: "{{trigger.body}}"
    next: [confirm]

  - id: confirm
    type: task
    task:
      kind: feedback_gate
      prompt: "New email from {{steps.receive.outputs.sender}} about '{{steps.receive.outputs.subject}}'. Should we auto-reply?"
      choices:
        - "Yes, reply"
        - "No, ignore"
      allow_freeform: false
    outputs:
      decision: "{{result.response}}"
    next: [check_decision]

  - id: check_decision
    type: control_flow
    control:
      kind: branch
      condition: "{{steps.confirm.outputs.decision}}"
      then: [reply_agent]
      else: [skip_reply]

  - id: reply_agent
    type: task
    task:
      kind: invoke_agent
      persona_id: support-agent
      task: "Reply to email from {{steps.receive.outputs.sender}}: {{steps.receive.outputs.body}}. Use comm.send_external_message with connector_id={{steps.receive.outputs.connector_id}}."
      async: false
    outputs:
      reply_result: "{{result.result}}"
    next: [done]

  - id: skip_reply
    type: control_flow
    control:
      kind: end_workflow

  - id: done
    type: control_flow
    control:
      kind: end_workflow
output:
  reply_result: "{{steps.reply_agent.outputs.reply_result}}"
"#;

    let id = save_and_launch(&harness.engine, &harness.store, yaml, email_event_payload()).await;
    tick().await;

    // Workflow should be paused waiting for feedback
    let inst = harness.store.get_instance(id).unwrap().unwrap();
    assert!(
        matches!(inst.status, WorkflowStatus::WaitingOnInput | WorkflowStatus::Running),
        "workflow should be waiting for feedback, got {:?}",
        inst.status
    );

    // No agent should have run yet — no LLM calls
    assert!(
        harness.llm_recorder.lock().is_empty(),
        "LLM should NOT have been called before feedback approval"
    );
    assert!(
        harness.comm_send_inputs.lock().is_empty(),
        "comm.send_external_message should NOT have been called before approval"
    );

    // Approve
    harness
        .engine
        .respond_to_gate(id, "confirm", json!({ "response": "Yes, reply" }))
        .await
        .unwrap();
    tick().await;

    // Now the workflow should complete
    let inst = harness.store.get_instance(id).unwrap().unwrap();
    assert_eq!(
        inst.status,
        WorkflowStatus::Completed,
        "workflow should complete after approval, got {:?}: {:?}",
        inst.status,
        inst.error
    );

    // LLM should have been called (agent ran)
    let requests = harness.llm_recorder.lock();
    assert!(!requests.is_empty(), "LLM should have been called after approval");
    assert!(requests[0].prompt.contains("alice@example.com"), "agent prompt should contain sender");
    drop(requests);

    // comm.send_external_message should have been called by the agent
    let inputs = harness.comm_send_inputs.lock();
    assert_eq!(inputs.len(), 1, "comm.send_external_message should be called once");
    assert_eq!(inputs[0]["to"], "alice@example.com");
}

// ===========================================================================
// Test 4: Workflow-level permissions propagate through real agent pipeline
//
// The workflow has permissions defined in YAML + caller-provided permissions.
// The agent runs through the real loop.  We verify both merge correctly
// on the workflow instance.
// ===========================================================================

#[tokio::test]
async fn test_email_workflow_permissions_propagate_to_real_agent() {
    let harness = make_harness(vec![ScriptProvider::text("Noted.  I will process the refund.")]);

    let yaml = r#"
name: email-with-permissions
version: "1.0"
permissions:
  - tool_id: "comm.send_external_message"
    approval: auto
  - tool_id: "filesystem.*"
    resource: "/tmp/*"
    approval: deny
steps:
  - id: receive
    type: trigger
    trigger:
      type: manual
      inputs:
      - name: connector_id
        type: string
      - name: from
        type: string
      - name: subject
        type: string
      - name: body
        type: string
    outputs:
      connector_id: "{{trigger.connector_id}}"
      sender: "{{trigger.from}}"
      subject: "{{trigger.subject}}"
      body: "{{trigger.body}}"
    next: [reply_agent]

  - id: reply_agent
    type: task
    task:
      kind: invoke_agent
      persona_id: email-responder
      task: "Reply to {{steps.receive.outputs.sender}} about {{steps.receive.outputs.subject}}"
      async: false
    outputs:
      reply: "{{result.result}}"
    next: [done]

  - id: done
    type: control_flow
    control:
      kind: end_workflow
output:
  reply: "{{steps.reply_agent.outputs.reply}}"
"#;

    let caller_permissions = vec![PermissionEntry {
        tool_id: "comm.read_messages".to_string(),
        resource: Some("*".to_string()),
        approval: ToolApprovalLevel::Auto,
    }];

    let id = save_and_launch_with_permissions(
        &harness.engine,
        &harness.store,
        yaml,
        email_event_payload(),
        caller_permissions,
    )
    .await;
    tick().await;

    let inst = harness.store.get_instance(id).unwrap().unwrap();
    assert_eq!(
        inst.status,
        WorkflowStatus::Completed,
        "workflow should complete, got {:?}: {:?}",
        inst.status,
        inst.error
    );

    // Verify merged permissions on the instance
    let perms = &inst.permissions;
    assert!(
        perms.len() >= 3,
        "expected at least 3 permissions (1 caller + 2 definition), got {}",
        perms.len()
    );
    // Caller permission first (higher priority)
    assert_eq!(perms[0].tool_id, "comm.read_messages");
    assert_eq!(perms[0].approval, ToolApprovalLevel::Auto);
    // Definition permissions
    assert_eq!(perms[1].tool_id, "comm.send_external_message");
    assert_eq!(perms[1].approval, ToolApprovalLevel::Auto);
    assert_eq!(perms[2].tool_id, "filesystem.*");
    assert_eq!(perms[2].resource.as_deref(), Some("/tmp/*"));
    assert_eq!(perms[2].approval, ToolApprovalLevel::Deny);

    // Verify the agent actually ran (LLM was called)
    let requests = harness.llm_recorder.lock();
    assert!(!requests.is_empty(), "LLM should have been called");
    assert!(requests[0].prompt.contains("alice@example.com"));

    // Verify workflow output
    let output = inst.output.unwrap();
    assert!(output["reply"].as_str().unwrap().contains("refund"));
}

// ===========================================================================
// Test 5: Email trigger → parallel agents (both run real loops) → merge
//
// Two agents run in parallel through the real LoopExecutor.  Each produces
// output via the scripted LLM.  The workflow merges their outputs and calls
// comm.send_external_message as a tool step.
// ===========================================================================

#[tokio::test]
async fn test_email_workflow_parallel_agents_merge_into_reply() {
    // We need enough scripted LLM responses for BOTH agents.
    // Agent 1 (sentiment): returns text
    // Agent 2 (drafter): returns text
    // Then the workflow's call_tool step runs comm.send_external_message.
    let harness = make_harness(vec![
        // Agent 1's response
        ScriptProvider::text("sentiment=frustrated, priority=high"),
        // Agent 2's response
        ScriptProvider::text("Dear Alice, your refund is being processed."),
    ]);

    let yaml = r#"
name: email-parallel-agents
version: "1.0"
steps:
  - id: receive
    type: trigger
    trigger:
      type: manual
      inputs:
      - name: connector_id
        type: string
      - name: from
        type: string
      - name: subject
        type: string
      - name: body
        type: string
    outputs:
      connector_id: "{{trigger.connector_id}}"
      sender: "{{trigger.from}}"
      subject: "{{trigger.subject}}"
      body: "{{trigger.body}}"
    next: [analyze_sentiment, draft_reply]

  - id: analyze_sentiment
    type: task
    task:
      kind: invoke_agent
      persona_id: sentiment-analyzer
      task: "Analyze sentiment of email from {{steps.receive.outputs.sender}}: {{steps.receive.outputs.body}}"
      async: false
    outputs:
      analysis: "{{result.result}}"
    next: [compose_final]

  - id: draft_reply
    type: task
    task:
      kind: invoke_agent
      persona_id: reply-drafter
      task: "Draft a reply to {{steps.receive.outputs.sender}} about {{steps.receive.outputs.subject}}: {{steps.receive.outputs.body}}"
      async: false
    outputs:
      draft: "{{result.result}}"
    next: [compose_final]

  - id: compose_final
    type: task
    task:
      kind: call_tool
      tool_id: comm.send_external_message
      arguments:
        connector_id: "{{steps.receive.outputs.connector_id}}"
        to: "{{steps.receive.outputs.sender}}"
        subject: "Re: {{steps.receive.outputs.subject}}"
        body: "[{{steps.analyze_sentiment.outputs.analysis}}] {{steps.draft_reply.outputs.draft}}"
    outputs:
      send_result: "{{result.status}}"
    next: [done]

  - id: done
    type: control_flow
    control:
      kind: end_workflow
output:
  analysis: "{{steps.analyze_sentiment.outputs.analysis}}"
  draft: "{{steps.draft_reply.outputs.draft}}"
  send_result: "{{steps.compose_final.outputs.send_result}}"
"#;

    let id = save_and_launch(&harness.engine, &harness.store, yaml, email_event_payload()).await;
    tick().await;

    let inst = harness.store.get_instance(id).unwrap().unwrap();
    assert_eq!(
        inst.status,
        WorkflowStatus::Completed,
        "workflow should complete, got {:?}: {:?}",
        inst.status,
        inst.error
    );

    // Both agents should have triggered LLM calls
    let requests = harness.llm_recorder.lock();
    assert_eq!(requests.len(), 2, "two LLM calls expected (one per agent), got {}", requests.len());
    drop(requests);

    // comm.send_external_message should have been called with merged outputs
    let inputs = harness.comm_send_inputs.lock();
    assert_eq!(inputs.len(), 1, "comm.send_external_message called once");
    assert_eq!(inputs[0]["connector_id"], "conn-microsoft-001");
    assert_eq!(inputs[0]["to"], "alice@example.com");
    assert_eq!(inputs[0]["subject"], "Re: Help with billing issue");

    // Body should contain outputs from BOTH agents
    let body = inputs[0]["body"].as_str().unwrap();
    assert!(
        body.contains("frustrated") || body.contains("sentiment"),
        "body should contain sentiment analysis: {body}"
    );
    assert!(body.contains("refund"), "body should contain draft reply: {body}");
    drop(inputs);

    // Verify workflow output
    let output = inst.output.unwrap();
    assert_eq!(output["send_result"], "sent");
    assert!(output["analysis"].as_str().unwrap().contains("frustrated"));
    assert!(output["draft"].as_str().unwrap().contains("refund"));
}

// ===========================================================================
// Test: Incoming email event → TriggerManager → workflow launch → invoke_agent
//       → agent calls tool
//
// This is the full end-to-end flow:
//   1. EventBus receives a `comm.message.received.*` event (simulating an
//      incoming email from a connector)
//   2. TriggerManager matches it to an `incoming_message` trigger
//   3. Workflow instance is auto-launched with email payload as inputs
//   4. The workflow's `invoke_agent` step runs a real agent (via LoopExecutor)
//   5. The agent calls `comm.send_external_message` (mocked tool)
//   6. We assert the tool received the right arguments and the workflow completed
// ===========================================================================

#[tokio::test]
async fn test_incoming_email_triggers_workflow_agent_calls_tool() {
    use hive_core::EventBus;
    use hive_workflow_service::{TriggerManager, WorkflowService};

    // --- Build the full-stack executor (same as other tests) ---
    let provider = ScriptProvider::new(
        "test-model",
        vec![
            ScriptProvider::tool_call(
                "comm.send_external_message",
                json!({
                    "connector_id": "test-connector",
                    "to": "sender@example.com",
                    "subject": "Re: Need help",
                    "body": "We are looking into your request."
                }),
            ),
            ScriptProvider::text("I have sent the reply."),
        ],
    );
    let llm_recorder = provider.recorder();

    let mut router = ModelRouter::new();
    router.register_provider(provider);
    let model_router = Arc::new(ArcSwap::from_pointee(router));
    let loop_executor = Arc::new(LoopExecutor::new(Arc::new(ReActStrategy)));

    let mut tools = ToolRegistry::new();
    let comm_send = MockTool::new("comm.send_external_message").with_response(json!({
        "message_id": "sent-msg-789",
        "status": "sent",
        "data_class": "public"
    }));
    let comm_send_inputs = comm_send.inputs_handle();
    tools.register(Arc::new(comm_send) as Arc<dyn Tool>).unwrap();
    tools.register(Arc::new(MockTool::new("core.ask_user")) as Arc<dyn Tool>).unwrap();
    let tools = Arc::new(tools);

    let exec = Arc::new(FullStackExecutor::new(
        Arc::clone(&tools),
        Arc::clone(&model_router),
        loop_executor,
        Arc::clone(&llm_recorder),
    ));

    // --- Build WorkflowService backed by the FullStackExecutor ---
    let store: Arc<dyn WorkflowPersistence> = Arc::new(WorkflowStore::in_memory().unwrap());
    let event_bus = EventBus::new(1024);

    let svc = Arc::new(WorkflowService::with_deps(
        Arc::clone(&store),
        exec as Arc<dyn StepExecutor>,
        Arc::new(NullEventEmitter),
    ));

    // --- Build TriggerManager and wire it up ---
    let tm = Arc::new(TriggerManager::new(event_bus.clone(), Arc::clone(&store)));
    tm.set_workflow_service(Arc::clone(&svc)).await;
    tm.start().await;

    // --- Save a workflow definition with an incoming_message trigger ---
    let yaml = r#"
name: email-triggered-reply
version: "1.0"
steps:
  - id: receive
    type: trigger
    trigger:
      type: incoming_message
      channel_id: test-connector
    outputs:
      connector_id: "{{trigger.channel_id}}"
      sender: "{{trigger.from}}"
      subject: "{{trigger.subject}}"
      body: "{{trigger.body}}"
    next: [reply_agent]

  - id: reply_agent
    type: task
    task:
      kind: invoke_agent
      persona_id: email-responder
      task: "Reply to email from {{steps.receive.outputs.sender}} about '{{steps.receive.outputs.subject}}'. Body: {{steps.receive.outputs.body}}. Use comm.send_external_message with connector_id={{steps.receive.outputs.connector_id}} to={{steps.receive.outputs.sender}}."
      async: false
      timeout_secs: 30
    next: [done]

  - id: done
    type: control_flow
    control:
      kind: end_workflow
"#;

    let def: WorkflowDefinition = serde_yaml::from_str(yaml).unwrap();
    store.save_definition(yaml, &def).unwrap();
    tm.register_definition(&def).await;

    // --- Simulate an incoming email via EventBus ---
    let email_payload = json!({
        "channel_id": "test-connector",
        "provider": "microsoft",
        "external_id": "email-unique-001",
        "from": "sender@example.com",
        "to": "us@company.com",
        "subject": "Need help",
        "body": "I need assistance with my account. Customer ID: C-5678.",
        "timestamp_ms": 1711000000000u64,
        "metadata": {
            "channel_id": "inbox",
            "thread_id": "thread-42"
        }
    });

    let _ =
        event_bus.publish("comm.message.received.test-connector", "connector-poll", email_payload);

    // --- Wait for the workflow to complete ---
    // The trigger manager processes events asynchronously.
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(15);
    let mut instance_id = None;
    loop {
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

        // Check if any instance was created for our definition
        let filter = InstanceFilter {
            definition_names: vec!["email-triggered-reply".to_string()],
            ..Default::default()
        };
        let result = store.list_instances(&filter).unwrap();
        let instances = &result.items;
        if let Some(inst) = instances.first() {
            if inst.status == WorkflowStatus::Completed || inst.status == WorkflowStatus::Failed {
                instance_id = Some(inst.id.clone());
                break;
            }
        }

        if tokio::time::Instant::now() >= deadline {
            let statuses: Vec<_> =
                instances.iter().map(|i| format!("{}: {:?}", i.id, i.status)).collect();
            panic!("Timed out waiting for workflow to complete. Instances: {statuses:?}");
        }
    }

    let id = instance_id.unwrap();
    let inst = store.get_instance(id).unwrap().unwrap();

    // --- Assert: workflow completed successfully ---
    assert_eq!(
        inst.status,
        WorkflowStatus::Completed,
        "workflow should complete, got {:?}: {:?}",
        inst.status,
        inst.error
    );

    // --- Assert: LLM received prompt with email content ---
    let requests = llm_recorder.lock();
    assert!(!requests.is_empty(), "LLM should have received at least one request");
    let first_prompt = &requests[0].prompt;
    assert!(
        first_prompt.contains("sender@example.com"),
        "prompt should contain sender: {first_prompt}"
    );
    assert!(first_prompt.contains("Need help"), "prompt should contain subject: {first_prompt}");
    assert!(first_prompt.contains("C-5678"), "prompt should contain body content: {first_prompt}");
    drop(requests);

    // --- Assert: comm.send_external_message tool was called ---
    let inputs = comm_send_inputs.lock();
    assert_eq!(inputs.len(), 1, "comm.send_external_message should be called once");
    assert_eq!(inputs[0]["connector_id"], "test-connector");
    assert_eq!(inputs[0]["to"], "sender@example.com");

    // Clean up
    tm.stop().await;
}
