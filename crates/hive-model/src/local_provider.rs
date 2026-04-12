use std::sync::Arc;

use anyhow::{anyhow, Result};
use hive_inference::{
    ChatMessage, InferenceRequest, LocalModelRegistry, ModelRegistryStore, ModelStatus,
    RuntimeManager,
};

use hive_contracts::ToolDefinition;

use crate::{
    CompletionChunk, CompletionRequest, CompletionResponse, CompletionStream, FinishReason,
    ModelProvider, ModelSelection, ProviderDescriptor,
};

/// A [`ModelProvider`] backed by locally-installed models.
///
/// It looks up models in a [`LocalModelRegistry`], ensures they are loaded via
/// the [`RuntimeManager`], and delegates inference to the appropriate runtime
/// (Candle, ONNX, or llama.cpp).
pub struct LocalModelProvider {
    descriptor: ProviderDescriptor,
    registry: LocalModelRegistry,
    runtime_manager: Arc<RuntimeManager>,
}

impl LocalModelProvider {
    pub fn new(
        descriptor: ProviderDescriptor,
        registry: LocalModelRegistry,
        runtime_manager: Arc<RuntimeManager>,
    ) -> Self {
        Self { descriptor, registry, runtime_manager }
    }
}

/// Formats tool definitions as a compact catalog for local models.
///
/// Instead of dumping full schemas (which overwhelms small models), this
/// provides a lightweight listing of tool names + one-line descriptions and
/// instructs the model to call `core.discover_tools` to get full parameter
/// details before using any tool.
fn format_tools_for_prompt(tools: &[ToolDefinition]) -> String {
    if tools.is_empty() {
        return String::new();
    }

    let mut section = String::from(
        "\n\n<available_tools>\n\
         You have access to tools. To use a tool, respond with a JSON object wrapped in <tool_call> tags:\n\n\
         <tool_call>\n{\"tool\": \"tool_id\", \"input\": {\"param\": \"value\"}}\n\
         </tool_call>\n\n\
         IMPORTANT: Before using a tool for the first time, call core.discover_tools to learn its parameters.\n\
         Example: <tool_call>{\"tool\": \"core.discover_tools\", \"input\": {\"tool_ids\": [\"filesystem.read\"]}}</tool_call>\n\n\
         Available tools:\n",
    );

    for tool in tools {
        // Only include tool ID and a short description (first sentence).
        let short_desc = first_sentence(&tool.description);
        section.push_str(&format!("- {}: {}\n", tool.id, short_desc));
    }

    section.push_str("</available_tools>\n");
    section
}

/// Extract the first sentence from a description for the compact catalog.
fn first_sentence(s: &str) -> &str {
    // Find the end of the first sentence (period followed by space or end of string).
    if let Some(pos) = s.find(". ") {
        &s[..=pos]
    } else if s.len() > 120 {
        // Truncate very long descriptions
        let end = s.char_indices().nth(120).map_or(s.len(), |(i, _)| i);
        &s[..end]
    } else {
        s
    }
}

/// Build structured [`ChatMessage`]s for the inference request.
///
/// Includes tool definitions in the first system message when present, and
/// appends the user prompt as the final user message.
fn build_chat_messages(request: &CompletionRequest) -> Vec<ChatMessage> {
    let tool_section = if !request.tools.is_empty() {
        format_tools_for_prompt(&request.tools)
    } else {
        String::new()
    };

    let mut messages: Vec<ChatMessage> = Vec::with_capacity(request.messages.len() + 1);

    // Inject tool definitions into the first system message's content.
    let mut tool_section_injected = tool_section.is_empty();
    for msg in &request.messages {
        let content = if !tool_section_injected && msg.role == "system" {
            tool_section_injected = true;
            format!("{}\n{}", msg.content, tool_section)
        } else {
            msg.content.clone()
        };
        messages.push(ChatMessage { role: msg.role.clone(), content });
    }

    // If there were tools but no system message, prepend a system message.
    if !tool_section_injected {
        messages.insert(0, ChatMessage { role: "system".to_string(), content: tool_section });
    }

    // Append the user prompt as the final user message.
    if !request.prompt.is_empty() {
        messages.push(ChatMessage { role: "user".to_string(), content: request.prompt.clone() });
    }

    messages
}

/// Flat text fallback used only as the `prompt` field (runtimes that don't
/// support structured messages will read this).
fn format_messages_for_prompt(request: &CompletionRequest) -> String {
    let mut parts = request
        .messages
        .iter()
        .map(|message| match message.role.as_str() {
            "system" => format!("[System]\n{}", message.content),
            "assistant" => format!("[Assistant]\n{}", message.content),
            "user" => message.content.clone(),
            other => format!("[{}]\n{}", other, message.content),
        })
        .collect::<Vec<_>>();
    parts.push(request.prompt.clone());
    parts.join("\n\n")
}

fn do_complete(
    descriptor: &ProviderDescriptor,
    model_id: &str,
    registry: &LocalModelRegistry,
    runtime_manager: &RuntimeManager,
    request: &CompletionRequest,
) -> Result<CompletionResponse> {
    // 1. Look up the model in the registry
    let model =
        registry.get(model_id).map_err(|e| anyhow!("failed to look up model '{model_id}': {e}"))?;

    // 2. Ensure the model is in a usable state
    if model.status != ModelStatus::Available {
        return Err(anyhow!("model '{}' is not available (status: {:?})", model_id, model.status));
    }

    let runtime_kind = model.runtime;

    // 3. Load the model into the runtime if it isn't already
    if !runtime_manager.is_loaded(model_id) {
        runtime_manager
            .load_model(model_id, &model.local_path, runtime_kind)
            .map_err(|e| anyhow!("failed to load model '{model_id}': {e}"))?;
    }

    // 4. Build the inference request, injecting tool definitions when present
    let chat_messages = build_chat_messages(request);
    let prompt = format_messages_for_prompt(request);
    let prompt = if !request.tools.is_empty() {
        format!("{}{}", format_tools_for_prompt(&request.tools), prompt)
    } else {
        prompt
    };

    let inference_request = InferenceRequest {
        prompt,
        max_tokens: model.inference_params.max_tokens.or(Some(2048)),
        temperature: model.inference_params.temperature.or(Some(0.7)),
        top_p: model.inference_params.top_p,
        repeat_penalty: model.inference_params.repeat_penalty.or(Some(1.1)),
        context_length: model.inference_params.context_length.or(Some(4096)),
        stop_sequences: Vec::new(),
        messages: chat_messages,
    };

    // 5. Run inference
    let output = runtime_manager
        .infer(model_id, &inference_request)
        .map_err(|e| anyhow!("inference failed for model '{model_id}': {e}"))?;

    // 6. Map the output to a CompletionResponse
    Ok(CompletionResponse {
        provider_id: descriptor.id.clone(),
        model: model_id.to_string(),
        content: output.text,
        tool_calls: vec![],
    })
}

impl ModelProvider for LocalModelProvider {
    fn descriptor(&self) -> &ProviderDescriptor {
        &self.descriptor
    }

    fn complete_stream(
        &self,
        request: &CompletionRequest,
        selection: &ModelSelection,
    ) -> Result<CompletionStream> {
        // Build everything we need before spawning so the closure is 'static + Send.
        let descriptor = self.descriptor.clone();
        let model_id = selection.model.clone();
        let registry = self.registry.clone();
        let runtime_manager = Arc::clone(&self.runtime_manager);
        let request = request.clone();

        let stream = async_stream::try_stream! {
            // Run the heavy blocking work (model load + inference) off the
            // tokio async runtime so we don't block the executor.
            let response = tokio::task::spawn_blocking(move || {
                do_complete(&descriptor, &model_id, &registry, &runtime_manager, &request)
            })
            .await
            .map_err(|e| anyhow!("local model task panicked: {e}"))??;

            yield CompletionChunk {
                delta: response.content,
                finish_reason: Some(FinishReason::Stop),
                tool_calls: response.tool_calls,
            };
        };

        Ok(Box::pin(stream))
    }

    fn complete(
        &self,
        request: &CompletionRequest,
        selection: &ModelSelection,
    ) -> Result<CompletionResponse> {
        do_complete(
            &self.descriptor,
            &selection.model,
            &self.registry,
            &self.runtime_manager,
            request,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hive_contracts::InferenceRuntimeKind;
    use hive_contracts::{
        Capability, InferenceParams, InstalledModel, ModelCapabilities, ModelStatus, ProviderKind,
    };
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    fn test_descriptor(models: Vec<String>) -> ProviderDescriptor {
        let model_capabilities =
            models.iter().map(|m| (m.clone(), [Capability::Chat].into_iter().collect())).collect();
        ProviderDescriptor {
            id: "test-local".to_string(),
            name: None,
            kind: ProviderKind::LocalRuntime,
            models,
            model_capabilities,
            priority: 50,
            available: true,
        }
    }

    fn test_installed_model(id: &str, status: ModelStatus) -> InstalledModel {
        InstalledModel {
            id: id.to_string(),
            hub_repo: "test-org/test-model".to_string(),
            filename: "model.gguf".to_string(),
            runtime: InferenceRuntimeKind::LlamaCpp,
            capabilities: ModelCapabilities::default(),
            status,
            size_bytes: 1_000_000,
            local_path: PathBuf::from("/models/model.gguf"),
            sha256: Some("abc".to_string()),
            installed_at: "2025-01-01T00:00:00Z".to_string(),
            inference_params: InferenceParams::default(),
        }
    }

    fn test_request() -> CompletionRequest {
        CompletionRequest {
            prompt: "Hello".to_string(),
            prompt_content_parts: vec![],
            messages: vec![],
            required_capabilities: BTreeSet::new(),
            preferred_models: None,
            tools: vec![],
        }
    }

    fn test_selection(model: &str) -> ModelSelection {
        ModelSelection { provider_id: "test-local".to_string(), model: model.to_string() }
    }

    #[test]
    fn descriptor_returns_expected_fields() {
        let registry = LocalModelRegistry::open_in_memory().unwrap();
        let rt = Arc::new(RuntimeManager::new(2));
        let desc = test_descriptor(vec!["m1".to_string()]);
        let provider = LocalModelProvider::new(desc.clone(), registry, rt);

        let result = provider.descriptor();
        assert_eq!(result.id, "test-local");
        assert_eq!(result.kind, ProviderKind::LocalRuntime);
        assert_eq!(result.models, vec!["m1".to_string()]);
    }

    #[test]
    fn complete_model_not_in_registry_returns_error() {
        let registry = LocalModelRegistry::open_in_memory().unwrap();
        let rt = Arc::new(RuntimeManager::new(2));
        let provider =
            LocalModelProvider::new(test_descriptor(vec!["missing".into()]), registry, rt);

        let result = provider.complete(&test_request(), &test_selection("missing"));
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("failed to look up model 'missing'"),
            "unexpected error: {err_msg}"
        );
    }

    #[test]
    fn complete_model_not_available_returns_error() {
        let registry = LocalModelRegistry::open_in_memory().unwrap();
        let model = test_installed_model("downloading-model", ModelStatus::Downloading);
        registry.insert(&model).unwrap();

        let rt = Arc::new(RuntimeManager::new(2));
        let provider = LocalModelProvider::new(
            test_descriptor(vec!["downloading-model".into()]),
            registry,
            rt,
        );

        let result = provider.complete(&test_request(), &test_selection("downloading-model"));
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("not available"), "unexpected error: {err_msg}");
    }

    #[test]
    fn complete_model_with_error_status_returns_error() {
        let registry = LocalModelRegistry::open_in_memory().unwrap();
        let model = test_installed_model("error-model", ModelStatus::Error);
        registry.insert(&model).unwrap();

        let rt = Arc::new(RuntimeManager::new(2));
        let provider =
            LocalModelProvider::new(test_descriptor(vec!["error-model".into()]), registry, rt);

        let result = provider.complete(&test_request(), &test_selection("error-model"));
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("not available"), "unexpected error: {err_msg}");
    }

    #[test]
    fn complete_available_model_attempts_load() {
        let registry = LocalModelRegistry::open_in_memory().unwrap();
        let model = test_installed_model("good-model", ModelStatus::Available);
        registry.insert(&model).unwrap();

        let rt = Arc::new(RuntimeManager::new(2));
        let provider =
            LocalModelProvider::new(test_descriptor(vec!["good-model".into()]), registry, rt);

        // The model file doesn't exist on disk, so load_model will fail.
        // This validates the registry-to-runtime integration path.
        let result = provider.complete(&test_request(), &test_selection("good-model"));
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("failed to load model 'good-model'"),
            "unexpected error: {err_msg}"
        );
    }
}
