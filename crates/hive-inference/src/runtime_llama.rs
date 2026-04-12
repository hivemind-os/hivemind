//! llama.cpp-based inference runtime via the `llama-cpp-2` crate.
//!
//! Primary runtime for text generation with GGUF quantized models.
//! Supports Gemma, Llama, Mistral, and other architectures in GGUF format.

use encoding_rs::UTF_8;
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaChatMessage, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};

use crate::runtime::{
    FinishReason, InferenceError, InferenceOutput, InferenceRequest, InferenceRuntime, RuntimeInfo,
};
use hive_core::InferenceRuntimeKind;

// ---------------------------------------------------------------------------
// Internal model storage
// ---------------------------------------------------------------------------

struct LoadedLlamaModel {
    model: LlamaModel,
    #[allow(dead_code)]
    model_path: PathBuf,
    memory_bytes: u64,
}

// SAFETY: LoadedLlamaModel wraps llama.cpp model pointer that is read-only after
// loading. LlamaCppRuntime serializes all inference calls through &self.
unsafe impl Send for LoadedLlamaModel {}
unsafe impl Sync for LoadedLlamaModel {}

// ---------------------------------------------------------------------------
// LlamaCppRuntime
// ---------------------------------------------------------------------------

pub struct LlamaCppRuntime {
    backend: Mutex<Option<LlamaBackend>>,
    loaded_models: Mutex<HashMap<String, LoadedLlamaModel>>,
}

// SAFETY: LoadedLlamaModel wraps llama.cpp model pointer that is read-only after
// loading. LlamaCppRuntime serializes all inference calls through &self.
unsafe impl Send for LlamaCppRuntime {}
unsafe impl Sync for LlamaCppRuntime {}

impl Default for LlamaCppRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl LlamaCppRuntime {
    pub fn new() -> Self {
        Self { backend: Mutex::new(None), loaded_models: Mutex::new(HashMap::new()) }
    }

    /// Ensure the llama.cpp backend is initialized (lazy init on first use).
    fn ensure_backend(&self) -> Result<(), InferenceError> {
        let mut backend = self.backend.lock();
        if backend.is_none() {
            let b = LlamaBackend::init().map_err(|e| InferenceError::RuntimeUnavailable {
                runtime: InferenceRuntimeKind::LlamaCpp,
                reason: format!("Failed to initialize llama.cpp backend: {e}"),
            })?;
            *backend = Some(b);
        }
        Ok(())
    }

    /// Apply the model's native chat template to structured messages.
    ///
    /// Falls back to a simple text concatenation if the model has no embedded
    /// chat template or if template application fails.
    fn apply_chat_template_for_messages(
        &self,
        model_data: &LoadedLlamaModel,
        messages: &[crate::runtime::ChatMessage],
    ) -> Result<String, InferenceError> {
        // Try to get the model's embedded chat template
        let tmpl = match model_data.model.chat_template(None) {
            Ok(t) => t,
            Err(_) => {
                tracing::warn!("model has no embedded chat template, using fallback formatting");
                return Ok(Self::fallback_format_messages(messages));
            }
        };

        // Convert to LlamaChatMessage (strips NUL bytes to avoid CString errors)
        let chat_messages: Vec<LlamaChatMessage> = messages
            .iter()
            .filter_map(|m| {
                let role = m.role.replace('\0', "");
                let content = m.content.replace('\0', "");
                LlamaChatMessage::new(role, content).ok()
            })
            .collect();

        // add_ass=true so the template ends with the assistant opening tag,
        // priming the model to generate an assistant response.
        match model_data.model.apply_chat_template(&tmpl, &chat_messages, true) {
            Ok(formatted) => Ok(formatted),
            Err(e) => {
                tracing::warn!("chat template application failed: {e}, using fallback formatting");
                Ok(Self::fallback_format_messages(messages))
            }
        }
    }

    /// Simple fallback for models without a chat template.
    fn fallback_format_messages(messages: &[crate::runtime::ChatMessage]) -> String {
        messages
            .iter()
            .map(|m| match m.role.as_str() {
                "system" => format!("[System]\n{}", m.content),
                "assistant" => format!("[Assistant]\n{}", m.content),
                "user" => format!("[User]\n{}", m.content),
                other => format!("[{}]\n{}", other, m.content),
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

impl InferenceRuntime for LlamaCppRuntime {
    fn kind(&self) -> InferenceRuntimeKind {
        InferenceRuntimeKind::LlamaCpp
    }

    fn is_available(&self) -> bool {
        true // llama.cpp is compiled in when the feature is enabled
    }

    fn info(&self) -> RuntimeInfo {
        let loaded = self.loaded_models.lock();
        let memory: u64 = loaded.values().map(|m| m.memory_bytes).sum();
        RuntimeInfo {
            kind: InferenceRuntimeKind::LlamaCpp,
            version: "b5000".to_string(),
            supports_gpu: false, // GPU support requires CUDA/Vulkan features
            loaded_model: loaded.keys().next().cloned(),
            memory_used_bytes: memory,
        }
    }

    fn load_model(&self, model_id: &str, model_path: &Path) -> Result<(), InferenceError> {
        if !model_path.exists() {
            return Err(InferenceError::ModelFileNotFound(model_path.display().to_string()));
        }

        // Validate file format
        let ext = model_path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if !["gguf", "ggml"].contains(&ext) {
            return Err(InferenceError::UnsupportedFormat {
                runtime: InferenceRuntimeKind::LlamaCpp,
                format: ext.to_string(),
            });
        }

        self.ensure_backend()?;

        let backend_guard = self.backend.lock();
        let backend = backend_guard.as_ref().ok_or_else(|| InferenceError::RuntimeUnavailable {
            runtime: InferenceRuntimeKind::LlamaCpp,
            reason: "Backend not initialized".to_string(),
        })?;

        let model_params = LlamaModelParams::default();
        let model_params = std::pin::pin!(model_params);

        let model = LlamaModel::load_from_file(backend, model_path, &model_params)
            .map_err(|e| InferenceError::LoadFailed(format!("Failed to load GGUF model: {e}")))?;

        let memory_bytes = std::fs::metadata(model_path).map(|m| m.len()).unwrap_or(0);

        let loaded = LoadedLlamaModel { model, model_path: model_path.to_path_buf(), memory_bytes };

        // Drop the backend guard before locking loaded_models to avoid deadlock potential
        drop(backend_guard);

        self.loaded_models.lock().insert(model_id.to_string(), loaded);
        tracing::info!(
            runtime = "llama.cpp",
            model_id,
            path = %model_path.display(),
            "model loaded"
        );
        Ok(())
    }

    fn unload_model(&self, model_id: &str) -> Result<(), InferenceError> {
        self.loaded_models.lock().remove(model_id);
        tracing::info!(runtime = "llama.cpp", model_id, "model unloaded");
        Ok(())
    }

    fn is_model_loaded(&self, model_id: &str) -> bool {
        self.loaded_models.lock().contains_key(model_id)
    }

    fn infer(
        &self,
        model_id: &str,
        request: &InferenceRequest,
    ) -> Result<InferenceOutput, InferenceError> {
        // Check model loaded first (before backend), for better error messages
        {
            let loaded = self.loaded_models.lock();
            if !loaded.contains_key(model_id) {
                return Err(InferenceError::ModelNotLoaded { model_id: model_id.to_string() });
            }
        }

        // We need to hold both the backend and models locks
        let backend_guard = self.backend.lock();
        let backend = backend_guard.as_ref().ok_or_else(|| InferenceError::RuntimeUnavailable {
            runtime: InferenceRuntimeKind::LlamaCpp,
            reason: "Backend not initialized".to_string(),
        })?;

        let loaded = self.loaded_models.lock();
        let model_data = loaded
            .get(model_id)
            .ok_or_else(|| InferenceError::ModelNotLoaded { model_id: model_id.to_string() })?;

        let max_tokens = request.max_tokens.unwrap_or(2048) as i32;
        let temperature = request.temperature.unwrap_or(0.7);

        // Build the prompt text. When structured messages are provided, use
        // the model's native chat template for correct formatting (e.g. Llama
        // special tokens). Fall back to the raw prompt string otherwise.
        let prompt_text = if !request.messages.is_empty() {
            self.apply_chat_template_for_messages(model_data, &request.messages)?
        } else {
            request.prompt.clone()
        };

        // Tokenize the prompt first so we can size the context properly.
        // Use AddBos::Never when we applied a chat template (the template
        // already includes the BOS token).
        let add_bos = if !request.messages.is_empty() { AddBos::Never } else { AddBos::Always };
        let tokens = model_data
            .model
            .str_to_token(&prompt_text, add_bos)
            .map_err(|e| InferenceError::InferenceFailed(format!("Tokenization failed: {e}")))?;

        if tokens.is_empty() {
            return Err(InferenceError::InferenceFailed(
                "tokenization produced no tokens".to_string(),
            ));
        }

        let n_prompt = i32::try_from(tokens.len()).map_err(|_| {
            InferenceError::InferenceFailed("prompt too long for llama context".to_string())
        })?;
        let n_len = n_prompt
            .checked_add(max_tokens)
            .ok_or_else(|| InferenceError::InferenceFailed("prompt length overflow".to_string()))?;

        // Create a context sized to fit the full conversation (prompt + generation).
        let context_length = request.context_length.unwrap_or(4096);
        let min_ctx = (n_len as u32).max(context_length);
        let n_ctx = NonZeroU32::new(min_ctx).unwrap_or(NonZeroU32::new(4096).unwrap());
        // n_batch must be >= the largest single-decode chunk. We set it to n_ctx
        // so any prompt that fits the context can be decoded in one call.
        let n_batch = min_ctx;
        let ctx_params =
            LlamaContextParams::default().with_n_ctx(Some(n_ctx)).with_n_batch(n_batch);

        let mut ctx = model_data.model.new_context(backend, ctx_params).map_err(|e| {
            InferenceError::InferenceFailed(format!("Failed to create context: {e}"))
        })?;

        // Evaluate the prompt in chunks of n_batch tokens so we never exceed
        // the context's batch limit.
        let batch_size = n_batch as usize;
        for (chunk_idx, chunk) in tokens.chunks(batch_size).enumerate() {
            let offset = chunk_idx * batch_size;
            let is_last_chunk = offset + chunk.len() == tokens.len();
            let mut batch = LlamaBatch::new(chunk.len(), 1);
            for (i, token) in chunk.iter().enumerate() {
                let pos = (offset + i) as i32;
                let logits = is_last_chunk && i == chunk.len() - 1;
                batch.add(*token, pos, &[0], logits).map_err(|e| {
                    InferenceError::InferenceFailed(format!("Batch add failed: {e}"))
                })?;
            }
            ctx.decode(&mut batch).map_err(|e| {
                InferenceError::InferenceFailed(format!("Prompt evaluation failed: {e}"))
            })?;
        }

        // Set up sampler — apply repeat penalty before temperature scaling
        // to prevent local models from entering repetitive generation loops.
        let repeat_penalty = request.repeat_penalty.unwrap_or(1.1);
        let penalty_last_n = 64; // penalize over last 64 tokens
        let mut sampler = if temperature < 0.01 {
            if repeat_penalty > 1.0 {
                LlamaSampler::chain_simple([
                    LlamaSampler::penalties(penalty_last_n, repeat_penalty, 0.0, 0.0),
                    LlamaSampler::greedy(),
                ])
            } else {
                LlamaSampler::chain_simple([LlamaSampler::greedy()])
            }
        } else if repeat_penalty > 1.0 {
            LlamaSampler::chain_simple([
                LlamaSampler::penalties(penalty_last_n, repeat_penalty, 0.0, 0.0),
                LlamaSampler::temp(temperature),
                LlamaSampler::dist(1234),
            ])
        } else {
            LlamaSampler::chain_simple([LlamaSampler::temp(temperature), LlamaSampler::dist(1234)])
        };

        // Autoregressive generation loop
        let mut output_text = String::new();
        let mut n_cur = n_prompt;
        let mut n_generated: u64 = 0;
        let mut finish_reason = FinishReason::Length;
        let mut decoder = UTF_8.new_decoder();

        // For sampling: after prompt eval the logits are at the last batch position.
        // On the first iteration we use the last chunk's batch size - 1; after that
        // each decode has exactly 1 token at index 0.
        let last_chunk_len = tokens.len() % batch_size;
        let last_chunk_len = if last_chunk_len == 0 { batch_size } else { last_chunk_len };
        let mut sample_idx = (last_chunk_len - 1) as i32;

        while n_cur < n_len {
            let token = sampler.sample(&ctx, sample_idx);
            sampler.accept(token);

            // Check for end of generation
            if model_data.model.is_eog_token(token) {
                finish_reason = FinishReason::Stop;
                break;
            }

            // Decode token to text
            match model_data.model.token_to_piece(token, &mut decoder, true, None) {
                Ok(piece) => {
                    // Check stop sequences
                    output_text.push_str(&piece);
                    let should_stop =
                        request.stop_sequences.iter().any(|stop| output_text.contains(stop));
                    if should_stop {
                        // Trim the stop sequence from output
                        for stop in &request.stop_sequences {
                            if let Some(idx) = output_text.find(stop) {
                                output_text.truncate(idx);
                                break;
                            }
                        }
                        finish_reason = FinishReason::Stop;
                        break;
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to decode token {}: {}", token.0, e);
                }
            }

            n_generated += 1;

            // Prepare next batch (single token)
            let mut batch = LlamaBatch::new(1, 1);
            batch
                .add(token, n_cur, &[0], true)
                .map_err(|e| InferenceError::InferenceFailed(format!("Batch add failed: {e}")))?;

            ctx.decode(&mut batch)
                .map_err(|e| InferenceError::InferenceFailed(format!("Decode failed: {e}")))?;

            n_cur += 1;
            // After first generation decode, batch always has 1 token at index 0.
            sample_idx = 0;
        }

        Ok(InferenceOutput {
            text: output_text,
            tokens_used: n_generated + tokens.len() as u64,
            finish_reason,
        })
    }

    fn embed(&self, model_id: &str, text: &str) -> Result<Vec<f32>, InferenceError> {
        // Check model loaded first
        {
            let loaded = self.loaded_models.lock();
            if !loaded.contains_key(model_id) {
                return Err(InferenceError::ModelNotLoaded { model_id: model_id.to_string() });
            }
        }

        let backend_guard = self.backend.lock();
        let backend = backend_guard.as_ref().ok_or_else(|| InferenceError::RuntimeUnavailable {
            runtime: InferenceRuntimeKind::LlamaCpp,
            reason: "Backend not initialized".to_string(),
        })?;

        let loaded = self.loaded_models.lock();
        let model_data = loaded
            .get(model_id)
            .ok_or_else(|| InferenceError::ModelNotLoaded { model_id: model_id.to_string() })?;

        // Create context with embedding mode enabled, batch size matching context
        let n_ctx = NonZeroU32::new(2048).unwrap();
        let ctx_params = LlamaContextParams::default()
            .with_n_ctx(Some(n_ctx))
            .with_n_batch(2048)
            .with_embeddings(true);

        let mut ctx = model_data.model.new_context(backend, ctx_params).map_err(|e| {
            InferenceError::InferenceFailed(format!("Failed to create embedding context: {e}"))
        })?;

        // Tokenize
        let tokens = model_data
            .model
            .str_to_token(text, AddBos::Always)
            .map_err(|e| InferenceError::InferenceFailed(format!("Tokenization failed: {e}")))?;

        // Create batch with all tokens
        let mut batch = LlamaBatch::new(tokens.len().max(1), 1);
        for (i, token) in tokens.iter().enumerate() {
            batch
                .add(*token, i as i32, &[0], true)
                .map_err(|e| InferenceError::InferenceFailed(format!("Batch add failed: {e}")))?;
        }

        // Run forward pass
        ctx.decode(&mut batch).map_err(|e| {
            InferenceError::InferenceFailed(format!("Embedding forward pass failed: {e}"))
        })?;

        // Extract embeddings (pooled over sequence)
        let embeddings = ctx.embeddings_seq_ith(0).map_err(|e| {
            InferenceError::InferenceFailed(format!("Failed to extract embeddings: {e}"))
        })?;

        Ok(embeddings.to_vec())
    }

    fn supported_formats(&self) -> Vec<String> {
        vec!["gguf".into(), "ggml".into()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn llama_runtime_reports_correct_kind() {
        let rt = LlamaCppRuntime::new();
        assert_eq!(rt.kind(), InferenceRuntimeKind::LlamaCpp);
    }

    #[test]
    fn llama_runtime_is_available() {
        let rt = LlamaCppRuntime::new();
        assert!(rt.is_available());
    }

    #[test]
    fn llama_runtime_supported_formats() {
        let rt = LlamaCppRuntime::new();
        let formats = rt.supported_formats();
        assert!(formats.contains(&"gguf".to_string()));
    }
}
