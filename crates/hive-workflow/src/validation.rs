use crate::error::WorkflowError;
use crate::types::*;
use std::collections::{HashMap, HashSet, VecDeque};

/// Normalize a cron expression to the 6-field (with seconds) format expected
/// by the `cron` crate. Standard 5-field Unix cron expressions are converted
/// by prepending `0` for seconds.
pub fn normalize_cron(expr: &str) -> String {
    let fields: Vec<&str> = expr.split_whitespace().collect();
    if fields.len() == 5 {
        // Standard Unix cron (min hour dom month dow) → prepend seconds
        format!("0 {}", expr.trim())
    } else {
        expr.trim().to_string()
    }
}

/// Validate a workflow definition for structural correctness.
pub fn validate_definition(def: &WorkflowDefinition) -> Result<(), WorkflowError> {
    validate_step_ids_unique(def)?;
    validate_step_references(def)?;
    validate_no_cycles(def)?;
    validate_has_entry_point(def)?;
    validate_reachability(def)?;
    validate_task_fields(def)?;
    validate_attachment_refs(def)?;
    validate_trigger_expressions(def)?;
    Ok(())
}

/// All step IDs must be unique.
fn validate_step_ids_unique(def: &WorkflowDefinition) -> Result<(), WorkflowError> {
    let mut seen = HashSet::new();
    for step in &def.steps {
        if !seen.insert(&step.id) {
            return Err(WorkflowError::InvalidDefinition {
                reason: format!("Duplicate step ID: {}", step.id),
            });
        }
    }
    Ok(())
}

/// All `next` references and control flow step references must point to existing steps.
fn validate_step_references(def: &WorkflowDefinition) -> Result<(), WorkflowError> {
    let valid_ids: HashSet<&str> = def.steps.iter().map(|s| s.id.as_str()).collect();
    let valid_ids_list: Vec<&str> = def.steps.iter().map(|s| s.id.as_str()).collect();

    for step in &def.steps {
        for next_id in &step.next {
            if !valid_ids.contains(next_id.as_str()) {
                return Err(WorkflowError::InvalidDefinition {
                    reason: format!(
                        "Step '{}' references non-existent next step '{}'. Available step IDs: [{}]",
                        step.id, next_id, valid_ids_list.join(", ")
                    ),
                });
            }
        }

        // Check control flow references
        if let StepType::ControlFlow { ref control } = step.step_type {
            let refs = control_flow_step_refs(control);
            for ref_id in refs {
                if !valid_ids.contains(ref_id) {
                    return Err(WorkflowError::InvalidDefinition {
                        reason: format!(
                            "Step '{}' control flow references non-existent step '{}'. Available step IDs: [{}]",
                            step.id, ref_id, valid_ids_list.join(", ")
                        ),
                    });
                }
            }
        }

        // Check GoTo error strategy
        if let Some(ErrorStrategy::GoTo { ref step_id }) = step.on_error {
            if !valid_ids.contains(step_id.as_str()) {
                return Err(WorkflowError::InvalidDefinition {
                    reason: format!(
                        "Step '{}' GoTo error strategy references non-existent step '{}'. Available step IDs: [{}]",
                        step.id, step_id, valid_ids_list.join(", ")
                    ),
                });
            }
        }
    }
    Ok(())
}

/// Check for cycles in the step graph using the `next` edges.
/// Note: `ControlFlowDef::While` and `ControlFlowDef::ForEach` body references are
/// allowed to create back-edges (they are explicit loop primitives). Only `next` edges
/// and `Branch` then/else edges are checked for cycles.
fn validate_no_cycles(def: &WorkflowDefinition) -> Result<(), WorkflowError> {
    // Build adjacency list from `next` + Branch then/else
    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
    for step in &def.steps {
        let mut neighbors: Vec<&str> = step.next.iter().map(|s| s.as_str()).collect();
        if let StepType::ControlFlow {
            control: ControlFlowDef::Branch { ref then, ref else_branch, .. },
        } = step.step_type
        {
            // While/ForEach bodies are intentional cycles — skip
            for id in then {
                neighbors.push(id.as_str());
            }
            for id in else_branch {
                neighbors.push(id.as_str());
            }
        }
        adj.insert(step.id.as_str(), neighbors);
    }

    // Topological sort via Kahn's algorithm
    let mut in_degree: HashMap<&str, usize> = HashMap::new();
    for step in &def.steps {
        in_degree.entry(step.id.as_str()).or_insert(0);
    }
    for neighbors in adj.values() {
        for &n in neighbors {
            if let Some(entry) = in_degree.get_mut(n) {
                *entry += 1;
            }
        }
    }

    let mut queue: VecDeque<&str> = VecDeque::new();
    for (&id, &deg) in &in_degree {
        if deg == 0 {
            queue.push_back(id);
        }
    }

    let mut visited = 0;
    while let Some(node) = queue.pop_front() {
        visited += 1;
        if let Some(neighbors) = adj.get(node) {
            for &n in neighbors {
                if let Some(deg) = in_degree.get_mut(n) {
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push_back(n);
                    }
                }
            }
        }
    }

    if visited < def.steps.len() {
        // Find a step involved in the cycle for error reporting
        let cycled: Vec<&str> =
            in_degree.iter().filter(|(_, &deg)| deg > 0).map(|(&id, _)| id).collect();
        return Err(WorkflowError::CycleDetected {
            step_id: cycled.first().unwrap_or(&"unknown").to_string(),
        });
    }

    Ok(())
}

/// At least one Trigger step must exist (entry point).
fn validate_has_entry_point(def: &WorkflowDefinition) -> Result<(), WorkflowError> {
    let has_trigger = def.steps.iter().any(|s| matches!(s.step_type, StepType::Trigger { .. }));
    if !has_trigger {
        return Err(WorkflowError::InvalidDefinition {
            reason: "Workflow must have at least one Trigger step as an entry point. \
                Add a step with type: trigger and a trigger definition (e.g., type: manual)."
                .into(),
        });
    }
    Ok(())
}

/// All steps must be reachable from a trigger step via `next`, control flow,
/// or GoTo error strategy edges. Unreachable steps indicate authoring errors.
fn validate_reachability(def: &WorkflowDefinition) -> Result<(), WorkflowError> {
    let all_ids: HashSet<&str> = def.steps.iter().map(|s| s.id.as_str()).collect();

    // Build adjacency list: step → reachable neighbors
    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
    for step in &def.steps {
        let entry = adj.entry(step.id.as_str()).or_default();
        // next edges
        for next_id in &step.next {
            entry.push(next_id.as_str());
        }
        // Control flow edges
        if let StepType::ControlFlow { ref control } = step.step_type {
            for ref_id in control_flow_step_refs(control) {
                entry.push(ref_id);
            }
        }
        // GoTo error strategy edges
        if let Some(ErrorStrategy::GoTo { ref step_id }) = step.on_error {
            entry.push(step_id.as_str());
        }
    }

    // BFS from all trigger steps
    let mut visited: HashSet<&str> = HashSet::new();
    let mut queue: VecDeque<&str> = VecDeque::new();
    for step in &def.steps {
        if matches!(step.step_type, StepType::Trigger { .. }) {
            queue.push_back(step.id.as_str());
            visited.insert(step.id.as_str());
        }
    }
    while let Some(node) = queue.pop_front() {
        if let Some(neighbors) = adj.get(node) {
            for &n in neighbors {
                if visited.insert(n) {
                    queue.push_back(n);
                }
            }
        }
    }

    let unreachable: Vec<&str> = all_ids.difference(&visited).copied().collect();
    if !unreachable.is_empty() {
        let mut sorted = unreachable;
        sorted.sort();
        return Err(WorkflowError::InvalidDefinition {
            reason: format!(
                "Unreachable step(s) not connected to any trigger: {}. \
                 Ensure these steps are referenced in a 'next', 'then', 'else', or 'body' field of a reachable step.",
                sorted.join(", ")
            ),
        });
    }
    Ok(())
}

/// Extract step IDs referenced by a control flow definition.
fn control_flow_step_refs(control: &ControlFlowDef) -> Vec<&str> {
    match control {
        ControlFlowDef::Branch { then, else_branch, .. } => {
            let mut refs: Vec<&str> = then.iter().map(|s| s.as_str()).collect();
            refs.extend(else_branch.iter().map(|s| s.as_str()));
            refs
        }
        ControlFlowDef::ForEach { body, .. } => body.iter().map(|s| s.as_str()).collect(),
        ControlFlowDef::While { body, .. } => body.iter().map(|s| s.as_str()).collect(),
        ControlFlowDef::EndWorkflow => vec![],
    }
}

/// Compute the set of "entry" steps — steps that have no predecessors
/// (no other step's `next` or control flow targets list them).
pub fn find_entry_steps(def: &WorkflowDefinition) -> Vec<&str> {
    let all_ids: HashSet<&str> = def.steps.iter().map(|s| s.id.as_str()).collect();
    let mut referenced: HashSet<&str> = HashSet::new();

    for step in &def.steps {
        for next_id in &step.next {
            referenced.insert(next_id.as_str());
        }
        if let StepType::ControlFlow { ref control } = step.step_type {
            for ref_id in control_flow_step_refs(control) {
                referenced.insert(ref_id);
            }
        }
    }

    all_ids.difference(&referenced).copied().collect()
}

/// Validate that InvokeAgent steps only reference attachment IDs defined at the
/// workflow level.
fn validate_attachment_refs(def: &WorkflowDefinition) -> Result<(), WorkflowError> {
    let valid_ids: HashSet<&str> = def.attachments.iter().map(|a| a.id.as_str()).collect();
    for step in &def.steps {
        if let StepType::Task { task: TaskDef::InvokeAgent { ref attachments, .. } } =
            step.step_type
        {
            for att_id in attachments {
                if !valid_ids.contains(att_id.as_str()) {
                    let available: Vec<&str> =
                        def.attachments.iter().map(|a| a.id.as_str()).collect();
                    let hint = if available.is_empty() {
                        "No attachments are defined at the workflow level. Add an 'attachments' section.".to_string()
                    } else {
                        format!("Available attachment IDs: [{}]", available.join(", "))
                    };
                    return Err(WorkflowError::InvalidDefinition {
                        reason: format!(
                            "Step '{}' references non-existent attachment '{}'. {}",
                            step.id, att_id, hint
                        ),
                    });
                }
            }
        }
    }
    Ok(())
}

/// Validate task-level fields for correctness.
fn validate_task_fields(def: &WorkflowDefinition) -> Result<(), WorkflowError> {
    for step in &def.steps {
        if let StepType::Task { task: TaskDef::SetVariable { ref assignments } } = step.step_type {
            if assignments.is_empty() {
                return Err(WorkflowError::InvalidDefinition {
                    reason: format!("SetVariable step '{}' has no assignments", step.id,),
                });
            }
            for a in assignments {
                if a.variable.trim().is_empty() {
                    return Err(WorkflowError::InvalidDefinition {
                        reason: format!(
                            "SetVariable step '{}' has an assignment with empty variable name",
                            step.id,
                        ),
                    });
                }
                if a.value.trim().is_empty() {
                    return Err(WorkflowError::InvalidDefinition {
                        reason: format!(
                            "SetVariable step '{}' has an empty value for variable '{}'",
                            step.id, a.variable,
                        ),
                    });
                }
            }
        }
    }
    Ok(())
}

/// Validate trigger expressions (cron syntax, non-empty topics, etc.).
fn validate_trigger_expressions(def: &WorkflowDefinition) -> Result<(), WorkflowError> {
    for step in &def.steps {
        if let StepType::Trigger { trigger: TriggerDef { trigger_type } } = &step.step_type {
            match trigger_type {
                TriggerType::Schedule { cron } => {
                    use std::str::FromStr;
                    let normalized = normalize_cron(cron);
                    if cron::Schedule::from_str(&normalized).is_err() {
                        return Err(WorkflowError::InvalidDefinition {
                            reason: format!(
                                "Trigger step '{}' has an invalid cron expression: '{}'. \
                                 Example valid cron: '0 9 * * MON-FRI' (weekdays at 9am), '*/5 * * * *' (every 5 minutes).",
                                step.id, cron,
                            ),
                        });
                    }
                }
                TriggerType::EventPattern { topic, .. } => {
                    if topic.trim().is_empty() {
                        return Err(WorkflowError::InvalidDefinition {
                            reason: format!(
                                "Trigger step '{}' has an empty event pattern topic",
                                step.id,
                            ),
                        });
                    }
                }
                TriggerType::IncomingMessage { channel_id, .. } => {
                    if channel_id.trim().is_empty() {
                        return Err(WorkflowError::InvalidDefinition {
                            reason: format!("Trigger step '{}' has an empty channel_id", step.id,),
                        });
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_def(steps: Vec<StepDef>) -> WorkflowDefinition {
        WorkflowDefinition {
            id: generate_workflow_id(),
            name: "test".into(),
            version: "1.0".into(),
            description: None,
            variables: serde_json::json!({"type": "object"}),
            steps,
            output: None,
            requested_tools: vec![],
            permissions: vec![],
            attachments: vec![],
            tests: vec![],
            mode: WorkflowMode::default(),
            result_message: None,
            bundled: false,
            archived: false,
            triggers_paused: false,
        }
    }

    fn manual_trigger() -> TriggerDef {
        TriggerDef { trigger_type: TriggerType::Manual { inputs: vec![], input_schema: None } }
    }

    #[test]
    fn test_valid_linear_workflow() {
        let def = minimal_def(vec![
            StepDef {
                id: "start".into(),
                step_type: StepType::Trigger { trigger: manual_trigger() },
                outputs: HashMap::new(),
                on_error: None,
                next: vec!["process".into()],
                timeout_secs: None,
                designer_x: None,
                designer_y: None,
            },
            StepDef {
                id: "process".into(),
                step_type: StepType::Task { task: TaskDef::Delay { duration_secs: 1 } },
                outputs: HashMap::new(),
                on_error: None,
                next: vec!["end".into()],
                timeout_secs: None,
                designer_x: None,
                designer_y: None,
            },
            StepDef {
                id: "end".into(),
                step_type: StepType::ControlFlow { control: ControlFlowDef::EndWorkflow },
                outputs: HashMap::new(),
                on_error: None,
                next: vec![],
                timeout_secs: None,
                designer_x: None,
                designer_y: None,
            },
        ]);
        assert!(validate_definition(&def).is_ok());
    }

    #[test]
    fn test_duplicate_step_ids() {
        let def = minimal_def(vec![
            StepDef {
                id: "start".into(),
                step_type: StepType::Trigger { trigger: manual_trigger() },
                outputs: HashMap::new(),
                on_error: None,
                next: vec![],
                timeout_secs: None,
                designer_x: None,
                designer_y: None,
            },
            StepDef {
                id: "start".into(),
                step_type: StepType::Task { task: TaskDef::Delay { duration_secs: 1 } },
                outputs: HashMap::new(),
                on_error: None,
                next: vec![],
                timeout_secs: None,
                designer_x: None,
                designer_y: None,
            },
        ]);
        let err = validate_definition(&def).unwrap_err();
        assert!(matches!(err, WorkflowError::InvalidDefinition { .. }));
    }

    #[test]
    fn test_invalid_next_reference() {
        let def = minimal_def(vec![StepDef {
            id: "start".into(),
            step_type: StepType::Trigger { trigger: manual_trigger() },
            outputs: HashMap::new(),
            on_error: None,
            next: vec!["nonexistent".into()],
            timeout_secs: None,
            designer_x: None,
            designer_y: None,
        }]);
        let err = validate_definition(&def).unwrap_err();
        assert!(matches!(err, WorkflowError::InvalidDefinition { .. }));
    }

    #[test]
    fn test_cycle_detected() {
        let def = minimal_def(vec![
            StepDef {
                id: "a".into(),
                step_type: StepType::Trigger { trigger: manual_trigger() },
                outputs: HashMap::new(),
                on_error: None,
                next: vec!["b".into()],
                timeout_secs: None,
                designer_x: None,
                designer_y: None,
            },
            StepDef {
                id: "b".into(),
                step_type: StepType::Task { task: TaskDef::Delay { duration_secs: 1 } },
                outputs: HashMap::new(),
                on_error: None,
                next: vec!["a".into()],
                timeout_secs: None,
                designer_x: None,
                designer_y: None,
            },
        ]);
        let err = validate_definition(&def).unwrap_err();
        assert!(matches!(err, WorkflowError::CycleDetected { .. }));
    }

    #[test]
    fn test_no_trigger_entry() {
        let def = WorkflowDefinition {
            id: generate_workflow_id(),
            name: "test".into(),
            version: "1.0".into(),
            description: None,
            variables: serde_json::json!({}),
            steps: vec![StepDef {
                id: "process".into(),
                step_type: StepType::Task { task: TaskDef::Delay { duration_secs: 1 } },
                outputs: HashMap::new(),
                on_error: None,
                next: vec![],
                timeout_secs: None,
                designer_x: None,
                designer_y: None,
            }],
            output: None,
            requested_tools: vec![],
            permissions: vec![],
            attachments: vec![],
            tests: vec![],
            mode: WorkflowMode::default(),
            result_message: None,
            bundled: false,
            archived: false,
            triggers_paused: false,
        };
        let err = validate_definition(&def).unwrap_err();
        assert!(matches!(err, WorkflowError::InvalidDefinition { .. }));
    }

    #[test]
    fn test_branching_workflow() {
        let def = minimal_def(vec![
            StepDef {
                id: "start".into(),
                step_type: StepType::Trigger { trigger: manual_trigger() },
                outputs: HashMap::new(),
                on_error: None,
                next: vec!["check".into()],
                timeout_secs: None,
                designer_x: None,
                designer_y: None,
            },
            StepDef {
                id: "check".into(),
                step_type: StepType::ControlFlow {
                    control: ControlFlowDef::Branch {
                        condition: "true".into(),
                        then: vec!["path_a".into()],
                        else_branch: vec!["path_b".into()],
                    },
                },
                outputs: HashMap::new(),
                on_error: None,
                next: vec![],
                timeout_secs: None,
                designer_x: None,
                designer_y: None,
            },
            StepDef {
                id: "path_a".into(),
                step_type: StepType::Task { task: TaskDef::Delay { duration_secs: 1 } },
                outputs: HashMap::new(),
                on_error: None,
                next: vec!["end".into()],
                timeout_secs: None,
                designer_x: None,
                designer_y: None,
            },
            StepDef {
                id: "path_b".into(),
                step_type: StepType::Task { task: TaskDef::Delay { duration_secs: 2 } },
                outputs: HashMap::new(),
                on_error: None,
                next: vec!["end".into()],
                timeout_secs: None,
                designer_x: None,
                designer_y: None,
            },
            StepDef {
                id: "end".into(),
                step_type: StepType::ControlFlow { control: ControlFlowDef::EndWorkflow },
                outputs: HashMap::new(),
                on_error: None,
                next: vec![],
                timeout_secs: None,
                designer_x: None,
                designer_y: None,
            },
        ]);
        assert!(validate_definition(&def).is_ok());
    }

    #[test]
    fn test_find_entry_steps() {
        let def = minimal_def(vec![
            StepDef {
                id: "start".into(),
                step_type: StepType::Trigger { trigger: manual_trigger() },
                outputs: HashMap::new(),
                on_error: None,
                next: vec!["process".into()],
                timeout_secs: None,
                designer_x: None,
                designer_y: None,
            },
            StepDef {
                id: "process".into(),
                step_type: StepType::Task { task: TaskDef::Delay { duration_secs: 1 } },
                outputs: HashMap::new(),
                on_error: None,
                next: vec![],
                timeout_secs: None,
                designer_x: None,
                designer_y: None,
            },
        ]);
        let entries = find_entry_steps(&def);
        assert_eq!(entries.len(), 1);
        assert!(entries.contains(&"start"));
    }
}
