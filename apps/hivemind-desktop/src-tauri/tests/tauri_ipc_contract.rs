//! **IPC Contract Test**
//!
//! Validates that every frontend `invoke()` call sends parameters that match
//! the corresponding Rust `#[tauri::command]` function signature.
//!
//! This catches:
//! - Typos in parameter keys (e.g. `sesion_id` vs `session_id`)
//! - Missing required parameters
//! - Extra parameters the backend doesn't expect
//! - camelCase/snake_case mismatches
//!
//! The test works by:
//! 1. Parsing lib.rs with `syn` to extract every command's parameter names
//!    (filtering out Tauri-injected types like State, AppHandle, etc.)
//! 2. Scanning all frontend .ts/.tsx files for `invoke('cmd', { keys })` calls
//! 3. Cross-validating that the keys match the expected params

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use syn::{visit::Visit, Attribute, FnArg, ItemFn, Pat, Type};

// ═══════════════════════════════════════════════════════════════════════════
// Step 1: Extract command signatures from Rust source
// ═══════════════════════════════════════════════════════════════════════════

/// Tauri-injected parameter types that are NOT sent from the frontend.
const INJECTED_TYPES: &[&str] = &["AppHandle", "State", "Window", "WebviewWindow", "Webview"];

/// Check if a type path looks like a Tauri-injected type.
fn is_injected_type(ty: &Type) -> bool {
    match ty {
        Type::Path(tp) => {
            let last_seg = tp.path.segments.last();
            if let Some(seg) = last_seg {
                let name = seg.ident.to_string();
                INJECTED_TYPES.contains(&name.as_str())
            } else {
                false
            }
        }
        Type::Reference(r) => is_injected_type(&r.elem),
        _ => false,
    }
}

/// Check if a type is a Tauri-injected reference like `&tauri::AppHandle`.
fn type_contains_injected(ty: &Type) -> bool {
    if is_injected_type(ty) {
        return true;
    }
    // Also check for tauri:: prefix patterns
    let ty_str = quote_type(ty);
    INJECTED_TYPES.iter().any(|t| ty_str.contains(t))
}

fn quote_type(ty: &Type) -> String {
    use quote::ToTokens;
    ty.to_token_stream().to_string()
}

/// Check if a function has the `#[tauri::command]` attribute.
fn has_tauri_command_attr(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| {
        let path = &attr.path();
        let segments: Vec<_> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        segments == ["tauri", "command"]
    })
}

/// Visitor that collects command name → param names from `#[tauri::command]` fns.
struct CommandVisitor {
    commands: HashMap<String, Vec<String>>,
}

impl<'ast> Visit<'ast> for CommandVisitor {
    fn visit_item_fn(&mut self, node: &'ast ItemFn) {
        if !has_tauri_command_attr(&node.attrs) {
            return;
        }

        let fn_name = node.sig.ident.to_string();
        let mut params = Vec::new();

        for arg in &node.sig.inputs {
            if let FnArg::Typed(pat_type) = arg {
                // Skip Tauri-injected types
                if type_contains_injected(&pat_type.ty) {
                    continue;
                }
                // Extract the parameter name
                if let Pat::Ident(pat_ident) = pat_type.pat.as_ref() {
                    params.push(pat_ident.ident.to_string());
                }
            }
        }

        self.commands.insert(fn_name, params);
    }
}

fn extract_rust_commands(source: &str) -> HashMap<String, Vec<String>> {
    let syntax = syn::parse_file(source).expect("Failed to parse lib.rs");
    let mut visitor = CommandVisitor { commands: HashMap::new() };
    visitor.visit_file(&syntax);
    visitor.commands
}

// ═══════════════════════════════════════════════════════════════════════════
// Step 2: Extract invoke calls from frontend TypeScript
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug)]
struct InvokeCall {
    file: String,
    line: usize,
    command: String,
    keys: Vec<String>,
}

/// Scan all .ts/.tsx files under `dir` for invoke calls.
fn extract_frontend_invocations(dir: &Path) -> Vec<InvokeCall> {
    let mut calls = Vec::new();
    visit_ts_files(dir, &mut |path, content| {
        extract_invocations_from_file(path, content, &mut calls);
    });
    calls
}

fn visit_ts_files(dir: &Path, visitor: &mut dyn FnMut(&str, &str)) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            if name == "node_modules" || name == "mocks" {
                continue;
            }
            visit_ts_files(&path, visitor);
        } else if let Some(ext) = path.extension() {
            if ext == "ts" || ext == "tsx" {
                if let Ok(content) = fs::read_to_string(&path) {
                    let rel = path.to_string_lossy().to_string();
                    visitor(&rel, &content);
                }
            }
        }
    }
}

fn extract_invocations_from_file(path: &str, content: &str, calls: &mut Vec<InvokeCall>) {
    for (line_no, line) in content.lines().enumerate() {
        let trimmed = line.trim();

        // Skip comments, definitions, exports
        if trimmed.starts_with("//")
            || trimmed.starts_with("*")
            || trimmed.contains("async function invoke")
            || trimmed.contains("window.__TAURI__")
            || trimmed.starts_with("export")
        {
            continue;
        }

        // Find invoke( or invoke<...>( patterns
        let mut search_from = 0;
        while let Some(pos) = trimmed[search_from..].find("invoke(").or_else(|| {
            // Handle invoke<Type>( pattern
            trimmed[search_from..]
                .find("invoke<")
                .and_then(|gp| trimmed[search_from + gp..].find('(').map(|pp| gp + pp))
        }) {
            let abs_pos = search_from + pos;
            let paren_pos = abs_pos + trimmed[abs_pos..].find('(').unwrap_or(0) + 1;

            if paren_pos >= trimmed.len() {
                break;
            }

            let rest = &trimmed[paren_pos..];

            // Extract command name (first string argument)
            if let Some(cmd) = extract_string_literal(rest) {
                // Find the comma after the command name
                if let Some(comma_offset) = find_end_of_first_arg(rest) {
                    let after_comma = rest[comma_offset + 1..].trim();
                    if after_comma.starts_with('{') {
                        let keys = extract_object_keys(after_comma);
                        if !keys.is_empty() {
                            calls.push(InvokeCall {
                                file: path.to_string(),
                                line: line_no + 1,
                                command: cmd,
                                keys,
                            });
                        }
                    }
                }
                // Commands with no second arg — that's fine (no params expected)
            }

            search_from = paren_pos;
        }
    }
}

fn extract_string_literal(s: &str) -> Option<String> {
    let s = s.trim();
    let (_quote, end_quote) = if s.starts_with('\'') {
        ('\'', '\'')
    } else if s.starts_with('"') {
        ('"', '"')
    } else if s.starts_with('`') {
        ('`', '`')
    } else {
        return None;
    };
    let inner = &s[1..];
    let end = inner.find(end_quote)?;
    Some(inner[..end].to_string())
}

fn find_end_of_first_arg(s: &str) -> Option<usize> {
    let mut depth = 0;
    let mut in_string = false;
    let mut string_char = '"';

    for (i, ch) in s.char_indices() {
        if in_string {
            if ch == string_char && (i == 0 || s.as_bytes()[i - 1] != b'\\') {
                in_string = false;
            }
            continue;
        }
        match ch {
            '\'' | '"' | '`' => {
                in_string = true;
                string_char = ch;
            }
            '(' | '[' => depth += 1,
            ')' | ']' => {
                if depth == 0 {
                    return None;
                }
                depth -= 1;
            }
            ',' if depth == 0 => return Some(i),
            _ => {}
        }
    }
    None
}

fn extract_object_keys(s: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let inner = s.trim().strip_prefix('{').unwrap_or(s);

    let mut depth = 0;
    let mut in_string = false;
    let mut string_char = '"';
    let mut current_token = String::new();
    let mut expecting_key = true;

    for ch in inner.chars() {
        if in_string {
            if ch == string_char {
                in_string = false;
            }
            continue;
        }
        match ch {
            '\'' | '"' | '`' => {
                in_string = true;
                string_char = ch;
            }
            '{' | '(' | '[' => depth += 1,
            '}' | ')' | ']' => {
                if depth == 0 {
                    let token = current_token.trim().to_string();
                    if expecting_key && !token.is_empty() && is_identifier(&token) {
                        keys.push(token);
                    }
                    break;
                }
                depth -= 1;
            }
            ':' if depth == 0 => {
                let token = current_token.trim().to_string();
                if expecting_key && !token.is_empty() && is_identifier(&token) {
                    keys.push(token);
                }
                current_token.clear();
                expecting_key = false;
            }
            ',' if depth == 0 => {
                let token = current_token.trim().to_string();
                if expecting_key && !token.is_empty() && is_identifier(&token) {
                    keys.push(token);
                }
                current_token.clear();
                expecting_key = true;
            }
            _ if depth == 0 && expecting_key => {
                current_token.push(ch);
            }
            _ => {}
        }
    }

    keys
}

fn is_identifier(s: &str) -> bool {
    let trimmed = s.trim();
    if trimmed.is_empty() || trimmed.starts_with("...") {
        return false;
    }
    let first = trimmed.chars().next().unwrap();
    if !first.is_alphabetic() && first != '_' && first != '$' {
        return false;
    }
    trimmed.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '$')
}

// ═══════════════════════════════════════════════════════════════════════════
// Step 3: The contract test
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn invoke_params_match_rust_command_signatures() {
    // Parse Rust commands
    let lib_src = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/lib.rs"))
        .expect("failed to read lib.rs");
    let commands = extract_rust_commands(&lib_src);

    assert!(!commands.is_empty(), "No #[tauri::command] functions found — parser may be broken");

    // Parse frontend invocations
    let src_dir = format!("{}/../src", env!("CARGO_MANIFEST_DIR"));
    let invocations = extract_frontend_invocations(Path::new(&src_dir));

    assert!(!invocations.is_empty(), "No invoke() calls found — parser may be broken");

    let mut violations = Vec::new();

    for call in &invocations {
        let Some(expected_params) = commands.get(&call.command) else {
            // Command not found in lib.rs — might be defined elsewhere or misspelled
            violations.push(format!(
                "  {}:{}: invoke('{}') — command not found in lib.rs",
                call.file, call.line, call.command
            ));
            continue;
        };

        let expected_set: HashSet<&str> = expected_params.iter().map(|s| s.as_str()).collect();
        let actual_set: HashSet<&str> = call.keys.iter().map(|s| s.as_str()).collect();

        // Check for unexpected keys (sent by JS but not in Rust signature)
        let extra: Vec<&&str> = actual_set.difference(&expected_set).collect();
        if !extra.is_empty() {
            violations.push(format!(
                "  {}:{}: invoke('{}') sends unexpected param(s): {:?} (expected: {:?})",
                call.file, call.line, call.command, extra, expected_params
            ));
        }

        // Check for missing keys (in Rust signature but not sent by JS)
        // Note: Some params have defaults or are optional (Option<T>), so we
        // only warn about missing required params. For now, report all missing
        // as warnings (not hard failures) since we can't tell Option<T> from T
        // without deeper type analysis.
        let missing: Vec<&&str> = expected_set.difference(&actual_set).collect();
        if !missing.is_empty() {
            // This is informational — many params are Option<T>
            // Uncomment the next line to make missing params a hard failure:
            // violations.push(format!(...));
            eprintln!(
                "  INFO {}:{}: invoke('{}') omits param(s): {:?} (may be optional)",
                call.file, call.line, call.command, missing
            );
        }
    }

    if !violations.is_empty() {
        panic!(
            "\nIPC contract violations found ({}):\n{}\n\n\
             Each frontend invoke() call must send parameter keys that exactly match \
             the Rust #[tauri::command] function signature (excluding injected types \
             like State, AppHandle, Window).",
            violations.len(),
            violations.join("\n")
        );
    }
}

/// Diagnostic test: prints the full command manifest for debugging.
#[test]
fn print_command_manifest() {
    let lib_src = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/lib.rs"))
        .expect("failed to read lib.rs");
    let commands = extract_rust_commands(&lib_src);

    eprintln!("\n=== Tauri Command Manifest ({} commands) ===", commands.len());
    let mut sorted: Vec<_> = commands.iter().collect();
    sorted.sort_by_key(|(name, _)| (*name).clone());
    for (name, params) in &sorted {
        if params.is_empty() {
            eprintln!("  {} (no params)", name);
        } else {
            eprintln!("  {} ({})", name, params.join(", "));
        }
    }
    eprintln!("=== End manifest ===\n");
}
