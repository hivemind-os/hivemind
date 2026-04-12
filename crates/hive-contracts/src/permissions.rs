use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::tools::ToolApproval;

// ---------------------------------------------------------------------------
// Permission Rule
// ---------------------------------------------------------------------------

/// A scoped permission rule for a specific tool.
///
/// Rules match tool calls by `tool_pattern` (glob-style) and resource
/// `scope` (path glob for filesystem tools, URL pattern for http, etc.).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PermissionRule {
    /// Tool ID pattern — exact id or glob (e.g. `"filesystem.*"`, `"http.request"`).
    #[serde(alias = "toolPattern")]
    pub tool_pattern: String,
    /// Resource scope. Meaning depends on tool namespace:
    ///   - `filesystem.*`: directory glob (e.g. `"/workspace/**"`)
    ///   - `http.request`: URL pattern (e.g. `"https://api.github.com/*"`)
    ///   - `shell.execute`: `"*"` (all commands)
    ///   - other: `"*"` (any resource)
    pub scope: String,
    /// What action to take: Auto (allow), Ask (prompt), Deny (block).
    pub decision: ToolApproval,
}

// ---------------------------------------------------------------------------
// Session Permissions
// ---------------------------------------------------------------------------

/// Per-session permission state — a collection of rules checked before
/// the tool definition's built-in `ToolApproval`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SessionPermissions {
    pub rules: Vec<PermissionRule>,
}

impl SessionPermissions {
    pub fn new() -> Self {
        Self { rules: Vec::new() }
    }

    /// Creates permissions pre-populated with the given rules.
    pub fn with_rules(rules: Vec<PermissionRule>) -> Self {
        Self { rules }
    }

    /// Adds a rule. If an identical tool_pattern + scope already exists,
    /// it is replaced with the new decision.
    pub fn add_rule(&mut self, rule: PermissionRule) {
        if let Some(existing) = self
            .rules
            .iter_mut()
            .find(|r| r.tool_pattern == rule.tool_pattern && r.scope == rule.scope)
        {
            existing.decision = rule.decision;
        } else {
            self.rules.push(rule);
        }
    }

    /// Removes rules matching the given tool_pattern and scope.
    pub fn remove_rule(&mut self, tool_pattern: &str, scope: &str) {
        self.rules.retain(|r| r.tool_pattern != tool_pattern || r.scope != scope);
    }

    /// Resolves the effective permission for a tool call.
    ///
    /// Returns the `ToolApproval` from the most specific matching rule,
    /// or `None` if no rule matches (caller should fall back to tool
    /// definition's built-in approval).
    ///
    /// **Specificity**: longer/more-specific scope wins. If two rules
    /// match at equal specificity, `Deny` wins over `Ask` wins over `Auto`.
    pub fn resolve(&self, tool_id: &str, resource: &str) -> Option<ToolApproval> {
        let mut best: Option<(usize, ToolApproval)> = None;

        for rule in &self.rules {
            if !tool_pattern_matches(&rule.tool_pattern, tool_id) {
                continue;
            }
            if !scope_matches(&rule.scope, resource) {
                continue;
            }

            let specificity = scope_specificity(&rule.scope);

            let dominated = match best {
                Some((best_spec, best_decision)) => {
                    if specificity > best_spec {
                        false
                    } else if specificity == best_spec {
                        // Equal specificity: Deny > Ask > Auto (stricter wins)
                        approval_priority(rule.decision) <= approval_priority(best_decision)
                    } else {
                        true
                    }
                }
                None => false,
            };

            if !dominated {
                best = Some((specificity, rule.decision));
            }
        }

        best.map(|(_, decision)| decision)
    }
}

// ---------------------------------------------------------------------------
// Scope inference
// ---------------------------------------------------------------------------

/// Infers a scope string from a tool call's arguments.
///
/// Used when the user clicks "Allow for session" — we derive the
/// broadest reasonable scope from the current call.
pub fn infer_scope(tool_id: &str, input: &Value) -> String {
    infer_scope_with_workspace(tool_id, input, None)
}

/// Workspace-aware variant of [`infer_scope`].
///
/// When `workspace_root` is provided and the filesystem tool input contains
/// a relative path, the path is resolved against the workspace root so
/// that the resulting scope is absolute. This is critical for matching
/// against workspace auto-grant rules (which use absolute paths).
pub fn infer_scope_with_workspace(
    tool_id: &str,
    input: &Value,
    workspace_root: Option<&str>,
) -> String {
    if tool_id.starts_with("filesystem.") {
        let raw_path = input
            .get("path")
            .and_then(|v| v.as_str())
            .or_else(|| input.get("pattern").and_then(|v| v.as_str()));
        if let Some(raw) = raw_path {
            let resolved = resolve_filesystem_path(raw, workspace_root);
            return infer_filesystem_scope(&resolved);
        }
    }

    if tool_id == "http.request" {
        if let Some(url) = input.get("url").and_then(|v| v.as_str()) {
            return infer_url_scope(url);
        }
    }

    if tool_id.starts_with("comm.") {
        return infer_comm_scope(input);
    }

    if tool_id.starts_with("calendar.") {
        return infer_calendar_scope(input);
    }

    if tool_id.starts_with("drive.") {
        return infer_drive_scope(input);
    }

    if tool_id.starts_with("contacts.") {
        return infer_contacts_scope(input);
    }

    // Narrow scope to this specific tool only — never grant wildcard.
    format!("tool:{}:single_call", tool_id)
}

/// Given a file path, returns the parent directory + `/**`.
fn infer_filesystem_scope(path: &str) -> String {
    // Normalize the path to resolve `.` and `..` sequences before inferring scope.
    use std::path::{Component, PathBuf};
    let normalized = PathBuf::from(path).components().fold(PathBuf::new(), |mut acc, c| {
        match c {
            Component::ParentDir => {
                acc.pop();
            }
            Component::CurDir => {}
            other => acc.push(other),
        }
        acc
    });
    // Use forward slashes for scope strings regardless of platform.
    let path = normalized.to_string_lossy().replace('\\', "/");
    let path = path.trim_end_matches('/');

    // If path contains a parent, scope to that directory.
    if let Some(idx) = path.rfind('/') {
        let dir = &path[..idx];
        if dir.is_empty() {
            return "/**".to_string();
        }
        return format!("{dir}/**");
    }

    // Relative path with no slash — scope to current dir.
    "./**".to_string()
}

/// Resolves a filesystem path against a workspace root.
///
/// If the path is already absolute, returns it unchanged.  If relative and
/// `workspace_root` is provided, joins them to produce an absolute path.
fn resolve_filesystem_path(path: &str, workspace_root: Option<&str>) -> String {
    use std::path::Path;
    if Path::new(path).is_absolute() {
        return path.to_string();
    }
    if let Some(root) = workspace_root {
        let joined = Path::new(root).join(path);
        return joined.to_string_lossy().replace('\\', "/");
    }
    path.to_string()
}

/// Given a URL, returns the origin (scheme + host) + `/*`.
fn infer_url_scope(url: &str) -> String {
    // Find the end of scheme + host (after "://host")
    if let Some(scheme_end) = url.find("://") {
        let rest = &url[scheme_end + 3..];
        if let Some(path_start) = rest.find('/') {
            let origin = &url[..scheme_end + 3 + path_start];
            return format!("{origin}/*");
        }
        // No path component — just the origin.
        return format!("{url}/*");
    }
    // Not a valid URL — use a non-matching scope that won't grant broad access.
    "http:invalid_url".to_string()
}

/// Given a communication tool input, returns `comm:{channel_id}:{address}`.
///
/// For send: scopes to the destination address on the channel.
/// For read: scopes to all messages on the channel.
/// For list/search: scopes broadly.
fn infer_comm_scope(input: &Value) -> String {
    let channel_id = input.get("channel_id").and_then(|v| v.as_str()).unwrap_or("_unspecified");

    if let Some(to) = input.get("to").and_then(|v| v.as_str()) {
        // Extract domain for a broad-enough scope.
        if let Some(at_idx) = to.find('@') {
            let domain = &to[at_idx..]; // "@domain.com"
            return format!("comm:{channel_id}:*{domain}");
        }
        return format!("comm:{channel_id}:{to}");
    }

    format!("comm:{channel_id}:*")
}

/// Given a calendar tool input, returns `calendar:{connector_id}:*`.
fn infer_calendar_scope(input: &Value) -> String {
    let connector_id = input.get("connector_id").and_then(|v| v.as_str()).unwrap_or("_unspecified");
    format!("calendar:{connector_id}:*")
}

/// Given a drive tool input, returns `drive:{connector_id}:{path_pattern}`.
///
/// Scopes to the parent directory when a file path is provided.
fn infer_drive_scope(input: &Value) -> String {
    let connector_id = input.get("connector_id").and_then(|v| v.as_str()).unwrap_or("_unspecified");
    if let Some(path) = input.get("path").and_then(|v| v.as_str()) {
        if let Some(idx) = path.rfind('/') {
            let dir = &path[..idx];
            if !dir.is_empty() {
                return format!("drive:{connector_id}:{dir}/*");
            }
        }
        return format!("drive:{connector_id}:{path}");
    }
    format!("drive:{connector_id}:*")
}

/// Given a contacts tool input, returns `contacts:{connector_id}:*`.
fn infer_contacts_scope(input: &Value) -> String {
    let connector_id = input.get("connector_id").and_then(|v| v.as_str()).unwrap_or("_unspecified");
    format!("contacts:{connector_id}:*")
}

// ---------------------------------------------------------------------------
// Pattern matching helpers
// ---------------------------------------------------------------------------

/// Checks if a tool pattern matches a specific tool ID.
/// Supports `*` (matches everything) and `namespace.*` (matches all tools
/// in that namespace).
fn tool_pattern_matches(pattern: &str, tool_id: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if pattern == tool_id {
        return true;
    }
    // "namespace.*" matches "namespace.anything"
    if let Some(prefix) = pattern.strip_suffix(".*") {
        if let Some(tool_ns) = tool_id.split('.').next() {
            return tool_ns == prefix;
        }
    }
    false
}

/// Checks if a scope pattern matches a specific resource string.
///
/// - `*` matches everything
/// - `/path/**` matches `/path/` and anything under it
/// - `https://host/*` matches any single-level path on that host
/// - Exact match
fn scope_matches(scope: &str, resource: &str) -> bool {
    if scope == "*" {
        return true;
    }
    if scope == resource {
        return true;
    }
    // `comm:channel:*@domain` — communication address glob matching.
    if scope.starts_with("comm:") && resource.starts_with("comm:") {
        return comm_scope_matches(scope, resource);
    }
    // Bare address pattern (e.g. `*@domain.com`) against a comm resource.
    // Treat as `comm:*:<pattern>` so users can write intuitive deny rules
    // without knowing the internal `comm:channel:address` format.
    if !scope.starts_with("comm:") && scope.contains('@') && resource.starts_with("comm:") {
        let promoted = format!("comm:*:{scope}");
        return comm_scope_matches(&promoted, resource);
    }
    // `calendar:connector:operation` — calendar scope matching.
    if scope.starts_with("calendar:") && resource.starts_with("calendar:") {
        return service_scope_matches(scope, resource);
    }
    // `drive:connector:path` — drive scope matching.
    if scope.starts_with("drive:") && resource.starts_with("drive:") {
        return service_scope_matches(scope, resource);
    }
    // `contacts:connector:operation` — contacts scope matching.
    if scope.starts_with("contacts:") && resource.starts_with("contacts:") {
        return service_scope_matches(scope, resource);
    }
    // `dir/**` — matches dir and everything beneath.
    if let Some(prefix) = scope.strip_suffix("/**") {
        let prefix_normalized = normalize_filesystem_path(prefix);
        let resource_normalized = normalize_filesystem_path(resource);
        let prefix_normalized = prefix_normalized.trim_end_matches('/');
        let resource_normalized = resource_normalized.trim_end_matches('/');
        // Exact dir match or resource is inside dir.
        return resource_normalized == prefix_normalized
            || resource_normalized.starts_with(&format!("{prefix_normalized}/"));
    }
    // `origin/*` — matches origin + any single-level path.
    if let Some(prefix) = scope.strip_suffix("/*") {
        if !resource.starts_with(prefix) {
            return false;
        }
        let rest = &resource[prefix.len()..];
        if !rest.starts_with('/') {
            return false;
        }
        let rest = &rest[1..];
        return !rest.is_empty() && !rest.contains('/');
    }
    false
}

fn normalize_filesystem_path(path: &str) -> String {
    if !looks_like_filesystem_path(path) {
        return path.to_string();
    }
    use std::path::{Component, PathBuf};
    let normalized = PathBuf::from(path).components().fold(PathBuf::new(), |mut acc, c| {
        match c {
            Component::ParentDir => {
                acc.pop();
            }
            Component::CurDir => {}
            other => acc.push(other),
        }
        acc
    });
    let mut path = normalized.to_string_lossy().replace('\\', "/");
    if path.is_empty() {
        path.push('.');
    }
    path
}

fn looks_like_filesystem_path(value: &str) -> bool {
    if value.contains("://") {
        return false;
    }
    if value.starts_with("comm:")
        || value.starts_with("calendar:")
        || value.starts_with("drive:")
        || value.starts_with("contacts:")
    {
        return false;
    }
    value.starts_with('/')
        || value.starts_with("./")
        || value.starts_with("../")
        || value.contains('\\')
        || value.as_bytes().get(1) == Some(&b':')
}

/// Match communication scopes of the form `comm:{channel}:{address_pattern}`.
///
/// The address portion supports `*` wildcards:
/// - `comm:ch:*@gmail.com` matches `comm:ch:user@gmail.com`
/// - `comm:*:*` matches any channel and address
/// - `comm:ch:boss@gmail.com` exact match
fn comm_scope_matches(scope: &str, resource: &str) -> bool {
    let scope_parts: Vec<&str> = scope.splitn(3, ':').collect();
    let resource_parts: Vec<&str> = resource.splitn(3, ':').collect();

    if scope_parts.len() < 3 || resource_parts.len() < 3 {
        return scope == resource;
    }

    // Match channel part
    let scope_channel = scope_parts[1];
    let resource_channel = resource_parts[1];
    if scope_channel != "*" && scope_channel != resource_channel {
        return false;
    }

    // Match address part (glob-style)
    let scope_addr = scope_parts[2].to_lowercase();
    let resource_addr = resource_parts[2].to_lowercase();
    glob_match(&scope_addr, &resource_addr)
}

/// Match service scopes of the form `{service}:{connector_id}:{resource_pattern}`.
///
/// Works for calendar, drive, contacts, etc.
/// The resource portion supports `*` wildcards.
fn service_scope_matches(scope: &str, resource: &str) -> bool {
    let scope_parts: Vec<&str> = scope.splitn(3, ':').collect();
    let resource_parts: Vec<&str> = resource.splitn(3, ':').collect();

    if scope_parts.len() < 3 || resource_parts.len() < 3 {
        return scope == resource;
    }

    // Match connector_id
    let scope_connector = scope_parts[1];
    let resource_connector = resource_parts[1];
    if scope_connector != "*" && scope_connector != resource_connector {
        return false;
    }

    // Match resource part (glob-style)
    let scope_resource = scope_parts[2].to_lowercase();
    let resource_resource = resource_parts[2].to_lowercase();
    glob_match(&scope_resource, &resource_resource)
}

/// Simple glob matching: `*` matches any sequence of characters.
fn glob_match(pattern: &str, text: &str) -> bool {
    if pattern == "*" {
        return true;
    }

    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }

    let mut remaining = text;
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if i == 0 {
            if !remaining.starts_with(part) {
                return false;
            }
            remaining = &remaining[part.len()..];
        } else if i == parts.len() - 1 {
            if !remaining.ends_with(part) {
                return false;
            }
            remaining = "";
        } else {
            match remaining.find(part) {
                Some(idx) => remaining = &remaining[idx + part.len()..],
                None => return false,
            }
        }
    }
    true
}

/// Returns a specificity score for a scope pattern.
/// More specific (longer, less wildcard-y) = higher score.
fn scope_specificity(scope: &str) -> usize {
    if scope == "*" {
        return 0;
    }
    // Length of the non-wildcard prefix.
    scope.find("/*").unwrap_or(scope.len())
}

/// Priority for tie-breaking: Deny(2) > Ask(1) > Auto(0).
fn approval_priority(a: ToolApproval) -> u8 {
    match a {
        ToolApproval::Deny => 2,
        ToolApproval::Ask => 1,
        ToolApproval::Auto => 0,
    }
}

// ---------------------------------------------------------------------------
// Helpers for workspace auto-granting
// ---------------------------------------------------------------------------

/// Creates default permission rules for a workspace directory.
/// Grants `Auto` approval for all filesystem tools in the workspace.
pub fn workspace_permission_rules(workspace_path: &str) -> Vec<PermissionRule> {
    let scope = format!("{}/**", workspace_path.trim_end_matches('/'));
    vec![PermissionRule {
        tool_pattern: "filesystem.*".to_string(),
        scope,
        decision: ToolApproval::Auto,
    }]
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_pattern_exact_match() {
        assert!(tool_pattern_matches("filesystem.read", "filesystem.read"));
        assert!(!tool_pattern_matches("filesystem.read", "filesystem.write"));
    }

    #[test]
    fn tool_pattern_wildcard() {
        assert!(tool_pattern_matches("*", "filesystem.read"));
        assert!(tool_pattern_matches("*", "http.request"));
    }

    #[test]
    fn tool_pattern_namespace_wildcard() {
        assert!(tool_pattern_matches("filesystem.*", "filesystem.read"));
        assert!(tool_pattern_matches("filesystem.*", "filesystem.write"));
        assert!(!tool_pattern_matches("filesystem.*", "http.request"));
    }

    #[test]
    fn scope_matches_star() {
        assert!(scope_matches("*", "/any/path"));
        assert!(scope_matches("*", "https://any.url"));
    }

    #[test]
    fn scope_matches_exact() {
        assert!(scope_matches("/workspace/file.txt", "/workspace/file.txt"));
        assert!(!scope_matches("/workspace/file.txt", "/workspace/other.txt"));
    }

    #[test]
    fn scope_matches_dir_glob() {
        assert!(scope_matches("/workspace/**", "/workspace/src/main.rs"));
        assert!(scope_matches("/workspace/**", "/workspace/"));
        assert!(scope_matches("/workspace/**", "/workspace"));
        assert!(!scope_matches("/workspace/**", "/other/file.txt"));
    }

    #[test]
    fn scope_matches_url_glob() {
        assert!(scope_matches("https://api.github.com/*", "https://api.github.com/repos"));
        assert!(!scope_matches("https://api.github.com/*", "https://api.github.com/users/me"));
        assert!(!scope_matches("https://api.github.com/*", "https://evil.com/api"));
    }

    #[test]
    fn resolve_no_rules_returns_none() {
        let perms = SessionPermissions::new();
        assert_eq!(perms.resolve("filesystem.read", "/any"), None);
    }

    #[test]
    fn resolve_matching_rule() {
        let mut perms = SessionPermissions::new();
        perms.add_rule(PermissionRule {
            tool_pattern: "filesystem.*".to_string(),
            scope: "/workspace/**".to_string(),
            decision: ToolApproval::Auto,
        });
        assert_eq!(
            perms.resolve("filesystem.write", "/workspace/src/main.rs"),
            Some(ToolApproval::Auto)
        );
        assert_eq!(perms.resolve("filesystem.read", "/other/file.txt"), None);
    }

    #[test]
    fn resolve_most_specific_wins() {
        let mut perms = SessionPermissions::new();
        perms.add_rule(PermissionRule {
            tool_pattern: "filesystem.*".to_string(),
            scope: "/workspace/**".to_string(),
            decision: ToolApproval::Auto,
        });
        perms.add_rule(PermissionRule {
            tool_pattern: "filesystem.*".to_string(),
            scope: "/workspace/secrets/**".to_string(),
            decision: ToolApproval::Deny,
        });
        assert_eq!(
            perms.resolve("filesystem.write", "/workspace/src/main.rs"),
            Some(ToolApproval::Auto)
        );
        assert_eq!(
            perms.resolve("filesystem.write", "/workspace/secrets/key.pem"),
            Some(ToolApproval::Deny)
        );
    }

    #[test]
    fn resolve_deny_wins_at_equal_specificity() {
        let mut perms = SessionPermissions::new();
        perms.add_rule(PermissionRule {
            tool_pattern: "filesystem.read".to_string(),
            scope: "/workspace/**".to_string(),
            decision: ToolApproval::Auto,
        });
        perms.add_rule(PermissionRule {
            tool_pattern: "filesystem.*".to_string(),
            scope: "/workspace/**".to_string(),
            decision: ToolApproval::Deny,
        });
        assert_eq!(
            perms.resolve("filesystem.read", "/workspace/file.txt"),
            Some(ToolApproval::Deny)
        );
    }

    #[test]
    fn add_rule_replaces_duplicate() {
        let mut perms = SessionPermissions::new();
        perms.add_rule(PermissionRule {
            tool_pattern: "filesystem.*".to_string(),
            scope: "/workspace/**".to_string(),
            decision: ToolApproval::Ask,
        });
        perms.add_rule(PermissionRule {
            tool_pattern: "filesystem.*".to_string(),
            scope: "/workspace/**".to_string(),
            decision: ToolApproval::Auto,
        });
        assert_eq!(perms.rules.len(), 1);
        assert_eq!(perms.rules[0].decision, ToolApproval::Auto);
    }

    #[test]
    fn remove_rule_works() {
        let mut perms = SessionPermissions::new();
        perms.add_rule(PermissionRule {
            tool_pattern: "filesystem.*".to_string(),
            scope: "/workspace/**".to_string(),
            decision: ToolApproval::Auto,
        });
        perms.remove_rule("filesystem.*", "/workspace/**");
        assert!(perms.rules.is_empty());
    }

    #[test]
    fn infer_scope_filesystem() {
        let input = serde_json::json!({"path": "/home/user/project/src/main.rs"});
        assert_eq!(infer_scope("filesystem.write", &input), "/home/user/project/src/**");
    }

    #[test]
    fn infer_scope_filesystem_dir() {
        let input = serde_json::json!({"path": "/home/user/project/"});
        assert_eq!(infer_scope("filesystem.list", &input), "/home/user/**");
    }

    #[test]
    fn infer_scope_http() {
        let input = serde_json::json!({"url": "https://api.github.com/repos/user/repo"});
        assert_eq!(infer_scope("http.request", &input), "https://api.github.com/*");
    }

    #[test]
    fn infer_scope_shell() {
        let input = serde_json::json!({"command": "ls -la"});
        assert_eq!(infer_scope("shell.execute", &input), "tool:shell.execute:single_call");
    }

    #[test]
    fn infer_scope_unknown_tool() {
        let input = serde_json::json!({"anything": "value"});
        assert_eq!(infer_scope("custom.tool", &input), "tool:custom.tool:single_call");
    }

    // ── Workspace-aware scope inference ──────────────────────────────

    #[test]
    fn infer_scope_with_workspace_resolves_relative_path() {
        let input = serde_json::json!({"path": "src/main.rs"});
        assert_eq!(
            infer_scope_with_workspace("filesystem.write", &input, Some("/home/user/project")),
            "/home/user/project/src/**"
        );
    }

    #[test]
    fn infer_scope_with_workspace_preserves_absolute_path() {
        let input = serde_json::json!({"path": "/other/dir/file.txt"});
        assert_eq!(
            infer_scope_with_workspace("filesystem.write", &input, Some("/home/user/project")),
            "/other/dir/**"
        );
    }

    #[test]
    fn infer_scope_with_workspace_no_root_returns_relative() {
        let input = serde_json::json!({"path": "src/main.rs"});
        assert_eq!(infer_scope_with_workspace("filesystem.write", &input, None), "src/**");
    }

    #[test]
    fn workspace_autogrant_matches_relative_write() {
        // Scenario: workspace auto-grant rule + LLM sends relative path
        let mut perms = SessionPermissions::new();
        for rule in workspace_permission_rules("/home/user/project") {
            perms.add_rule(rule);
        }
        // LLM writes to "src/main.rs" → with workspace resolution → "/home/user/project/src/**"
        let input = serde_json::json!({"path": "src/main.rs"});
        let resource =
            infer_scope_with_workspace("filesystem.write", &input, Some("/home/user/project"));
        assert_eq!(resource, "/home/user/project/src/**");
        assert_eq!(perms.resolve("filesystem.write", &resource), Some(ToolApproval::Auto));
    }

    #[test]
    fn workspace_autogrant_matches_nested_relative_write() {
        // Rule: /home/user/project/**
        // Tool writes to mysql-cdc-poc/src/main.rs → resolved to /home/user/project/mysql-cdc-poc/src/**
        let mut perms = SessionPermissions::new();
        for rule in workspace_permission_rules("/home/user/project") {
            perms.add_rule(rule);
        }
        let input = serde_json::json!({"path": "mysql-cdc-poc/src/main.rs"});
        let resource =
            infer_scope_with_workspace("filesystem.write", &input, Some("/home/user/project"));
        assert_eq!(resource, "/home/user/project/mysql-cdc-poc/src/**");
        assert_eq!(perms.resolve("filesystem.write", &resource), Some(ToolApproval::Auto));
    }

    #[test]
    fn parent_glob_matches_child_glob() {
        // User's exact scenario: rule mysql-cdc-poc/** should match mysql-cdc-poc/src/**
        let mut perms = SessionPermissions::new();
        perms.add_rule(PermissionRule {
            tool_pattern: "filesystem.write".to_string(),
            scope: "mysql-cdc-poc/**".to_string(),
            decision: ToolApproval::Auto,
        });
        assert_eq!(
            perms.resolve("filesystem.write", "mysql-cdc-poc/src/**"),
            Some(ToolApproval::Auto)
        );
    }

    #[test]
    fn workspace_rules_creates_filesystem_auto() {
        let rules = workspace_permission_rules("/home/user/project");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].tool_pattern, "filesystem.*");
        assert_eq!(rules[0].scope, "/home/user/project/**");
        assert_eq!(rules[0].decision, ToolApproval::Auto);
    }

    #[test]
    fn workspace_rules_trims_trailing_slash() {
        let rules = workspace_permission_rules("/home/user/project/");
        assert_eq!(rules[0].scope, "/home/user/project/**");
    }

    // ── Communication scope tests ──────────────────────────────────

    #[test]
    fn infer_scope_comm_send_with_address() {
        let input = serde_json::json!({
            "channel_id": "work-email",
            "to": "alice@outlook.com",
            "body": "hello"
        });
        assert_eq!(
            infer_scope("comm.send_external_message", &input),
            "comm:work-email:*@outlook.com"
        );
    }

    #[test]
    fn infer_scope_comm_send_no_domain() {
        let input = serde_json::json!({
            "channel_id": "slack",
            "to": "#general",
            "body": "hello"
        });
        assert_eq!(infer_scope("comm.send_external_message", &input), "comm:slack:#general");
    }

    #[test]
    fn infer_scope_comm_read() {
        let input = serde_json::json!({ "channel_id": "work-email" });
        assert_eq!(infer_scope("comm.read_messages", &input), "comm:work-email:*");
    }

    #[test]
    fn infer_scope_connector_list() {
        let input = serde_json::json!({});
        assert_eq!(infer_scope("connector.list", &input), "tool:connector.list:single_call");
    }

    #[test]
    fn comm_scope_matches_exact() {
        assert!(scope_matches(
            "comm:work-email:alice@outlook.com",
            "comm:work-email:alice@outlook.com"
        ));
    }

    #[test]
    fn comm_scope_matches_domain_glob() {
        assert!(scope_matches(
            "comm:work-email:*@outlook.com",
            "comm:work-email:alice@outlook.com"
        ));
        assert!(!scope_matches("comm:work-email:*@outlook.com", "comm:work-email:alice@gmail.com"));
    }

    #[test]
    fn comm_scope_matches_wildcard_channel() {
        assert!(scope_matches("comm:*:*@outlook.com", "comm:work-email:alice@outlook.com"));
    }

    #[test]
    fn comm_scope_matches_all() {
        assert!(scope_matches("comm:*:*", "comm:work-email:anyone@anywhere.com"));
    }

    #[test]
    fn comm_scope_resolve_rules() {
        let mut perms = SessionPermissions::new();
        perms.add_rule(PermissionRule {
            tool_pattern: "comm.send_external_message".to_string(),
            scope: "comm:work-email:*@outlook.com".to_string(),
            decision: ToolApproval::Auto,
        });
        perms.add_rule(PermissionRule {
            tool_pattern: "comm.send_external_message".to_string(),
            scope: "comm:work-email:*@gmail.com".to_string(),
            decision: ToolApproval::Ask,
        });
        perms.add_rule(PermissionRule {
            tool_pattern: "comm.send_external_message".to_string(),
            scope: "comm:work-email:boss@gmail.com".to_string(),
            decision: ToolApproval::Deny,
        });

        assert_eq!(
            perms.resolve("comm.send_external_message", "comm:work-email:alice@outlook.com"),
            Some(ToolApproval::Auto)
        );
        assert_eq!(
            perms.resolve("comm.send_external_message", "comm:work-email:random@gmail.com"),
            Some(ToolApproval::Ask)
        );
        assert_eq!(
            perms.resolve("comm.send_external_message", "comm:work-email:boss@gmail.com"),
            Some(ToolApproval::Deny)
        );
    }

    // ── Connector service scope tests ──────────────────────────────

    #[test]
    fn infer_calendar_scope_with_connector() {
        let input = serde_json::json!({"connector_id": "work-ms"});
        assert_eq!(infer_scope("calendar.list_events", &input), "calendar:work-ms:*");
    }

    #[test]
    fn infer_drive_scope_with_path() {
        let input =
            serde_json::json!({"connector_id": "work-ms", "path": "/Documents/report.docx"});
        assert_eq!(infer_scope("drive.read_file", &input), "drive:work-ms:/Documents/*");
    }

    #[test]
    fn infer_contacts_scope_test() {
        let input = serde_json::json!({"connector_id": "work-ms"});
        assert_eq!(infer_scope("contacts.search", &input), "contacts:work-ms:*");
    }

    #[test]
    fn service_scope_matches_calendar() {
        assert!(scope_matches("calendar:work-ms:*", "calendar:work-ms:read"));
        assert!(scope_matches("calendar:*:*", "calendar:work-ms:write"));
        assert!(!scope_matches("calendar:work-ms:*", "calendar:personal:read"));
    }

    #[test]
    fn service_scope_matches_drive() {
        assert!(scope_matches(
            "drive:work-ms:/Documents/*",
            "drive:work-ms:/Documents/report.docx"
        ));
        assert!(scope_matches("drive:work-ms:*", "drive:work-ms:/any/path"));
        assert!(!scope_matches("drive:work-ms:*", "drive:personal:file.txt"));
    }

    #[test]
    fn service_scope_matches_contacts() {
        assert!(scope_matches("contacts:*:*", "contacts:work-ms:read"));
        assert!(scope_matches("contacts:work-ms:*", "contacts:work-ms:search"));
        assert!(!scope_matches("contacts:work-ms:*", "contacts:personal:read"));
    }

    #[test]
    fn bare_email_pattern_matches_comm_resource() {
        // User writes `*@domain.com` → should match `comm:any-channel:*@domain.com`
        assert!(scope_matches("*@domain.com", "comm:work-email:*@domain.com"));
        assert!(scope_matches("*@domain.com", "comm:personal:user@domain.com"));
        assert!(!scope_matches("*@domain.com", "comm:work-email:*@other.com"));

        // Exact email address
        assert!(scope_matches("boss@corp.com", "comm:ch1:boss@corp.com"));
        assert!(!scope_matches("boss@corp.com", "comm:ch1:other@corp.com"));

        // Non-email scope should NOT be promoted
        assert!(!scope_matches("some-pattern", "comm:ch:addr"));
    }

    #[test]
    fn bare_email_deny_rule_blocks_send() {
        let mut perms = SessionPermissions::new();
        perms.add_rule(PermissionRule {
            tool_pattern: "comm.send_external_message".to_string(),
            scope: "*@domain.com".to_string(),
            decision: ToolApproval::Deny,
        });

        // infer_scope for comm.send_external_message to me@domain.com
        // produces something like "comm:*:*@domain.com"
        let input = serde_json::json!({ "to": "me@domain.com" });
        let resource = infer_scope("comm.send_external_message", &input);

        let decision = perms.resolve("comm.send_external_message", &resource);
        assert_eq!(
            decision,
            Some(ToolApproval::Deny),
            "bare email pattern must deny; resource={resource}"
        );
    }
}
