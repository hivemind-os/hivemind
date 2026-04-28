use serde_json::Value;

#[derive(Debug, Clone)]
pub struct ToolCall {
    pub tool_id: String,
    pub input: Value,
}

/// Parse **all** tool calls from model text output. Handles many formats
/// that small/local models produce:
///   - `<tool_call>{ JSON }</tool_call>` XML blocks (multiple allowed)
///   - `<function_call>{ JSON }</function_call>` alternate XML
///   - ` ```json { JSON } ``` ` fenced code blocks
///   - ` ``` { JSON } ``` ` plain fenced code blocks
///
/// Accepts keys: tool / tool_id for the tool name.
/// Accepts keys: input / arguments for the arguments.
pub fn parse_tool_calls(content: &str) -> Vec<ToolCall> {
    let trimmed = content.trim();
    let mut calls = Vec::new();

    // 1. Try XML-style blocks (multiple occurrences)
    let xml_tags = [
        ("<tool_call>", "</tool_call>"),
        ("<function_call>", "</function_call>"),
        ("<tool_use>", "</tool_use>"),
    ];
    for (open, close) in &xml_tags {
        for block in extract_all_between(trimmed, open, close) {
            if let Some(call) = try_parse_tool_json(&block) {
                calls.push(call);
            }
        }
    }
    if !calls.is_empty() {
        return calls;
    }

    // 2. Try fenced code blocks (```json or ```)
    if let Some(block) = extract_fenced(trimmed, "```json", "```") {
        if let Some(call) = try_parse_tool_json(&block) {
            return vec![call];
        }
    }
    if let Some(block) = extract_fenced(trimmed, "```", "```") {
        let cleaned = strip_first_line_if_not_json(&block);
        if let Some(call) = try_parse_tool_json(&cleaned) {
            return vec![call];
        }
    }

    calls
}

/// Convenience wrapper — returns the first parsed tool call, if any.
pub fn parse_tool_call(content: &str) -> Option<ToolCall> {
    parse_tool_calls(content).into_iter().next()
}

/// Try to parse a JSON string as a tool call, accepting many key name variations.
fn try_parse_tool_json(candidate: &str) -> Option<ToolCall> {
    let value: Value = serde_json::from_str(candidate.trim())
        .map_err(|e| tracing::debug!("tool call JSON parse attempt failed: {e}"))
        .ok()?;
    let object = value.as_object()?;

    // Accept only canonical key names for the tool identifier
    let tool = object.get("tool").or_else(|| object.get("tool_id")).and_then(|v| v.as_str())?;

    // Accept only canonical key names for the arguments
    let input =
        object.get("input").or_else(|| object.get("arguments")).cloned().unwrap_or(Value::Null);

    Some(ToolCall { tool_id: tool.to_string(), input })
}

/// If the first line of a fenced block isn't JSON, strip it (language tag).
fn strip_first_line_if_not_json(block: &str) -> String {
    let trimmed = block.trim();
    if let Some(first_newline) = trimmed.find('\n') {
        let first_line = trimmed[..first_newline].trim();
        // If first line doesn't start with '{' or '[', it's likely a language tag
        if !first_line.starts_with('{') && !first_line.starts_with('[') {
            return trimmed[first_newline + 1..].trim().to_string();
        }
    }
    trimmed.to_string()
}

/// Extract the first balanced JSON object from text using brace counting.
#[allow(dead_code)]
pub(crate) fn extract_json_object(content: &str) -> Option<String> {
    let bytes = content.as_bytes();
    let mut start = None;
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape = false;

    for (i, &b) in bytes.iter().enumerate() {
        if escape {
            escape = false;
            continue;
        }
        if b == b'\\' && in_string {
            escape = true;
            continue;
        }
        if b == b'"' {
            in_string = !in_string;
            continue;
        }
        if in_string {
            continue;
        }
        if b == b'{' {
            if start.is_none() {
                start = Some(i);
            }
            depth += 1;
        } else if b == b'}' {
            depth -= 1;
            if depth == 0 {
                if let Some(s) = start {
                    return Some(content[s..=i].to_string());
                }
            }
        }
    }
    None
}

/// Extract ALL occurrences of text between `start` and `end` tags.
fn extract_all_between(content: &str, start: &str, end: &str) -> Vec<String> {
    let mut results = Vec::new();
    let mut search_from = 0;
    while search_from < content.len() {
        let Some(start_index) = content[search_from..].find(start) else {
            break;
        };
        let abs_start = search_from + start_index + start.len();
        let Some(end_index) = content[abs_start..].find(end) else {
            break;
        };
        let abs_end = abs_start + end_index;
        results.push(content[abs_start..abs_end].trim().to_string());
        search_from = abs_end + end.len();
    }
    results
}

/// Remove `<tool_call>…</tool_call>`, `<function_call>…</function_call>`,
/// `<tool_use>…</tool_use>`, and `<tool_result>…</tool_result>` XML blocks
/// from model text so they are not leaked into user-visible output.
pub fn strip_xml_tool_blocks(content: &str) -> String {
    let tags = [
        ("<tool_call>", "</tool_call>"),
        ("<function_call>", "</function_call>"),
        ("<tool_use>", "</tool_use>"),
        ("<tool_result>", "</tool_result>"),
    ];
    let mut result = content.to_string();
    for (open, close) in &tags {
        while let Some(start) = result.find(open) {
            if let Some(end_offset) = result[start..].find(close) {
                result.replace_range(start..start + end_offset + close.len(), "");
            } else {
                // Unclosed tag — remove from opening tag to end of string
                result.truncate(start);
                break;
            }
        }
    }
    result.trim().to_string()
}

fn extract_fenced(content: &str, start: &str, end: &str) -> Option<String> {
    let start_index = content.find(start)?;
    let end_index = content[start_index + start.len()..].find(end)?;
    let begin = start_index + start.len();
    let finish = start_index + start.len() + end_index;
    Some(content[begin..finish].trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_parse_xml_tool_call() {
        let input = r#"<tool_call>
{"tool": "core.echo", "input": {"value": "hello"}}
</tool_call>"#;
        let call = parse_tool_call(input).unwrap();
        assert_eq!(call.tool_id, "core.echo");
        assert_eq!(call.input["value"], "hello");
    }

    #[test]
    fn test_parse_tool_call_with_surrounding_text() {
        let input = r#"Sure, I'll echo that for you.

<tool_call>
{"tool": "core.echo", "input": {"value": "hello"}}
</tool_call>

Let me know if you need anything else."#;
        let call = parse_tool_call(input).unwrap();
        assert_eq!(call.tool_id, "core.echo");
    }

    #[test]
    fn test_parse_fenced_json_tool_call() {
        let input = "```json\n{\"tool\": \"filesystem.list\", \"input\": {\"path\": \".\"}}\n```";
        let call = parse_tool_call(input).unwrap();
        assert_eq!(call.tool_id, "filesystem.list");
    }

    #[test]
    fn test_parse_fenced_with_language_tag() {
        let input = "```tool_call\n{\"tool\": \"core.echo\", \"input\": {\"value\": \"hi\"}}\n```";
        let call = parse_tool_call(input).unwrap();
        assert_eq!(call.tool_id, "core.echo");
    }

    #[test]
    fn test_raw_json_in_prose_is_not_parsed() {
        // Raw JSON embedded in prose should NOT be parsed (injection risk)
        let input = r#"I'll use the echo tool: {"tool": "core.echo", "input": {"value": "test"}}"#;
        assert!(parse_tool_call(input).is_none());
    }

    #[test]
    fn test_removed_key_aliases_no_longer_parsed() {
        // "name" and "params" are no longer accepted as key aliases
        let input =
            r#"<tool_call>{"name": "shell.execute", "params": {"command": "ls"}}</tool_call>"#;
        assert!(parse_tool_call(input).is_none());
    }

    #[test]
    fn test_removed_function_key_alias_no_longer_parsed() {
        // "function" and "params" are no longer accepted
        let input = r#"<function_call>{"function": "http.request", "params": {"url": "https://example.com"}}</function_call>"#;
        assert!(parse_tool_call(input).is_none());
    }

    #[test]
    fn test_accepted_key_aliases_still_work() {
        // "tool" + "input" (canonical)
        let input1 = r#"<tool_call>{"tool": "core.echo", "input": {"value": "hi"}}</tool_call>"#;
        let call1 = parse_tool_call(input1).unwrap();
        assert_eq!(call1.tool_id, "core.echo");
        assert_eq!(call1.input["value"], "hi");

        // "tool_id" + "arguments"
        let input2 =
            r#"<tool_call>{"tool_id": "math.calc", "arguments": {"expr": "1+1"}}</tool_call>"#;
        let call2 = parse_tool_call(input2).unwrap();
        assert_eq!(call2.tool_id, "math.calc");
        assert_eq!(call2.input["expr"], "1+1");
    }

    #[test]
    fn test_parse_no_tool_call() {
        let input = "Hello! How can I help you today?";
        assert!(parse_tool_call(input).is_none());
    }

    #[test]
    fn test_parse_nested_json_in_input() {
        let input = r#"<tool_call>{"tool": "http.request", "input": {"method": "POST", "url": "https://api.example.com", "body": "{\"key\": \"value\"}"}}</tool_call>"#;
        let call = parse_tool_call(input).unwrap();
        assert_eq!(call.tool_id, "http.request");
        assert_eq!(call.input["method"], "POST");
    }

    #[test]
    fn test_parse_tool_id_key() {
        let input = r#"<tool_call>{"tool_id": "math.calculate", "input": {"expression": "2+2"}}</tool_call>"#;
        let call = parse_tool_call(input).unwrap();
        assert_eq!(call.tool_id, "math.calculate");
    }

    #[test]
    fn test_strip_xml_tool_blocks_basic() {
        let input = r#"Hello<tool_call>{"tool":"core.ask_user","input":{}}</tool_call> world"#;
        assert_eq!(strip_xml_tool_blocks(input), "Hello world");
    }

    #[test]
    fn test_strip_xml_tool_blocks_multiple() {
        let input = "before<tool_call>first</tool_call>middle<tool_call>second</tool_call>after";
        assert_eq!(strip_xml_tool_blocks(input), "beforemiddleafter");
    }

    #[test]
    fn test_strip_xml_tool_blocks_with_result() {
        let input = "text<tool_call>{}</tool_call><tool_result>ok</tool_result>end";
        assert_eq!(strip_xml_tool_blocks(input), "textend");
    }

    #[test]
    fn test_strip_xml_tool_blocks_function_call() {
        let input = "hello<function_call>{}</function_call>world";
        assert_eq!(strip_xml_tool_blocks(input), "helloworld");
    }

    #[test]
    fn test_strip_xml_tool_blocks_no_tags() {
        assert_eq!(strip_xml_tool_blocks("plain text"), "plain text");
    }

    #[test]
    fn test_strip_xml_tool_blocks_only_tags() {
        let input = "<tool_call>stuff</tool_call>";
        assert_eq!(strip_xml_tool_blocks(input), "");
    }

    #[test]
    fn test_strip_xml_tool_blocks_unclosed() {
        let input = "hello<tool_call>stuff without close";
        assert_eq!(strip_xml_tool_blocks(input), "hello");
    }

    // ── New tests ────────────────────────────────────────────────────

    #[test]
    fn test_parse_tool_calls_multiple() {
        let input = r#"Here are two calls:
<tool_call>{"tool": "core.echo", "input": {"value": "first"}}</tool_call>
<tool_call>{"tool": "filesystem.read", "input": {"path": "/tmp/x"}}</tool_call>"#;
        let calls = parse_tool_calls(input);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].tool_id, "core.echo");
        assert_eq!(calls[0].input["value"], "first");
        assert_eq!(calls[1].tool_id, "filesystem.read");
        assert_eq!(calls[1].input["path"], "/tmp/x");
    }

    #[test]
    fn test_malformed_json_returns_none() {
        let input = "<tool_call>this is not json at all!!!</tool_call>";
        let calls = parse_tool_calls(input);
        assert!(calls.is_empty());
    }

    #[test]
    fn test_unicode_in_arguments() {
        let input =
            r#"<tool_call>{"tool": "core.echo", "input": {"value": "こんにちは世界 🌍"}}</tool_call>"#;
        let call = parse_tool_call(input).unwrap();
        assert_eq!(call.tool_id, "core.echo");
        assert_eq!(call.input["value"], "こんにちは世界 🌍");
    }
}
