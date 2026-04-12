//! Shared tool-call policy evaluation.
//!
//! Extracted from duplicated logic in `execute_tool_call` and
//! `DataClassificationMiddleware::before_tool_call`.

use hive_classification::DataClass;
use hive_contracts::{infer_scope_with_workspace, SessionPermissions};
use hive_tools::{ToolApproval, ToolDefinition};

/// The result of resolving the base approval requirement from session
/// permissions and the tool definition's default.
///
/// This does NOT consider channel-class — it only resolves whether the
/// permission/definition says Auto, Ask, or Deny.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedApproval {
    /// Permission/definition says auto-approve.
    Auto,
    /// Permission/definition says ask the user.
    Ask,
    /// Permission/definition denies the tool outright.
    Deny { reason: String },
}

/// The result of evaluating whether a tool call should proceed.
#[derive(Debug, PartialEq)]
pub enum ToolPolicyDecision {
    /// Tool call is allowed without user interaction.
    Allow,
    /// Tool call is denied outright.
    Deny { tool_id: String, reason: String },
    /// Tool call requires user approval before proceeding.
    NeedsApproval { reason: String },
}

/// Resolves the effective approval from session permissions and tool
/// definition default, for a pre-computed resource scope.
///
/// Returns [`ResolvedApproval::Ask`] when the user should be prompted,
/// [`ResolvedApproval::Deny`] when the tool is blocked, or
/// [`ResolvedApproval::Auto`] when the tool may proceed automatically.
pub fn resolve_tool_approval(
    tool_id: &str,
    resource: &str,
    definition: &ToolDefinition,
    permissions: &SessionPermissions,
) -> ResolvedApproval {
    match permissions.resolve(tool_id, resource) {
        Some(ToolApproval::Auto) => ResolvedApproval::Auto,
        Some(ToolApproval::Deny) => ResolvedApproval::Deny {
            reason: format!(
                "Tool '{}' is denied by session permission rule (scope: {})",
                tool_id, resource
            ),
        },
        Some(ToolApproval::Ask) => ResolvedApproval::Ask,
        None => match definition.approval {
            ToolApproval::Auto => ResolvedApproval::Auto,
            ToolApproval::Deny => {
                ResolvedApproval::Deny { reason: "Tool is configured as Deny".to_string() }
            }
            ToolApproval::Ask => ResolvedApproval::Ask,
        },
    }
}

/// Pure policy evaluation — no side effects, no user interaction.
///
/// Checks session permissions, tool-definition defaults, and
/// channel-class compatibility.
pub fn evaluate_tool_policy(
    tool_id: &str,
    input: &serde_json::Value,
    definition: &ToolDefinition,
    permissions: &SessionPermissions,
    effective_data_class: DataClass,
) -> ToolPolicyDecision {
    evaluate_tool_policy_with_workspace(
        tool_id,
        input,
        definition,
        permissions,
        effective_data_class,
        None,
    )
}

/// Workspace-aware policy evaluation. Same as [`evaluate_tool_policy`] but
/// resolves relative filesystem paths against `workspace_root`.
pub fn evaluate_tool_policy_with_workspace(
    tool_id: &str,
    input: &serde_json::Value,
    definition: &ToolDefinition,
    permissions: &SessionPermissions,
    effective_data_class: DataClass,
    workspace_root: Option<&str>,
) -> ToolPolicyDecision {
    let resource = infer_scope_with_workspace(tool_id, input, workspace_root);
    let approval = resolve_tool_approval(tool_id, &resource, definition, permissions);

    let needs_approval = match approval {
        ResolvedApproval::Auto => false,
        ResolvedApproval::Deny { reason } => {
            return ToolPolicyDecision::Deny { tool_id: tool_id.to_string(), reason };
        }
        ResolvedApproval::Ask => true,
    };

    let channel_violation = !definition.channel_class.allows(effective_data_class);

    if needs_approval || channel_violation {
        let reason = if channel_violation {
            format!(
                "Tool '{}' operates on {:?} channel but session data is classified as {:?}",
                tool_id, definition.channel_class, effective_data_class
            )
        } else {
            format!("Tool '{}' requires approval", tool_id)
        };
        ToolPolicyDecision::NeedsApproval { reason }
    } else {
        ToolPolicyDecision::Allow
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hive_classification::ChannelClass;
    use hive_contracts::PermissionRule;
    use serde_json::json;

    fn make_definition(approval: ToolApproval, channel_class: ChannelClass) -> ToolDefinition {
        ToolDefinition {
            id: "test.tool".to_string(),
            name: "Test".to_string(),
            description: String::new(),
            input_schema: json!({}),
            output_schema: None,
            channel_class,
            side_effects: false,
            approval,
            annotations: hive_contracts::ToolAnnotations {
                title: "Test".to_string(),
                read_only_hint: None,
                destructive_hint: None,
                idempotent_hint: None,
                open_world_hint: None,
            },
        }
    }

    // ── evaluate_tool_policy tests ─────────────────────────────────────

    #[test]
    fn auto_approval_allows() {
        let def = make_definition(ToolApproval::Auto, ChannelClass::Internal);
        let perms = SessionPermissions::default();
        let result =
            evaluate_tool_policy("test.tool", &json!({}), &def, &perms, DataClass::Internal);
        assert_eq!(result, ToolPolicyDecision::Allow);
    }

    #[test]
    fn deny_approval_denies() {
        let def = make_definition(ToolApproval::Deny, ChannelClass::Internal);
        let perms = SessionPermissions::default();
        let result =
            evaluate_tool_policy("test.tool", &json!({}), &def, &perms, DataClass::Internal);
        assert!(matches!(result, ToolPolicyDecision::Deny { .. }));
    }

    #[test]
    fn ask_approval_needs_approval() {
        let def = make_definition(ToolApproval::Ask, ChannelClass::Internal);
        let perms = SessionPermissions::default();
        let result =
            evaluate_tool_policy("test.tool", &json!({}), &def, &perms, DataClass::Internal);
        assert!(matches!(result, ToolPolicyDecision::NeedsApproval { .. }));
    }

    #[test]
    fn channel_violation_needs_approval() {
        let def = make_definition(ToolApproval::Auto, ChannelClass::Internal);
        let perms = SessionPermissions::default();
        let result =
            evaluate_tool_policy("test.tool", &json!({}), &def, &perms, DataClass::Confidential);
        assert!(matches!(result, ToolPolicyDecision::NeedsApproval { .. }));
    }

    #[test]
    fn session_permission_override_deny() {
        let def = make_definition(ToolApproval::Auto, ChannelClass::Internal);
        let mut perms = SessionPermissions::default();
        perms.add_rule(PermissionRule {
            tool_pattern: "test.tool".to_string(),
            scope: "*".to_string(),
            decision: ToolApproval::Deny,
        });
        let result =
            evaluate_tool_policy("test.tool", &json!({}), &def, &perms, DataClass::Internal);
        assert!(matches!(result, ToolPolicyDecision::Deny { .. }));
    }

    #[test]
    fn session_permission_override_auto_allows() {
        let def = make_definition(ToolApproval::Ask, ChannelClass::Internal);
        let mut perms = SessionPermissions::default();
        perms.add_rule(PermissionRule {
            tool_pattern: "test.tool".to_string(),
            scope: "*".to_string(),
            decision: ToolApproval::Auto,
        });
        let result =
            evaluate_tool_policy("test.tool", &json!({}), &def, &perms, DataClass::Internal);
        assert_eq!(result, ToolPolicyDecision::Allow);
    }

    // ── resolve_tool_approval tests ────────────────────────────────────

    #[test]
    fn resolve_auto_from_definition() {
        let def = make_definition(ToolApproval::Auto, ChannelClass::Internal);
        let perms = SessionPermissions::default();
        let result = resolve_tool_approval("test.tool", "*", &def, &perms);
        assert_eq!(result, ResolvedApproval::Auto);
    }

    #[test]
    fn resolve_ask_from_definition() {
        let def = make_definition(ToolApproval::Ask, ChannelClass::Internal);
        let perms = SessionPermissions::default();
        let result = resolve_tool_approval("test.tool", "*", &def, &perms);
        assert_eq!(result, ResolvedApproval::Ask);
    }

    #[test]
    fn resolve_deny_from_definition() {
        let def = make_definition(ToolApproval::Deny, ChannelClass::Internal);
        let perms = SessionPermissions::default();
        let result = resolve_tool_approval("test.tool", "*", &def, &perms);
        assert!(matches!(result, ResolvedApproval::Deny { .. }));
    }

    #[test]
    fn session_permission_overrides_definition() {
        let def = make_definition(ToolApproval::Auto, ChannelClass::Internal);
        let mut perms = SessionPermissions::default();
        perms.add_rule(PermissionRule {
            tool_pattern: "test.tool".to_string(),
            scope: "*".to_string(),
            decision: ToolApproval::Ask,
        });
        let result = resolve_tool_approval("test.tool", "*", &def, &perms);
        assert_eq!(result, ResolvedApproval::Ask);
    }

    #[test]
    fn session_auto_permission_with_channel_violation_needs_approval() {
        // When session grants auto-permission but there's a channel violation,
        // the tool should still require approval (for the classification mismatch).
        let def = make_definition(ToolApproval::Ask, ChannelClass::Public);
        let mut perms = SessionPermissions::default();
        perms.add_rule(PermissionRule {
            tool_pattern: "test.tool".to_string(),
            scope: "*".to_string(),
            decision: ToolApproval::Auto,
        });
        let result =
            evaluate_tool_policy("test.tool", &json!({}), &def, &perms, DataClass::Internal);
        assert!(
            matches!(result, ToolPolicyDecision::NeedsApproval { .. }),
            "auto-permission should not bypass channel-class violation"
        );
    }
}
