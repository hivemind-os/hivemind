use crate::error::WorkflowError;
use hive_contracts::prompt_sanitize::escape_prompt_tags;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// Resolve all `{{...}}` template expressions in a string against the given context.
///
/// Supported paths:
/// - `variables.foo.bar`    → lookup in variables bag
/// - `steps.step_id.outputs.field` → lookup in step outputs
/// - `trigger.field`        → lookup in trigger data
/// - `error`                → the current step error string
/// - `result`               → shorthand for the current step result
/// - `result.field`         → field within current step result
pub fn resolve_template(template: &str, ctx: &ExpressionContext) -> Result<String, WorkflowError> {
    resolve_template_inner(template, ctx, false)
}

/// Like [`resolve_template`], but wraps values whose root variable is in
/// `ctx.untrusted_vars` with `<external_data>` trust-boundary markers.
///
/// Use this **only** when the resolved text will be sent as a prompt to a
/// language model.  All other internal workflow logic (branch conditions,
/// variable assignments, tool arguments, etc.) should use [`resolve_template`].
pub fn resolve_template_for_prompt(
    template: &str,
    ctx: &ExpressionContext,
) -> Result<String, WorkflowError> {
    resolve_template_inner(template, ctx, true)
}

fn resolve_template_inner(
    template: &str,
    ctx: &ExpressionContext,
    wrap_untrusted: bool,
) -> Result<String, WorkflowError> {
    let mut result = String::with_capacity(template.len());
    let mut rest = template;

    while let Some(start) = rest.find("{{") {
        result.push_str(&rest[..start]);
        let after_open = &rest[start + 2..];
        let end = after_open
            .find("}}")
            .ok_or_else(|| WorkflowError::Expression("Unclosed {{ expression".into()))?;
        let expr = after_open[..end].trim();
        let value = resolve_path(expr, ctx)?;
        let text = value_to_string(&value);
        let root_var = expr.split('.').next().unwrap_or(expr);
        if wrap_untrusted && ctx.untrusted_vars.contains(root_var) {
            let safe = escape_prompt_tags(&text);
            result.push_str(&format!(
                "<external_data source=\"workflow_var:{root_var}\">{safe}</external_data>"
            ));
        } else {
            result.push_str(&text);
        }
        rest = &after_open[end + 2..];
    }
    result.push_str(rest);
    Ok(result)
}

/// Resolve a single dot-path expression to a JSON value.
pub fn resolve_path(path: &str, ctx: &ExpressionContext) -> Result<Value, WorkflowError> {
    let parts: Vec<&str> = path.split('.').collect();
    if parts.is_empty() {
        return Err(WorkflowError::Expression("Empty expression path".into()));
    }

    match parts[0] {
        "variables" => drill_into(&ctx.variables, &parts[1..]),
        "steps" => {
            if parts.len() < 3 {
                return Err(WorkflowError::Expression(format!(
                    "Steps path needs at least steps.<id>.<field>: {path}"
                )));
            }
            let step_id = parts[1];
            let step_outputs = ctx.step_outputs.get(step_id).cloned().unwrap_or(Value::Null);
            // steps.<id>.outputs.<field> or steps.<id>.outputs
            if parts[2] == "outputs" {
                drill_into(&step_outputs, &parts[3..])
            } else {
                Err(WorkflowError::Expression(format!(
                    "Unknown step field '{}', expected 'outputs'",
                    parts[2]
                )))
            }
        }
        "trigger" => drill_into(&ctx.trigger_data, &parts[1..]),
        "event" => drill_into(&ctx.trigger_data, &parts[1..]),
        "result" => drill_into(&ctx.current_result, &parts[1..]),
        "error" => Ok(Value::String(ctx.current_error.clone().unwrap_or_default())),
        _ => {
            // Try as a bare variable name
            drill_into(&ctx.variables, &parts)
        }
    }
}

/// Evaluate a condition expression and return a boolean.
///
/// Supports:
/// - Simple template truthiness: `{{variables.flag}}`
/// - Comparison: `{{a}} == {{b}}`, `{{a}} != {{b}}`, `<`, `>`, `<=`, `>=`
/// - Logical: `expr && expr`, `expr || expr`, `!expr`
/// - Literals: `true`, `false`, `null`, quoted strings, numbers
pub fn evaluate_condition(condition: &str, ctx: &ExpressionContext) -> Result<bool, WorkflowError> {
    let trimmed = condition.trim();

    // Try logical operators on the raw string first.
    // split_logical_op already skips {{ }} depth, so operators inside
    // template expressions are not matched.  Template resolution happens
    // at leaf level inside eval_sub_condition, which avoids both
    // double-resolution (injection) and resolved values containing
    // operator characters corrupting the parse.
    if let Some(result) = try_logical_or(trimmed, ctx)? {
        return Ok(result);
    }

    // No logical operators — resolve templates now and evaluate
    let resolved = resolve_template(condition, ctx)?;
    let trimmed = resolved.trim();

    // Try comparison operators
    if let Some(result) = try_comparison(trimmed)? {
        return Ok(result);
    }

    // Fall back to truthiness
    Ok(is_truthy(trimmed))
}

/// Resolve a map of expressions: key -> expression_template.
/// Returns the resolved values.
pub fn resolve_output_map(
    mappings: &HashMap<String, String>,
    ctx: &ExpressionContext,
) -> Result<Value, WorkflowError> {
    let mut map = serde_json::Map::new();
    for (key, expr) in mappings {
        // If expression is a pure template (single {{...}}), preserve the JSON type
        if is_pure_template(expr) {
            let path = expr.trim().trim_start_matches("{{").trim_end_matches("}}").trim();
            let value = resolve_path(path, ctx)?;
            map.insert(key.clone(), value);
        } else {
            let resolved = resolve_template(expr, ctx)?;
            map.insert(key.clone(), Value::String(resolved));
        }
    }
    Ok(Value::Object(map))
}

// ---------------------------------------------------------------------------
// Expression context
// ---------------------------------------------------------------------------

/// All data available for expression resolution within a step.
#[derive(Debug, Clone, Default)]
pub struct ExpressionContext {
    /// The workflow's variable bag
    pub variables: Value,
    /// Per-step outputs: step_id -> outputs JSON
    pub step_outputs: HashMap<String, Value>,
    /// Data from the trigger that started this workflow
    pub trigger_data: Value,
    /// Result of the current step execution (set by executor before resolving outputs)
    pub current_result: Value,
    /// Error string from current step (if failed)
    pub current_error: Option<String>,
    /// Top-level expression roots that originate from external/untrusted sources.
    /// When a resolved expression's root is in this set, its value is escaped
    /// and wrapped in `<external_data>` trust boundary markers.
    pub untrusted_vars: HashSet<String>,
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn drill_into(value: &Value, path: &[&str]) -> Result<Value, WorkflowError> {
    let mut current = value.clone();
    for &segment in path {
        current = match current {
            Value::Object(ref map) => map.get(segment).cloned().unwrap_or(Value::Null),
            Value::Array(ref arr) => {
                if let Ok(idx) = segment.parse::<usize>() {
                    arr.get(idx).cloned().unwrap_or(Value::Null)
                } else {
                    Value::Null
                }
            }
            _ => Value::Null,
        };
    }
    Ok(current)
}

fn value_to_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        _ => serde_json::to_string(value).unwrap_or_default(),
    }
}

fn is_truthy(s: &str) -> bool {
    if matches!(s, "" | "null" | "false") {
        return false;
    }
    // Catch all numeric zero representations (0, 0.0, -0, 0.00, etc.)
    if let Ok(n) = s.parse::<f64>() {
        return n != 0.0;
    }
    true
}

pub fn is_pure_template(s: &str) -> bool {
    let t = s.trim();
    t.starts_with("{{")
        && t.ends_with("}}")
        && t.matches("{{").count() == 1
        && t.matches("}}").count() == 1
}

/// Try to parse `expr` as `left || right` (short-circuit OR).
fn try_logical_or(expr: &str, ctx: &ExpressionContext) -> Result<Option<bool>, WorkflowError> {
    // Split on || that is not inside {{ }}
    if let Some((left, right)) = split_logical_op(expr, "||") {
        let l = eval_sub_condition(left.trim(), ctx)?;
        if l {
            return Ok(Some(true));
        }
        let r = eval_sub_condition(right.trim(), ctx)?;
        return Ok(Some(r));
    }
    // Try AND
    if let Some((left, right)) = split_logical_op(expr, "&&") {
        let l = eval_sub_condition(left.trim(), ctx)?;
        if !l {
            return Ok(Some(false));
        }
        let r = eval_sub_condition(right.trim(), ctx)?;
        return Ok(Some(r));
    }
    // Try NOT (but not != comparison)
    let trimmed = expr.trim();
    if let Some(inner) = trimmed.strip_prefix('!') {
        if !inner.starts_with('=') {
            let v = eval_sub_condition(inner.trim(), ctx)?;
            return Ok(Some(!v));
        }
    }
    Ok(None)
}

fn eval_sub_condition(s: &str, ctx: &ExpressionContext) -> Result<bool, WorkflowError> {
    // Recursively handle nested logical operators (&&, ||, !)
    // on the raw/partially-raw string before resolving templates.
    if let Some(result) = try_logical_or(s, ctx)? {
        return Ok(result);
    }

    // Leaf: resolve templates once, then compare/truthiness
    let resolved = resolve_template(s, ctx)?;
    let trimmed = resolved.trim();

    if let Some(result) = try_comparison(trimmed)? {
        Ok(result)
    } else {
        Ok(is_truthy(trimmed))
    }
}

fn split_logical_op<'a>(expr: &'a str, op: &str) -> Option<(&'a str, &'a str)> {
    let mut depth = 0i32;
    let mut in_quotes = false;
    let mut quote_char = 0u8;
    let bytes = expr.as_bytes();
    let op_bytes = op.as_bytes();
    let op_len = op_bytes.len();

    let mut i = 0;
    while i < bytes.len() {
        // Double-quote tracking
        if bytes[i] == b'"' && depth == 0 {
            if !in_quotes {
                in_quotes = true;
                quote_char = b'"';
            } else if quote_char == b'"' {
                in_quotes = false;
            }
            i += 1;
            continue;
        }
        // Single-quote tracking (only at word boundaries to avoid apostrophes)
        if bytes[i] == b'\'' && depth == 0 {
            if !in_quotes {
                let prev_is_boundary = i == 0
                    || matches!(bytes[i - 1], b' ' | b'=' | b'!' | b'<' | b'>' | b'|' | b'&');
                if prev_is_boundary {
                    in_quotes = true;
                    quote_char = b'\'';
                }
            } else if quote_char == b'\'' {
                in_quotes = false;
            }
            i += 1;
            continue;
        }
        if !in_quotes {
            if bytes[i] == b'{' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
                depth += 1;
                i += 2;
                continue;
            }
            if bytes[i] == b'}' && i + 1 < bytes.len() && bytes[i + 1] == b'}' {
                depth -= 1;
                i += 2;
                continue;
            }
            if depth == 0 && i + op_len <= bytes.len() && &bytes[i..i + op_len] == op_bytes {
                return Some((&expr[..i], &expr[i + op_len..]));
            }
        }
        i += 1;
    }
    None
}

/// Try comparison operators: ==, !=, <=, >=, <, >
fn try_comparison(expr: &str) -> Result<Option<bool>, WorkflowError> {
    // Order matters: check two-char ops before single-char
    for op in &["==", "!=", "<=", ">=", "<", ">"] {
        if let Some((left, right)) = split_comparison(expr, op) {
            let l = left.trim();
            let r = right.trim();
            let result = compare_values(l, r, op);
            return Ok(Some(result));
        }
    }
    Ok(None)
}

fn split_comparison<'a>(expr: &'a str, op: &str) -> Option<(&'a str, &'a str)> {
    // Don't confuse `==` with `<=` or `>=` or `!=`
    let mut i = 0;
    let mut in_quotes = false;
    let mut quote_char = 0u8;
    let mut depth = 0i32; // template brace depth
    let bytes = expr.as_bytes();
    let op_bytes = op.as_bytes();
    let op_len = op_bytes.len();

    while i + op_len <= bytes.len() {
        // Double-quote tracking
        if bytes[i] == b'"' && depth == 0 {
            if !in_quotes {
                in_quotes = true;
                quote_char = b'"';
            } else if quote_char == b'"' {
                in_quotes = false;
            }
            i += 1;
            continue;
        }
        // Single-quote tracking (only at word boundaries to avoid apostrophes)
        if bytes[i] == b'\'' && depth == 0 {
            if !in_quotes {
                let prev_is_boundary = i == 0
                    || matches!(bytes[i - 1], b' ' | b'=' | b'!' | b'<' | b'>' | b'|' | b'&');
                if prev_is_boundary {
                    in_quotes = true;
                    quote_char = b'\'';
                }
            } else if quote_char == b'\'' {
                in_quotes = false;
            }
            i += 1;
            continue;
        }
        if !in_quotes {
            if bytes[i] == b'{' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
                depth += 1;
                i += 2;
                continue;
            }
            if bytes[i] == b'}' && i + 1 < bytes.len() && bytes[i + 1] == b'}' {
                depth -= 1;
                i += 2;
                continue;
            }
        }
        if !in_quotes && depth == 0 && &bytes[i..i + op_len] == op_bytes {
            // For single-char `<` or `>`, make sure it's not part of `<=`, `>=`, `!=`, `==`
            if op_len == 1 && i + 1 < bytes.len() && bytes[i + 1] == b'=' {
                i += 1;
                continue;
            }
            return Some((&expr[..i], &expr[i + op_len..]));
        }
        i += 1;
    }
    None
}

/// Approximate float equality using a relative epsilon.
fn float_approx_eq(a: f64, b: f64) -> bool {
    if a == b {
        return true; // handles infinities and exact matches
    }
    let diff = (a - b).abs();
    let largest = a.abs().max(b.abs());
    diff <= largest * 1e-10
}

fn compare_values(left: &str, right: &str, op: &str) -> bool {
    // Try numeric comparison first
    if let (Ok(l), Ok(r)) = (left.parse::<f64>(), right.parse::<f64>()) {
        return match op {
            "==" => float_approx_eq(l, r),
            "!=" => !float_approx_eq(l, r),
            "<" => l < r,
            ">" => l > r,
            "<=" => l <= r,
            ">=" => l >= r,
            _ => false,
        };
    }

    // String comparison (strip quotes if present)
    let l = strip_quotes(left);
    let r = strip_quotes(right);
    match op {
        "==" => l == r,
        "!=" => l != r,
        "<" => l < r,
        ">" => l > r,
        "<=" => l <= r,
        ">=" => l >= r,
        _ => false,
    }
}

fn strip_quotes(s: &str) -> &str {
    let t = s.trim();
    if t.len() >= 2
        && ((t.starts_with('"') && t.ends_with('"')) || (t.starts_with('\'') && t.ends_with('\'')))
    {
        &t[1..t.len() - 1]
    } else {
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_ctx() -> ExpressionContext {
        let mut step_outputs = HashMap::new();
        step_outputs.insert(
            "step1".to_string(),
            serde_json::json!({
                "name": "Alice",
                "count": 42,
                "valid": true
            }),
        );
        step_outputs.insert(
            "step2".to_string(),
            serde_json::json!({
                "result": "ok"
            }),
        );

        ExpressionContext {
            variables: serde_json::json!({
                "approved": false,
                "amount": 1500,
                "name": "Test Workflow",
                "nested": { "deep": { "value": "found" } }
            }),
            step_outputs,
            trigger_data: serde_json::json!({
                "request_title": "Buy supplies",
                "amount": 2000
            }),
            current_result: serde_json::json!({
                "status": "success",
                "data": { "id": 123 }
            }),
            current_error: None,
            untrusted_vars: HashSet::new(),
        }
    }

    #[test]
    fn test_simple_variable_template() {
        let ctx = make_ctx();
        let result = resolve_template("Hello {{variables.name}}", &ctx).unwrap();
        assert_eq!(result, "Hello Test Workflow");
    }

    #[test]
    fn test_nested_variable_path() {
        let ctx = make_ctx();
        let result = resolve_template("{{variables.nested.deep.value}}", &ctx).unwrap();
        assert_eq!(result, "found");
    }

    #[test]
    fn test_step_output_reference() {
        let ctx = make_ctx();
        let result = resolve_template("Name: {{steps.step1.outputs.name}}", &ctx).unwrap();
        assert_eq!(result, "Name: Alice");
    }

    #[test]
    fn test_trigger_reference() {
        let ctx = make_ctx();
        let result = resolve_template("Title: {{trigger.request_title}}", &ctx).unwrap();
        assert_eq!(result, "Title: Buy supplies");
    }

    #[test]
    fn test_result_reference() {
        let ctx = make_ctx();
        let result = resolve_template("{{result.status}}", &ctx).unwrap();
        assert_eq!(result, "success");
    }

    #[test]
    fn test_multiple_templates() {
        let ctx = make_ctx();
        let result =
            resolve_template("{{trigger.request_title}} costs ${{trigger.amount}}", &ctx).unwrap();
        assert_eq!(result, "Buy supplies costs $2000");
    }

    #[test]
    fn test_null_path_returns_empty() {
        let ctx = make_ctx();
        let result = resolve_template("{{variables.nonexistent}}", &ctx).unwrap();
        assert_eq!(result, "");
    }

    #[test]
    fn test_no_templates() {
        let ctx = make_ctx();
        let result = resolve_template("plain text", &ctx).unwrap();
        assert_eq!(result, "plain text");
    }

    #[test]
    fn test_unclosed_template() {
        let ctx = make_ctx();
        assert!(resolve_template("{{variables.name", &ctx).is_err());
    }

    // Condition evaluation tests

    #[test]
    fn test_truthiness_true() {
        let ctx = make_ctx();
        assert!(evaluate_condition("{{variables.amount}}", &ctx).unwrap());
    }

    #[test]
    fn test_truthiness_false() {
        let ctx = make_ctx();
        assert!(!evaluate_condition("{{variables.approved}}", &ctx).unwrap());
    }

    #[test]
    fn test_numeric_comparison_gt() {
        let ctx = make_ctx();
        assert!(evaluate_condition("{{variables.amount}} > 1000", &ctx).unwrap());
    }

    #[test]
    fn test_numeric_comparison_lt() {
        let ctx = make_ctx();
        assert!(!evaluate_condition("{{variables.amount}} < 1000", &ctx).unwrap());
    }

    #[test]
    fn test_equality() {
        let ctx = make_ctx();
        assert!(evaluate_condition("{{steps.step1.outputs.name}} == Alice", &ctx).unwrap());
    }

    #[test]
    fn test_inequality() {
        let ctx = make_ctx();
        assert!(evaluate_condition("{{steps.step1.outputs.name}} != Bob", &ctx).unwrap());
    }

    #[test]
    fn test_logical_and() {
        let ctx = make_ctx();
        assert!(evaluate_condition(
            "{{variables.amount}} > 1000 && {{steps.step1.outputs.valid}}",
            &ctx
        )
        .unwrap());
    }

    #[test]
    fn test_logical_or() {
        let ctx = make_ctx();
        assert!(evaluate_condition("{{variables.approved}} || {{variables.amount}} > 1000", &ctx)
            .unwrap());
    }

    #[test]
    fn test_logical_not() {
        let ctx = make_ctx();
        assert!(evaluate_condition("!{{variables.approved}}", &ctx).unwrap());
    }

    #[test]
    fn test_pure_template_preserves_type() {
        let ctx = make_ctx();
        let mappings = HashMap::from([
            ("count".to_string(), "{{steps.step1.outputs.count}}".to_string()),
            ("label".to_string(), "Step: {{steps.step1.outputs.name}}".to_string()),
        ]);
        let result = resolve_output_map(&mappings, &ctx).unwrap();
        assert_eq!(result["count"], serde_json::json!(42));
        assert_eq!(result["label"], serde_json::json!("Step: Alice"));
    }

    #[test]
    fn test_is_pure_template_rejects_trailing_braces() {
        assert!(is_pure_template("{{foo}}"));
        assert!(is_pure_template("  {{foo}}  "));
        assert!(!is_pure_template("{{foo}} extra }}"));
        assert!(!is_pure_template("{{foo}}bar}}"));
        assert!(!is_pure_template("{{a}} {{b}}"));
        assert!(!is_pure_template("hello {{foo}}"));
        assert!(!is_pure_template("{{foo}} world"));
    }

    #[test]
    fn test_resolve_arguments_from_yaml_roundtrip() {
        // Simulate YAML that the UI produces with single-quoted template strings
        let yaml = r#"
kind: call_tool
tool_id: comms.send_message
arguments:
  body: 'Hello {{trigger.request_title}}, your order {{steps.step1.outputs.count}} is ready'
  connector_id: test-conn
"#;
        let task: crate::types::TaskDef = serde_yaml::from_str(yaml).unwrap();
        if let crate::types::TaskDef::CallTool { arguments, .. } = &task {
            assert_eq!(
                arguments["body"],
                "Hello {{trigger.request_title}}, your order {{steps.step1.outputs.count}} is ready"
            );

            let ctx = make_ctx();
            let resolved = crate::executor::resolve_arguments(arguments, &ctx).unwrap();
            assert_eq!(
                resolved["body"],
                serde_json::json!("Hello Buy supplies, your order 42 is ready")
            );
            assert_eq!(resolved["connector_id"], serde_json::json!("test-conn"));
        } else {
            panic!("expected CallTool");
        }
    }

    #[test]
    fn test_quoted_string_comparison() {
        let ctx = make_ctx();
        assert!(evaluate_condition("{{steps.step1.outputs.name}} == \"Alice\"", &ctx).unwrap());
    }

    #[test]
    fn test_quoted_string_with_spaces() {
        let mut ctx = make_ctx();
        ctx.step_outputs.insert("greeting".to_string(), serde_json::json!("Hello World"));
        assert!(evaluate_condition("{{steps.greeting.outputs}} == \"Hello World\"", &ctx).unwrap());
    }

    #[test]
    fn test_quoted_string_protects_operators_in_rhs() {
        let ctx = make_ctx();
        // Quotes on the RHS protect the operator characters from being parsed
        assert!(!evaluate_condition("{{steps.step1.outputs.name}} == \"a && b\"", &ctx).unwrap());
        // Without quotes, "Alice == a" would be truthy split on &&
        // With quotes, it compares "Alice" to "a && b" → false
    }

    // Chained logical operator tests (3+ operands)

    #[test]
    fn test_chained_and_all_true() {
        let ctx = make_ctx();
        // amount=1500 > 1000, step1.count=42 > 10, step2.result="ok" != ""
        assert!(evaluate_condition(
            "{{variables.amount}} > 1000 && {{steps.step1.outputs.count}} > 10 && {{steps.step2.outputs.result}} == ok",
            &ctx
        ).unwrap());
    }

    #[test]
    fn test_chained_and_last_false() {
        let ctx = make_ctx();
        assert!(!evaluate_condition(
            "{{variables.amount}} > 1000 && {{steps.step1.outputs.count}} > 10 && {{variables.approved}}",
            &ctx
        ).unwrap());
    }

    #[test]
    fn test_chained_or_all_false() {
        let ctx = make_ctx();
        // approved=false, amount < 10000 is false (amount=1500), nonexistent="" is falsy
        assert!(!evaluate_condition(
            "{{variables.approved}} || {{variables.amount}} > 10000 || {{variables.nonexistent}}",
            &ctx
        )
        .unwrap());
    }

    #[test]
    fn test_chained_or_last_true() {
        let ctx = make_ctx();
        assert!(evaluate_condition(
            "{{variables.approved}} || {{variables.amount}} > 10000 || {{variables.amount}} > 1000",
            &ctx
        )
        .unwrap());
    }

    #[test]
    fn test_mixed_and_or_three_operands() {
        let ctx = make_ctx();
        // OR has lower precedence (split first), so this is: (approved) || (amount > 1000 && count > 10)
        // = false || (true && true) = true
        assert!(evaluate_condition(
            "{{variables.approved}} || {{variables.amount}} > 1000 && {{steps.step1.outputs.count}} > 10",
            &ctx
        ).unwrap());
    }

    // is_truthy numeric zero consistency tests

    #[test]
    fn test_is_truthy_negative_zero() {
        assert!(!is_truthy("-0"));
        assert!(!is_truthy("-0.0"));
    }

    #[test]
    fn test_is_truthy_zero_variants() {
        assert!(!is_truthy("0"));
        assert!(!is_truthy("0.0"));
        assert!(!is_truthy("0.00"));
        assert!(!is_truthy("00"));
    }

    #[test]
    fn test_is_truthy_nonzero_numbers() {
        assert!(is_truthy("1"));
        assert!(is_truthy("-1"));
        assert!(is_truthy("0.1"));
        assert!(is_truthy("42"));
    }

    #[test]
    fn test_while_condition_empty_ne_string() {
        // Simulates the while condition in software-feature workflow:
        // plan_approved defaults to "" and condition is "{{variables.plan_approved}} != Approve"
        let ctx = ExpressionContext {
            variables: serde_json::json!({
                "plan_approved": ""
            }),
            step_outputs: HashMap::new(),
            trigger_data: Value::Null,
            current_result: Value::Null,
            current_error: None,
            untrusted_vars: HashSet::new(),
        };

        // The resolved condition should be " != Approve" (empty string + " != Approve")
        let resolved = resolve_template("{{variables.plan_approved}} != Approve", &ctx).unwrap();
        eprintln!("Resolved condition: '{}'", resolved);

        let result = evaluate_condition("{{variables.plan_approved}} != Approve", &ctx).unwrap();
        eprintln!("Condition result: {}", result);
        assert!(result, "Empty string != 'Approve' should be true");
    }

    #[test]
    fn test_strip_quotes_single_char_no_panic() {
        // A lone quote character must not panic (was: &t[1..0] out of bounds)
        assert_eq!(strip_quotes("\""), "\"");
        assert_eq!(strip_quotes("'"), "'");
    }

    #[test]
    fn test_strip_quotes_empty_and_minimal() {
        assert_eq!(strip_quotes(""), "");
        assert_eq!(strip_quotes("\"\""), "");
        assert_eq!(strip_quotes("''"), "");
        assert_eq!(strip_quotes("\"hello\""), "hello");
        assert_eq!(strip_quotes("'hello'"), "hello");
        assert_eq!(strip_quotes("hello"), "hello");
    }

    #[test]
    fn test_no_double_resolution_injection() {
        // A variable whose value contains {{...}} must NOT be expanded
        // when logical operators cause eval_sub_condition to be called.
        let ctx = ExpressionContext {
            variables: serde_json::json!({
                "user_input": "{{trigger.amount}}",
                "flag": true
            }),
            step_outputs: HashMap::new(),
            trigger_data: serde_json::json!({ "amount": 9999 }),
            current_result: Value::Null,
            current_error: None,
            untrusted_vars: HashSet::new(),
        };

        // Without logical ops
        let r1 = evaluate_condition("{{variables.user_input}} == 9999", &ctx).unwrap();
        // With logical ops (previously triggered double-resolution)
        let r2 = evaluate_condition("{{variables.flag}} && {{variables.user_input}} == 9999", &ctx)
            .unwrap();

        // Both should be false — the literal "{{trigger.amount}}" != "9999"
        assert!(!r1, "Literal template syntax must not match resolved value");
        assert!(!r2, "Logical operators must not cause double-resolution");
    }

    #[test]
    fn test_resolved_value_with_operators_not_corrupted() {
        // A resolved value containing && must not corrupt logical parsing
        let ctx = ExpressionContext {
            variables: serde_json::json!({
                "status": "ready && waiting"
            }),
            step_outputs: HashMap::new(),
            trigger_data: Value::Null,
            current_result: Value::Null,
            current_error: None,
            untrusted_vars: HashSet::new(),
        };

        // "ready && waiting" compared with "ready && waiting" should be true
        let result =
            evaluate_condition("{{variables.status}} == \"ready && waiting\"", &ctx).unwrap();
        assert!(result, "Value with && should compare correctly when quoted");
    }

    #[test]
    fn test_single_quoted_string_protects_logical_ops() {
        let ctx = make_ctx();
        // Single-quoted string containing && must not be split
        assert!(
            !evaluate_condition("{{steps.step1.outputs.name}} == 'Alice && Bob'", &ctx).unwrap()
        );
    }

    #[test]
    fn test_single_quoted_string_protects_or_op() {
        let ctx = make_ctx();
        // Single-quoted string containing || must not be split
        assert!(
            !evaluate_condition("{{steps.step1.outputs.name}} == 'Alice || Bob'", &ctx).unwrap()
        );
    }

    #[test]
    fn test_apostrophe_in_word_not_treated_as_quote() {
        // "don't" contains an apostrophe that must NOT open a quote context
        let mut ctx = make_ctx();
        ctx.step_outputs.insert("review".to_string(), serde_json::json!("Skip (don't reply)"));
        assert!(evaluate_condition("{{steps.review.outputs}} == Skip (don't reply)", &ctx).unwrap());
    }

    #[test]
    fn test_double_quotes_inside_single_quotes() {
        let ctx = make_ctx();
        // Double quotes inside single quotes must not interfere
        assert!(!evaluate_condition("{{steps.step1.outputs.name}} == 'say \"hi\" && bye'", &ctx)
            .unwrap());
    }
}
