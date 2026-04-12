use std::collections::HashMap;

use crate::types::AgentKitError;

/// Known YAML field names that hold persona references.
const PERSONA_REF_FIELDS: &[&str] = &["persona_id"];

/// Known YAML field names that hold workflow references.
const WORKFLOW_REF_FIELDS: &[&str] = &["workflow_name", "definition"];

/// Rewrite cross-references in a workflow YAML string.
///
/// Walks the YAML value tree and replaces string values for known reference
/// fields (`persona_id`, `workflow_name`, `definition`) when they match a key
/// in `remap`.  References that do not appear in `remap` are left untouched.
///
/// Returns the modified YAML string and a list of unresolved external
/// references (field values that looked like references but were not in the
/// remap table).
pub fn rewrite_workflow_references(
    yaml: &str,
    remap: &HashMap<String, String>,
) -> Result<(String, Vec<String>), AgentKitError> {
    let mut value: serde_yaml::Value = serde_yaml::from_str(yaml)?;
    let mut external_refs = Vec::new();

    rewrite_value(&mut value, remap, &mut external_refs);

    let output = serde_yaml::to_string(&value)?;
    Ok((output, external_refs))
}

/// Collect all referenced persona and workflow IDs from a workflow YAML string.
pub fn collect_references(yaml: &str) -> Result<Vec<String>, AgentKitError> {
    let value: serde_yaml::Value = serde_yaml::from_str(yaml)?;
    let mut refs = Vec::new();
    collect_refs_from_value(&value, &mut refs);
    refs.sort();
    refs.dedup();
    Ok(refs)
}

fn collect_refs_from_value(value: &serde_yaml::Value, refs: &mut Vec<String>) {
    match value {
        serde_yaml::Value::Mapping(map) => {
            for (k, v) in map {
                if let serde_yaml::Value::String(key) = k {
                    if is_ref_field(key) {
                        if let serde_yaml::Value::String(val) = v {
                            refs.push(val.clone());
                        }
                    }
                }
                collect_refs_from_value(v, refs);
            }
        }
        serde_yaml::Value::Sequence(seq) => {
            for item in seq {
                collect_refs_from_value(item, refs);
            }
        }
        _ => {}
    }
}

fn is_ref_field(field_name: &str) -> bool {
    PERSONA_REF_FIELDS.contains(&field_name) || WORKFLOW_REF_FIELDS.contains(&field_name)
}

fn rewrite_value(
    value: &mut serde_yaml::Value,
    remap: &HashMap<String, String>,
    external_refs: &mut Vec<String>,
) {
    match value {
        serde_yaml::Value::Mapping(map) => {
            for (k, v) in map.iter_mut() {
                if let serde_yaml::Value::String(key) = k {
                    if is_ref_field(key) {
                        if let serde_yaml::Value::String(ref mut val) = v {
                            if let Some(new_val) = remap.get(val.as_str()) {
                                *val = new_val.clone();
                            } else if !val.is_empty() {
                                external_refs.push(val.clone());
                            }
                        }
                        continue;
                    }
                }
                rewrite_value(v, remap, external_refs);
            }
        }
        serde_yaml::Value::Sequence(seq) => {
            for item in seq {
                rewrite_value(item, remap, external_refs);
            }
        }
        _ => {}
    }
}

/// Compute the new namespaced ID by replacing the root namespace.
///
/// Example: `remap_id("acme/sales/bot", "myteam")` → `"myteam/sales/bot"`
pub fn remap_id(original: &str, target_namespace: &str) -> String {
    match original.find('/') {
        Some(pos) => format!("{}{}", target_namespace, &original[pos..]),
        None => format!("{}/{}", target_namespace, original),
    }
}

/// Build a remap table for all personas and workflows in a kit.
pub fn build_remap_table(
    persona_ids: &[String],
    workflow_names: &[String],
    target_namespace: &str,
) -> HashMap<String, String> {
    let mut remap = HashMap::new();
    for id in persona_ids {
        remap.insert(id.clone(), remap_id(id, target_namespace));
    }
    for name in workflow_names {
        remap.insert(name.clone(), remap_id(name, target_namespace));
    }
    remap
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remap_id_replaces_root_namespace() {
        assert_eq!(remap_id("acme/bot", "myteam"), "myteam/bot");
        assert_eq!(remap_id("acme/sub/bot", "myteam"), "myteam/sub/bot");
        assert_eq!(remap_id("solo", "myteam"), "myteam/solo");
    }

    #[test]
    fn rewrite_persona_id_in_invoke_agent() {
        let yaml = r#"
name: acme/flow
steps:
  - id: step1
    type: task
    task:
      kind: invoke_agent
      persona_id: acme/bot
      task: "do something"
"#;
        let mut remap = HashMap::new();
        remap.insert("acme/bot".to_string(), "team/bot".to_string());
        remap.insert("acme/flow".to_string(), "team/flow".to_string());

        let (output, external) = rewrite_workflow_references(yaml, &remap).unwrap();
        assert!(output.contains("persona_id: team/bot"));
        assert!(external.is_empty());
    }

    #[test]
    fn rewrite_workflow_name_in_launch_workflow() {
        let yaml = r#"
name: acme/parent
steps:
  - id: step1
    type: task
    task:
      kind: launch_workflow
      workflow_name: acme/child
"#;
        let mut remap = HashMap::new();
        remap.insert("acme/child".to_string(), "ns/child".to_string());

        let (output, external) = rewrite_workflow_references(yaml, &remap).unwrap();
        assert!(output.contains("workflow_name: ns/child"));
        assert!(external.is_empty());
    }

    #[test]
    fn external_refs_collected_for_unmapped_ids() {
        let yaml = r#"
name: acme/flow
steps:
  - id: step1
    type: task
    task:
      kind: invoke_agent
      persona_id: external/agent
      task: "do something"
"#;
        let remap = HashMap::new();
        let (_, external) = rewrite_workflow_references(yaml, &remap).unwrap();
        assert_eq!(external, vec!["external/agent"]);
    }

    #[test]
    fn partial_rewrite_only_mapped_refs() {
        let yaml = r#"
name: acme/flow
steps:
  - id: s1
    type: task
    task:
      kind: invoke_agent
      persona_id: acme/bot
      task: "a"
  - id: s2
    type: task
    task:
      kind: invoke_agent
      persona_id: other/agent
      task: "b"
"#;
        let mut remap = HashMap::new();
        remap.insert("acme/bot".to_string(), "ns/bot".to_string());

        let (output, external) = rewrite_workflow_references(yaml, &remap).unwrap();
        assert!(output.contains("persona_id: ns/bot"));
        assert!(output.contains("persona_id: other/agent"));
        assert_eq!(external, vec!["other/agent"]);
    }

    #[test]
    fn build_remap_table_maps_all_items() {
        let personas = vec!["a/p1".to_string(), "a/p2".to_string()];
        let workflows = vec!["a/w1".to_string()];
        let table = build_remap_table(&personas, &workflows, "b");
        assert_eq!(table.get("a/p1").unwrap(), "b/p1");
        assert_eq!(table.get("a/p2").unwrap(), "b/p2");
        assert_eq!(table.get("a/w1").unwrap(), "b/w1");
    }

    #[test]
    fn collect_references_from_yaml() {
        let yaml = r#"
name: test/flow
steps:
  - id: s1
    type: task
    task:
      kind: invoke_agent
      persona_id: test/bot
      task: "x"
  - id: s2
    type: task
    task:
      kind: launch_workflow
      workflow_name: test/sub
"#;
        let refs = collect_references(yaml).unwrap();
        assert!(refs.contains(&"test/bot".to_string()));
        assert!(refs.contains(&"test/sub".to_string()));
    }

    #[test]
    fn rewrite_schedule_task_definition_field() {
        let yaml = r#"
name: acme/flow
steps:
  - id: s1
    type: task
    task:
      kind: schedule_task
      schedule:
        action:
          definition: acme/child
          inputs: {}
"#;
        let mut remap = HashMap::new();
        remap.insert("acme/child".to_string(), "ns/child".to_string());

        let (output, _) = rewrite_workflow_references(yaml, &remap).unwrap();
        assert!(output.contains("definition: ns/child"));
    }
}
