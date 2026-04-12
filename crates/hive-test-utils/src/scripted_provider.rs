//! A programmable mock LLM provider for integration tests.
//!
//! `ScriptedProvider` implements [`ModelProvider`] and returns pre-scripted
//! [`CompletionResponse`] sequences, including tool calls.  Responses can be
//! routed to different agents by matching on the system-prompt substring
//! (typically the persona name).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use hive_model::{
    Capability, CompletionChunk, CompletionRequest, CompletionResponse, CompletionStream,
    FinishReason, ModelProvider, ModelSelection, ProviderDescriptor, ProviderKind,
    ToolCallResponse,
};
use serde_json::Value as JsonValue;

/// A single scripted rule: if the concatenated conversation contains `needle`,
/// use the associated FIFO queue of responses.
struct ScriptedRule {
    needle: String,
    responses: VecDeque<CompletionResponse>,
}

struct ScriptedProviderState {
    rules: Vec<ScriptedRule>,
    default_responses: VecDeque<CompletionResponse>,
    calls: Vec<RecordedCall>,
}

/// A record of every `complete()` invocation, for post-test assertions.
#[derive(Debug, Clone)]
pub struct RecordedCall {
    pub prompt: String,
    pub system_prompt: String,
    pub tool_names: Vec<String>,
}

/// A programmable mock LLM that returns scripted responses from per-persona
/// FIFO queues, matched by system-prompt substring.
pub struct ScriptedProvider {
    descriptor: ProviderDescriptor,
    state: Mutex<ScriptedProviderState>,
    /// Optional shared observer for recording calls even after the provider
    /// is moved into a `ModelRouter`.
    shared_calls: Option<Arc<Mutex<Vec<RecordedCall>>>>,
}

impl ScriptedProvider {
    pub fn new(provider_id: &str, model: &str) -> Self {
        Self {
            descriptor: ProviderDescriptor {
                id: provider_id.to_string(),
                name: Some(provider_id.to_string()),
                kind: ProviderKind::Mock,
                models: vec![model.to_string()],
                model_capabilities: BTreeMap::from([(
                    model.to_string(),
                    BTreeSet::from([Capability::Chat, Capability::ToolUse]),
                )]),
                priority: 10,
                available: true,
            },
            state: Mutex::new(ScriptedProviderState {
                rules: Vec::new(),
                default_responses: VecDeque::new(),
                calls: Vec::new(),
            }),
            shared_calls: None,
        }
    }

    /// Attach a shared call recorder. After `build_model_router` consumes the
    /// provider, you can still read recorded calls via this `Arc`.
    pub fn with_shared_calls(mut self, shared: Arc<Mutex<Vec<RecordedCall>>>) -> Self {
        self.shared_calls = Some(shared);
        self
    }

    /// Add a rule: when the conversation's system prompt contains `needle`,
    /// dequeue from these scripted responses (in order).
    pub fn on_system_contains(
        self,
        needle: impl Into<String>,
        responses: Vec<CompletionResponse>,
    ) -> Self {
        self.state
            .lock()
            .expect("poisoned")
            .rules
            .push(ScriptedRule { needle: needle.into(), responses: VecDeque::from(responses) });
        self
    }

    /// Fallback responses when no rule matches.
    pub fn default_responses(self, responses: Vec<CompletionResponse>) -> Self {
        self.state.lock().expect("poisoned").default_responses = VecDeque::from(responses);
        self
    }

    /// Return all recorded calls for assertions.
    pub fn calls(&self) -> Vec<RecordedCall> {
        self.state.lock().expect("poisoned").calls.clone()
    }

    /// Return the total number of `complete()` invocations.
    pub fn call_count(&self) -> usize {
        self.state.lock().expect("poisoned").calls.len()
    }

    // ── Helper constructors for scripted responses ──────────────────────

    /// Build a text-only response (no tool calls).
    pub fn text_response(provider_id: &str, model: &str, content: &str) -> CompletionResponse {
        CompletionResponse {
            provider_id: provider_id.to_string(),
            model: model.to_string(),
            content: content.to_string(),
            tool_calls: vec![],
        }
    }

    /// Build a response that invokes a single tool call.
    pub fn tool_call_response(
        provider_id: &str,
        model: &str,
        call_id: &str,
        tool_name: &str,
        arguments: JsonValue,
    ) -> CompletionResponse {
        CompletionResponse {
            provider_id: provider_id.to_string(),
            model: model.to_string(),
            content: String::new(),
            tool_calls: vec![ToolCallResponse {
                id: call_id.to_string(),
                name: tool_name.to_string(),
                arguments,
            }],
        }
    }

    /// Build a response with multiple tool calls.
    pub fn multi_tool_response(
        provider_id: &str,
        model: &str,
        calls: Vec<(String, String, JsonValue)>,
    ) -> CompletionResponse {
        CompletionResponse {
            provider_id: provider_id.to_string(),
            model: model.to_string(),
            content: String::new(),
            tool_calls: calls
                .into_iter()
                .map(|(id, name, args)| ToolCallResponse { id, name, arguments: args })
                .collect(),
        }
    }
}

impl ModelProvider for ScriptedProvider {
    fn descriptor(&self) -> &ProviderDescriptor {
        &self.descriptor
    }

    fn complete(
        &self,
        request: &CompletionRequest,
        selection: &ModelSelection,
    ) -> Result<CompletionResponse> {
        let mut state = self.state.lock().expect("poisoned");

        // Extract the system prompt from the conversation messages.
        let system_prompt = request
            .messages
            .iter()
            .filter(|m| m.role == "system")
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        // Record the call.
        let recorded = RecordedCall {
            prompt: request.prompt.clone(),
            system_prompt: system_prompt.clone(),
            tool_names: request.tools.iter().map(|t| t.name.clone()).collect(),
        };
        state.calls.push(recorded.clone());
        // Also push to shared observer if attached.
        drop(state);
        if let Some(ref shared) = self.shared_calls {
            shared.lock().expect("shared poisoned").push(recorded);
        }
        let mut state = self.state.lock().expect("poisoned");

        // Try each rule in order; first match with a queued response wins.
        for rule in &mut state.rules {
            if system_prompt.contains(&rule.needle) || request.prompt.contains(&rule.needle) {
                if let Some(mut resp) = rule.responses.pop_front() {
                    resp.provider_id = selection.provider_id.clone();
                    resp.model = selection.model.clone();
                    return Ok(resp);
                }
            }
        }

        // Fallback to default queue.
        if let Some(mut resp) = state.default_responses.pop_front() {
            resp.provider_id = selection.provider_id.clone();
            resp.model = selection.model.clone();
            return Ok(resp);
        }

        // Last resort: a plain "done" text.
        Ok(CompletionResponse {
            provider_id: selection.provider_id.clone(),
            model: selection.model.clone(),
            content: "Task complete.".to_string(),
            tool_calls: vec![],
        })
    }

    fn complete_stream(
        &self,
        request: &CompletionRequest,
        selection: &ModelSelection,
    ) -> Result<CompletionStream> {
        let response = self.complete(request, selection)?;
        let finish_reason = if response.tool_calls.is_empty() {
            FinishReason::Stop
        } else {
            FinishReason::ToolCalls
        };
        let chunk = CompletionChunk {
            delta: response.content,
            finish_reason: Some(finish_reason),
            tool_calls: response.tool_calls,
        };
        Ok(Box::pin(tokio_stream::once(Ok(chunk))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_by_system_prompt() {
        let provider = ScriptedProvider::new("mock", "m1")
            .on_system_contains(
                "researcher",
                vec![ScriptedProvider::text_response("mock", "m1", "I am researching")],
            )
            .on_system_contains(
                "executor",
                vec![ScriptedProvider::text_response("mock", "m1", "I am executing")],
            )
            .default_responses(vec![ScriptedProvider::text_response("mock", "m1", "default")]);

        let sel = ModelSelection { provider_id: "mock".into(), model: "m1".into() };

        // Researcher persona match
        let req = CompletionRequest {
            prompt: "Do research".into(),
            prompt_content_parts: vec![],
            messages: vec![hive_model::CompletionMessage::text(
                "system",
                "You are a researcher agent",
            )],
            required_capabilities: BTreeSet::new(),
            preferred_models: None,
            tools: vec![],
        };
        let resp = provider.complete(&req, &sel).unwrap();
        assert_eq!(resp.content, "I am researching");

        // Executor persona match
        let req2 = CompletionRequest {
            prompt: "Execute task".into(),
            prompt_content_parts: vec![],
            messages: vec![hive_model::CompletionMessage::text(
                "system",
                "You are an executor agent",
            )],
            required_capabilities: BTreeSet::new(),
            preferred_models: None,
            tools: vec![],
        };
        let resp2 = provider.complete(&req2, &sel).unwrap();
        assert_eq!(resp2.content, "I am executing");

        // No match → default
        let req3 = CompletionRequest {
            prompt: "Something else".into(),
            prompt_content_parts: vec![],
            messages: vec![hive_model::CompletionMessage::text("system", "You are a bot")],
            required_capabilities: BTreeSet::new(),
            preferred_models: None,
            tools: vec![],
        };
        let resp3 = provider.complete(&req3, &sel).unwrap();
        assert_eq!(resp3.content, "default");

        assert_eq!(provider.call_count(), 3);
    }

    #[test]
    fn returns_tool_calls() {
        let provider = ScriptedProvider::new("mock", "m1").default_responses(vec![
            ScriptedProvider::tool_call_response(
                "mock",
                "m1",
                "tc1",
                "core.ask_user",
                serde_json::json!({"question": "What color?"}),
            ),
        ]);

        let sel = ModelSelection { provider_id: "mock".into(), model: "m1".into() };
        let req = CompletionRequest {
            prompt: "Ask a question".into(),
            prompt_content_parts: vec![],
            messages: vec![],
            required_capabilities: BTreeSet::new(),
            preferred_models: None,
            tools: vec![],
        };

        let resp = provider.complete(&req, &sel).unwrap();
        assert_eq!(resp.tool_calls.len(), 1);
        assert_eq!(resp.tool_calls[0].name, "core.ask_user");
    }
}
