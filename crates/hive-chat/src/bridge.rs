//! Bridge adapters that implement hive-loop workflow engine traits
//! using HiveMind-specific types (ModelRouter, ToolRegistry, EventBus).

use std::collections::BTreeSet;
use std::sync::Arc;

use arc_swap::ArcSwap;
use hive_classification::{ChannelClass, DataClass};
use hive_contracts::{ToolAnnotations, ToolApproval, ToolDefinition};
use hive_loop::traits::{
    Message, MessageRole, ModelBackend, ModelRequest, ModelResponse, ToolBackend, ToolCall,
    ToolResult, ToolSchema, WorkflowEvent, WorkflowEventSink,
};
use hive_loop::{LoopEvent, WorkflowError, WorkflowResult};
use hive_model::{
    Capability, CompletionMessage, CompletionRequest, ModelRouter, ModelRouterError, RoutingRequest,
};
use hive_tools::ToolRegistry;

// ---------------------------------------------------------------------------
// ModelBackend adapter
// ---------------------------------------------------------------------------

/// Adapts HiveMind OS's [`ModelRouter`] to the workflow engine's [`ModelBackend`] trait.
pub struct HiveModelBackend {
    router: Arc<ArcSwap<ModelRouter>>,
    _data_class: DataClass,
}

impl HiveModelBackend {
    pub fn new(router: Arc<ArcSwap<ModelRouter>>, data_class: DataClass) -> Self {
        Self { router, _data_class: data_class }
    }
}

#[async_trait::async_trait]
impl ModelBackend for HiveModelBackend {
    async fn complete(&self, request: &ModelRequest) -> WorkflowResult<ModelResponse> {
        let router = self.router.load_full();

        let routing_prompt = messages_to_prompt(&request.messages);
        let (prompt, messages) = completion_request_parts(&request.messages);

        let tool_defs: Vec<ToolDefinition> =
            request.tools.iter().map(tool_schema_to_definition).collect();

        let routing_request = RoutingRequest {
            prompt: routing_prompt,
            required_capabilities: BTreeSet::from([Capability::Chat]),
            preferred_models: None,
        };

        let decision = router.route(&routing_request).map_err(|e| WorkflowError::Model {
            message: e.to_string(),
            error_code: None,
            http_status: None,
            provider_id: None,
            model: None,
        })?;

        let completion_request = CompletionRequest {
            prompt,
            prompt_content_parts: vec![],
            messages,
            required_capabilities: BTreeSet::from([Capability::Chat]),
            preferred_models: None,
            tools: tool_defs,
        };

        let response = router
            .complete_with_decision(&completion_request, &decision)
            .map_err(model_router_error_to_workflow_error)?;

        let tool_calls: Vec<ToolCall> = response
            .tool_calls
            .into_iter()
            .map(|tc| ToolCall { id: tc.id, name: tc.name, arguments: tc.arguments })
            .collect();

        let mut metadata = serde_json::Map::new();
        metadata.insert("provider_id".into(), serde_json::Value::String(response.provider_id));
        metadata.insert("model".into(), serde_json::Value::String(response.model));

        Ok(ModelResponse { content: response.content, tool_calls, metadata })
    }
}

/// Convert a [`ModelRouterError`] into a [`WorkflowError::Model`] preserving
/// structured error fields when available.
fn model_router_error_to_workflow_error(error: ModelRouterError) -> WorkflowError {
    match &error {
        ModelRouterError::ProviderExecutionFailed { error_kind, http_status, .. } => {
            WorkflowError::Model {
                error_code: error_kind.map(|k| format!("{k:?}").to_lowercase()),
                http_status: *http_status,
                provider_id: None,
                model: None,
                message: error.to_string(),
            }
        }
        _ => WorkflowError::Model {
            message: error.to_string(),
            error_code: None,
            http_status: None,
            provider_id: None,
            model: None,
        },
    }
}

// ---------------------------------------------------------------------------
// ToolBackend adapter
// ---------------------------------------------------------------------------

/// Adapts HiveMind OS's [`ToolRegistry`] to the workflow engine's [`ToolBackend`] trait.
pub struct HiveToolBackend {
    registry: Arc<ToolRegistry>,
}

impl HiveToolBackend {
    pub fn new(registry: Arc<ToolRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait::async_trait]
impl ToolBackend for HiveToolBackend {
    async fn list_tools(&self) -> WorkflowResult<Vec<ToolSchema>> {
        Ok(self
            .registry
            .list_definitions()
            .into_iter()
            .map(|d| ToolSchema {
                name: d.name,
                description: d.description,
                parameters: d.input_schema,
            })
            .collect())
    }

    async fn execute(&self, call: &ToolCall) -> WorkflowResult<ToolResult> {
        let tool = self
            .registry
            .get(&call.name)
            .ok_or_else(|| WorkflowError::ToolNotFound { tool_id: call.name.clone() })?;

        match tool.execute(call.arguments.clone()).await {
            Ok(result) => {
                let content = match result.output {
                    serde_json::Value::String(s) => s,
                    other => other.to_string(),
                };
                Ok(ToolResult {
                    call_id: call.id.clone(),
                    name: call.name.clone(),
                    content,
                    is_error: false,
                })
            }
            Err(e) => Ok(ToolResult {
                call_id: call.id.clone(),
                name: call.name.clone(),
                content: e.to_string(),
                is_error: true,
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// WorkflowEventSink adapter
// ---------------------------------------------------------------------------

/// Adapts a tokio broadcast channel to the workflow engine's [`WorkflowEventSink`].
///
/// Converts [`WorkflowEvent`]s to [`LoopEvent`]s for backward compatibility
/// with the existing streaming infrastructure.
pub struct BroadcastEventSink {
    tx: tokio::sync::broadcast::Sender<LoopEvent>,
}

impl BroadcastEventSink {
    pub fn new(tx: tokio::sync::broadcast::Sender<LoopEvent>) -> Self {
        Self { tx }
    }
}

#[async_trait::async_trait]
impl WorkflowEventSink for BroadcastEventSink {
    async fn emit(&self, event: WorkflowEvent) {
        let loop_event = match event {
            WorkflowEvent::TokenDelta { delta, .. } => Some(LoopEvent::Token { delta }),
            WorkflowEvent::ModelCallCompleted { content_preview, .. } => {
                Some(LoopEvent::ModelDone {
                    content: content_preview,
                    provider_id: String::new(),
                    model: String::new(),
                })
            }
            WorkflowEvent::ToolCallStarted { tool_name, .. } => {
                Some(LoopEvent::ToolCallStart { tool_id: tool_name, input: String::new() })
            }
            WorkflowEvent::ToolCallCompleted { tool_name, is_error, .. } => {
                Some(LoopEvent::ToolCallResult {
                    tool_id: tool_name,
                    output: String::new(),
                    is_error,
                })
            }
            WorkflowEvent::Completed { result, .. } => Some(LoopEvent::Done {
                content: result,
                provider_id: String::new(),
                model: String::new(),
            }),
            WorkflowEvent::Failed {
                error, error_code, http_status, provider_id, model, ..
            } => Some(LoopEvent::Error {
                message: error,
                error_code,
                http_status,
                provider_id,
                model,
            }),
            WorkflowEvent::ModelRetry {
                provider_id,
                model,
                attempt,
                max_attempts,
                error_kind,
                http_status,
                backoff_ms,
                ..
            } => Some(LoopEvent::ModelRetry {
                provider_id,
                model,
                attempt,
                max_attempts,
                error_kind,
                http_status,
                backoff_ms,
            }),
            _ => None,
        };

        if let Some(evt) = loop_event {
            if self.tx.send(evt).is_err() {
                tracing::debug!("broadcast event dropped: no active receivers");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Conversion helpers
// ---------------------------------------------------------------------------

/// Convert a [`ToolSchema`] (workflow engine) to a [`ToolDefinition`] (hive-contracts).
fn tool_schema_to_definition(schema: &ToolSchema) -> ToolDefinition {
    ToolDefinition {
        id: schema.name.clone(),
        name: schema.name.clone(),
        description: schema.description.clone(),
        input_schema: schema.parameters.clone(),
        output_schema: None,
        channel_class: ChannelClass::Internal,
        side_effects: false,
        approval: ToolApproval::Auto,
        annotations: ToolAnnotations {
            title: schema.name.clone(),
            read_only_hint: None,
            destructive_hint: None,
            idempotent_hint: None,
            open_world_hint: None,
        },
    }
}

/// Convert workflow [`Message`] list to a single prompt string.
fn messages_to_prompt(messages: &[Message]) -> String {
    let mut parts = Vec::new();
    for msg in messages {
        match msg.role {
            MessageRole::System => parts.push(format!("[System]\n{}", msg.content)),
            MessageRole::User => parts.push(msg.content.clone()),
            MessageRole::Assistant => parts.push(format!("[Assistant]\n{}", msg.content)),
            MessageRole::Tool => parts.push(format!("[Tool Result]\n{}", msg.content)),
        }
    }
    parts.join("\n\n")
}

fn completion_request_parts(messages: &[Message]) -> (String, Vec<CompletionMessage>) {
    let Some((last, history)) = messages.split_last() else {
        return (String::new(), Vec::new());
    };

    if last.role != MessageRole::User {
        return (messages_to_prompt(messages), Vec::new());
    }

    let mut completion_messages = Vec::with_capacity(history.len());
    for message in history {
        let role = match message.role {
            MessageRole::System => "system",
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::Tool => "tool",
        };
        completion_messages.push(CompletionMessage {
            role: role.to_string(),
            content: message.content.clone(),
            content_parts: vec![],
            blocks: vec![],
        });
    }

    (last.content.clone(), completion_messages)
}
