//! Tool bridge: generates Python function stubs from tool definitions
//! and provides the bidirectional RPC protocol for tool calls from
//! within the code executor.
//!
//! The bridge injects a `__hivemind_call_tool__` function into the Python
//! session's globals. When user code calls a bridged tool function, it
//! serializes a JSON request to the real stdout, blocks reading the result
//! from stdin, and returns the parsed response.
//!
//! On the host (Rust) side, the executor's output-reading loop detects
//! tool-call frames and dispatches them via a `ToolCallHandler`.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

// ── Sentinel markers for tool-call RPC ────────────────────────────────

pub const TOOL_CALL_START: &str = "__HIVEMIND_TOOL_CALL__";
pub const TOOL_CALL_END: &str = "__HIVEMIND_TOOL_CALL_END__";

// ── Tool call handler trait ───────────────────────────────────────────

/// Request sent by the Python bridge function to the host.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRequest {
    /// Monotonically increasing ID for matching request/response.
    pub request_id: u64,
    /// Fully-qualified tool ID (e.g., "filesystem.read").
    pub tool_id: String,
    /// Tool arguments as a JSON object.
    pub args: Value,
}

/// Response sent back from the host to the Python bridge function.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallResponse {
    /// Echoed request ID for correlation.
    pub request_id: u64,
    /// Successful result (tool output as JSON).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Error message if the tool call failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Whether the result was truncated.
    #[serde(default)]
    pub truncated: bool,
}

/// Trait implemented by the host to handle tool calls from the executor.
///
/// Implementations should run tool calls through the same policy/approval
/// pipeline as native structured tool calls — the bridge must NOT bypass
/// security or approval gates.
#[async_trait::async_trait]
pub trait ToolCallHandler: Send + Sync {
    async fn handle_tool_call(&self, request: ToolCallRequest) -> ToolCallResponse;
}

// ── Execution options (extends CodeExecutor without changing its trait) ─

/// Optional context passed alongside `execute()` calls when tool
/// bridging is active.
pub struct ExecutionOptions<'a> {
    /// Handler for tool calls originating from the Python bridge.
    /// If `None`, any tool call from Python raises a RuntimeError.
    pub tool_call_handler: Option<&'a dyn ToolCallHandler>,
}

impl<'a> Default for ExecutionOptions<'a> {
    fn default() -> Self {
        Self {
            tool_call_handler: None,
        }
    }
}

// ── Tool classification ───────────────────────────────────────────────

/// How a tool is exposed in a CodeAct session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeActToolMode {
    /// Structured JSON tool call only (e.g., ask_user, delegate_task).
    Native,
    /// Python function stub only.
    Bridged,
    /// Available both ways (LLM chooses).
    Both,
}

/// Simplified tool info used for Python stub generation.
#[derive(Debug, Clone)]
pub struct BridgedToolInfo {
    pub tool_id: String,
    pub description: String,
    /// JSON Schema for input parameters.
    pub input_schema: Value,
    pub mode: CodeActToolMode,
}

// ── Python code generation ────────────────────────────────────────────

/// Generate the complete Python bridge preamble: the `__hivemind_call_tool__`
/// function and all tool stubs. This is injected at session creation.
pub fn generate_bridge_code(tools: &[BridgedToolInfo]) -> String {
    let mut code = String::with_capacity(4096);
    code.push_str(&generate_preamble());
    code.push('\n');
    for tool in tools {
        if tool.mode == CodeActToolMode::Native {
            continue; // Native-only tools don't get Python stubs
        }
        code.push_str(&generate_tool_stub(tool));
        code.push('\n');
    }
    code
}

/// The base bridge function that all tool stubs call.
fn generate_preamble() -> String {
    r#"
import json as _json

# Original I/O handles, captured before any exec() redirection.
# These are set by the wrapper script and available in globals.
_original_stdout = __import__('sys').__stdout__
_original_stdin = __import__('sys').__stdin__
_tool_call_counter = 0

def __hivemind_call_tool__(tool_id, args):
    """Call a host tool via the bridge and return the result."""
    global _tool_call_counter
    _tool_call_counter += 1
    req_id = _tool_call_counter
    request = _json.dumps({"request_id": req_id, "tool_id": tool_id, "args": args})
    _original_stdout.write("__HIVEMIND_TOOL_CALL__" + request + "__HIVEMIND_TOOL_CALL_END__\n")
    _original_stdout.flush()
    # Block waiting for the host response
    line = _original_stdin.readline()
    if not line:
        raise RuntimeError("Host disconnected during tool call")
    resp = _json.loads(line.strip())
    if resp.get("error"):
        raise RuntimeError(f"Tool call failed ({tool_id}): {resp['error']}")
    return resp.get("result")
"#
    .to_string()
}

/// Generate a Python function stub for one tool.
fn generate_tool_stub(tool: &BridgedToolInfo) -> String {
    let func_name = tool_id_to_python_name(&tool.tool_id);
    let (params, body_args) = build_params_from_schema(&tool.input_schema);
    let docstring = build_docstring(&tool.description, &tool.input_schema);

    format!(
        r#"def {func_name}({params}):
    """{docstring}"""
    {body_args}
    return __hivemind_call_tool__("{tool_id}", _args)
"#,
        func_name = func_name,
        params = params,
        docstring = docstring,
        body_args = body_args,
        tool_id = tool.tool_id,
    )
}

/// Convert a tool ID like "filesystem.read" to a valid Python identifier.
///
/// Rules:
/// - Dots → underscores
/// - Dashes → underscores
/// - Prefix with `_` if the result is a Python keyword or starts with a digit
fn tool_id_to_python_name(id: &str) -> String {
    let mut name: String = id.chars().map(|c| if c == '.' || c == '-' { '_' } else { c }).collect();

    // Avoid Python keywords
    if is_python_keyword(&name) {
        name.insert(0, '_');
    }
    // Identifiers can't start with a digit
    if name.starts_with(|c: char| c.is_ascii_digit()) {
        name.insert(0, '_');
    }
    name
}

fn is_python_keyword(s: &str) -> bool {
    matches!(
        s,
        "False" | "None" | "True" | "and" | "as" | "assert" | "async" | "await"
            | "break" | "class" | "continue" | "def" | "del" | "elif" | "else"
            | "except" | "finally" | "for" | "from" | "global" | "if" | "import"
            | "in" | "is" | "lambda" | "nonlocal" | "not" | "or" | "pass"
            | "raise" | "return" | "try" | "while" | "with" | "yield"
    )
}

/// Schema type → Python type hint string.
fn json_type_to_python(ty: &str) -> &str {
    match ty {
        "string" => "str",
        "number" => "float",
        "integer" => "int",
        "boolean" => "bool",
        "array" => "list",
        "object" => "dict",
        _ => "object",
    }
}

/// Build the function parameter list and the `_args = {...}` body from
/// the input_schema JSON Schema.
///
/// For simple flat schemas, we generate named keyword-only params.
/// For complex schemas (nested objects, anyOf, etc.), we fall back to `**kwargs`.
fn build_params_from_schema(schema: &Value) -> (String, String) {
    let props = match schema.get("properties").and_then(|v| v.as_object()) {
        Some(p) if !p.is_empty() => p,
        _ => {
            // No properties or empty — use **kwargs fallback
            return (
                "**kwargs".to_string(),
                "_args = dict(kwargs)".to_string(),
            );
        }
    };

    let required: Vec<&str> = schema
        .get("required")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();

    // Check if schema is "simple" (all properties are scalar types)
    let is_simple = props.values().all(|v| {
        v.get("type")
            .and_then(|t| t.as_str())
            .map(|t| matches!(t, "string" | "number" | "integer" | "boolean"))
            .unwrap_or(false)
            || v.get("enum").is_some()
    });

    if !is_simple {
        // Complex schema — use **kwargs with individual extraction
        let mut params = Vec::new();
        let mut body_lines = vec!["_args = {}".to_string()];

        // Required params first, then optional
        let mut sorted_props: Vec<_> = props.iter().collect();
        sorted_props.sort_by_key(|(name, _)| (!required.contains(&name.as_str()), *name));

        for (name, prop) in &sorted_props {
            let py_name = sanitize_param_name(name);
            let type_hint = prop
                .get("type")
                .and_then(|t| t.as_str())
                .map(json_type_to_python)
                .unwrap_or("object");

            if required.contains(&name.as_str()) {
                params.push(format!("{py_name}: {type_hint}"));
                body_lines.push(format!("    _args[\"{name}\"] = {py_name}"));
            } else {
                params.push(format!("{py_name}: {type_hint} = None"));
                body_lines.push(format!(
                    "    if {py_name} is not None: _args[\"{name}\"] = {py_name}"
                ));
            }
        }

        return (params.join(", "), body_lines.join("\n"));
    }

    // Simple schema — nice keyword params
    let mut required_params = Vec::new();
    let mut optional_params = Vec::new();
    let mut body_lines = vec!["_args = {}".to_string()];

    // Collect and sort: required first (stable order), then optional
    let mut sorted_props: Vec<_> = props.iter().collect();
    sorted_props.sort_by_key(|(name, _)| (!required.contains(&name.as_str()), *name));

    for (name, prop) in &sorted_props {
        let py_name = sanitize_param_name(name);
        let type_hint = prop
            .get("type")
            .and_then(|t| t.as_str())
            .map(json_type_to_python)
            .unwrap_or("str");

        if required.contains(&name.as_str()) {
            required_params.push(format!("{py_name}: {type_hint}"));
            body_lines.push(format!("    _args[\"{name}\"] = {py_name}"));
        } else {
            optional_params.push(format!("{py_name}: {type_hint} = None"));
            body_lines.push(format!(
                "    if {py_name} is not None: _args[\"{name}\"] = {py_name}"
            ));
        }
    }

    let mut all_params = required_params;
    all_params.extend(optional_params);
    (all_params.join(", "), body_lines.join("\n"))
}

/// Sanitize a JSON property name into a valid Python parameter name.
fn sanitize_param_name(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' })
        .collect();
    if sanitized.is_empty() {
        return "_param".to_string();
    }
    if is_python_keyword(&sanitized) || sanitized.starts_with(|c: char| c.is_ascii_digit()) {
        format!("_{sanitized}")
    } else {
        sanitized
    }
}

/// Build a docstring from the tool description and schema.
fn build_docstring(description: &str, schema: &Value) -> String {
    let mut doc = description.replace('\\', "\\\\").replace('"', "'");
    // Truncate very long descriptions
    if doc.len() > 200 {
        doc.truncate(200);
        doc.push_str("...");
    }

    if let Some(props) = schema.get("properties").and_then(|v| v.as_object()) {
        if !props.is_empty() {
            doc.push_str("\n\n    Args:");
            for (name, prop) in props {
                let desc = prop
                    .get("description")
                    .and_then(|d| d.as_str())
                    .unwrap_or("");
                let short_desc = if desc.len() > 80 {
                    format!("{}...", &desc[..80])
                } else {
                    desc.to_string()
                };
                doc.push_str(&format!("\n        {name}: {short_desc}"));
            }
        }
    }

    doc
}

/// Build a mapping from Python function names to tool IDs.
/// Useful for detecting and resolving name collisions.
pub fn build_name_registry(tools: &[BridgedToolInfo]) -> HashMap<String, String> {
    let mut registry = HashMap::new();
    for tool in tools {
        let name = tool_id_to_python_name(&tool.tool_id);
        registry.insert(name, tool.tool_id.clone());
    }
    registry
}

/// Determine the default `CodeActToolMode` for a tool based on its ID.
///
/// Tools requiring UI interaction or agent orchestration default to `Native`.
/// Everything else defaults to `Bridged`.
pub fn default_tool_mode(tool_id: &str) -> CodeActToolMode {
    // Tools that need UI interaction gates or agent orchestration
    if tool_id.starts_with("core.ask_user")
        || tool_id.starts_with("core.delegate_task")
        || tool_id.starts_with("core.spawn_agent")
        || tool_id.starts_with("workflow.")
    {
        return CodeActToolMode::Native;
    }
    CodeActToolMode::Bridged
}

// ── Parse tool call from executor output ──────────────────────────────

/// Try to parse a tool call request from a line of executor output.
///
/// Returns `Some(request)` if the line contains a valid tool call frame,
/// `None` otherwise.
pub fn parse_tool_call_line(line: &str) -> Option<ToolCallRequest> {
    let trimmed = line.trim();
    if !trimmed.starts_with(TOOL_CALL_START) {
        return None;
    }
    let after_start = &trimmed[TOOL_CALL_START.len()..];
    let json_str = after_start.strip_suffix(TOOL_CALL_END)?;
    serde_json::from_str(json_str).ok()
}

/// Serialize a tool call response for writing back to the executor's stdin.
pub fn serialize_tool_response(response: &ToolCallResponse) -> String {
    serde_json::to_string(response).unwrap_or_else(|_| {
        format!(
            r#"{{"request_id":{},"error":"failed to serialize response"}}"#,
            response.request_id
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tool_id_to_name_basic() {
        assert_eq!(tool_id_to_python_name("filesystem.read"), "filesystem_read");
        assert_eq!(tool_id_to_python_name("core.ask_user"), "core_ask_user");
        assert_eq!(
            tool_id_to_python_name("mcp.github.search_code"),
            "mcp_github_search_code"
        );
    }

    #[test]
    fn tool_id_to_name_avoids_keywords() {
        assert_eq!(tool_id_to_python_name("import"), "_import");
        assert_eq!(tool_id_to_python_name("class"), "_class");
    }

    #[test]
    fn tool_id_to_name_digit_prefix() {
        assert_eq!(tool_id_to_python_name("3d.model"), "_3d_model");
    }

    #[test]
    fn generate_stub_simple_schema() {
        let tool = BridgedToolInfo {
            tool_id: "filesystem.read".into(),
            description: "Read file contents".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path"},
                    "start_line": {"type": "integer", "description": "Start line"},
                },
                "required": ["path"]
            }),
            mode: CodeActToolMode::Bridged,
        };

        let stub = generate_tool_stub(&tool);
        assert!(stub.contains("def filesystem_read("));
        assert!(stub.contains("path: str"));
        assert!(stub.contains("start_line: int = None"));
        assert!(stub.contains("__hivemind_call_tool__(\"filesystem.read\""));
        assert!(stub.contains("Read file contents"));
    }

    #[test]
    fn generate_stub_no_properties() {
        let tool = BridgedToolInfo {
            tool_id: "system.status".into(),
            description: "Get system status".into(),
            input_schema: json!({"type": "object", "properties": {}}),
            mode: CodeActToolMode::Bridged,
        };

        let stub = generate_tool_stub(&tool);
        assert!(stub.contains("**kwargs"));
    }

    #[test]
    fn default_mode_classification() {
        assert_eq!(default_tool_mode("core.ask_user"), CodeActToolMode::Native);
        assert_eq!(default_tool_mode("workflow.start"), CodeActToolMode::Native);
        assert_eq!(
            default_tool_mode("filesystem.read"),
            CodeActToolMode::Bridged
        );
        assert_eq!(
            default_tool_mode("mcp.github.search"),
            CodeActToolMode::Bridged
        );
    }

    #[test]
    fn parse_tool_call_valid() {
        let line = r#"__HIVEMIND_TOOL_CALL__{"request_id":1,"tool_id":"filesystem.read","args":{"path":"test.txt"}}__HIVEMIND_TOOL_CALL_END__"#;
        let req = parse_tool_call_line(line).unwrap();
        assert_eq!(req.request_id, 1);
        assert_eq!(req.tool_id, "filesystem.read");
        assert_eq!(req.args["path"], "test.txt");
    }

    #[test]
    fn parse_tool_call_invalid() {
        assert!(parse_tool_call_line("normal output line").is_none());
        assert!(parse_tool_call_line("__HIVEMIND_TOOL_CALL__bad json__HIVEMIND_TOOL_CALL_END__").is_none());
    }

    #[test]
    fn serialize_response_ok() {
        let resp = ToolCallResponse {
            request_id: 42,
            result: Some(json!({"content": "file data"})),
            error: None,
            truncated: false,
        };
        let s = serialize_tool_response(&resp);
        assert!(s.contains("\"request_id\":42"));
        assert!(s.contains("file data"));
    }

    #[test]
    fn full_bridge_code_generation() {
        let tools = vec![
            BridgedToolInfo {
                tool_id: "filesystem.read".into(),
                description: "Read a file".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"}
                    },
                    "required": ["path"]
                }),
                mode: CodeActToolMode::Bridged,
            },
            BridgedToolInfo {
                tool_id: "core.ask_user".into(),
                description: "Ask user a question".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "question": {"type": "string"}
                    },
                    "required": ["question"]
                }),
                mode: CodeActToolMode::Native, // Should be skipped
            },
        ];

        let code = generate_bridge_code(&tools);
        assert!(code.contains("def __hivemind_call_tool__"));
        assert!(code.contains("def filesystem_read("));
        assert!(!code.contains("def core_ask_user(")); // Native = skipped
    }

    #[test]
    fn param_name_sanitization() {
        assert_eq!(sanitize_param_name("normal_name"), "normal_name");
        assert_eq!(sanitize_param_name("kebab-name"), "kebab_name");
        assert_eq!(sanitize_param_name("class"), "_class");
        assert_eq!(sanitize_param_name("123abc"), "_123abc");
        assert_eq!(sanitize_param_name(""), "_param");
    }
}
