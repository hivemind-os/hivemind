//! Verifies that all Tauri commands use `rename_all = "snake_case"` so the
//! frontend can pass parameter keys in snake_case (e.g. `session_id`).
//!
//! Without this annotation Tauri defaults to camelCase, meaning the frontend
//! would need to send `sessionId` — which contradicts our snake_case API
//! convention and silently fails at runtime.
//!
//! Also scans frontend TypeScript source to ensure all `invoke()` call
//! parameter keys are snake_case, catching the exact class of bug where the
//! backend expects `session_id` but the frontend sends `sessionId`.

use std::fs;

/// Scan lib.rs to ensure every `#[tauri::command` annotation includes
/// `rename_all = "snake_case"`.  Catches someone adding a new command
/// without the annotation.
#[test]
fn all_tauri_commands_use_snake_case_rename() {
    let src = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/lib.rs"))
        .expect("failed to read lib.rs");

    let mut violations = Vec::new();

    for (line_no, line) in src.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("#[tauri::command") {
            if !trimmed.contains("rename_all") || !trimmed.contains("snake_case") {
                violations.push(format!("  line {}: {}", line_no + 1, trimmed));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "\nFound #[tauri::command] without rename_all = \"snake_case\":\n{}\n\
         All Tauri commands must use #[tauri::command(rename_all = \"snake_case\")] \
         so the frontend can pass snake_case parameter keys.",
        violations.join("\n")
    );
}

/// Scan all frontend TypeScript/TSX files for `invoke(` calls and verify
/// that every object-literal key in the second argument is snake_case.
///
/// This catches the most common regression: someone writes
///   `invoke('some_command', { sessionId: x })` instead of `{ session_id: x }`
#[test]
fn frontend_invoke_keys_are_snake_case() {
    let src_dir = format!("{}/../src", env!("CARGO_MANIFEST_DIR"));
    let mut violations = Vec::new();

    visit_ts_files(std::path::Path::new(&src_dir), &mut |path, content| {
        scan_invoke_keys(path, content, &mut violations);
    });

    assert!(
        violations.is_empty(),
        "\nFound invoke() calls with non-snake_case parameter keys:\n{}\n\
         All invoke parameter keys must be snake_case (e.g. session_id, not sessionId).",
        violations.join("\n")
    );
}

/// Recursively visit .ts and .tsx files.
fn visit_ts_files(dir: &std::path::Path, visitor: &mut dyn FnMut(&str, &str)) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Skip node_modules and test fixtures
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

/// Scan a file for `invoke(` calls and check that the object literal keys
/// in the second argument are snake_case.
///
/// Uses a lightweight heuristic parser:
///   1. Find `invoke(` or `invoke<...>(`
///   2. Skip past the first arg (command name string)
///   3. If second arg starts with `{`, extract keys from the object literal
///   4. Check each key is snake_case (all lowercase + underscores, or single chars)
fn scan_invoke_keys(path: &str, content: &str, violations: &mut Vec<String>) {
    // Known exceptions: keys that are part of third-party/external protocols
    // or are single-word lowercase (which are the same in camelCase and snake_case)
    let is_snake_case = |key: &str| -> bool {
        if key.is_empty() {
            return true;
        }
        // Single lowercase word — same in both conventions
        if !key.contains(|c: char| c.is_uppercase()) && !key.contains('-') {
            return true;
        }
        false
    };

    for (line_no, line) in content.lines().enumerate() {
        let trimmed = line.trim();

        // Find invoke calls — match both `invoke(` and `invoke<Type>(`
        let mut search_from = 0;
        while let Some(pos) = trimmed[search_from..].find("invoke(") {
            let invoke_start = search_from + pos;
            // Also check invoke<...>( pattern
            let args_start = if let Some(generic_pos) = trimmed[search_from..].find("invoke<") {
                if search_from + generic_pos < invoke_start {
                    // This is invoke<Type>(...), find the opening paren
                    if let Some(paren) = trimmed[search_from + generic_pos..].find('(') {
                        search_from + generic_pos + paren + 1
                    } else {
                        search_from = invoke_start + 7;
                        continue;
                    }
                } else {
                    invoke_start + 7 // skip "invoke("
                }
            } else {
                invoke_start + 7 // skip "invoke("
            };

            // Don't flag tauri-api-bridge's own invoke replacement or type definitions
            if trimmed.contains("async function invoke")
                || trimmed.contains("window.__TAURI__")
                || trimmed.starts_with("//")
                || trimmed.starts_with("*")
                || trimmed.starts_with("export")
            {
                search_from = args_start;
                continue;
            }

            // Extract remainder after invoke(
            let rest = &trimmed[args_start..];

            // Skip the first argument (command name) — find the comma
            if let Some(comma_offset) = find_end_of_first_arg(rest) {
                let after_comma = rest[comma_offset + 1..].trim();
                if after_comma.starts_with('{') {
                    // Extract keys from the object literal
                    let keys = extract_object_keys(after_comma);
                    for key in keys {
                        if !is_snake_case(&key) {
                            violations.push(format!(
                                "  {}:{}: key \"{}\" in invoke() call",
                                path,
                                line_no + 1,
                                key
                            ));
                        }
                    }
                }
            }

            search_from = args_start;
        }
    }
}

/// Find the end of the first argument to invoke (the command name string).
/// Returns the index of the comma separating first and second args.
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
                    return None; // no second arg
                }
                depth -= 1;
            }
            ',' if depth == 0 => return Some(i),
            _ => {}
        }
    }
    None
}

/// Extract top-level keys from a JS object literal string like `{ key1: val, key2: val }`.
/// Handles shorthand properties (`{ key }`) and computed properties (`{ [expr]: val }`).
fn extract_object_keys(s: &str) -> Vec<String> {
    let mut keys = Vec::new();
    // Strip outer braces
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
                    // End of object — capture any pending shorthand
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
                // If we were still expecting_key, this was a shorthand property
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
            _ => {
                if !expecting_key {
                    // skip value content at depth 0
                }
            }
        }
    }

    keys
}

/// Check if a string is a valid JS identifier (for filtering out spread operators, etc.)
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

// ---------------------------------------------------------------------------
// IPC smoke-test: prove that `rename_all = "snake_case"` actually makes Tauri
// accept snake_case keys and reject camelCase keys.
// ---------------------------------------------------------------------------

#[tauri::command(rename_all = "snake_case")]
fn test_snake_cmd(user_name: String, age_years: u32) -> String {
    format!("{}:{}", user_name, age_years)
}

#[tauri::command]
fn test_camel_cmd(user_name: String) -> String {
    format!("camel:{}", user_name)
}

fn make_request(cmd: &str, body: serde_json::Value) -> tauri::webview::InvokeRequest {
    tauri::webview::InvokeRequest {
        cmd: cmd.into(),
        callback: tauri::ipc::CallbackFn(0),
        error: tauri::ipc::CallbackFn(1),
        url: "http://tauri.localhost".parse().unwrap(),
        body: tauri::ipc::InvokeBody::Json(body),
        headers: Default::default(),
        invoke_key: tauri::test::INVOKE_KEY.to_string(),
    }
}

#[test]
fn snake_case_command_accepts_snake_case_keys() {
    let app = tauri::test::mock_builder()
        .invoke_handler(tauri::generate_handler![test_snake_cmd, test_camel_cmd])
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("failed to build app");

    let webview =
        tauri::WebviewWindowBuilder::new(&app, "main", Default::default()).build().unwrap();

    // snake_case keys should succeed for rename_all = "snake_case" command
    let res = tauri::test::get_ipc_response(
        &webview,
        make_request(
            "test_snake_cmd",
            serde_json::json!({ "user_name": "alice", "age_years": 30 }),
        ),
    );
    assert!(res.is_ok(), "snake_case keys should be accepted: {res:?}");
    let body = res.unwrap().deserialize::<String>().unwrap();
    assert_eq!(body, "alice:30");

    // camelCase keys should FAIL for rename_all = "snake_case" command
    let res = tauri::test::get_ipc_response(
        &webview,
        make_request("test_snake_cmd", serde_json::json!({ "userName": "bob", "ageYears": 25 })),
    );
    assert!(res.is_err(), "camelCase keys should be rejected by snake_case command");
}

#[test]
fn default_camel_command_rejects_snake_case_keys() {
    let app = tauri::test::mock_builder()
        .invoke_handler(tauri::generate_handler![test_snake_cmd, test_camel_cmd])
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("failed to build app");

    let webview =
        tauri::WebviewWindowBuilder::new(&app, "main", Default::default()).build().unwrap();

    // Default (camelCase) command should reject snake_case keys
    let res = tauri::test::get_ipc_response(
        &webview,
        make_request("test_camel_cmd", serde_json::json!({ "user_name": "alice" })),
    );
    assert!(res.is_err(), "snake_case keys should be rejected by default camelCase command");

    // Default (camelCase) command should accept camelCase keys
    let res = tauri::test::get_ipc_response(
        &webview,
        make_request("test_camel_cmd", serde_json::json!({ "userName": "alice" })),
    );
    assert!(res.is_ok(), "camelCase keys should be accepted by default command: {res:?}");
}
