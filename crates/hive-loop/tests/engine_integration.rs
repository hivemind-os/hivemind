//! Integration tests for the hive-loop workflow engine.
//!
//! Exercises the full workflow engine end-to-end with mock backends,
//! covering YAML parsing, step execution, branching, model calls,
//! tool calls, state persistence, event emission, and all built-in workflows.

use hive_loop::{
    InMemoryStore, ModelBackend, ModelRequest, ModelResponse, NullEventSink, ToolBackend,
    ToolSchema, WfToolCall, WfToolResult, WorkflowDefinition, WorkflowEngine, WorkflowEvent,
    WorkflowEventSink, WorkflowResult, WorkflowStatus, WorkflowStore,
};

use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;

// ===========================================================================
// Mock implementations
// ===========================================================================

/// A model backend that returns configurable responses and tracks call count.
struct MockModel {
    responses: Mutex<Vec<ModelResponse>>,
    call_count: AtomicUsize,
}

impl MockModel {
    fn new(responses: Vec<ModelResponse>) -> Self {
        Self { responses: Mutex::new(responses), call_count: AtomicUsize::new(0) }
    }

    /// Single text response with no tool calls.
    fn single(content: &str) -> Self {
        Self::new(vec![ModelResponse {
            content: content.into(),
            tool_calls: vec![],
            metadata: Default::default(),
        }])
    }

    /// First response has a tool call, second response is a final text answer.
    fn with_tool_then_answer(tool_name: &str, tool_args: Value, final_answer: &str) -> Self {
        Self::new(vec![
            ModelResponse {
                content: String::new(),
                tool_calls: vec![WfToolCall {
                    id: "tc_1".into(),
                    name: tool_name.into(),
                    arguments: tool_args,
                }],
                metadata: Default::default(),
            },
            ModelResponse {
                content: final_answer.into(),
                tool_calls: vec![],
                metadata: Default::default(),
            },
        ])
    }

    fn calls(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl ModelBackend for MockModel {
    async fn complete(&self, _request: &ModelRequest) -> WorkflowResult<ModelResponse> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        let mut responses = self.responses.lock().await;
        if responses.is_empty() {
            Ok(ModelResponse {
                content: "default".into(),
                tool_calls: vec![],
                metadata: Default::default(),
            })
        } else {
            Ok(responses.remove(0))
        }
    }
}

/// A tool backend that records calls and returns configurable results.
struct MockTools {
    tools: Vec<ToolSchema>,
    results: Mutex<HashMap<String, String>>,
    calls: Mutex<Vec<(String, Value)>>,
}

impl MockTools {
    fn empty() -> Self {
        Self { tools: vec![], results: Mutex::new(HashMap::new()), calls: Mutex::new(vec![]) }
    }

    fn with_tool(name: &str, result: &str) -> Self {
        let tools = vec![ToolSchema {
            name: name.into(),
            description: format!("Mock tool {name}"),
            parameters: json!({"type": "object"}),
        }];
        let mut results = HashMap::new();
        results.insert(name.into(), result.into());
        Self { tools, results: Mutex::new(results), calls: Mutex::new(vec![]) }
    }

    async fn recorded_calls(&self) -> Vec<(String, Value)> {
        self.calls.lock().await.clone()
    }
}

#[async_trait]
impl ToolBackend for MockTools {
    async fn list_tools(&self) -> WorkflowResult<Vec<ToolSchema>> {
        Ok(self.tools.clone())
    }

    async fn execute(&self, call: &WfToolCall) -> WorkflowResult<WfToolResult> {
        self.calls.lock().await.push((call.name.clone(), call.arguments.clone()));
        let content =
            self.results.lock().await.get(&call.name).cloned().unwrap_or_else(|| "ok".into());
        Ok(WfToolResult {
            call_id: call.id.clone(),
            name: call.name.clone(),
            content,
            is_error: false,
        })
    }
}

/// An event sink that collects all events for assertion.
struct EventCollector {
    events: Mutex<Vec<WorkflowEvent>>,
}

impl EventCollector {
    fn new() -> Self {
        Self { events: Mutex::new(vec![]) }
    }

    async fn events(&self) -> Vec<WorkflowEvent> {
        self.events.lock().await.clone()
    }
}

#[async_trait]
impl WorkflowEventSink for EventCollector {
    async fn emit(&self, event: WorkflowEvent) {
        self.events.lock().await.push(event);
    }
}

// ===========================================================================
// Helper constructors
// ===========================================================================

fn engine_with(
    model: Arc<dyn ModelBackend>,
    tools: Arc<dyn ToolBackend>,
    store: Arc<dyn WorkflowStore>,
    events: Arc<dyn WorkflowEventSink>,
) -> WorkflowEngine {
    WorkflowEngine::new(model, tools, store, events)
}

fn simple_engine(model: Arc<MockModel>) -> WorkflowEngine {
    engine_with(
        model,
        Arc::new(MockTools::empty()),
        Arc::new(InMemoryStore::new()),
        Arc::new(NullEventSink),
    )
}

fn inputs_with_user(text: &str) -> serde_json::Map<String, Value> {
    let mut m = serde_json::Map::new();
    m.insert("user_input".into(), json!(text));
    m
}

// ===========================================================================
// Tests
// ===========================================================================

// 1. Sequential built-in workflow -------------------------------------------

#[tokio::test]
async fn test_sequential_builtin() {
    let model = Arc::new(MockModel::single("Hello back!"));
    let engine = simple_engine(model);

    let result =
        engine.run_builtin("sequential", "seq-1".into(), inputs_with_user("hello")).await.unwrap();

    assert_eq!(result, Value::String("Hello back!".into()));
}

// 2. ReAct with no tools needed ---------------------------------------------

#[tokio::test]
async fn test_react_no_tools_needed() {
    let model = Arc::new(MockModel::single("4"));
    let engine = simple_engine(Arc::clone(&model));

    let result = engine
        .run_builtin("react", "react-1".into(), inputs_with_user("what is 2+2?"))
        .await
        .unwrap();

    assert_eq!(result, Value::String("4".into()));
    assert_eq!(model.calls(), 1);
}

// 3. ReAct with a tool call -------------------------------------------------

#[tokio::test]
async fn test_react_with_tool_call() {
    let model = Arc::new(MockModel::with_tool_then_answer(
        "calculator",
        json!({"expr": "2+2"}),
        "The answer is 4",
    ));
    let tools = Arc::new(MockTools::with_tool("calculator", "4"));

    let engine = engine_with(
        Arc::clone(&model) as Arc<dyn ModelBackend>,
        Arc::clone(&tools) as Arc<dyn ToolBackend>,
        Arc::new(InMemoryStore::new()),
        Arc::new(NullEventSink),
    );

    let result = engine
        .run_builtin("react", "react-2".into(), inputs_with_user("what is 2+2?"))
        .await
        .unwrap();

    assert_eq!(result, Value::String("The answer is 4".into()));
    assert_eq!(model.calls(), 2);

    let recorded = tools.recorded_calls().await;
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].0, "calculator");
    assert_eq!(recorded[0].1, json!({"expr": "2+2"}));
}

// 4. Custom workflow – branch true ------------------------------------------

#[tokio::test]
async fn test_custom_workflow_branch_true() {
    let yaml = r#"
name: test-branch
version: "1.0"
config:
  max_iterations: 10
  max_tool_calls: 0
steps:
  - id: set_flag
    action:
      type: set_variable
      name: flag
      value: "true"
  - id: check
    action:
      type: branch
      condition: "{{flag}}"
      then_step: yes
      else_step: no
  - id: yes
    action:
      type: return_value
      value: "took-yes-branch"
  - id: no
    action:
      type: return_value
      value: "took-no-branch"
"#;
    let wf = WorkflowDefinition::from_yaml(yaml).unwrap();
    let model = Arc::new(MockModel::single("unused"));
    let engine = simple_engine(model);

    let result = engine.run(&wf, "branch-t-1".into(), serde_json::Map::new()).await.unwrap();

    assert_eq!(result, Value::String("took-yes-branch".into()));
}

// 5. Custom workflow – branch false -----------------------------------------

#[tokio::test]
async fn test_custom_workflow_branch_false() {
    let yaml = r#"
name: test-branch-f
version: "1.0"
config:
  max_iterations: 10
  max_tool_calls: 0
steps:
  - id: set_flag
    action:
      type: set_variable
      name: flag
      value: "false"
  - id: check
    action:
      type: branch
      condition: "{{flag}}"
      then_step: yes
      else_step: no
  - id: yes
    action:
      type: return_value
      value: "took-yes-branch"
  - id: no
    action:
      type: return_value
      value: "took-no-branch"
"#;
    let wf = WorkflowDefinition::from_yaml(yaml).unwrap();
    let model = Arc::new(MockModel::single("unused"));
    let engine = simple_engine(model);

    let result = engine.run(&wf, "branch-f-1".into(), serde_json::Map::new()).await.unwrap();

    assert_eq!(result, Value::String("took-no-branch".into()));
}

// 6. Missing required input -------------------------------------------------

#[tokio::test]
async fn test_missing_required_input() {
    let model = Arc::new(MockModel::single("ignored"));
    let engine = simple_engine(model);

    let result = engine.run_builtin("sequential", "miss-1".into(), serde_json::Map::new()).await;

    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("missing required input"),
        "expected 'missing required input', got: {err_msg}"
    );
}

// 7. Optional input uses default --------------------------------------------

#[tokio::test]
async fn test_optional_input_uses_default() {
    let model = Arc::new(MockModel::single("works fine"));
    let engine = simple_engine(model);

    // Provide only user_input – system_prompt should use its default.
    let result =
        engine.run_builtin("sequential", "opt-1".into(), inputs_with_user("hi")).await.unwrap();

    assert_eq!(result, Value::String("works fine".into()));
}

// 8. State persisted to store -----------------------------------------------

#[tokio::test]
async fn test_state_persisted_to_store() {
    let store = Arc::new(InMemoryStore::new());
    let model = Arc::new(MockModel::single("persisted"));

    let engine = engine_with(
        model,
        Arc::new(MockTools::empty()),
        Arc::clone(&store) as Arc<dyn WorkflowStore>,
        Arc::new(NullEventSink),
    );

    engine.run_builtin("sequential", "persist-1".into(), inputs_with_user("test")).await.unwrap();

    let state = store.load("persist-1").await.unwrap();
    assert!(state.is_some(), "state should be in the store");
    let state = state.unwrap();
    assert_eq!(state.status, WorkflowStatus::Completed);
    assert_eq!(state.workflow_name, "sequential");
}

// 9. Resume non-existent run ------------------------------------------------

#[tokio::test]
async fn test_resume_not_found() {
    let model = Arc::new(MockModel::single("ignored"));
    let engine = simple_engine(model);
    let wf = WorkflowDefinition::from_yaml(
        r#"
name: dummy
version: "1.0"
steps:
  - id: s
    action:
      type: return_value
      value: "x"
"#,
    )
    .unwrap();

    let result = engine.resume(&wf, "no-such-run").await;
    assert!(result.is_err());

    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("not found") || err_msg.contains("store"),
        "expected store/not-found error, got: {err_msg}"
    );
}

// 10. Workflow validation error – duplicate step IDs ------------------------

#[tokio::test]
async fn test_workflow_validation_error() {
    let yaml = r#"
name: dup-ids
version: "1.0"
steps:
  - id: step1
    action:
      type: return_value
      value: "a"
  - id: step1
    action:
      type: return_value
      value: "b"
"#;
    let wf = WorkflowDefinition::from_yaml(yaml).unwrap();
    let model = Arc::new(MockModel::single("ignored"));
    let engine = simple_engine(model);

    let result = engine.run(&wf, "dup-1".into(), serde_json::Map::new()).await;

    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("duplicate step id"),
        "expected schema error about duplicate IDs, got: {err_msg}"
    );
}

// 11. Events are emitted ----------------------------------------------------

#[tokio::test]
async fn test_events_emitted() {
    let collector = Arc::new(EventCollector::new());
    let model = Arc::new(MockModel::single("event-test"));

    let engine = engine_with(
        model,
        Arc::new(MockTools::empty()),
        Arc::new(InMemoryStore::new()),
        Arc::clone(&collector) as Arc<dyn WorkflowEventSink>,
    );

    engine.run_builtin("sequential", "evt-1".into(), inputs_with_user("hi")).await.unwrap();

    let events = collector.events().await;

    // Serialise event tags for easy matching.
    let tags: Vec<String> = events
        .iter()
        .map(|e| serde_json::to_value(e).unwrap()["type"].as_str().unwrap().to_string())
        .collect();

    assert!(tags.contains(&"started".to_string()), "expected Started event, got: {tags:?}");
    assert!(
        tags.contains(&"step_started".to_string()),
        "expected StepStarted event, got: {tags:?}"
    );
    assert!(
        tags.contains(&"step_completed".to_string()),
        "expected StepCompleted event, got: {tags:?}"
    );
    assert!(tags.contains(&"completed".to_string()), "expected Completed event, got: {tags:?}");
}

// 12. Plan-then-execute built-in --------------------------------------------

#[tokio::test]
async fn test_plan_then_execute_builtin() {
    let model = Arc::new(MockModel::new(vec![
        // First call: the planning phase returns a plan.
        ModelResponse {
            content: "1. Think\n2. Answer".into(),
            tool_calls: vec![],
            metadata: Default::default(),
        },
        // Second call: execution phase returns the final answer.
        ModelResponse {
            content: "The final answer is 42".into(),
            tool_calls: vec![],
            metadata: Default::default(),
        },
    ]));

    let engine = simple_engine(Arc::clone(&model));

    let result = engine
        .run_builtin(
            "plan-then-execute",
            "pte-1".into(),
            inputs_with_user("what is the meaning of life?"),
        )
        .await
        .unwrap();

    assert_eq!(result, Value::String("The final answer is 42".into()));
    assert_eq!(model.calls(), 2);
}
