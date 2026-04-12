use std::collections::HashSet;

use hive_contracts::prompt_sanitize::escape_prompt_tags;
use regex::Regex;
use serde_json::{Map, Value};

use crate::error::{WorkflowError, WorkflowResult};

/// Navigate a dot-notation path (supporting array indices) into a JSON value.
///
/// Path segments are split on `.`; a purely numeric segment indexes into an array.
fn resolve_path<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = root;
    for segment in path.split('.') {
        match current {
            Value::Object(map) => {
                current = map.get(segment)?;
            }
            Value::Array(arr) => {
                let idx: usize = segment.parse().ok()?;
                current = arr.get(idx)?;
            }
            _ => return None,
        }
    }
    Some(current)
}

/// Convert a JSON value to its string representation for template insertion.
fn value_to_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".to_string(),
        // Objects and arrays are serialized to compact JSON
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

/// Resolve template expressions in a string, replacing `{{var}}` patterns with
/// values looked up from the variable store.
///
/// Supports dot notation (`{{response.content}}`) and array indexing
/// (`{{tool_calls.0.name}}`). Returns an error if any referenced variable is
/// missing.
///
/// When `untrusted_vars` is provided, any expression whose root variable name
/// is in the set will have its resolved value escaped (prompt sentinel tags
/// neutralised) and wrapped in `<external_data>` trust boundary markers.
pub fn resolve_template(
    template: &str,
    variables: &Map<String, Value>,
    untrusted_vars: Option<&HashSet<String>>,
) -> WorkflowResult<String> {
    let re = Regex::new(r"\{\{(\s*[\w][\w.]*\s*)\}\}").unwrap();

    let mut result = String::with_capacity(template.len());
    let mut last_end = 0;

    for cap in re.captures_iter(template) {
        let full_match = cap.get(0).unwrap();
        result.push_str(&template[last_end..full_match.start()]);

        let path = cap[1].trim();
        let root_key = path.split('.').next().unwrap();

        let root_val = variables
            .get(root_key)
            .ok_or_else(|| WorkflowError::Expression(format!("variable '{path}' not found")))?;

        // If path has dots, navigate deeper; otherwise use root directly
        let value = if path.contains('.') {
            let rest = &path[root_key.len() + 1..];
            resolve_path(root_val, rest)
                .ok_or_else(|| WorkflowError::Expression(format!("variable '{path}' not found")))?
        } else {
            root_val
        };

        let text = value_to_string(value);
        let is_untrusted = untrusted_vars.map(|vars| vars.contains(root_key)).unwrap_or(false);
        if is_untrusted {
            let safe = escape_prompt_tags(&text);
            result.push_str(&format!(
                "<external_data source=\"workflow_var:{root_key}\">{safe}</external_data>"
            ));
        } else {
            result.push_str(&text);
        }
        last_end = full_match.end();
    }

    result.push_str(&template[last_end..]);
    Ok(result)
}

/// Resolve a template and return the underlying JSON value when possible.
///
/// If the entire template is a single `{{var}}` reference, the original JSON
/// value is returned with its type preserved (e.g. an array stays an array).
/// If the template mixes literal text with template expressions, the fully
/// resolved string is returned as a `Value::String`.
pub fn resolve_value(template: &str, variables: &Map<String, Value>) -> WorkflowResult<Value> {
    let re = Regex::new(r"^\{\{(\s*[\w][\w.]*\s*)\}\}$").unwrap();

    // Fast path: the entire template is a single {{var}} reference
    if let Some(cap) = re.captures(template) {
        let path = cap[1].trim();
        let root_key = path.split('.').next().unwrap();

        let root_val = variables
            .get(root_key)
            .ok_or_else(|| WorkflowError::Expression(format!("variable '{path}' not found")))?;

        let value = if path.contains('.') {
            let rest = &path[root_key.len() + 1..];
            resolve_path(root_val, rest)
                .ok_or_else(|| WorkflowError::Expression(format!("variable '{path}' not found")))?
        } else {
            root_val
        };

        return Ok(value.clone());
    }

    // Slow path: mixed template — resolve to string
    let resolved = resolve_template(template, variables, None)?;
    Ok(Value::String(resolved))
}

/// Evaluate a condition string to a boolean.
///
/// Template expressions inside the condition are resolved first, then the
/// resulting string is interpreted as a boolean using these rules:
/// - `"true"` → true, `"false"` → false
/// - `"0"`, `"null"`, `"none"`, `""` → false
/// - Any other non-empty string → true
pub fn evaluate_condition(condition: &str, variables: &Map<String, Value>) -> WorkflowResult<bool> {
    // Resolve the condition — if it's a single {{var}}, get the raw value
    let value = resolve_value(condition, variables)?;
    Ok(value_is_truthy(&value))
}

/// Determine whether a JSON value is "truthy".
fn value_is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => {
            // 0 (int or float) is falsy
            if let Some(i) = n.as_i64() {
                i != 0
            } else if let Some(f) = n.as_f64() {
                f != 0.0
            } else {
                true
            }
        }
        Value::String(s) => string_is_truthy(s),
        Value::Array(arr) => !arr.is_empty(),
        Value::Object(map) => !map.is_empty(),
    }
}

/// Determine whether a plain string is "truthy".
fn string_is_truthy(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let lower = s.trim().to_lowercase();
    !matches!(lower.as_str(), "false" | "0" | "null" | "none")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_vars(value: Value) -> Map<String, Value> {
        match value {
            Value::Object(map) => map,
            _ => panic!("expected object"),
        }
    }

    // ---- resolve_template tests ----

    #[test]
    fn simple_variable_substitution() {
        let vars = make_vars(json!({"name": "Alice"}));
        let result = resolve_template("Hello {{name}}!", &vars, None).unwrap();
        assert_eq!(result, "Hello Alice!");
    }

    #[test]
    fn literal_without_templates() {
        let vars = make_vars(json!({}));
        let result = resolve_template("no templates here", &vars, None).unwrap();
        assert_eq!(result, "no templates here");
    }

    #[test]
    fn multiple_templates_in_one_string() {
        let vars = make_vars(json!({"name": "Alice", "count": 5}));
        let result =
            resolve_template("Hello {{name}}, you have {{count}} messages", &vars, None).unwrap();
        assert_eq!(result, "Hello Alice, you have 5 messages");
    }

    #[test]
    fn dot_notation_navigation() {
        let vars = make_vars(json!({
            "response": {
                "content": "world"
            }
        }));
        let result = resolve_template("Hello {{response.content}}", &vars, None).unwrap();
        assert_eq!(result, "Hello world");
    }

    #[test]
    fn array_indexing() {
        let vars = make_vars(json!({
            "tool_calls": [
                {"name": "read_file"},
                {"name": "write_file"}
            ]
        }));
        let result = resolve_template("{{tool_calls.0.name}}", &vars, None).unwrap();
        assert_eq!(result, "read_file");

        let result = resolve_template("{{tool_calls.1.name}}", &vars, None).unwrap();
        assert_eq!(result, "write_file");
    }

    #[test]
    fn missing_variable_error() {
        let vars = make_vars(json!({"name": "Alice"}));
        let err = resolve_template("Hello {{missing}}", &vars, None).unwrap_err();
        assert!(matches!(err, WorkflowError::Expression(_)));
        assert!(err.to_string().contains("missing"));
    }

    #[test]
    fn missing_nested_path_error() {
        let vars = make_vars(json!({"response": {"a": 1}}));
        let err = resolve_template("{{response.nonexistent}}", &vars, None).unwrap_err();
        assert!(matches!(err, WorkflowError::Expression(_)));
    }

    #[test]
    fn object_value_serialized_to_json() {
        let vars = make_vars(json!({"data": {"key": "value"}}));
        let result = resolve_template("{{data}}", &vars, None).unwrap();
        assert_eq!(result, r#"{"key":"value"}"#);
    }

    #[test]
    fn boolean_value_to_string() {
        let vars = make_vars(json!({"flag": true}));
        let result = resolve_template("flag is {{flag}}", &vars, None).unwrap();
        assert_eq!(result, "flag is true");
    }

    #[test]
    fn whitespace_in_braces() {
        let vars = make_vars(json!({"name": "Alice"}));
        let result = resolve_template("{{ name }}", &vars, None).unwrap();
        assert_eq!(result, "Alice");
    }

    // ---- resolve_value tests ----

    #[test]
    fn resolve_value_preserves_array() {
        let vars = make_vars(json!({
            "tool_calls": [1, 2, 3]
        }));
        let result = resolve_value("{{tool_calls}}", &vars).unwrap();
        assert_eq!(result, json!([1, 2, 3]));
        assert!(result.is_array());
    }

    #[test]
    fn resolve_value_preserves_object() {
        let vars = make_vars(json!({
            "config": {"debug": true}
        }));
        let result = resolve_value("{{config}}", &vars).unwrap();
        assert_eq!(result, json!({"debug": true}));
    }

    #[test]
    fn resolve_value_preserves_number() {
        let vars = make_vars(json!({"count": 42}));
        let result = resolve_value("{{count}}", &vars).unwrap();
        assert_eq!(result, json!(42));
    }

    #[test]
    fn resolve_value_preserves_bool() {
        let vars = make_vars(json!({"flag": false}));
        let result = resolve_value("{{flag}}", &vars).unwrap();
        assert_eq!(result, json!(false));
    }

    #[test]
    fn resolve_value_mixed_returns_string() {
        let vars = make_vars(json!({"name": "Alice"}));
        let result = resolve_value("Hello {{name}}", &vars).unwrap();
        assert_eq!(result, Value::String("Hello Alice".to_string()));
    }

    #[test]
    fn resolve_value_plain_literal() {
        let vars = make_vars(json!({}));
        let result = resolve_value("just a literal", &vars).unwrap();
        assert_eq!(result, Value::String("just a literal".to_string()));
    }

    // ---- evaluate_condition tests ----

    #[test]
    fn condition_true_string() {
        let vars = make_vars(json!({"flag": "true"}));
        assert!(evaluate_condition("{{flag}}", &vars).unwrap());
    }

    #[test]
    fn condition_false_string() {
        let vars = make_vars(json!({"flag": "false"}));
        assert!(!evaluate_condition("{{flag}}", &vars).unwrap());
    }

    #[test]
    fn condition_bool_true() {
        let vars = make_vars(json!({"flag": true}));
        assert!(evaluate_condition("{{flag}}", &vars).unwrap());
    }

    #[test]
    fn condition_bool_false() {
        let vars = make_vars(json!({"flag": false}));
        assert!(!evaluate_condition("{{flag}}", &vars).unwrap());
    }

    #[test]
    fn condition_zero_is_falsy() {
        let vars = make_vars(json!({"val": 0}));
        assert!(!evaluate_condition("{{val}}", &vars).unwrap());
    }

    #[test]
    fn condition_nonzero_is_truthy() {
        let vars = make_vars(json!({"val": 42}));
        assert!(evaluate_condition("{{val}}", &vars).unwrap());
    }

    #[test]
    fn condition_null_is_falsy() {
        let vars = make_vars(json!({"val": null}));
        assert!(!evaluate_condition("{{val}}", &vars).unwrap());
    }

    #[test]
    fn condition_empty_array_is_falsy() {
        let vars = make_vars(json!({"arr": []}));
        assert!(!evaluate_condition("{{arr}}", &vars).unwrap());
    }

    #[test]
    fn condition_nonempty_array_is_truthy() {
        let vars = make_vars(json!({"arr": [1]}));
        assert!(evaluate_condition("{{arr}}", &vars).unwrap());
    }

    #[test]
    fn condition_empty_object_is_falsy() {
        let vars = make_vars(json!({"obj": {}}));
        assert!(!evaluate_condition("{{obj}}", &vars).unwrap());
    }

    #[test]
    fn condition_nonempty_object_is_truthy() {
        let vars = make_vars(json!({"obj": {"a": 1}}));
        assert!(evaluate_condition("{{obj}}", &vars).unwrap());
    }

    #[test]
    fn condition_string_none_is_falsy() {
        let vars = make_vars(json!({"val": "none"}));
        assert!(!evaluate_condition("{{val}}", &vars).unwrap());
    }

    #[test]
    fn condition_string_null_is_falsy() {
        let vars = make_vars(json!({"val": "null"}));
        assert!(!evaluate_condition("{{val}}", &vars).unwrap());
    }

    #[test]
    fn condition_nonempty_string_is_truthy() {
        let vars = make_vars(json!({"val": "hello"}));
        assert!(evaluate_condition("{{val}}", &vars).unwrap());
    }

    #[test]
    fn condition_nested_bool() {
        let vars = make_vars(json!({
            "response": {"has_tool_calls": true}
        }));
        assert!(evaluate_condition("{{response.has_tool_calls}}", &vars).unwrap());
    }
}
