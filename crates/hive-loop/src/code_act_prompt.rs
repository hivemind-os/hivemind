//! System prompt construction for the CodeAct strategy.
//!
//! Generates supplementary instructions that are appended to the persona's
//! system prompt. These tell the LLM to write Python code in fenced blocks,
//! list available bridged tool functions, and explain the observation format.

use hive_code_executor::{BridgedToolInfo, CodeActToolMode};

/// Build the CodeAct supplement that is appended to the persona system prompt.
///
/// This tells the LLM:
/// 1. It can write Python code in fenced ` ```python ` blocks to take actions
/// 2. Which tool functions are available as Python calls
/// 3. How execution results (stdout/stderr) appear as observations
/// 4. When to use native tool calls vs code execution
pub fn build_code_act_instructions(
    bridged_tools: &[BridgedToolInfo],
    native_tool_ids: &[String],
) -> String {
    let mut parts = Vec::new();

    parts.push(CODE_ACT_HEADER.to_string());

    // List bridged tool functions
    let tool_funcs: Vec<&BridgedToolInfo> = bridged_tools
        .iter()
        .filter(|t| t.mode != CodeActToolMode::Native)
        .collect();

    if !tool_funcs.is_empty() {
        parts.push("\n## Available Python Functions\n".to_string());
        parts.push(
            "The following functions are pre-loaded in your Python environment. \
             Call them directly in your code:\n"
                .to_string(),
        );

        for tool in &tool_funcs {
            let sig = build_function_signature(tool);
            parts.push(format!("- `{sig}` — {}", tool.description));
        }
    }

    // Mention native tools
    if !native_tool_ids.is_empty() {
        parts.push("\n## Structured Tool Calls\n".to_string());
        parts.push(
            "The following tools are available as structured tool calls (not Python functions). \
             Use them via the normal tool call mechanism when needed:\n"
                .to_string(),
        );
        for id in native_tool_ids {
            parts.push(format!("- `{id}`"));
        }
    }

    parts.push(OBSERVATION_FORMAT.to_string());
    parts.push(COMPLETION_RULES.to_string());

    parts.join("\n")
}

/// Build a concise function signature for display in the system prompt.
fn build_function_signature(tool: &BridgedToolInfo) -> String {
    let func_name = tool_id_to_display_name(&tool.tool_id);

    let params = match tool
        .input_schema
        .get("properties")
        .and_then(|v| v.as_object())
    {
        Some(props) if !props.is_empty() => {
            let required: Vec<&str> = tool
                .input_schema
                .get("required")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();

            let mut req_params = Vec::new();
            let mut opt_params = Vec::new();

            for (name, prop) in props {
                let ty = prop
                    .get("type")
                    .and_then(|t| t.as_str())
                    .map(json_type_to_python_hint)
                    .unwrap_or("");

                if required.contains(&name.as_str()) {
                    if ty.is_empty() {
                        req_params.push(name.clone());
                    } else {
                        req_params.push(format!("{name}: {ty}"));
                    }
                } else if ty.is_empty() {
                    opt_params.push(format!("{name}=None"));
                } else {
                    opt_params.push(format!("{name}: {ty} = None"));
                }
            }

            req_params.extend(opt_params);
            req_params.join(", ")
        }
        _ => "**kwargs".to_string(),
    };

    format!("{func_name}({params})")
}

fn tool_id_to_display_name(id: &str) -> String {
    id.chars()
        .map(|c| if c == '.' || c == '-' { '_' } else { c })
        .collect()
}

fn json_type_to_python_hint(ty: &str) -> &str {
    match ty {
        "string" => "str",
        "number" => "float",
        "integer" => "int",
        "boolean" => "bool",
        "array" => "list",
        "object" => "dict",
        _ => "",
    }
}

// ── Prompt fragments ──────────────────────────────────────────────────

const CODE_ACT_HEADER: &str = r#"
## Code Execution

You have access to a **persistent Python environment**. To take actions, write Python code inside fenced code blocks:

```python
# Your code here
result = some_function(arg)
print(result)
```

**Key behaviors:**
- Variables, imports, and state **persist** across code blocks within this conversation.
- Use `print()` to output results — printed output appears as observations.
- You can write multiple code blocks in a single response; they execute sequentially.
- If code raises an exception, you'll see the traceback and can fix it in the next block.
- You may also use standard Python libraries (json, os, pathlib, re, math, etc.).
"#;

const OBSERVATION_FORMAT: &str = r#"
## Observation Format

After each code block executes, you'll receive an observation with the output:

```
[Code Execution Output]
<stdout from your code>
```

If there was an error:
```
[Code Execution Error]
<traceback>
```

Use these observations to decide your next action.
"#;

const COMPLETION_RULES: &str = r#"
## Completion

When you have finished the task:
- Provide your final answer in plain text (no code blocks).
- A response with **no code blocks and no tool calls** signals that you are done.
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn instructions_include_bridged_tools() {
        let tools = vec![
            BridgedToolInfo {
                tool_id: "filesystem.read".into(),
                description: "Read a file".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {"path": {"type": "string"}},
                    "required": ["path"]
                }),
                mode: CodeActToolMode::Bridged,
            },
            BridgedToolInfo {
                tool_id: "core.ask_user".into(),
                description: "Ask the user".into(),
                input_schema: json!({"type": "object", "properties": {}}),
                mode: CodeActToolMode::Native,
            },
        ];

        let native_ids = vec!["core.ask_user".to_string()];
        let prompt = build_code_act_instructions(&tools, &native_ids);

        assert!(prompt.contains("filesystem_read(path: str)"));
        assert!(prompt.contains("Read a file"));
        assert!(prompt.contains("core.ask_user"));
        assert!(prompt.contains("persistent Python environment"));
        assert!(prompt.contains("no code blocks and no tool calls"));
    }

    #[test]
    fn instructions_with_no_tools() {
        let prompt = build_code_act_instructions(&[], &[]);
        assert!(prompt.contains("persistent Python environment"));
        assert!(!prompt.contains("Available Python Functions"));
        assert!(!prompt.contains("Structured Tool Calls"));
    }

    #[test]
    fn function_signature_required_and_optional() {
        let tool = BridgedToolInfo {
            tool_id: "fs.write".into(),
            description: "Write file".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "content": {"type": "string"},
                    "append": {"type": "boolean"}
                },
                "required": ["path", "content"]
            }),
            mode: CodeActToolMode::Bridged,
        };

        let sig = build_function_signature(&tool);
        // Required params appear, optional has default
        assert!(sig.contains("path: str"));
        assert!(sig.contains("content: str"));
        assert!(sig.contains("append: bool = None"));
    }
}
