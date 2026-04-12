//! Built-in workflow definitions embedded at compile time.

use crate::error::WorkflowResult;
use crate::schema::WorkflowDefinition;

/// Built-in ReAct workflow YAML.
pub const REACT_YAML: &str = include_str!("../workflows/react.yaml");

/// Built-in Sequential workflow YAML.
pub const SEQUENTIAL_YAML: &str = include_str!("../workflows/sequential.yaml");

/// Built-in Plan-Then-Execute workflow YAML.
pub const PLAN_THEN_EXECUTE_YAML: &str = include_str!("../workflows/plan_then_execute.yaml");

/// Load a built-in workflow by name.
pub fn load_builtin(name: &str) -> WorkflowResult<WorkflowDefinition> {
    let yaml = match name {
        "react" => REACT_YAML,
        "sequential" => SEQUENTIAL_YAML,
        "plan-then-execute" | "plan_then_execute" => PLAN_THEN_EXECUTE_YAML,
        _ => {
            return Err(crate::error::WorkflowError::Schema(format!(
                "unknown built-in workflow: {name}"
            )))
        }
    };
    let def = WorkflowDefinition::from_yaml(yaml)?;
    def.validate()?;
    Ok(def)
}

/// List the names of all available built-in workflows.
pub fn list_builtins() -> Vec<&'static str> {
    vec!["react", "sequential", "plan-then-execute"]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_react() {
        let wf = load_builtin("react").unwrap();
        assert_eq!(wf.name, "react");
    }

    #[test]
    fn load_sequential() {
        let wf = load_builtin("sequential").unwrap();
        assert_eq!(wf.name, "sequential");
    }

    #[test]
    fn load_plan_then_execute() {
        let wf = load_builtin("plan-then-execute").unwrap();
        assert_eq!(wf.name, "plan-then-execute");
    }

    #[test]
    fn load_unknown_returns_error() {
        assert!(load_builtin("nonexistent").is_err());
    }

    #[test]
    fn all_builtins_parse_and_validate() {
        for name in list_builtins() {
            let wf = load_builtin(name).unwrap_or_else(|_| panic!("failed to load {name}"));
            wf.validate().unwrap_or_else(|_| panic!("failed to validate {name}"));
        }
    }
}
