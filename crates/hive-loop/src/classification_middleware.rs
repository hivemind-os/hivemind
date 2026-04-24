//! Data-classification middleware for the agent loop.
//!
//! Responsibilities:
//! - **`after_tool_result`**: For file-read tools (`filesystem.read`,
//!   `filesystem.read_document`), resolves the workspace classification for the
//!   file's path, overrides `result.data_class` with the resolved class, and
//!   escalates the session's `effective_data_class` high-water mark.
//! - **`before_tool_call`**: For outbound-channel tools, checks whether the
//!   session's `effective_data_class` is compatible with the tool's
//!   `channel_class`.  Returns `ToolDenied` when the channel cannot carry the
//!   data and no approval override is possible.

use crate::legacy::{
    AgentContext, ConversationContext, LoopContext, LoopError, LoopMiddleware, RoutingConfig,
    SecurityContext, ToolsContext,
};
use hive_classification::DataClass;
use hive_connectors::ConnectorServiceHandle;
use hive_tools::ToolResult;
use serde_json::Value;
use std::sync::Arc;

/// Middleware that enforces data-classification policy on tool calls.
///
/// Construction requires an optional `ConnectorServiceHandle` — when present,
/// the connector's outbound classification is checked for send tools.
pub struct DataClassificationMiddleware {
    connector_service: Option<Arc<dyn ConnectorServiceHandle>>,
}

impl DataClassificationMiddleware {
    pub fn new(connector_service: Option<Arc<dyn ConnectorServiceHandle>>) -> Self {
        Self { connector_service }
    }
}

/// Tool IDs whose execution reads file content and should resolve workspace
/// classification.
const FILE_READ_TOOLS: &[&str] =
    &["fs.read", "fs.read_document", "filesystem.read", "filesystem.read_document"];

impl LoopMiddleware for DataClassificationMiddleware {
    fn before_tool_call(
        &self,
        context: &LoopContext,
        call: crate::legacy::ToolCall,
    ) -> Result<crate::legacy::ToolCall, LoopError> {
        let tool = match context.tools().get(&call.tool_id) {
            Some(t) => t,
            None => return Ok(call), // unknown tool — let execute_tool_call handle it
        };
        let definition = tool.definition();
        let effective_dc = context.effective_data_class();

        // ── Channel-class hard denial ──────────────────────────────────
        // If the tool's channel cannot carry data at the current
        // sensitivity AND the tool/session has no approval path, deny
        // immediately.
        if !definition.channel_class.allows(effective_dc) {
            let has_ask_path = {
                let perms = context.permissions().lock();
                let workspace_str =
                    context.workspace_path().map(|p| p.to_string_lossy().to_string());
                let resource = hive_contracts::infer_scope_with_workspace(
                    &call.tool_id,
                    &call.input,
                    workspace_str.as_deref(),
                );
                !matches!(
                    crate::tool_policy::resolve_tool_approval(
                        &call.tool_id,
                        &resource,
                        definition,
                        &perms,
                    ),
                    crate::tool_policy::ResolvedApproval::Deny { .. }
                )
            };

            if !has_ask_path {
                return Err(LoopError::ToolDenied {
                    tool_id: call.tool_id.clone(),
                    reason: format!(
                        "{} data cannot be sent over {} channel (tool allows up to {})",
                        effective_dc,
                        definition.channel_class,
                        definition.channel_class.max_allowed(),
                    ),
                });
            }
            // When approval IS possible the inline approval-dialog code in
            // execute_tool_call will present the violation reason to the user.
        }

        // ── Connector output-class hard denial ─────────────────────────
        // For send tools, check whether the connector's outbound class
        // can carry the session data.  Only hard-deny here; the async
        // approval override path is handled inline in execute_tool_call.
        if call.tool_id == "comm.send_external_message" {
            if let Some(ref svc) = self.connector_service {
                let connector_id = call.input.get("connector_id").and_then(|v| v.as_str());
                let to = call.input.get("to").and_then(|v| v.as_str());
                if let (Some(cid), Some(dest)) = (connector_id, to) {
                    let output_class =
                        svc.resolve_output_class(cid, dest).unwrap_or(DataClass::Internal);
                    if output_class < effective_dc {
                        tracing::info!(
                            connector_id = cid,
                            output_class = %output_class,
                            effective_dc = %effective_dc,
                            "connector output-class violation detected"
                        );
                        // Don't hard-deny here — the inline code will present
                        // an approval dialog.  We just annotate the call so
                        // the inline code knows a violation exists.
                        // (The inline code re-checks the same condition.)
                    }
                }
            }
        }

        Ok(call)
    }

    fn after_tool_result(
        &self,
        context: &LoopContext,
        tool_id: &str,
        tool_input: Option<&Value>,
        mut result: ToolResult,
    ) -> Result<ToolResult, LoopError> {
        // Only file-read tools resolve workspace classification.
        if !FILE_READ_TOOLS.contains(&tool_id) {
            return Ok(result);
        }

        let path = match tool_input.and_then(|v| v.get("path")).and_then(|v| v.as_str()) {
            Some(p) => p,
            None => return Ok(result),
        };

        let classification = match context.workspace_classification() {
            Some(c) => c,
            None => return Ok(result),
        };

        let resolved_class = classification.resolve(path);
        tracing::info!(
            tool_id = %tool_id,
            path = %path,
            resolved_class = %resolved_class,
            ws_default = %classification.default,
            overrides = ?classification.overrides,
            "workspace classification resolved for file read"
        );

        result.data_class = resolved_class;

        // Escalate the session high-water mark.
        let before = context.effective_data_class();
        context.escalate_data_class(resolved_class);
        let after = context.effective_data_class();
        if after != before {
            tracing::info!(
                before = %before,
                after = %after,
                "effective_data_class escalated"
            );
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::legacy::{LoopContext, ToolCall};
    use hive_classification::DataClass;
    use hive_contracts::{
        PermissionRule, SessionPermissions, ToolExecutionMode, WorkspaceClassification,
    };
    use hive_tools::{Tool, ToolApproval, ToolDefinition, ToolRegistry};
    use serde_json::json;
    use std::collections::BTreeSet;
    use std::sync::atomic::{AtomicU8, Ordering};
    use std::sync::Arc;

    /// Minimal read tool for testing.
    struct FakeReadTool(ToolDefinition);
    impl FakeReadTool {
        fn new() -> Self {
            Self(ToolDefinition {
                id: "filesystem.read".to_string(),
                name: "Read".to_string(),
                description: "mock".to_string(),
                input_schema: json!({"type": "object"}),
                output_schema: None,
                channel_class: hive_classification::ChannelClass::LocalOnly,
                side_effects: false,
                approval: ToolApproval::Auto,
                annotations: hive_tools::ToolAnnotations {
                    title: "Read".to_string(),
                    read_only_hint: Some(true),
                    destructive_hint: None,
                    idempotent_hint: None,
                    open_world_hint: None,
                },
            })
        }
    }
    impl Tool for FakeReadTool {
        fn definition(&self) -> &ToolDefinition {
            &self.0
        }
        fn execute(
            &self,
            _input: Value,
        ) -> hive_tools::BoxFuture<'_, Result<ToolResult, hive_tools::ToolError>> {
            Box::pin(async {
                Ok(ToolResult {
                    output: json!({"content": "file data"}),
                    data_class: DataClass::Internal, // hardcoded — middleware should override
                })
            })
        }
    }

    fn make_context(wc: WorkspaceClassification, effective_dc: Arc<AtomicU8>) -> LoopContext {
        LoopContext {
            conversation: ConversationContext {
                session_id: "test".to_string(),
                message_id: "msg".to_string(),
                prompt: String::new(),
                prompt_content_parts: vec![],
                history: vec![],
                conversation_journal: None,
                initial_tool_iterations: 0,
            },
            routing: RoutingConfig {
                required_capabilities: BTreeSet::new(),
                preferred_models: None,
                loop_strategy: None,
                routing_decision: None,
            },
            security: SecurityContext {
                data_class: DataClass::Public,
                permissions: Arc::new(parking_lot::Mutex::new(SessionPermissions::new())),
                workspace_classification: Some(Arc::new(wc)),
                effective_data_class: effective_dc,
                connector_service: None,
                shadow_mode: false,
            },
            tools_ctx: ToolsContext {
                tools: Arc::new(hive_tools::ToolRegistry::new()),
                skill_catalog: None,
                knowledge_query_handler: None,
                tool_execution_mode: ToolExecutionMode::default(),
            },
            agent: AgentContext {
                persona: None,
                agent_orchestrator: None,
                workspace_path: None,
                personas: Vec::new(),
                current_agent_id: None,
                parent_agent_id: None,
                keep_alive: false,
                session_messaged: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            },
            tool_limits: hive_contracts::ToolLimitsConfig::default(),
            preempt_signal: None,
            cancellation_token: None,
        }
    }

    #[test]
    fn after_tool_result_resolves_classification_for_file_read() {
        let mut wc = WorkspaceClassification::new(DataClass::Public);
        wc.set_override("secret.txt", DataClass::Restricted);

        let effective_dc = Arc::new(AtomicU8::new(DataClass::Public.to_i64() as u8));
        let ctx = make_context(wc, effective_dc.clone());

        let mw = DataClassificationMiddleware::new(None);
        let result = ToolResult {
            output: json!({"content": "data"}),
            data_class: DataClass::Internal, // tool hardcoded
        };
        let input = json!({"path": "secret.txt"});

        let out = mw.after_tool_result(&ctx, "filesystem.read", Some(&input), result).unwrap();

        assert_eq!(out.data_class, DataClass::Restricted, "must resolve to override");
        assert_eq!(
            DataClass::from_i64(effective_dc.load(Ordering::Relaxed) as i64).unwrap(),
            DataClass::Restricted,
            "must escalate effective_data_class"
        );
    }

    #[test]
    fn after_tool_result_ignores_non_read_tools() {
        let wc = WorkspaceClassification::new(DataClass::Restricted);
        let effective_dc = Arc::new(AtomicU8::new(DataClass::Public.to_i64() as u8));
        let ctx = make_context(wc, effective_dc.clone());

        let mw = DataClassificationMiddleware::new(None);
        let result = ToolResult { output: json!({"ok": true}), data_class: DataClass::Internal };
        let input = json!({"path": "."});

        let out = mw.after_tool_result(&ctx, "filesystem.list", Some(&input), result).unwrap();

        assert_eq!(out.data_class, DataClass::Internal, "must NOT override for non-read tool");
        assert_eq!(
            DataClass::from_i64(effective_dc.load(Ordering::Relaxed) as i64).unwrap(),
            DataClass::Public,
            "must NOT escalate for non-read tool"
        );
    }

    #[test]
    fn after_tool_result_uses_default_when_no_override() {
        let wc = WorkspaceClassification::default(); // Internal default
        let effective_dc = Arc::new(AtomicU8::new(DataClass::Internal.to_i64() as u8));
        let ctx = make_context(wc, effective_dc.clone());

        let mw = DataClassificationMiddleware::new(None);
        let result = ToolResult {
            output: json!({"content": "data"}),
            data_class: DataClass::Internal, // tool hardcoded
        };
        let input = json!({"path": "any_file.txt"});

        let out = mw.after_tool_result(&ctx, "filesystem.read", Some(&input), result).unwrap();

        assert_eq!(out.data_class, DataClass::Internal, "default is Internal");
        assert_eq!(
            DataClass::from_i64(effective_dc.load(Ordering::Relaxed) as i64).unwrap(),
            DataClass::Internal,
            "stays at Internal"
        );
    }

    fn make_channel_tool(
        id: &str,
        channel_class: hive_classification::ChannelClass,
        approval: ToolApproval,
    ) -> Arc<dyn Tool> {
        struct ChannelTool(ToolDefinition);
        impl Tool for ChannelTool {
            fn definition(&self) -> &ToolDefinition {
                &self.0
            }
            fn execute(
                &self,
                _input: Value,
            ) -> hive_tools::BoxFuture<'_, Result<ToolResult, hive_tools::ToolError>> {
                Box::pin(async {
                    Ok(ToolResult { output: json!({"ok": true}), data_class: DataClass::Public })
                })
            }
        }
        Arc::new(ChannelTool(ToolDefinition {
            id: id.to_string(),
            name: id.to_string(),
            description: "mock".to_string(),
            input_schema: json!({"type": "object"}),
            output_schema: None,
            channel_class,
            side_effects: true,
            approval,
            annotations: hive_tools::ToolAnnotations {
                title: id.to_string(),
                read_only_hint: None,
                destructive_hint: None,
                idempotent_hint: None,
                open_world_hint: None,
            },
        }))
    }

    #[test]
    fn before_tool_call_denies_channel_violation_when_tool_denied() {
        use hive_classification::ChannelClass;

        let tool_id = "test.public_channel";
        let mut registry = ToolRegistry::new();
        registry
            .register(make_channel_tool(tool_id, ChannelClass::Public, ToolApproval::Deny))
            .unwrap();

        // effective_data_class = Internal, channel only allows Public,
        // AND tool is Deny → should hard-deny in middleware
        let effective_dc = Arc::new(AtomicU8::new(DataClass::Internal.to_i64() as u8));
        let wc = WorkspaceClassification::default();
        let mut ctx = make_context(wc, effective_dc);
        ctx.tools_ctx.tools = Arc::new(registry);

        let mw = DataClassificationMiddleware::new(None);
        let call = ToolCall { tool_id: tool_id.to_string(), input: json!({}) };

        let err = mw.before_tool_call(&ctx, call).unwrap_err();
        match err {
            LoopError::ToolDenied { reason, .. } => {
                assert!(reason.contains("cannot be sent"), "reason: {reason}");
            }
            other => panic!("expected ToolDenied, got: {other}"),
        }
    }

    #[test]
    fn before_tool_call_allows_channel_violation_with_auto_approval() {
        use hive_classification::ChannelClass;

        // Tool has Auto approval (definition default) + channel violation.
        // Middleware should pass through so execute_tool_call shows the
        // approval dialog for the classification mismatch.
        let tool_id = "test.public_channel";
        let mut registry = ToolRegistry::new();
        registry
            .register(make_channel_tool(tool_id, ChannelClass::Public, ToolApproval::Auto))
            .unwrap();

        let effective_dc = Arc::new(AtomicU8::new(DataClass::Internal.to_i64() as u8));
        let wc = WorkspaceClassification::default();
        let mut ctx = make_context(wc, effective_dc);
        ctx.tools_ctx.tools = Arc::new(registry);

        let mw = DataClassificationMiddleware::new(None);
        let call = ToolCall { tool_id: tool_id.to_string(), input: json!({}) };

        // Should NOT deny — must pass through to execute_tool_call
        let result = mw.before_tool_call(&ctx, call);
        assert!(result.is_ok(), "Auto-approved tool with channel violation should pass through to approval dialog, not hard-deny");
    }

    #[test]
    fn before_tool_call_allows_channel_violation_with_session_auto_permission() {
        use hive_classification::ChannelClass;
        use hive_contracts::PermissionRule;

        // Tool has Ask approval by default, but session grants Auto.
        // Channel violation should still pass through (not hard-deny).
        let tool_id = "http.request";
        let mut registry = ToolRegistry::new();
        registry
            .register(make_channel_tool(tool_id, ChannelClass::Public, ToolApproval::Ask))
            .unwrap();

        let effective_dc = Arc::new(AtomicU8::new(DataClass::Internal.to_i64() as u8));
        let wc = WorkspaceClassification::default();
        let mut ctx = make_context(wc, effective_dc);
        ctx.tools_ctx.tools = Arc::new(registry);

        // Grant auto-permission for http.request via session permissions
        {
            let mut perms = ctx.security.permissions.lock();
            perms.add_rule(PermissionRule {
                tool_pattern: tool_id.to_string(),
                scope: "*".to_string(),
                decision: ToolApproval::Auto,
            });
        }

        let mw = DataClassificationMiddleware::new(None);
        let call = ToolCall { tool_id: tool_id.to_string(), input: json!({}) };

        let result = mw.before_tool_call(&ctx, call);
        assert!(result.is_ok(), "Session auto-permission with channel violation should pass through to approval dialog, not hard-deny");
    }

    #[test]
    fn before_tool_call_allows_channel_violation_with_ask_approval() {
        use hive_classification::ChannelClass;

        // Tool has Ask approval + channel violation → pass through
        let tool_id = "test.public_channel";
        let mut registry = ToolRegistry::new();
        registry
            .register(make_channel_tool(tool_id, ChannelClass::Public, ToolApproval::Ask))
            .unwrap();

        let effective_dc = Arc::new(AtomicU8::new(DataClass::Internal.to_i64() as u8));
        let wc = WorkspaceClassification::default();
        let mut ctx = make_context(wc, effective_dc);
        ctx.tools_ctx.tools = Arc::new(registry);

        let mw = DataClassificationMiddleware::new(None);
        let call = ToolCall { tool_id: tool_id.to_string(), input: json!({}) };

        let result = mw.before_tool_call(&ctx, call);
        assert!(
            result.is_ok(),
            "Ask-approved tool with channel violation should pass through to approval dialog"
        );
    }
}
