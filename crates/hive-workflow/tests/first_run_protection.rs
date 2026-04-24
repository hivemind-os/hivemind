//! Integration tests for first-run protection (Phase 4).
//!
//! These verify that:
//! - New definitions are marked as "untested"
//! - Successful normal-mode runs update the tracking metadata
//! - Shadow-mode runs do NOT update the tracking metadata
//! - Modifying a definition resets the untested status

use async_trait::async_trait;
use hive_workflow::*;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Helpers — minimal executor and shared utilities
// ---------------------------------------------------------------------------

struct NoOpExecutor;

#[async_trait]
impl StepExecutor for NoOpExecutor {
    async fn call_tool(
        &self,
        _tool_id: &str,
        _arguments: Value,
        _ctx: &ExecutionContext,
    ) -> Result<Value, String> {
        Ok(json!({"ok": true}))
    }

    async fn invoke_agent(
        &self,
        _persona_id: &str,
        _task: &str,
        _async_exec: bool,
        _timeout_secs: Option<u64>,
        _permissions: &[PermissionEntry],
        _agent_name: Option<&str>,
        _existing_agent_id: Option<&str>,
        _ctx: &ExecutionContext,
    ) -> Result<Value, String> {
        Ok(json!({"done": true}))
    }

    async fn signal_agent(
        &self,
        _target: &SignalTarget,
        _content: &str,
        _ctx: &ExecutionContext,
    ) -> Result<Value, String> {
        Ok(json!({}))
    }

    async fn wait_for_agent(
        &self,
        _agent_id: &str,
        _timeout_secs: Option<u64>,
        _ctx: &ExecutionContext,
    ) -> Result<Value, String> {
        Ok(json!({}))
    }

    async fn create_feedback_request(
        &self,
        _instance_id: i64,
        _step_id: &str,
        _prompt: &str,
        _choices: Option<&[String]>,
        _allow_freeform: bool,
        _ctx: &ExecutionContext,
    ) -> Result<String, String> {
        Ok("req-1".to_string())
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
        Ok("sub-1".to_string())
    }

    async fn launch_workflow(
        &self,
        _name: &str,
        _inputs: Value,
        _ctx: &ExecutionContext,
    ) -> Result<i64, String> {
        Ok(9999)
    }

    async fn schedule_task(
        &self,
        _schedule: &ScheduleTaskDef,
        _ctx: &ExecutionContext,
    ) -> Result<String, String> {
        Ok("sched-1".to_string())
    }

    async fn render_prompt_template(
        &self,
        _persona_id: &str,
        _prompt_id: &str,
        _parameters: Value,
        _ctx: &ExecutionContext,
    ) -> Result<String, String> {
        Ok("rendered".to_string())
    }
}

fn make_simple_def(name: &str) -> WorkflowDefinition {
    WorkflowDefinition {
        id: generate_workflow_id(),
        name: name.to_string(),
        version: "1.0".to_string(),
        description: None,
        mode: WorkflowMode::Background,
        variables: json!({}),
        steps: vec![
            StepDef {
                id: "trigger".to_string(),
                step_type: StepType::Trigger {
                    trigger: TriggerDef {
                        trigger_type: TriggerType::Manual {
                            inputs: vec![],
                            input_schema: None,
                        },
                    },
                },
                outputs: HashMap::new(),
                on_error: None,
                next: vec!["end".to_string()],
                timeout_secs: None,
                designer_x: None,
                designer_y: None,
            },
            StepDef {
                id: "end".to_string(),
                step_type: StepType::ControlFlow {
                    control: ControlFlowDef::EndWorkflow,
                },
                outputs: HashMap::new(),
                on_error: None,
                next: vec![],
                timeout_secs: None,
                designer_x: None,
                designer_y: None,
            },
        ],
        output: None,
        result_message: None,
        requested_tools: vec![],
        permissions: vec![],
        attachments: vec![],
        tests: vec![],
        bundled: false,
        archived: false,
        triggers_paused: false,
    }
}

fn build_engine(store: Arc<WorkflowStore>) -> WorkflowEngine {
    let executor: Arc<dyn StepExecutor> = Arc::new(NoOpExecutor);
    let emitter = Arc::new(NullEventEmitter);
    WorkflowEngine::new(store, executor, emitter)
}

async fn wait_for_terminal(store: &WorkflowStore, instance_id: i64) {
    for _ in 0..200 {
        let inst = store.get_instance(instance_id).unwrap().unwrap();
        if matches!(
            inst.status,
            WorkflowStatus::Completed | WorkflowStatus::Failed | WorkflowStatus::Killed
        ) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("instance {} did not reach terminal state in time", instance_id);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// New workflow definition with no prior runs should be untested.
#[tokio::test]
async fn new_definition_is_untested() {
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let def = make_simple_def("test/untested");
    let yaml = serde_yaml::to_string(&def).unwrap();
    store.save_definition(&yaml, &def).unwrap();

    let summaries = store.list_definitions().unwrap();
    let s = summaries.iter().find(|s| s.name == "test/untested").unwrap();
    assert!(s.is_untested, "new definition should be untested");
    assert!(s.last_successful_run_at_ms.is_none());
}

/// Successful normal-mode run marks the definition as tested.
#[tokio::test]
async fn normal_run_marks_tested() {
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let def = make_simple_def("test/normal-run");
    let yaml = serde_yaml::to_string(&def).unwrap();
    store.save_definition(&yaml, &def).unwrap();

    let engine = build_engine(store.clone());
    let instance_id = engine
        .launch_with_id(
            def.clone(),
            json!({}),
            "session-1".to_string(),
            None,
            vec![],
            Some("trigger".to_string()),
            None,
            ExecutionMode::Normal,
        )
        .await
        .unwrap();
    wait_for_terminal(&store, instance_id).await;

    let inst = store.get_instance(instance_id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);
    assert_eq!(inst.execution_mode, ExecutionMode::Normal);

    let summaries = store.list_definitions().unwrap();
    let s = summaries.iter().find(|s| s.name == "test/normal-run").unwrap();
    assert!(!s.is_untested, "successful normal run should mark as tested");
    assert!(s.last_successful_run_at_ms.is_some());
}

/// Shadow-mode run does NOT update the tested status.
#[tokio::test]
async fn shadow_run_does_not_mark_tested() {
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let def = make_simple_def("test/shadow-run");
    let yaml = serde_yaml::to_string(&def).unwrap();
    store.save_definition(&yaml, &def).unwrap();

    let engine = build_engine(store.clone());
    let instance_id = engine
        .launch_with_id(
            def.clone(),
            json!({}),
            "session-1".to_string(),
            None,
            vec![],
            Some("trigger".to_string()),
            None,
            ExecutionMode::Shadow,
        )
        .await
        .unwrap();
    wait_for_terminal(&store, instance_id).await;

    let inst = store.get_instance(instance_id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Completed);

    let summaries = store.list_definitions().unwrap();
    let s = summaries.iter().find(|s| s.name == "test/shadow-run").unwrap();
    assert!(s.is_untested, "shadow run should NOT mark as tested");
    assert!(s.last_successful_run_at_ms.is_none());
}

/// Modifying definition after successful run resets untested status.
#[tokio::test]
async fn modified_definition_becomes_untested() {
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    let mut def = make_simple_def("test/modified");
    let yaml = serde_yaml::to_string(&def).unwrap();
    store.save_definition(&yaml, &def).unwrap();

    // Run successfully
    let engine = build_engine(store.clone());
    let instance_id = engine
        .launch_with_id(
            def.clone(),
            json!({}),
            "session-1".to_string(),
            None,
            vec![],
            Some("trigger".to_string()),
            None,
            ExecutionMode::Normal,
        )
        .await
        .unwrap();
    wait_for_terminal(&store, instance_id).await;

    // Verify tested
    let summaries = store.list_definitions().unwrap();
    let s = summaries.iter().find(|s| s.name == "test/modified").unwrap();
    assert!(!s.is_untested);

    // Modify the definition (add a description)
    def.description = Some("now modified".to_string());
    let yaml2 = serde_yaml::to_string(&def).unwrap();
    store.save_definition(&yaml2, &def).unwrap();

    // Should be untested again (different JSON hash)
    let summaries = store.list_definitions().unwrap();
    let s = summaries.iter().find(|s| s.name == "test/modified").unwrap();
    assert!(s.is_untested, "modified definition should be untested again");
}

/// Failed normal-mode run does NOT mark as tested.
#[tokio::test]
async fn failed_run_does_not_mark_tested() {
    let store = Arc::new(WorkflowStore::in_memory().unwrap());
    // Create a definition with a step that references a non-existent next step
    // to force a failure path. Actually, let's use a simpler approach — create
    // a definition with a CallTool step that always errors.
    let def = WorkflowDefinition {
        id: generate_workflow_id(),
        name: "test/failed".to_string(),
        version: "1.0".to_string(),
        description: None,
        mode: WorkflowMode::Background,
        variables: json!({}),
        steps: vec![
            StepDef {
                id: "trigger".to_string(),
                step_type: StepType::Trigger {
                    trigger: TriggerDef {
                        trigger_type: TriggerType::Manual {
                            inputs: vec![],
                            input_schema: None,
                        },
                    },
                },
                outputs: HashMap::new(),
                on_error: None,
                next: vec!["fail_step".to_string()],
                timeout_secs: None,
                designer_x: None,
                designer_y: None,
            },
            StepDef {
                id: "fail_step".to_string(),
                step_type: StepType::Task {
                    task: TaskDef::CallTool {
                        tool_id: "always.fail".to_string(),
                        arguments: Default::default(),
                    },
                },
                outputs: HashMap::new(),
                on_error: None, // No error handler → instance fails
                next: vec![],
                timeout_secs: None,
                designer_x: None,
                designer_y: None,
            },
        ],
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
    let yaml = serde_yaml::to_string(&def).unwrap();
    store.save_definition(&yaml, &def).unwrap();

    // Use an executor that always fails
    struct FailExecutor;
    #[async_trait]
    impl StepExecutor for FailExecutor {
        async fn call_tool(&self, _: &str, _: Value, _: &ExecutionContext) -> Result<Value, String> {
            Err("deliberate failure".to_string())
        }
        async fn invoke_agent(&self, _: &str, _: &str, _: bool, _: Option<u64>, _: &[PermissionEntry], _: Option<&str>, _: Option<&str>, _: &ExecutionContext) -> Result<Value, String> {
            Err("fail".to_string())
        }
        async fn signal_agent(&self, _: &SignalTarget, _: &str, _: &ExecutionContext) -> Result<Value, String> {
            Ok(json!({}))
        }
        async fn wait_for_agent(&self, _: &str, _: Option<u64>, _: &ExecutionContext) -> Result<Value, String> {
            Ok(json!({}))
        }
        async fn create_feedback_request(&self, _: i64, _: &str, _: &str, _: Option<&[String]>, _: bool, _: &ExecutionContext) -> Result<String, String> {
            Ok("r".into())
        }
        async fn register_event_gate(&self, _: i64, _: &str, _: &str, _: Option<&str>, _: Option<u64>, _: &ExecutionContext) -> Result<String, String> {
            Ok("s".into())
        }
        async fn launch_workflow(&self, _: &str, _: Value, _: &ExecutionContext) -> Result<i64, String> {
            Ok(0)
        }
        async fn schedule_task(&self, _: &ScheduleTaskDef, _: &ExecutionContext) -> Result<String, String> {
            Ok("t".into())
        }
        async fn render_prompt_template(&self, _: &str, _: &str, _: Value, _: &ExecutionContext) -> Result<String, String> {
            Ok(String::new())
        }
    }

    let executor: Arc<dyn StepExecutor> = Arc::new(FailExecutor);
    let emitter = Arc::new(NullEventEmitter);
    let engine = WorkflowEngine::new(store.clone(), executor, emitter);

    let instance_id = engine
        .launch_with_id(
            def.clone(),
            json!({}),
            "session-1".to_string(),
            None,
            vec![],
            Some("trigger".to_string()),
            None,
            ExecutionMode::Normal,
        )
        .await
        .unwrap();
    wait_for_terminal(&store, instance_id).await;

    let inst = store.get_instance(instance_id).unwrap().unwrap();
    assert_eq!(inst.status, WorkflowStatus::Failed);

    let summaries = store.list_definitions().unwrap();
    let s = summaries.iter().find(|s| s.name == "test/failed").unwrap();
    assert!(s.is_untested, "failed run should NOT mark as tested");
}

/// record_successful_run only updates if newer (prevents stale overwrites).
#[test]
fn record_successful_run_newer_wins() {
    let store = WorkflowStore::in_memory().unwrap();
    let def = make_simple_def("test/ordering");
    let yaml = serde_yaml::to_string(&def).unwrap();
    store.save_definition(&yaml, &def).unwrap();

    // Record run at time 1000 with hash "aaa"
    store
        .record_successful_run("test/ordering", "1.0", "aaa", 1000)
        .unwrap();

    // Record run at time 2000 with hash "bbb" — should overwrite
    store
        .record_successful_run("test/ordering", "1.0", "bbb", 2000)
        .unwrap();

    let summaries = store.list_definitions().unwrap();
    let s = summaries.iter().find(|s| s.name == "test/ordering").unwrap();
    assert_eq!(s.last_successful_run_at_ms, Some(2000));

    // Record run at time 500 with hash "ccc" — should NOT overwrite (stale)
    store
        .record_successful_run("test/ordering", "1.0", "ccc", 500)
        .unwrap();

    let summaries = store.list_definitions().unwrap();
    let s = summaries.iter().find(|s| s.name == "test/ordering").unwrap();
    assert_eq!(s.last_successful_run_at_ms, Some(2000), "stale run should not overwrite");
}

/// sha256_hex helper produces consistent results.
#[test]
fn sha256_hex_consistent() {
    let h1 = sha256_hex("hello world");
    let h2 = sha256_hex("hello world");
    assert_eq!(h1, h2);
    assert_ne!(h1, sha256_hex("different input"));
    assert_eq!(h1.len(), 64); // SHA-256 hex is 64 chars
}
