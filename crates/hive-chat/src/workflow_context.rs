//! Workflow context builder — produces a compact system message describing
//! active workflow instances, their step progress, pending feedback gates,
//! and active sub-agents so the main chat agent can understand and control
//! running workflows.

use hive_agents::types::{AgentStatus, AgentSummary};
use hive_workflow_service::hive_workflow::types::{ExecutionMode, StepStatus, StepType, TaskDef, WorkflowStatus};
use hive_workflow_service::WorkflowService;
use std::collections::{HashMap, HashSet};
use std::fmt::Write;

/// Find the largest byte index ≤ `max` that is on a UTF-8 char boundary.
fn floor_char_boundary(s: &str, max: usize) -> usize {
    if max >= s.len() {
        return s.len();
    }
    let mut i = max;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Maximum character budget for the entire workflow context block.
const MAX_CONTEXT_CHARS: usize = 4000;

/// Maximum characters to include from a single variable value.
const MAX_VAR_VALUE_CHARS: usize = 200;

/// Variables that are typically large / unhelpful in the context summary.
const SKIPPED_VARIABLES: &[&str] = &["_internal", "_meta"];

// ── Static instruction block ─────────────────────────────────────────────

const WORKFLOW_INSTRUCTIONS: &str = "\
You are managing one or more active workflows in this session. \
The current state of each workflow is shown below. Use this information to \
answer the user's questions about workflow progress without needing to call tools.

**Available actions:**
- **Respond to a feedback gate:** `workflow.respond(instance_id, step_id, response)` — the response should be a JSON object with an optional `selected` field (string matching one of the choices) and an optional `text` field for freeform feedback.
- **Send a message to a sub-agent:** `core.signal_agent(agent_id, content)` — use this when the user wants to give additional instructions to a running agent.
- **Pause a workflow:** `workflow.pause(instance_id)`
- **Resume a paused workflow:** `workflow.resume(instance_id)`
- **Kill a workflow:** `workflow.kill(instance_id)`

**Guidelines:**
- When the user asks about workflow progress, summarize the state from the context below.
- When a feedback gate is waiting, tell the user what it's asking and offer to relay their decision.
- When the user wants to give feedback to a sub-agent, use `core.signal_agent` with the agent_id shown.
- Do NOT proactively respond to workflow state changes unless the user asks.
";

// ── Public API ────────────────────────────────────────────────────────────

/// Build a workflow context string for a given session.
///
/// Returns `None` if there are no active workflows for this session.
/// The returned string includes static instructions and dynamic state.
pub async fn build_workflow_context(
    workflow_service: &WorkflowService,
    session_id: &str,
    agents: &[AgentSummary],
    recent_agent_signals: &[(String, String)], // (agent_name, message)
) -> Option<String> {
    // Query active workflow instances for this session.
    let filter = hive_workflow_service::hive_workflow::types::InstanceFilter {
        parent_session_id: Some(session_id.to_string()),
        statuses: vec![
            WorkflowStatus::Running,
            WorkflowStatus::Paused,
            WorkflowStatus::WaitingOnInput,
            WorkflowStatus::WaitingOnEvent,
        ],
        ..Default::default()
    };

    let instances = workflow_service.list_instances(&filter).await.ok()?;
    if instances.items.is_empty() {
        return None;
    }

    // Collect child agent IDs across all active instances.
    let child_agent_ids = workflow_service.list_child_agent_ids().await.unwrap_or_default();

    let mut output = String::with_capacity(MAX_CONTEXT_CHARS);
    output.push_str(WORKFLOW_INSTRUCTIONS);
    output.push_str("\n---\n\n");

    for summary in &instances.items {
        // Fetch full instance for step-level details.
        let instance = match workflow_service.get_instance(summary.id).await {
            Ok(inst) => inst,
            Err(_) => continue,
        };

        // Workflow header
        let _ = writeln!(
            output,
            "### Workflow: {} (instance_id: `{}`)",
            summary.definition_name, summary.id,
        );
        let _ = writeln!(output, "**Status:** {}", instance.status);

        // Step progress
        write_step_progress(&mut output, &instance);

        // Pending feedback gates
        write_pending_gates(&mut output, &instance);

        // Active sub-agents for this instance
        let instance_agent_ids: HashSet<&str> = child_agent_ids
            .get(&instance.id)
            .map(|ids| ids.iter().map(String::as_str).collect())
            .unwrap_or_default();

        if !instance_agent_ids.is_empty() {
            let _ = writeln!(output, "\n**Active sub-agents:**");
            for agent in agents {
                if instance_agent_ids.contains(agent.agent_id.as_str()) {
                    let status_icon = match agent.status {
                        AgentStatus::Active => "🔄",
                        AgentStatus::Waiting => "⏳",
                        AgentStatus::Paused => "⏸️",
                        AgentStatus::Blocked => "🚫",
                        AgentStatus::Terminating => "💀",
                        AgentStatus::Done => "✅",
                        AgentStatus::Error => "❌",
                        AgentStatus::Spawning => "🔄",
                    };
                    let persona_label = agent.spec.persona_id.as_deref().unwrap_or("unknown");
                    let _ = writeln!(
                        output,
                        "- {} **{}** (agent_id: `{}`, persona: `{}`) — {:?}",
                        status_icon,
                        agent.spec.friendly_name,
                        agent.agent_id,
                        persona_label,
                        agent.status,
                    );
                }
            }
        }

        // Key variables (compact)
        write_key_variables(&mut output, &instance);

        output.push('\n');

        // Budget check — stop adding instances if we're over limit
        if output.len() > MAX_CONTEXT_CHARS {
            let boundary = floor_char_boundary(&output, MAX_CONTEXT_CHARS.saturating_sub(40));
            output.truncate(boundary);
            output.push_str("\n\n… (additional workflows truncated)\n");
            break;
        }
    }

    // Recent agent signals
    if !recent_agent_signals.is_empty() {
        let _ = writeln!(output, "---\n\n**Recent messages from sub-agents:**");
        for (agent_name, message) in recent_agent_signals {
            let truncated = if message.len() > 500 {
                let boundary = floor_char_boundary(message, 500);
                format!("{}…", &message[..boundary])
            } else {
                message.clone()
            };
            let _ = writeln!(output, "- **{}:** {}", agent_name, truncated);
        }
    }

    Some(output)
}

// ── Helpers ───────────────────────────────────────────────────────────────

fn write_step_progress(
    out: &mut String,
    instance: &hive_workflow_service::hive_workflow::types::WorkflowInstance,
) {
    let _ = write!(out, "\n**Progress:** ");
    let mut first = true;
    for step_def in &instance.definition.steps {
        // Skip trigger and end-type control flow steps for readability
        if matches!(step_def.step_type, StepType::Trigger { .. }) {
            continue;
        }

        let status =
            instance.step_states.get(&step_def.id).map(|s| s.status).unwrap_or(StepStatus::Pending);

        let icon = match status {
            StepStatus::Completed => "✓",
            StepStatus::Running => "▶",
            StepStatus::Failed => "✗",
            StepStatus::Skipped => "⊘",
            StepStatus::WaitingOnInput => "⏳",
            StepStatus::WaitingOnEvent => "⏳",
            StepStatus::WaitingForDelay => "⏳",
            StepStatus::LoopWaiting => "↻",
            StepStatus::Pending => "·",
        };

        if !first {
            let _ = write!(out, " → ");
        }
        first = false;

        let _ = write!(out, "{icon} {}", step_def.id);
    }
    out.push('\n');
}

fn write_pending_gates(
    out: &mut String,
    instance: &hive_workflow_service::hive_workflow::types::WorkflowInstance,
) {
    for step_def in &instance.definition.steps {
        let Some(state) = instance.step_states.get(&step_def.id) else {
            continue;
        };
        if state.status != StepStatus::WaitingOnInput {
            continue;
        }

        // Get gate details from step state (resolved values) or definition
        let (prompt, choices, allow_freeform) = match &step_def.step_type {
            StepType::Task { task: TaskDef::FeedbackGate { prompt, choices, allow_freeform } } => {
                let resolved_prompt =
                    state.interaction_prompt.as_deref().unwrap_or(prompt.as_str());
                let resolved_choices = state
                    .interaction_choices
                    .clone()
                    .unwrap_or_else(|| choices.clone().unwrap_or_default());
                let freeform = state.interaction_allow_freeform.unwrap_or(*allow_freeform);
                (resolved_prompt.to_string(), resolved_choices, freeform)
            }
            _ => continue,
        };

        let _ = writeln!(out, "\n**⏳ Feedback gate: `{}`**", step_def.id);
        // Truncate prompt to keep context manageable
        if prompt.len() > 800 {
            let boundary = floor_char_boundary(&prompt, 800);
            let _ = writeln!(out, "Prompt: {}…", &prompt[..boundary]);
        } else {
            let _ = writeln!(out, "Prompt: {}", prompt.trim());
        }
        if !choices.is_empty() {
            let _ = writeln!(out, "Choices: [{}]", choices.join(", "));
        }
        let _ = writeln!(out, "Freeform: {}", if allow_freeform { "yes" } else { "no" });
        let _ = writeln!(
            out,
            "→ Respond with: `workflow.respond(instance_id=\"{}\", step_id=\"{}\", response={{...}})`",
            instance.id, step_def.id,
        );
    }
}

fn write_key_variables(
    out: &mut String,
    instance: &hive_workflow_service::hive_workflow::types::WorkflowInstance,
) {
    if let Some(obj) = instance.variables.as_object() {
        let mut has_header = false;
        for (key, value) in obj {
            // Skip internal/meta variables and empty values
            if SKIPPED_VARIABLES.contains(&key.as_str()) {
                continue;
            }
            if value.is_null() || (value.is_string() && value.as_str().unwrap_or("").is_empty()) {
                continue;
            }

            if !has_header {
                let _ = writeln!(out, "\n**Key variables:**");
                has_header = true;
            }

            let display = match value {
                serde_json::Value::String(s) => {
                    if s.len() > MAX_VAR_VALUE_CHARS {
                        let boundary = floor_char_boundary(s, MAX_VAR_VALUE_CHARS);
                        format!("\"{}…\"", &s[..boundary])
                    } else {
                        format!("\"{}\"", s)
                    }
                }
                other => {
                    let s = other.to_string();
                    if s.len() > MAX_VAR_VALUE_CHARS {
                        let boundary = floor_char_boundary(&s, MAX_VAR_VALUE_CHARS);
                        format!("{}…", &s[..boundary])
                    } else {
                        s
                    }
                }
            };
            let _ = writeln!(out, "- `{}` = {}", key, display);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hive_workflow_service::hive_workflow::types::*;
    use std::collections::HashMap;

    fn make_step_def(id: &str, step_type: StepType) -> StepDef {
        StepDef {
            id: id.to_string(),
            step_type,
            outputs: HashMap::new(),
            on_error: None,
            next: vec![],
            timeout_secs: None,
            designer_x: None,
            designer_y: None,
        }
    }

    fn make_step_state(step_id: &str, status: StepStatus) -> StepState {
        StepState {
            step_id: step_id.to_string(),
            status,
            started_at_ms: None,
            completed_at_ms: None,
            outputs: None,
            error: None,
            retry_count: 0,
            retry_delay_secs: None,
            child_workflow_id: None,
            child_agent_id: None,
            interaction_request_id: None,
            interaction_prompt: None,
            interaction_choices: None,
            interaction_allow_freeform: None,
            resume_at_ms: None,
        }
    }

    fn make_test_instance() -> WorkflowInstance {
        let definition = WorkflowDefinition {
            id: "def-1".to_string(),
            name: "test/software-feature".to_string(),
            version: "1.0".to_string(),
            description: Some("Test workflow".to_string()),
            mode: WorkflowMode::Chat,
            variables: serde_json::json!({}),
            steps: vec![
                make_step_def(
                    "start",
                    StepType::Trigger {
                        trigger: TriggerDef {
                            trigger_type: TriggerType::Manual {
                                inputs: vec![],
                                input_schema: None,
                            },
                        },
                    },
                ),
                make_step_def(
                    "research",
                    StepType::Task {
                        task: TaskDef::InvokeAgent {
                            persona_id: "system/software/researcher".to_string(),
                            task: "Do research".to_string(),
                            async_exec: false,
                            timeout_secs: None,
                            permissions: vec![],
                            attachments: vec![],
                            agent_name: None,
                        },
                    },
                ),
                make_step_def(
                    "review_research",
                    StepType::Task {
                        task: TaskDef::FeedbackGate {
                            prompt: "Review the research findings.".to_string(),
                            choices: Some(vec!["Approve".to_string(), "Request More".to_string()]),
                            allow_freeform: true,
                        },
                    },
                ),
                make_step_def(
                    "plan",
                    StepType::Task {
                        task: TaskDef::InvokeAgent {
                            persona_id: "system/software/planner".to_string(),
                            task: "Plan the feature".to_string(),
                            async_exec: false,
                            timeout_secs: None,
                            permissions: vec![],
                            attachments: vec![],
                            agent_name: None,
                        },
                    },
                ),
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

        let mut step_states = HashMap::new();
        step_states
            .insert("research".to_string(), make_step_state("research", StepStatus::Completed));
        let mut gate_state = make_step_state("review_research", StepStatus::WaitingOnInput);
        gate_state.interaction_prompt = Some("Review the research findings.".to_string());
        gate_state.interaction_choices =
            Some(vec!["Approve".to_string(), "Request More".to_string()]);
        gate_state.interaction_allow_freeform = Some(true);
        step_states.insert("review_research".to_string(), gate_state);

        WorkflowInstance {
            id: 42,
            definition,
            status: WorkflowStatus::WaitingOnInput,
            variables: serde_json::json!({
                "feature_name": "Web Search",
                "research_findings": "Found several libraries..."
            }),
            step_states,
            parent_session_id: "session-1".to_string(),
            parent_agent_id: None,
            trigger_step_id: Some("start".to_string()),
            permissions: vec![],
            workspace_path: None,
            created_at_ms: 1000,
            updated_at_ms: 2000,
            completed_at_ms: None,
            output: None,
            error: None,
            resolved_result_message: None,
            goto_activated_steps: HashSet::new(),
            goto_source_steps: HashSet::new(),
            active_loops: HashMap::new(),
            execution_mode: ExecutionMode::default(),
            shadow_overrides: HashMap::new(),
        }
    }

    #[test]
    fn step_progress_shows_correct_icons() {
        let instance = make_test_instance();
        let mut out = String::new();
        write_step_progress(&mut out, &instance);

        // Trigger step (start) should be skipped
        assert!(!out.contains("start"), "trigger step should be skipped");
        // Research is completed
        assert!(out.contains("✓ research"), "completed step should show ✓");
        // Review is waiting
        assert!(out.contains("⏳ review_research"), "waiting step should show ⏳");
        // Plan is pending
        assert!(out.contains("· plan"), "pending step should show ·");
    }

    #[test]
    fn pending_gate_includes_prompt_and_choices() {
        let instance = make_test_instance();
        let mut out = String::new();
        write_pending_gates(&mut out, &instance);

        assert!(out.contains("Feedback gate: `review_research`"));
        assert!(out.contains("Review the research findings."));
        assert!(out.contains("Approve"));
        assert!(out.contains("Request More"));
        assert!(out.contains("Freeform: yes"));
        assert!(out.contains("workflow.respond"));
        assert!(out.contains(&instance.id.to_string()));
    }

    #[test]
    fn key_variables_skips_nulls_and_shows_values() {
        let instance = make_test_instance();
        let mut out = String::new();
        write_key_variables(&mut out, &instance);

        assert!(out.contains("feature_name"));
        assert!(out.contains("Web Search"));
        assert!(out.contains("research_findings"));
    }

    #[test]
    fn key_variables_truncates_long_values() {
        let mut instance = make_test_instance();
        instance.variables = serde_json::json!({
            "long_value": "x".repeat(500),
        });
        let mut out = String::new();
        write_key_variables(&mut out, &instance);

        assert!(out.contains("…"), "long values should be truncated");
        assert!(out.len() < 500, "output should be much shorter than the value");
    }

    #[test]
    fn no_output_when_no_pending_gates() {
        let mut instance = make_test_instance();
        // Change gate to completed
        instance.step_states.get_mut("review_research").unwrap().status = StepStatus::Completed;
        let mut out = String::new();
        write_pending_gates(&mut out, &instance);

        assert!(out.is_empty(), "no output when no pending gates");
    }
}
