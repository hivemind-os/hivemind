use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use hive_contracts::tools::ToolDefinition;

use crate::analyzer::{classify_tool, generate_from_schema, RiskLevel};
use crate::executor::{ExecutionContext, StepExecutor};
use crate::store::WorkflowPersistence;
use crate::types::{InterceptedAction, PermissionEntry, ScheduleTaskDef, SignalTarget};

// ---------------------------------------------------------------------------
// ToolInfoProvider — bridge to the tool registry
// ---------------------------------------------------------------------------

/// Provides tool metadata needed for shadow-mode risk classification.
///
/// Implementations live in the service layer (which owns the actual tool
/// registry) and are injected into the `ShadowStepExecutor`.
pub trait ToolInfoProvider: Send + Sync {
    fn get_tool_definition(&self, tool_id: &str) -> Option<ToolDefinition>;
}

/// Fallback provider that returns `None` for every tool, causing shadow mode
/// to treat all tools as unknown and intercept them (fail-closed).
pub struct NullToolInfoProvider;

impl ToolInfoProvider for NullToolInfoProvider {
    fn get_tool_definition(&self, _tool_id: &str) -> Option<ToolDefinition> {
        None
    }
}

// ---------------------------------------------------------------------------
// ShadowStepExecutor
// ---------------------------------------------------------------------------

/// Decorator around a real `StepExecutor` that intercepts side-effecting
/// operations in shadow mode.
///
/// - **Safe** tools (read-only) are passed through to the real executor so
///   that the workflow can branch on real data.
/// - **Caution/Danger/Unknown** tools are intercepted: a synthetic output is
///   returned and the action is persisted to the intercepted-actions table.
/// - Agent invocations, signals, child workflow launches, and scheduled tasks
///   are always intercepted.
/// - Feedback gates and prompt template rendering pass through unchanged.
/// - Event gates **fail immediately** in shadow mode because the engine state
///   machine requires a real subscription to resolve the waiting state.
pub struct ShadowStepExecutor {
    inner: Arc<dyn StepExecutor>,
    tool_info: Arc<dyn ToolInfoProvider>,
    store: Arc<dyn WorkflowPersistence>,
}

impl ShadowStepExecutor {
    pub fn new(
        inner: Arc<dyn StepExecutor>,
        tool_info: Arc<dyn ToolInfoProvider>,
        store: Arc<dyn WorkflowPersistence>,
    ) -> Self {
        Self {
            inner,
            tool_info,
            store,
        }
    }

    /// Persist an intercepted action to the store.
    fn record_interception(
        &self,
        instance_id: i64,
        step_id: &str,
        kind: &str,
        details: Value,
    ) -> Result<(), String> {
        let action = InterceptedAction {
            id: 0, // assigned by the store
            instance_id,
            step_id: step_id.to_string(),
            kind: kind.to_string(),
            timestamp_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            details,
        };
        self.store
            .save_intercepted_action(&action)
            .map_err(|e| format!("Failed to save intercepted action: {e}"))?;
        Ok(())
    }

    /// Generate a synthetic tool output for an intercepted call.
    fn synthetic_tool_output(&self, tool_id: &str, tool_def: Option<&ToolDefinition>) -> Value {
        if let Some(def) = tool_def {
            if let Some(schema) = &def.output_schema {
                return generate_from_schema(schema);
            }
        }
        json!({
            "shadow": true,
            "tool": tool_id,
            "message": format!("Tool '{}' was intercepted — no real action taken", tool_id)
        })
    }
}

#[async_trait]
impl StepExecutor for ShadowStepExecutor {
    async fn call_tool(
        &self,
        tool_id: &str,
        arguments: Value,
        ctx: &ExecutionContext,
    ) -> Result<Value, String> {
        let tool_def = self.tool_info.get_tool_definition(tool_id);
        let risk = tool_def
            .as_ref()
            .map(classify_tool)
            .unwrap_or(RiskLevel::Unknown);

        if !risk.should_intercept() {
            return self.inner.call_tool(tool_id, arguments, ctx).await;
        }

        self.record_interception(
            ctx.instance_id,
            &ctx.step_id,
            "tool_call",
            json!({
                "tool_id": tool_id,
                "arguments": arguments,
                "risk_level": risk,
            }),
        )?;
        Ok(self.synthetic_tool_output(tool_id, tool_def.as_ref()))
    }

    #[allow(clippy::too_many_arguments)]
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
        // Record the agent invocation for visibility in shadow results.
        self.record_interception(
            ctx.instance_id,
            &ctx.step_id,
            "agent_invocation",
            json!({
                "persona_id": persona_id,
                "task": task,
                "async": async_exec,
                "agent_name": agent_name,
                "sandbox": true,
            }),
        )?;

        // Delegate to the real executor — the agent will run with shadow_mode
        // on its AgentSpec, so its side-effecting tool calls are intercepted
        // at the LoopContext level while read-only tools pass through.
        self.inner
            .invoke_agent(
                persona_id,
                task,
                async_exec,
                timeout_secs,
                step_permissions,
                agent_name,
                existing_agent_id,
                ctx,
            )
            .await
    }

    async fn signal_agent(
        &self,
        target: &SignalTarget,
        content: &str,
        ctx: &ExecutionContext,
    ) -> Result<Value, String> {
        // Delegate to real executor — agents are real in shadow mode now.
        self.inner.signal_agent(target, content, ctx).await
    }

    async fn wait_for_agent(
        &self,
        agent_id: &str,
        timeout_secs: Option<u64>,
        ctx: &ExecutionContext,
    ) -> Result<Value, String> {
        // Delegate to real executor — agents are real in shadow mode now.
        self.inner.wait_for_agent(agent_id, timeout_secs, ctx).await
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
        // Pass through — user interaction still works in shadow mode
        self.inner
            .create_feedback_request(instance_id, step_id, prompt, choices, allow_freeform, ctx)
            .await
    }

    async fn register_event_gate(
        &self,
        _instance_id: i64,
        step_id: &str,
        topic: &str,
        _filter: Option<&str>,
        _timeout_secs: Option<u64>,
        ctx: &ExecutionContext,
    ) -> Result<String, String> {
        // Event gates cannot be meaningfully shadowed: the engine state
        // machine sets step status to WaitingOnEvent and only real event
        // delivery (via TriggerManager) can resolve it.  Returning a
        // synthetic subscription ID would cause the instance to hang
        // forever.  Fail-fast with a clear message instead.
        self.record_interception(
            ctx.instance_id,
            &ctx.step_id,
            "event_gate_blocked",
            json!({
                "step_id": step_id,
                "topic": topic,
                "reason": "Event gates are not supported in shadow mode"
            }),
        )?;
        Err("Event gates are not supported in shadow mode. \
             The workflow will stop at this step during test runs."
            .to_string())
    }

    async fn launch_workflow(
        &self,
        workflow_name: &str,
        inputs: Value,
        ctx: &ExecutionContext,
    ) -> Result<i64, String> {
        self.record_interception(
            ctx.instance_id,
            &ctx.step_id,
            "workflow_launch",
            json!({
                "workflow_name": workflow_name,
                "inputs": inputs,
            }),
        )?;
        // Return a unique negative ID that cannot collide with real DB rowids
        Ok(-(ctx.instance_id.abs() * 1000 + rand_offset()))
    }

    async fn schedule_task(
        &self,
        schedule: &ScheduleTaskDef,
        ctx: &ExecutionContext,
    ) -> Result<String, String> {
        self.record_interception(
            ctx.instance_id,
            &ctx.step_id,
            "scheduled_task",
            json!({
                "name": schedule.name,
                "schedule": schedule.schedule,
                "action": schedule.action,
            }),
        )?;
        Ok(format!("shadow-schedule-{}", uuid::Uuid::new_v4()))
    }

    async fn render_prompt_template(
        &self,
        persona_id: &str,
        prompt_id: &str,
        parameters: Value,
        ctx: &ExecutionContext,
    ) -> Result<String, String> {
        // Pass through — template rendering is side-effect-free
        self.inner
            .render_prompt_template(persona_id, prompt_id, parameters, ctx)
            .await
    }

    async fn on_instance_stopped(&self, instance_id: i64) -> Result<(), String> {
        self.inner.on_instance_stopped(instance_id).await
    }
}

/// Produce a pseudo-random offset for synthetic child workflow IDs.
fn rand_offset() -> i64 {
    use std::time::SystemTime;
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    (nanos % 999) as i64 + 1
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ExecutionMode;
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::Mutex;

    // -- Test doubles --------------------------------------------------------

    struct MockToolInfo {
        tools: HashMap<String, ToolDefinition>,
    }

    impl ToolInfoProvider for MockToolInfo {
        fn get_tool_definition(&self, tool_id: &str) -> Option<ToolDefinition> {
            self.tools.get(tool_id).cloned()
        }
    }

    /// Records every call_tool invocation for verification.
    struct RecordingExecutor {
        calls: Mutex<Vec<(String, Value)>>,
    }

    impl RecordingExecutor {
        fn new() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
            }
        }
        fn call_count(&self) -> usize {
            self.calls.lock().unwrap().len()
        }
    }

    #[async_trait]
    impl StepExecutor for RecordingExecutor {
        async fn call_tool(
            &self,
            tool_id: &str,
            arguments: Value,
            _ctx: &ExecutionContext,
        ) -> Result<Value, String> {
            self.calls
                .lock()
                .unwrap()
                .push((tool_id.to_string(), arguments.clone()));
            Ok(json!({ "real": true }))
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
            Ok(json!({}))
        }
        async fn signal_agent(
            &self,
            _: &SignalTarget,
            _: &str,
            _: &ExecutionContext,
        ) -> Result<Value, String> {
            Ok(json!({}))
        }
        async fn wait_for_agent(
            &self,
            _: &str,
            _: Option<u64>,
            _: &ExecutionContext,
        ) -> Result<Value, String> {
            Ok(json!({}))
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
            Ok("feedback-id".into())
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
            Ok("gate-id".into())
        }
        async fn launch_workflow(
            &self,
            _: &str,
            _: Value,
            _: &ExecutionContext,
        ) -> Result<i64, String> {
            Ok(42)
        }
        async fn schedule_task(
            &self,
            _: &ScheduleTaskDef,
            _: &ExecutionContext,
        ) -> Result<String, String> {
            Ok("sched-id".into())
        }
        async fn render_prompt_template(
            &self,
            _: &str,
            _: &str,
            _: Value,
            _: &ExecutionContext,
        ) -> Result<String, String> {
            Ok("rendered template".into())
        }
    }

    // -- Helpers --------------------------------------------------------------

    fn test_ctx() -> ExecutionContext {
        ExecutionContext {
            instance_id: 1,
            step_id: "step-1".into(),
            parent_session_id: "sess-1".into(),
            parent_agent_id: None,
            workspace_path: None,
            permissions: vec![],
            attachments_dir: None,
            selected_attachments: vec![],
            execution_mode: ExecutionMode::Shadow,
        }
    }

    fn make_tool(id: &str, read_only: bool, side_effects: bool) -> ToolDefinition {
        use hive_contracts::ChannelClass;
        use hive_contracts::tools::{ToolAnnotations, ToolApproval};
        ToolDefinition {
            id: id.to_string(),
            name: id.to_string(),
            description: String::new(),
            input_schema: json!({}),
            output_schema: None,
            channel_class: ChannelClass::Internal,
            side_effects,
            approval: ToolApproval::Auto,
            annotations: ToolAnnotations {
                title: id.to_string(),
                read_only_hint: if read_only { Some(true) } else { None },
                destructive_hint: None,
                idempotent_hint: None,
                open_world_hint: None,
            },
        }
    }

    fn build_shadow(
        inner: Arc<dyn StepExecutor>,
        tools: HashMap<String, ToolDefinition>,
    ) -> ShadowStepExecutor {
        let store = Arc::new(crate::store::WorkflowStore::in_memory().unwrap());
        let tool_info: Arc<dyn ToolInfoProvider> = Arc::new(MockToolInfo { tools });

        // Create a minimal instance so the FK constraint on intercepted_actions is satisfied
        let def = crate::types::WorkflowDefinition {
            id: "test-wf-id".into(),
            name: "test-wf".into(),
            version: "1.0".into(),
            description: None,
            mode: crate::types::WorkflowMode::default(),
            variables: json!({}),
            steps: vec![],
            output: None,
            result_message: None,
            requested_tools: vec![],
            permissions: vec![],
            attachments: vec![],
            tests: vec![],
            bundled: false,
            archived: false,
            triggers_paused: false,
        };
        let instance = crate::types::WorkflowInstance {
            id: 0,
            definition: def,
            status: crate::types::WorkflowStatus::Running,
            variables: json!({}),
            step_states: HashMap::new(),
            parent_session_id: "sess-1".into(),
            parent_agent_id: None,
            trigger_step_id: None,
            permissions: vec![],
            created_at_ms: 1000,
            updated_at_ms: 1000,
            completed_at_ms: None,
            output: None,
            error: None,
            workspace_path: None,
            resolved_result_message: None,
            goto_activated_steps: std::collections::HashSet::new(),
            goto_source_steps: std::collections::HashSet::new(),
            active_loops: HashMap::new(),
            execution_mode: ExecutionMode::Shadow,
            shadow_overrides: HashMap::new(),
        };
        // Instance gets ID 1, which matches test_ctx().instance_id
        store.create_instance(&instance).unwrap();

        ShadowStepExecutor::new(inner, tool_info, store)
    }

    // -- Tests ----------------------------------------------------------------

    #[tokio::test]
    async fn safe_tool_passes_through() {
        let inner = Arc::new(RecordingExecutor::new());
        let mut tools = HashMap::new();
        tools.insert("read.data".into(), make_tool("read.data", true, false));

        let shadow = build_shadow(inner.clone(), tools);
        let ctx = test_ctx();

        let result = shadow
            .call_tool("read.data", json!({"q": "test"}), &ctx)
            .await
            .unwrap();
        assert_eq!(result["real"], json!(true));
        assert_eq!(inner.call_count(), 1);
    }

    #[tokio::test]
    async fn side_effect_tool_is_intercepted() {
        let inner = Arc::new(RecordingExecutor::new());
        let mut tools = HashMap::new();
        tools.insert(
            "some.write".into(),
            make_tool("some.write", false, true),
        );

        let shadow = build_shadow(inner.clone(), tools);
        let ctx = test_ctx();

        let result = shadow
            .call_tool("some.write", json!({"data": "x"}), &ctx)
            .await
            .unwrap();
        assert_eq!(result["shadow"], json!(true));
        assert_eq!(inner.call_count(), 0); // not passed through
    }

    #[tokio::test]
    async fn unknown_tool_is_intercepted() {
        let inner = Arc::new(RecordingExecutor::new());
        let shadow = build_shadow(inner.clone(), HashMap::new());
        let ctx = test_ctx();

        let result = shadow
            .call_tool("nonexistent.tool", json!({}), &ctx)
            .await
            .unwrap();
        assert_eq!(result["shadow"], json!(true));
        assert_eq!(inner.call_count(), 0);
    }

    #[tokio::test]
    async fn invoke_agent_delegates_to_inner() {
        let inner = Arc::new(RecordingExecutor::new());
        let shadow = build_shadow(inner, HashMap::new());
        let ctx = test_ctx();

        // invoke_agent now delegates to the real executor (agent runs in sandbox)
        let result = shadow
            .invoke_agent("persona-1", "do something", false, None, &[], None, None, &ctx)
            .await
            .unwrap();
        // Result comes from RecordingExecutor which returns `{}`
        assert_eq!(result, json!({}));
    }

    #[tokio::test]
    async fn launch_workflow_returns_negative_id() {
        let inner = Arc::new(RecordingExecutor::new());
        let shadow = build_shadow(inner, HashMap::new());
        let ctx = test_ctx();

        let id = shadow
            .launch_workflow("child-wf", json!({}), &ctx)
            .await
            .unwrap();
        assert!(id < 0, "shadow workflow ID should be negative, got {id}");
    }

    #[tokio::test]
    async fn event_gate_fails_in_shadow() {
        let inner = Arc::new(RecordingExecutor::new());
        let shadow = build_shadow(inner, HashMap::new());
        let ctx = test_ctx();

        let result = shadow
            .register_event_gate(1, "step-1", "some.topic", None, None, &ctx)
            .await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .contains("not supported in shadow mode"));
    }

    #[tokio::test]
    async fn feedback_request_passes_through() {
        let inner = Arc::new(RecordingExecutor::new());
        let shadow = build_shadow(inner, HashMap::new());
        let ctx = test_ctx();

        let result = shadow
            .create_feedback_request(1, "step-1", "prompt", None, true, &ctx)
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn render_prompt_passes_through() {
        let inner = Arc::new(RecordingExecutor::new());
        let shadow = build_shadow(inner, HashMap::new());
        let ctx = test_ctx();

        let result = shadow
            .render_prompt_template("persona-1", "prompt-1", json!({}), &ctx)
            .await;
        assert_eq!(result.unwrap(), "rendered template");
    }

    #[tokio::test]
    async fn schema_aware_output() {
        let inner = Arc::new(RecordingExecutor::new());
        let mut tools = HashMap::new();
        let mut tool = make_tool("email.send", false, true);
        tool.output_schema = Some(json!({
            "type": "object",
            "properties": {
                "message_id": { "type": "string" },
                "status": { "type": "string", "enum": ["sent", "failed"] }
            }
        }));
        tools.insert("email.send".into(), tool);

        let shadow = build_shadow(inner, tools);
        let ctx = test_ctx();

        let result = shadow
            .call_tool("email.send", json!({"to": "a@b.c"}), &ctx)
            .await
            .unwrap();
        assert_eq!(result["message_id"], json!(""));
        assert_eq!(result["status"], json!("sent")); // first enum value
    }

    #[tokio::test]
    async fn schedule_task_returns_shadow_id() {
        let inner = Arc::new(RecordingExecutor::new());
        let shadow = build_shadow(inner, HashMap::new());
        let ctx = test_ctx();

        let schedule = ScheduleTaskDef {
            name: "w".into(),
            schedule: "* * * * *".into(),
            action: json!({}),
        };

        let result = shadow.schedule_task(&schedule, &ctx).await.unwrap();
        assert!(result.starts_with("shadow-schedule-"));
    }

    #[tokio::test]
    async fn intercepted_actions_are_persisted() {
        let inner = Arc::new(RecordingExecutor::new());
        let store = Arc::new(crate::store::WorkflowStore::in_memory().unwrap());
        let tool_info: Arc<dyn ToolInfoProvider> = Arc::new(MockToolInfo {
            tools: HashMap::new(),
        });

        // Create a minimal definition + instance so the FK constraint is satisfied
        let def = crate::types::WorkflowDefinition {
            id: "test-wf-id".into(),
            name: "test-wf".into(),
            version: "1.0".into(),
            description: None,
            mode: crate::types::WorkflowMode::default(),
            variables: json!({}),
            steps: vec![],
            output: None,
            result_message: None,
            requested_tools: vec![],
            permissions: vec![],
            attachments: vec![],
            tests: vec![],
            bundled: false,
            archived: false,
            triggers_paused: false,
        };
        let instance = crate::types::WorkflowInstance {
            id: 0,
            definition: def,
            status: crate::types::WorkflowStatus::Running,
            variables: json!({}),
            step_states: HashMap::new(),
            parent_session_id: "sess-1".into(),
            parent_agent_id: None,
            trigger_step_id: None,
            permissions: vec![],
            created_at_ms: 1000,
            updated_at_ms: 1000,
            completed_at_ms: None,
            output: None,
            error: None,
            workspace_path: None,
            resolved_result_message: None,
            goto_activated_steps: std::collections::HashSet::new(),
            goto_source_steps: std::collections::HashSet::new(),
            active_loops: HashMap::new(),
            execution_mode: ExecutionMode::Shadow,
            shadow_overrides: HashMap::new(),
        };
        let id = store.create_instance(&instance).unwrap();

        let shadow = ShadowStepExecutor::new(inner, tool_info, store.clone());
        let ctx = ExecutionContext {
            instance_id: id,
            step_id: "step-1".into(),
            parent_session_id: "sess-1".into(),
            parent_agent_id: None,
            workspace_path: None,
            permissions: vec![],
            attachments_dir: None,
            selected_attachments: vec![],
            execution_mode: ExecutionMode::Shadow,
        };

        // Make a few intercepted calls
        shadow
            .call_tool("unknown.tool", json!({"a": 1}), &ctx)
            .await
            .unwrap();
        shadow
            .launch_workflow("child", json!({}), &ctx)
            .await
            .unwrap();

        // Verify persisted
        let page = store.list_intercepted_actions(id, 100, 0).unwrap();
        assert_eq!(page.total, 2);
        assert_eq!(page.items[0].kind, "tool_call");
        assert_eq!(page.items[1].kind, "workflow_launch");
    }
}
