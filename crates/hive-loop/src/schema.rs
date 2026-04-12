use std::collections::HashSet;

use serde::Deserialize;

use crate::error::WorkflowError;

/// Top-level workflow definition, deserialized from a YAML file.
#[derive(Debug, Clone, Deserialize)]
pub struct WorkflowDefinition {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub config: WorkflowConfig,
    #[serde(default)]
    pub inputs: Vec<InputDef>,
    pub steps: Vec<StepDef>,
}

/// Global limits and defaults for the workflow.
#[derive(Debug, Clone, Deserialize)]
pub struct WorkflowConfig {
    #[serde(default = "default_max_iterations")]
    pub max_iterations: Option<usize>,
    #[serde(default = "default_max_tool_calls")]
    pub max_tool_calls: Option<usize>,
}

fn default_max_iterations() -> Option<usize> {
    Some(25)
}

fn default_max_tool_calls() -> Option<usize> {
    Some(50)
}

impl Default for WorkflowConfig {
    fn default() -> Self {
        Self { max_iterations: default_max_iterations(), max_tool_calls: default_max_tool_calls() }
    }
}

/// An expected input variable for the workflow.
#[derive(Debug, Clone, Deserialize)]
pub struct InputDef {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default = "default_true")]
    pub required: bool,
    #[serde(default)]
    pub default: Option<serde_json::Value>,
}

fn default_true() -> bool {
    true
}

/// A single step in the workflow.
#[derive(Debug, Clone, Deserialize)]
pub struct StepDef {
    pub id: String,
    #[serde(default)]
    pub description: Option<String>,
    pub action: ActionDef,
    #[serde(default)]
    pub on_error: Option<ErrorHandler>,
}

/// Error-handling policy for a step.
#[derive(Debug, Clone, Deserialize)]
pub struct ErrorHandler {
    #[serde(default)]
    pub retry: Option<RetryConfig>,
    #[serde(default)]
    pub fallback_step: Option<String>,
    #[serde(default)]
    pub return_error: Option<bool>,
}

/// Retry configuration for error handling.
#[derive(Debug, Clone, Deserialize)]
pub struct RetryConfig {
    pub max_attempts: usize,
    #[serde(default)]
    pub delay_ms: Option<u64>,
}

/// The concrete action a step performs.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ActionDef {
    ModelCall {
        prompt: String,
        #[serde(default)]
        system_prompt: Option<String>,
        #[serde(default)]
        result_var: Option<String>,
    },
    ToolCall {
        tool_name: String,
        #[serde(default)]
        arguments: Option<String>,
        #[serde(default)]
        result_var: Option<String>,
    },
    Branch {
        condition: String,
        then_step: String,
        #[serde(default)]
        else_step: Option<String>,
    },
    ReturnValue {
        value: String,
    },
    SetVariable {
        name: String,
        value: String,
    },
    Log {
        message: String,
        #[serde(default)]
        level: LogLevel,
    },
    Loop {
        condition: String,
        #[serde(default = "default_max_iterations_val")]
        max_iterations: usize,
        steps: Vec<StepDef>,
    },
    ParallelToolCalls {
        calls: String,
        #[serde(default)]
        result_var: Option<String>,
    },
}

fn default_max_iterations_val() -> usize {
    25
}

/// Log severity level.
#[derive(Debug, Clone, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Debug,
    #[default]
    Info,
    Warn,
    Error,
}

// ---------------------------------------------------------------------------
// Parsing & validation
// ---------------------------------------------------------------------------

impl WorkflowDefinition {
    /// Deserialize a `WorkflowDefinition` from a YAML string.
    pub fn from_yaml(yaml_str: &str) -> Result<Self, WorkflowError> {
        serde_yaml::from_str(yaml_str).map_err(|e| WorkflowError::Schema(e.to_string()))
    }

    /// Validate the workflow definition for internal consistency.
    ///
    /// Checks:
    /// - At least one step exists
    /// - All step IDs are unique (including nested loop steps)
    /// - Branch targets reference existing step IDs
    /// - Required inputs must not have defaults; optional inputs must have defaults
    pub fn validate(&self) -> Result<(), WorkflowError> {
        // 1. At least one step
        if self.steps.is_empty() {
            return Err(WorkflowError::Schema("workflow must contain at least one step".into()));
        }

        // 2. Collect all step IDs and check uniqueness
        let mut all_ids = HashSet::new();
        collect_step_ids(&self.steps, &mut all_ids)?;

        // 3. Validate branch / fallback targets
        validate_targets(&self.steps, &all_ids)?;

        // 4. Input invariants
        for input in &self.inputs {
            if input.required && input.default.is_some() {
                return Err(WorkflowError::Schema(format!(
                    "input `{}` is required but has a default value",
                    input.name
                )));
            }
            if !input.required && input.default.is_none() {
                return Err(WorkflowError::Schema(format!(
                    "input `{}` is optional but has no default value",
                    input.name
                )));
            }
        }

        Ok(())
    }
}

/// Recursively collect step IDs, returning an error on duplicates.
fn collect_step_ids(steps: &[StepDef], ids: &mut HashSet<String>) -> Result<(), WorkflowError> {
    for step in steps {
        if !ids.insert(step.id.clone()) {
            return Err(WorkflowError::Schema(format!("duplicate step id `{}`", step.id)));
        }
        if let ActionDef::Loop { steps: inner, .. } = &step.action {
            collect_step_ids(inner, ids)?;
        }
    }
    Ok(())
}

/// Validate that all branch targets and fallback steps reference known IDs.
fn validate_targets(steps: &[StepDef], all_ids: &HashSet<String>) -> Result<(), WorkflowError> {
    for step in steps {
        // Branch targets
        if let ActionDef::Branch { then_step, else_step, .. } = &step.action {
            if !all_ids.contains(then_step) {
                return Err(WorkflowError::Schema(format!(
                    "branch in step `{}` references unknown then_step `{}`",
                    step.id, then_step
                )));
            }
            if let Some(es) = else_step {
                if !all_ids.contains(es) {
                    return Err(WorkflowError::Schema(format!(
                        "branch in step `{}` references unknown else_step `{}`",
                        step.id, es
                    )));
                }
            }
        }

        // Fallback step in error handler
        if let Some(handler) = &step.on_error {
            if let Some(fb) = &handler.fallback_step {
                if !all_ids.contains(fb) {
                    return Err(WorkflowError::Schema(format!(
                        "on_error in step `{}` references unknown fallback_step `{}`",
                        step.id, fb
                    )));
                }
            }
        }

        // Recurse into loop steps
        if let ActionDef::Loop { steps: inner, .. } = &step.action {
            validate_targets(inner, all_ids)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_yaml() -> &'static str {
        r#"
name: test-workflow
version: "1.0"
steps:
  - id: greet
    action:
      type: return_value
      value: "hello"
"#
    }

    #[test]
    fn parse_minimal_workflow() {
        let wf = WorkflowDefinition::from_yaml(minimal_yaml()).unwrap();
        assert_eq!(wf.name, "test-workflow");
        assert_eq!(wf.version, "1.0");
        assert!(wf.description.is_none());
        assert_eq!(wf.config.max_iterations, Some(25));
        assert_eq!(wf.config.max_tool_calls, Some(50));
        assert_eq!(wf.steps.len(), 1);
        wf.validate().unwrap();
    }

    #[test]
    fn parse_full_workflow() {
        let yaml = r#"
name: agent-loop
version: "1.0"
description: A full agent loop
config:
  max_iterations: 10
  max_tool_calls: 30
inputs:
  - name: user_input
    description: The user prompt
  - name: temperature
    required: false
    default: 0.7
steps:
  - id: call_model
    description: Call the model
    action:
      type: model_call
      prompt: "{{user_input}}"
      system_prompt: "You are helpful."
      result_var: response
  - id: check_tools
    action:
      type: branch
      condition: "{{response.has_tool_calls}}"
      then_step: run_tools
      else_step: done
  - id: run_tools
    action:
      type: tool_call
      tool_name: "{{tool_name}}"
      arguments: "{{tool_args}}"
      result_var: tool_result
    on_error:
      retry:
        max_attempts: 3
        delay_ms: 100
      fallback_step: done
  - id: done
    action:
      type: return_value
      value: "{{response.content}}"
"#;
        let wf = WorkflowDefinition::from_yaml(yaml).unwrap();
        assert_eq!(wf.config.max_iterations, Some(10));
        assert_eq!(wf.inputs.len(), 2);
        assert!(wf.inputs[0].required);
        assert!(!wf.inputs[1].required);
        assert_eq!(wf.steps.len(), 4);
        wf.validate().unwrap();
    }

    #[test]
    fn parse_loop_and_parallel() {
        let yaml = r#"
name: loop-test
version: "1.0"
steps:
  - id: outer
    action:
      type: loop
      condition: "{{has_more}}"
      max_iterations: 5
      steps:
        - id: inner_log
          action:
            type: log
            message: "iteration"
            level: debug
        - id: inner_set
          action:
            type: set_variable
            name: counter
            value: "{{counter + 1}}"
  - id: parallel
    action:
      type: parallel_tool_calls
      calls: "{{tool_calls}}"
      result_var: results
"#;
        let wf = WorkflowDefinition::from_yaml(yaml).unwrap();
        assert_eq!(wf.steps.len(), 2);
        wf.validate().unwrap();
    }

    #[test]
    fn validate_duplicate_step_id() {
        let yaml = r#"
name: dup
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
        let err = wf.validate().unwrap_err();
        assert!(err.to_string().contains("duplicate step id"));
    }

    #[test]
    fn validate_unknown_branch_target() {
        let yaml = r#"
name: bad-branch
version: "1.0"
steps:
  - id: step1
    action:
      type: branch
      condition: "true"
      then_step: nonexistent
"#;
        let wf = WorkflowDefinition::from_yaml(yaml).unwrap();
        let err = wf.validate().unwrap_err();
        assert!(err.to_string().contains("unknown then_step"));
    }

    #[test]
    fn validate_no_steps() {
        let yaml = r#"
name: empty
version: "1.0"
steps: []
"#;
        let wf = WorkflowDefinition::from_yaml(yaml).unwrap();
        let err = wf.validate().unwrap_err();
        assert!(err.to_string().contains("at least one step"));
    }

    #[test]
    fn validate_required_input_with_default() {
        let yaml = r#"
name: bad-input
version: "1.0"
inputs:
  - name: x
    required: true
    default: 42
steps:
  - id: s
    action:
      type: return_value
      value: "ok"
"#;
        let wf = WorkflowDefinition::from_yaml(yaml).unwrap();
        let err = wf.validate().unwrap_err();
        assert!(err.to_string().contains("required but has a default"));
    }

    #[test]
    fn validate_optional_input_without_default() {
        let yaml = r#"
name: bad-input2
version: "1.0"
inputs:
  - name: y
    required: false
steps:
  - id: s
    action:
      type: return_value
      value: "ok"
"#;
        let wf = WorkflowDefinition::from_yaml(yaml).unwrap();
        let err = wf.validate().unwrap_err();
        assert!(err.to_string().contains("optional but has no default"));
    }

    #[test]
    fn default_log_level_is_info() {
        let yaml = r#"
name: log-default
version: "1.0"
steps:
  - id: s
    action:
      type: log
      message: "hello"
"#;
        let wf = WorkflowDefinition::from_yaml(yaml).unwrap();
        if let ActionDef::Log { level, .. } = &wf.steps[0].action {
            assert_eq!(*level, LogLevel::Info);
        } else {
            panic!("expected Log action");
        }
    }
}
