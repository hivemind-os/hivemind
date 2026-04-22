//! Proxy tool that bridges LLM tool calls to MCP App iframes via the
//! [`UserInteractionGate`](hive_loop::legacy::UserInteractionGate).
//!
//! When the LLM calls an app-registered tool, the proxy:
//! 1. Publishes an `AppToolCall` interaction request
//! 2. Awaits the oneshot response (frontend routes to iframe, gets result)
//! 3. Returns the tool result to the LLM

use std::sync::Arc;
use std::time::Duration;

use hive_classification::DataClass;
use hive_contracts::{InteractionKind, InteractionResponsePayload, ToolDefinition};
use serde_json::{json, Value};

use crate::{BoxFuture, Tool, ToolError, ToolResult};

/// Callback type for creating interaction requests and awaiting responses.
/// This decouples AppToolProxy from the concrete UserInteractionGate type
/// (which lives in hive-loop, a downstream crate).
pub type InteractionRequestFn = Arc<
    dyn Fn(String, InteractionKind) -> tokio::sync::oneshot::Receiver<hive_contracts::UserInteractionResponse>
        + Send
        + Sync,
>;

/// Callback type for publishing events when an app tool call is requested.
/// The daemon publishes this so the frontend can route the call to the iframe.
pub type AppToolEventFn = Arc<dyn Fn(AppToolCallEvent) + Send + Sync>;

/// Event emitted when the daemon needs the frontend to execute an app tool.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AppToolCallEvent {
    pub request_id: String,
    pub app_instance_id: String,
    pub tool_name: String,
    pub arguments: Value,
    pub session_id: String,
}

pub struct AppToolProxy {
    definition: ToolDefinition,
    app_instance_id: String,
    app_tool_name: String,
    session_id: String,
    interaction_request_fn: InteractionRequestFn,
    event_fn: AppToolEventFn,
    timeout: Duration,
}

impl AppToolProxy {
    pub fn new(
        tool_id: String,
        app_tool_name: String,
        description: String,
        input_schema: Value,
        app_instance_id: String,
        session_id: String,
        interaction_request_fn: InteractionRequestFn,
        event_fn: AppToolEventFn,
    ) -> Self {
        let definition = ToolDefinition {
            id: tool_id.clone(),
            name: tool_id,
            description,
            input_schema: input_schema,
            output_schema: None,
            channel_class: hive_classification::ChannelClass::Internal,
            side_effects: false,
            approval: hive_contracts::ToolApproval::Auto,
            annotations: hive_contracts::ToolAnnotations {
                title: app_tool_name.clone(),
                read_only_hint: Some(true),
                destructive_hint: Some(false),
                idempotent_hint: None,
                open_world_hint: None,
            },
        };

        Self {
            definition,
            app_instance_id,
            app_tool_name,
            session_id,
            interaction_request_fn,
            event_fn,
            timeout: Duration::from_secs(30),
        }
    }
}

impl Tool for AppToolProxy {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        let request_id = format!(
            "app-tool:{}:{}:{}",
            self.app_instance_id,
            self.app_tool_name,
            uuid::Uuid::new_v4()
        );

        let kind = InteractionKind::AppToolCall {
            app_instance_id: self.app_instance_id.clone(),
            tool_name: self.app_tool_name.clone(),
            arguments: input,
        };

        // Create the pending interaction (oneshot channel)
        let rx = (self.interaction_request_fn)(request_id.clone(), kind);

        // Publish event so frontend can route to the correct iframe
        (self.event_fn)(AppToolCallEvent {
            request_id,
            app_instance_id: self.app_instance_id.clone(),
            tool_name: self.app_tool_name.clone(),
            arguments: json!({}), // arguments are in the interaction kind
            session_id: self.session_id.clone(),
        });

        let timeout = self.timeout;

        Box::pin(async move {
            match tokio::time::timeout(timeout, rx).await {
                Ok(Ok(response)) => match response.payload {
                    InteractionResponsePayload::AppToolCallResult { content, is_error } => {
                        let output = if is_error {
                            json!({ "error": content })
                        } else {
                            content
                        };
                        Ok(ToolResult {
                            output,
                            data_class: DataClass::Internal,
                        })
                    }
                    _ => Err(ToolError::ExecutionFailed(
                        "unexpected interaction response type".to_string(),
                    )),
                },
                Ok(Err(_)) => Err(ToolError::ExecutionFailed(
                    "app tool call cancelled (bridge destroyed)".to_string(),
                )),
                Err(_) => Err(ToolError::ExecutionFailed(format!(
                    "app tool call timed out after {}s",
                    timeout.as_secs()
                ))),
            }
        })
    }
}
