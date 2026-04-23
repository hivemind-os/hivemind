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

#[cfg(test)]
mod tests {
    use super::*;
    use hive_contracts::UserInteractionResponse;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn make_proxy(
        interaction_fn: InteractionRequestFn,
        event_fn: AppToolEventFn,
    ) -> AppToolProxy {
        AppToolProxy::new(
            "app.test1234.get_data".to_string(),
            "get_data".to_string(),
            "Gets data from app".to_string(),
            json!({"type": "object"}),
            "test-instance-1234".to_string(),
            "session-1".to_string(),
            interaction_fn,
            event_fn,
        )
    }

    #[test]
    fn definition_has_correct_fields() {
        let interaction_fn: InteractionRequestFn = Arc::new(|_id, _kind| {
            let (_tx, rx) = tokio::sync::oneshot::channel();
            rx
        });
        let event_fn: AppToolEventFn = Arc::new(|_| {});
        let proxy = make_proxy(interaction_fn, event_fn);

        let def = proxy.definition();
        assert_eq!(def.id, "app.test1234.get_data");
        assert_eq!(def.annotations.title, "get_data");
        assert_eq!(def.approval, hive_contracts::ToolApproval::Auto);
    }

    #[tokio::test]
    async fn execute_success_returns_tool_result() {
        let interaction_fn: InteractionRequestFn = Arc::new(|_id, _kind| {
            let (tx, rx) = tokio::sync::oneshot::channel();
            tx.send(UserInteractionResponse {
                request_id: "ignored".to_string(),
                payload: InteractionResponsePayload::AppToolCallResult {
                    content: json!({"answer": 42}),
                    is_error: false,
                },
            })
            .unwrap();
            rx
        });
        let event_fired = Arc::new(AtomicBool::new(false));
        let event_fired2 = event_fired.clone();
        let event_fn: AppToolEventFn = Arc::new(move |_evt| {
            event_fired2.store(true, Ordering::SeqCst);
        });

        let proxy = make_proxy(interaction_fn, event_fn);
        let result = proxy.execute(json!({"query": "test"})).await.unwrap();
        assert_eq!(result.output["answer"], 42);
        assert!(event_fired.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn execute_error_response_wraps_in_error_json() {
        let interaction_fn: InteractionRequestFn = Arc::new(|_id, _kind| {
            let (tx, rx) = tokio::sync::oneshot::channel();
            tx.send(UserInteractionResponse {
                request_id: "ignored".to_string(),
                payload: InteractionResponsePayload::AppToolCallResult {
                    content: json!("something went wrong"),
                    is_error: true,
                },
            })
            .unwrap();
            rx
        });
        let event_fn: AppToolEventFn = Arc::new(|_| {});

        let proxy = make_proxy(interaction_fn, event_fn);
        let result = proxy.execute(json!({})).await.unwrap();
        assert_eq!(result.output["error"], "something went wrong");
    }

    #[tokio::test]
    async fn execute_cancelled_returns_error() {
        let interaction_fn: InteractionRequestFn = Arc::new(|_id, _kind| {
            let (tx, rx) = tokio::sync::oneshot::channel::<UserInteractionResponse>();
            drop(tx); // simulate bridge destroyed
            rx
        });
        let event_fn: AppToolEventFn = Arc::new(|_| {});

        let proxy = make_proxy(interaction_fn, event_fn);
        let err = proxy.execute(json!({})).await.unwrap_err();
        match err {
            ToolError::ExecutionFailed(msg) => {
                assert!(msg.contains("cancelled"), "got: {msg}");
            }
            _ => panic!("expected ExecutionFailed"),
        }
    }

    #[tokio::test]
    async fn execute_timeout_returns_error() {
        // We need the sender to stay alive (not dropped) so the receiver
        // doesn't get a RecvError before the timeout fires.
        use std::sync::Mutex;
        let held_tx: Arc<Mutex<Option<tokio::sync::oneshot::Sender<UserInteractionResponse>>>> =
            Arc::new(Mutex::new(None));
        let held_tx2 = held_tx.clone();

        let interaction_fn: InteractionRequestFn = Arc::new(move |_id, _kind| {
            let (tx, rx) = tokio::sync::oneshot::channel();
            *held_tx2.lock().unwrap() = Some(tx); // keep sender alive
            rx
        });
        let event_fn: AppToolEventFn = Arc::new(|_| {});

        let mut proxy = make_proxy(interaction_fn, event_fn);
        proxy.timeout = Duration::from_millis(10);

        let err = proxy.execute(json!({})).await.unwrap_err();
        match err {
            ToolError::ExecutionFailed(msg) => {
                assert!(msg.contains("timed out"), "got: {msg}");
            }
            _ => panic!("expected ExecutionFailed"),
        }
    }

    #[tokio::test]
    async fn execute_wrong_payload_type_errors() {
        let interaction_fn: InteractionRequestFn = Arc::new(|_id, _kind| {
            let (tx, rx) = tokio::sync::oneshot::channel();
            tx.send(UserInteractionResponse {
                request_id: "ignored".to_string(),
                payload: InteractionResponsePayload::Answer {
                    selected_choice: None,
                    selected_choices: None,
                    text: Some("wrong type".to_string()),
                },
            })
            .unwrap();
            rx
        });
        let event_fn: AppToolEventFn = Arc::new(|_| {});

        let proxy = make_proxy(interaction_fn, event_fn);
        let err = proxy.execute(json!({})).await.unwrap_err();
        match err {
            ToolError::ExecutionFailed(msg) => {
                assert!(msg.contains("unexpected"), "got: {msg}");
            }
            _ => panic!("expected ExecutionFailed"),
        }
    }
}
