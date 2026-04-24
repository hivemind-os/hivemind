//! Workflow unit-test runner.
//!
//! Launches each [`WorkflowTestCase`] in *Shadow* mode with per-step output
//! overrides (`shadow_outputs`), waits for completion, then compares the
//! resulting instance against [`TestExpectations`].

use serde_json::Value;

use crate::{
    WorkflowDefinition, WorkflowEngine, WorkflowError, WorkflowTestCase, TestExpectations,
    TestFailure, TestResult, StepStateSnapshot, InterceptedActionSnapshot, WorkflowStatus,
    ExpectedToolCall,
};

/// Run **all** test cases defined on a workflow definition.
pub async fn run_all_tests(
    engine: &WorkflowEngine,
    definition: &WorkflowDefinition,
    auto_respond: bool,
) -> Result<Vec<TestResult>, WorkflowError> {
    let mut results = Vec::with_capacity(definition.tests.len());
    for tc in &definition.tests {
        results.push(run_test_case(engine, definition, tc, auto_respond).await?);
    }
    Ok(results)
}

/// Run a **single** test case.
pub async fn run_test_case(
    engine: &WorkflowEngine,
    definition: &WorkflowDefinition,
    test_case: &WorkflowTestCase,
    auto_respond: bool,
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
            auto_respond,
        )
        .await?;

    // Validate: expected_tool_calls must not overlap with shadow_outputs
    // (a mocked step skips execution — no real intercepted actions are produced).
    for step_id in test_case.expected_tool_calls.keys() {
        if test_case.shadow_outputs.contains_key(step_id) {
            return Err(WorkflowError::Other(format!(
                "Test case '{}': expected_tool_calls and shadow_outputs both set for step '{}'. \
                 A mocked step produces no real intercepted actions, so assertions cannot match.",
                test_case.name, step_id,
            )));
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
    let failures = evaluate_expectations(
        &test_case.expectations,
        &test_case.expected_tool_calls,
        &inst,
        engine,
        instance_id,
    )?;

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
    // 4800 × 25ms = 120s — long enough for agent invocations with auto-respond.
    for _ in 0..4800 {
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
        "Test instance {instance_id} did not reach terminal state within 120 s"
    )))
}

/// Evaluate all expectations and return the list of failures.
fn evaluate_expectations(
    expect: &TestExpectations,
    expected_tool_calls: &std::collections::HashMap<String, Vec<ExpectedToolCall>>,
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

    // -- expected_tool_calls (bipartite matching) ---------------------------
    if !expected_tool_calls.is_empty() {
        // Load ALL intercepted actions for this instance.
        let page = engine.store().list_intercepted_actions(instance_id, 10000, 0)?;
        let all_actions = if page.total as usize > page.items.len() {
            // Re-fetch with full size.
            engine.store().list_intercepted_actions(instance_id, page.total as usize, 0)?.items
        } else {
            page.items
        };

        for (step_id, expected) in expected_tool_calls {
            let actual_for_step: Vec<_> = all_actions
                .iter()
                .filter(|a| a.step_id == *step_id && a.kind == "tool_call")
                .collect();

            if actual_for_step.is_empty() && !expected.is_empty() {
                failures.push(TestFailure {
                    expectation: format!("expected_tool_calls.{step_id}"),
                    expected: format!("{} tool call(s)", expected.len()),
                    actual: "no intercepted tool calls for this step".into(),
                });
                continue;
            }

            if actual_for_step.len() != expected.len() {
                failures.push(TestFailure {
                    expectation: format!("expected_tool_calls.{step_id}.count"),
                    expected: expected.len().to_string(),
                    actual: actual_for_step.len().to_string(),
                });
                // Still try to match what we can.
            }

            // Build compatibility matrix for bipartite matching.
            let n = expected.len();
            let m = actual_for_step.len();
            let compatible: Vec<Vec<bool>> = expected
                .iter()
                .map(|exp| {
                    actual_for_step
                        .iter()
                        .map(|act| expected_matches_actual(exp, act))
                        .collect()
                })
                .collect();

            let matching = bipartite_match(n, m, &compatible);

            for (i, slot) in matching.iter().enumerate() {
                if slot.is_none() {
                    let exp = &expected[i];
                    let args_preview = exp
                        .arguments
                        .as_ref()
                        .map(|a| serde_json::to_string(a).unwrap_or_default())
                        .unwrap_or_else(|| "(any)".into());
                    failures.push(TestFailure {
                        expectation: format!("expected_tool_calls.{step_id}[{i}]"),
                        expected: format!("{}({})", exp.tool_id, args_preview),
                        actual: format!(
                            "no matching call among {} intercepted actions",
                            actual_for_step.len()
                        ),
                    });
                }
            }
        }
    }

    Ok(failures)
}

/// Check whether an expected tool call matches an actual intercepted action.
fn expected_matches_actual(exp: &ExpectedToolCall, act: &crate::InterceptedAction) -> bool {
    let actual_tool_id = act.details.get("tool_id").and_then(|v| v.as_str());
    if actual_tool_id != Some(&exp.tool_id) {
        return false;
    }
    match &exp.arguments {
        None => true, // no arguments specified → match any
        Some(expected_args) => {
            let actual_args = act.details.get("arguments").unwrap_or(&Value::Null);
            partial_match(expected_args, actual_args)
        }
    }
}

/// Bipartite matching using augmenting paths (Hopcroft-Karp simplified).
///
/// `compatible[i][j]` is true if expected[i] can match actual[j].
/// Returns a vector where `result[i] = Some(j)` means expected[i] matched
/// actual[j], or `None` if no match found.
fn bipartite_match(n: usize, m: usize, compatible: &[Vec<bool>]) -> Vec<Option<usize>> {
    let mut match_right: Vec<Option<usize>> = vec![None; m];

    for i in 0..n {
        let mut visited = vec![false; m];
        augment(i, compatible, &mut match_right, &mut visited);
    }

    // Build left→right result from right→left mapping.
    let mut result = vec![None; n];
    for (j, &matched_left) in match_right.iter().enumerate() {
        if let Some(i) = matched_left {
            result[i] = Some(j);
        }
    }
    result
}

fn augment(
    u: usize,
    compatible: &[Vec<bool>],
    match_right: &mut [Option<usize>],
    visited: &mut [bool],
) -> bool {
    for v in 0..match_right.len() {
        if compatible[u][v] && !visited[v] {
            visited[v] = true;
            if match_right[v].is_none()
                || augment(match_right[v].unwrap(), compatible, match_right, visited)
            {
                match_right[v] = Some(u);
                return true;
            }
        }
    }
    false
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

    // -- bipartite_match tests ----------------------------------------------

    #[test]
    fn bipartite_match_empty() {
        let result = bipartite_match(0, 0, &[]);
        assert!(result.is_empty());
    }

    #[test]
    fn bipartite_match_perfect() {
        // 1:1 mapping — each expected matches exactly one actual
        let compat = vec![
            vec![true, false],
            vec![false, true],
        ];
        let result = bipartite_match(2, 2, &compat);
        assert_eq!(result, vec![Some(0), Some(1)]);
    }

    #[test]
    fn bipartite_match_ambiguous_resolves() {
        // Rubber-duck case: broad + specific expected, both could match actual[0]
        // expected[0] = broad (matches both), expected[1] = specific (matches only actual[0])
        let compat = vec![
            vec![true, true],   // broad: matches actual[0] and actual[1]
            vec![true, false],  // specific: matches only actual[0]
        ];
        let result = bipartite_match(2, 2, &compat);
        // Correct: specific gets actual[0], broad gets actual[1]
        assert!(result[0].is_some());
        assert!(result[1].is_some());
        assert_ne!(result[0], result[1]); // no double-matching
    }

    #[test]
    fn bipartite_match_unmatched() {
        let compat = vec![
            vec![false, false], // expected[0] matches nothing
            vec![false, true],  // expected[1] matches actual[1]
        ];
        let result = bipartite_match(2, 2, &compat);
        assert_eq!(result[0], None);
        assert_eq!(result[1], Some(1));
    }

    #[test]
    fn bipartite_match_more_expected_than_actual() {
        let compat = vec![
            vec![true],
            vec![true],  // both want actual[0] but only one can have it
        ];
        let result = bipartite_match(2, 1, &compat);
        let matched = result.iter().filter(|r| r.is_some()).count();
        assert_eq!(matched, 1); // only one can match
    }

    // -- expected_matches_actual tests --------------------------------------

    #[test]
    fn expected_matches_tool_id_only() {
        let exp = ExpectedToolCall { tool_id: "comm.send_email".into(), arguments: None };
        let act = crate::InterceptedAction {
            id: 1, instance_id: 1, step_id: "s1".into(),
            kind: "tool_call".into(), timestamp_ms: 0,
            details: json!({"tool_id": "comm.send_email", "arguments": {"to": "a@b.com"}}),
        };
        assert!(expected_matches_actual(&exp, &act));
    }

    #[test]
    fn expected_matches_with_partial_args() {
        let exp = ExpectedToolCall {
            tool_id: "comm.send_email".into(),
            arguments: Some(json!({"to": "a@b.com"})),
        };
        let act = crate::InterceptedAction {
            id: 1, instance_id: 1, step_id: "s1".into(),
            kind: "tool_call".into(), timestamp_ms: 0,
            details: json!({"tool_id": "comm.send_email", "arguments": {"to": "a@b.com", "subject": "Hi", "body": "..."}}),
        };
        assert!(expected_matches_actual(&exp, &act));
    }

    #[test]
    fn expected_rejects_wrong_tool_id() {
        let exp = ExpectedToolCall { tool_id: "comm.send_email".into(), arguments: None };
        let act = crate::InterceptedAction {
            id: 1, instance_id: 1, step_id: "s1".into(),
            kind: "tool_call".into(), timestamp_ms: 0,
            details: json!({"tool_id": "http.request", "arguments": {}}),
        };
        assert!(!expected_matches_actual(&exp, &act));
    }

    #[test]
    fn expected_rejects_mismatched_args() {
        let exp = ExpectedToolCall {
            tool_id: "comm.send_email".into(),
            arguments: Some(json!({"to": "wrong@addr.com"})),
        };
        let act = crate::InterceptedAction {
            id: 1, instance_id: 1, step_id: "s1".into(),
            kind: "tool_call".into(), timestamp_ms: 0,
            details: json!({"tool_id": "comm.send_email", "arguments": {"to": "a@b.com"}}),
        };
        assert!(!expected_matches_actual(&exp, &act));
    }
}
