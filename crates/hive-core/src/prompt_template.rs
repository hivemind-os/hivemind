use anyhow::{anyhow, Result};
use handlebars::Handlebars;
use hive_contracts::PromptTemplate;
use serde_json::Value;

/// Render a [`PromptTemplate`] with the given parameter values.
///
/// The `params` JSON object is validated against the template's
/// `input_schema` (if present) before rendering.  Schema defaults are
/// merged into `params` for any missing optional fields.
pub fn render_prompt_template(template: &PromptTemplate, params: &Value) -> Result<String> {
    let params = apply_schema_defaults(template, params)?;
    let params = coerce_to_schema_types(template, params);
    validate_params(template, &params)?;

    let mut hbs = Handlebars::new();
    hbs.set_strict_mode(true);

    hbs.register_template_string(&template.id, &template.template)
        .map_err(|e| anyhow!("invalid handlebars template '{}': {e}", template.id))?;

    hbs.render(&template.id, &params)
        .map_err(|e| anyhow!("failed to render prompt template '{}': {e}", template.id))
}

/// Merge schema `default` values into `params` for any missing properties.
fn apply_schema_defaults(template: &PromptTemplate, params: &Value) -> Result<Value> {
    let mut merged = params.clone();

    let schema = match &template.input_schema {
        Some(s) => s,
        None => return Ok(merged),
    };

    let properties = match schema.get("properties") {
        Some(Value::Object(props)) => props,
        _ => return Ok(merged),
    };

    let obj =
        merged.as_object_mut().ok_or_else(|| anyhow!("prompt parameters must be a JSON object"))?;

    for (key, prop_schema) in properties {
        if !obj.contains_key(key) {
            if let Some(default_val) = prop_schema.get("default") {
                obj.insert(key.clone(), default_val.clone());
            }
        }
    }

    Ok(merged)
}

/// Coerce parameter values to match schema-declared types where possible.
/// E.g., if the schema says `"type": "string"` but the value is an object or
/// array, serialize it to a JSON string so that prompt templates don't fail on
/// type mismatches from workflow expression resolution.
fn coerce_to_schema_types(template: &PromptTemplate, mut params: Value) -> Value {
    let schema = match &template.input_schema {
        Some(s) => s,
        None => return params,
    };
    let properties = match schema.get("properties").and_then(|p| p.as_object()) {
        Some(props) => props,
        None => return params,
    };
    let obj = match params.as_object_mut() {
        Some(o) => o,
        None => return params,
    };

    for (key, prop_schema) in properties {
        let expected = prop_schema.get("type").and_then(|t| t.as_str());
        if expected == Some("string") {
            if let Some(val) = obj.get(key) {
                if !val.is_string() {
                    let stringified = match val {
                        Value::Null => String::new(),
                        Value::Bool(b) => b.to_string(),
                        Value::Number(n) => n.to_string(),
                        other => serde_json::to_string(other).unwrap_or_default(),
                    };
                    obj.insert(key.clone(), Value::String(stringified));
                }
            }
        }
    }

    params
}

/// Validate `params` against the template's `input_schema`.
///
/// Currently checks required fields and basic type compatibility.
fn validate_params(template: &PromptTemplate, params: &Value) -> Result<()> {
    let schema = match &template.input_schema {
        Some(s) => s,
        None => return Ok(()),
    };

    let obj =
        params.as_object().ok_or_else(|| anyhow!("prompt parameters must be a JSON object"))?;

    // Check required fields.
    if let Some(Value::Array(required)) = schema.get("required") {
        for req in required {
            if let Some(field_name) = req.as_str() {
                if !obj.contains_key(field_name) {
                    return Err(anyhow!("missing required prompt parameter: '{field_name}'"));
                }
            }
        }
    }

    // Basic type checking for supplied values.
    let properties = match schema.get("properties") {
        Some(Value::Object(props)) => props,
        _ => return Ok(()),
    };

    for (key, value) in obj {
        if let Some(prop_schema) = properties.get(key) {
            if let Some(expected_type) = prop_schema.get("type").and_then(|t| t.as_str()) {
                if !type_matches(expected_type, value) {
                    return Err(anyhow!(
                        "prompt parameter '{key}' expected type '{expected_type}', got {}",
                        json_type_name(value)
                    ));
                }
            }
        }
    }

    Ok(())
}

fn type_matches(expected: &str, value: &Value) -> bool {
    match expected {
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => value.is_i64() || value.is_u64(),
        "boolean" => value.is_boolean(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        "null" => value.is_null(),
        _ => true,
    }
}

fn json_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Look up a [`PromptTemplate`] by ID within a persona's prompt list.
pub fn find_prompt_template<'a>(
    prompts: &'a [PromptTemplate],
    prompt_id: &str,
) -> Option<&'a PromptTemplate> {
    prompts.iter().find(|p| p.id == prompt_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_template() -> PromptTemplate {
        PromptTemplate {
            id: "test".into(),
            name: "Test".into(),
            description: String::new(),
            template: "Hello {{name}}, you are {{age}} years old.".into(),
            input_schema: Some(json!({
                "type": "object",
                "required": ["name"],
                "properties": {
                    "name": { "type": "string" },
                    "age": { "type": "integer", "default": 30 }
                }
            })),
        }
    }

    #[test]
    fn render_basic_template() {
        let tpl = sample_template();
        let params = json!({"name": "Alice"});
        let result = render_prompt_template(&tpl, &params).unwrap();
        assert_eq!(result, "Hello Alice, you are 30 years old.");
    }

    #[test]
    fn render_with_all_params() {
        let tpl = sample_template();
        let params = json!({"name": "Bob", "age": 25});
        let result = render_prompt_template(&tpl, &params).unwrap();
        assert_eq!(result, "Hello Bob, you are 25 years old.");
    }

    #[test]
    fn missing_required_param() {
        let tpl = sample_template();
        let params = json!({"age": 25});
        let err = render_prompt_template(&tpl, &params).unwrap_err();
        assert!(err.to_string().contains("missing required prompt parameter: 'name'"));
    }

    #[test]
    fn wrong_type_param_coerced() {
        let tpl = sample_template();
        // Numbers are coerced to strings when schema expects string
        let params = json!({"name": 123});
        let result = render_prompt_template(&tpl, &params).unwrap();
        assert!(result.contains("123"));
    }

    #[test]
    fn object_param_coerced_to_string() {
        let tpl = sample_template();
        // Objects/arrays are JSON-serialized when schema expects string
        let params = json!({"name": {"nested": "value"}});
        let result = render_prompt_template(&tpl, &params).unwrap();
        assert!(result.contains("nested"));
    }

    #[test]
    fn no_schema_renders_freely() {
        let tpl = PromptTemplate {
            id: "free".into(),
            name: "Free".into(),
            description: String::new(),
            template: "Say {{word}}!".into(),
            input_schema: None,
        };
        let params = json!({"word": "hello"});
        let result = render_prompt_template(&tpl, &params).unwrap();
        assert_eq!(result, "Say hello!");
    }

    #[test]
    fn each_helper_works() {
        let tpl = PromptTemplate {
            id: "list".into(),
            name: "List".into(),
            description: String::new(),
            template: "Items: {{#each items}}{{this}} {{/each}}".into(),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "items": { "type": "array", "items": { "type": "string" } }
                }
            })),
        };
        let params = json!({"items": ["a", "b", "c"]});
        let result = render_prompt_template(&tpl, &params).unwrap();
        assert_eq!(result, "Items: a b c ");
    }
}
