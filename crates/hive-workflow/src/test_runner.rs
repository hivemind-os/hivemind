//! Workflow unit-test runner.
//!
//! Launches each [`WorkflowTestCase`] in *Shadow* mode with per-step output
//! overrides (`shadow_outputs`), waits for completion, then compares the
//! resulting instance against [`TestExpectations`].

use serde_json::Value;

use crate::{
    WorkflowDefinition, WorkflowEngine, WorkflowError, WorkflowTestCase, TestExpectations,
    TestFailure, TestResult, StepStateSnapshot, InterceptedActionSnapshot, WorkflowStatus,
};

/// Run **all** test cases defined on a workflow definition.
pub async fn run_all_tests(
    engine: &WorkflowEngine,
    definition: &WorkflowDefinition,
) -> Result<Vec<TestResult>, WorkflowError> {
    let mut results = Vec::with_capacity(definition.tests.len());
    for tc in &definition.tests {
        results.push(run_test_case(engine, definition, tc).await?);
    }
    Ok(results)
}

/// Run a **single** test case.
pub async fn run_test_case(
    engine: &WorkflowEngine,
    definition: &WorkflowDefinition,
    test_case: &WorkflowTestCase,
) -> Result<TestResult, WorkflowError> {
    let start = std::time::Instant::now();

    // Launch in Shadow mode with the test-case overrides.
    let instance_id = engine
        .launch_test(
            definition.clone(),
            test_case.inputs.clone(),
            "test-runner".into(),
            test_case.trigger_step_id.clone(),
            test_case.shadow_outputs.clone(),
        )
        .await?;

    // Inject synthetic intercepted actions for mock_tool_calls.
    // These represent tool calls that mocked agent steps "would have made".
    for (step_id, calls) in &test_case.mock_tool_calls {
        for call in calls {
            let mut details = serde_json::json!({
                "tool_id": call.tool_id,
                "shadow": true,
                "message": format!("Simulated tool call '{}' from mocked step", call.tool_id),
            });
            if let Some(params) = &call.parameters {
                details["parameters"] = params.clone();
            }
            let action = crate::InterceptedAction {
                id: 0,
                instance_id,
                step_id: step_id.clone(),
                kind: "tool_call".to_string(),
                timestamp_ms: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64,
                details,
            };
            engine.store().save_intercepted_action(&action)?;
        }
    }

    // Wait for terminal state (poll with back-off).
    wait_for_terminal(engine, instance_id).await?;

    let inst = engine
        .store()
        .get_instance(instance_id)?
        .ok_or_else(|| WorkflowError::InstanceNotFound { id: instance_id })?;

    let duration_ms = start.elapsed().as_millis() as u64;

    // Compare expectations.
    let failures = evaluate_expectations(&test_case.expectations, &inst, engine, instance_id)?;

    // Gather actual step states in definition order.
    let step_results: Vec<StepStateSnapshot> = definition
        .steps
        .iter()
        .filter_map(|step_def| {
            inst.step_states.get(&step_def.id).map(|ss| StepStateSnapshot {
                step_id: step_def.id.clone(),
                status: serde_json::to_value(&ss.status)
                    .ok()
                    .and_then(|v| v.as_str().map(String::from))
                    .unwrap_or_else(|| format!("{:?}", ss.status)),
                outputs: ss.outputs.clone(),
                error: ss.error.clone(),
            })
        })
        .collect();

    // Gather intercepted actions (capped at 200 to keep payload lean).
    const ACTION_CAP: usize = 200;
    let action_page = engine.store().list_intercepted_actions(instance_id, ACTION_CAP, 0)?;
    let intercepted_actions: Vec<InterceptedActionSnapshot> = action_page
        .items
        .into_iter()
        .map(|a| InterceptedActionSnapshot {
            step_id: a.step_id,
            kind: a.kind,
            details: a.details,
        })
        .collect();

    let actual_status = serde_json::to_value(&inst.status)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_else(|| format!("{}", inst.status));

    Ok(TestResult {
        test_name: test_case.name.clone(),
        passed: failures.is_empty(),
        instance_id,
        failures,
        duration_ms,
        actual_status: Some(actual_status),
        actual_output: inst.output.clone(),
        step_results,
        intercepted_actions,
        intercepted_actions_total: action_page.total as u32,
    })
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Poll the store until the instance reaches a terminal state.
async fn wait_for_terminal(
    engine: &WorkflowEngine,
    instance_id: i64,
) -> Result<(), WorkflowError> {
    for _ in 0..400 {
        let inst = engine
            .store()
            .get_instance(instance_id)?
            .ok_or_else(|| WorkflowError::InstanceNotFound { id: instance_id })?;
        if matches!(
            inst.status,
            WorkflowStatus::Completed | WorkflowStatus::Failed | WorkflowStatus::Killed
        ) {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    Err(WorkflowError::Other(format!(
        "Test instance {instance_id} did not reach terminal state within 10 s"
    )))
}

/// Evaluate all expectations and return the list of failures.
fn evaluate_expectations(
    expect: &TestExpectations,
    inst: &crate::WorkflowInstance,
    engine: &WorkflowEngine,
    instance_id: i64,
) -> Result<Vec<TestFailure>, WorkflowError> {
    let mut failures = Vec::new();

    // -- status check -------------------------------------------------------
    if let Some(ref expected_status) = expect.status {
        let actual_status = format!("{}", inst.status);
        if actual_status != *expected_status {
            failures.push(TestFailure {
                expectation: "status".into(),
                expected: expected_status.clone(),
                actual: actual_status,
            });
        }
    }

    // -- output check (partial deep-equal) ----------------------------------
    if let Some(ref expected_output) = expect.output {
        let actual = inst.output.clone().unwrap_or(Value::Null);
        if !partial_match(expected_output, &actual) {
            failures.push(TestFailure {
                expectation: "output".into(),
                expected: serde_json::to_string_pretty(expected_output).unwrap_or_default(),
                actual: serde_json::to_string_pretty(&actual).unwrap_or_default(),
            });
        }
    }

    // -- steps_completed ----------------------------------------------------
    for step_id in &expect.steps_completed {
        let state = inst.step_states.get(step_id);
        let completed = state.is_some_and(|s| s.status == crate::StepStatus::Completed);
        if !completed {
            let actual_status = state
                .map(|s| format!("{:?}", s.status))
                .unwrap_or_else(|| "not found".into());
            failures.push(TestFailure {
                expectation: format!("steps_completed: {step_id}"),
                expected: "Completed".into(),
                actual: actual_status,
            });
        }
    }

    // -- steps_not_reached --------------------------------------------------
    for step_id in &expect.steps_not_reached {
        let state = inst.step_states.get(step_id);
        let reached = state.is_some_and(|s| {
            !matches!(
                s.status,
                crate::StepStatus::Pending | crate::StepStatus::Skipped
            )
        });
        if reached {
            let actual_status = state
                .map(|s| format!("{:?}", s.status))
                .unwrap_or_else(|| "not found".into());
            failures.push(TestFailure {
                expectation: format!("steps_not_reached: {step_id}"),
                expected: "Pending or Skipped".into(),
                actual: actual_status,
            });
        }
    }

    // -- intercepted_action_counts ------------------------------------------
    if !expect.intercepted_action_counts.is_empty() {
        let summary = engine.store().get_shadow_summary(instance_id)?;
        for (kind, &expected_count) in &expect.intercepted_action_counts {
            let actual_count = match kind.as_str() {
                "tool_calls" => summary.tool_calls_intercepted,
                "agent_invocations" => summary.agent_invocations_intercepted,
                "workflow_launches" => summary.workflow_launches_intercepted,
                "scheduled_tasks" => summary.scheduled_tasks_intercepted,
                "agent_signals" => summary.agent_signals_intercepted,
                "total" => summary.total_intercepted,
                _ => {
                    // Count by raw kind string in case callers use the
                    // action kind directly.
                    let page = engine.store().list_intercepted_actions(instance_id, 10000, 0)?;
                    page.items.iter().filter(|a| a.kind == *kind).count() as u32
                }
            };
            if actual_count != expected_count {
                failures.push(TestFailure {
                    expectation: format!("intercepted_action_counts.{kind}"),
                    expected: expected_count.to_string(),
                    actual: actual_count.to_string(),
                });
            }
        }
    }

    Ok(failures)
}

/// Partial deep-equal: every key/value in `expected` must be present (and
/// match) in `actual`, but `actual` may have extra keys.
fn partial_match(expected: &Value, actual: &Value) -> bool {
    match (expected, actual) {
        (Value::Object(exp), Value::Object(act)) => {
            exp.iter().all(|(k, v)| {
                act.get(k).map_or(false, |av| partial_match(v, av))
            })
        }
        (Value::Array(exp), Value::Array(act)) => {
            if exp.len() != act.len() {
                return false;
            }
            exp.iter().zip(act.iter()).all(|(e, a)| partial_match(e, a))
        }
        _ => expected == actual,
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn partial_match_exact() {
        assert!(partial_match(&json!({"a": 1}), &json!({"a": 1})));
    }

    #[test]
    fn partial_match_subset() {
        assert!(partial_match(
            &json!({"a": 1}),
            &json!({"a": 1, "b": 2})
        ));
    }

    #[test]
    fn partial_match_mismatch() {
        assert!(!partial_match(
            &json!({"a": 2}),
            &json!({"a": 1, "b": 2})
        ));
    }

    #[test]
    fn partial_match_nested() {
        assert!(partial_match(
            &json!({"outer": {"inner": 42}}),
            &json!({"outer": {"inner": 42, "extra": "yes"}, "top": true})
        ));
    }

    #[test]
    fn partial_match_arrays() {
        assert!(partial_match(&json!([1, 2, 3]), &json!([1, 2, 3])));
        assert!(!partial_match(&json!([1, 2]), &json!([1, 2, 3])));
    }

    #[test]
    fn partial_match_scalars() {
        assert!(partial_match(&json!("hello"), &json!("hello")));
        assert!(!partial_match(&json!("hello"), &json!("world")));
    }
}
