use hive_core::InferenceRuntimeKind;
#[cfg(not(all(feature = "candle", feature = "onnx", feature = "llama-cpp")))]
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
#[cfg(not(all(feature = "candle", feature = "onnx", feature = "llama-cpp")))]
use std::collections::HashMap;
use std::path::Path;
#[cfg(not(any(feature = "candle", feature = "onnx", feature = "llama-cpp")))]
use std::path::PathBuf;
use thiserror::Error;

// ---------------------------------------------------------------------------
// Core inference trait
// ---------------------------------------------------------------------------

/// Output from an inference request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceOutput {
    pub text: String,
    pub tokens_used: u64,
    pub finish_reason: FinishReason,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum FinishReason {
    Stop,
    Length,
    Error,
}

/// A role/content pair for structured chat messages.
///
/// When provided in [`InferenceRequest::messages`], runtimes that support
/// native chat templates (e.g. llama.cpp) will use them instead of the raw
/// `prompt` field.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

/// Request to run inference.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferenceRequest {
    pub prompt: String,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub repeat_penalty: Option<f32>,
    pub context_length: Option<u32>,
    pub stop_sequences: Vec<String>,
    /// Structured chat messages for runtimes that support native chat
    /// templates. When non-empty, runtimes should prefer these over `prompt`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<ChatMessage>,
}

impl Default for InferenceRequest {
    fn default() -> Self {
        Self {
            prompt: String::new(),
            max_tokens: Some(2048),
            temperature: Some(0.7),
            top_p: None,
            repeat_penalty: None,
            context_length: None,
            stop_sequences: Vec::new(),
            messages: Vec::new(),
        }
    }
}

/// Metadata about a loaded runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeInfo {
    pub kind: InferenceRuntimeKind,
    pub version: String,
    pub supports_gpu: bool,
    pub loaded_model: Option<String>,
    pub memory_used_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Error)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum InferenceError {
    #[error("model not loaded: {model_id}")]
    ModelNotLoaded { model_id: String },
    #[error("runtime {runtime:?} is not available: {reason}")]
    RuntimeUnavailable { runtime: InferenceRuntimeKind, reason: String },
    #[error("failed to load model: {0}")]
    LoadFailed(String),
    #[error("inference failed: {0}")]
    InferenceFailed(String),
    #[error("model file not found: {0}")]
    ModelFileNotFound(String),
    #[error("unsupported model format for runtime {runtime:?}: {format}")]
    UnsupportedFormat { runtime: InferenceRuntimeKind, format: String },
    #[error("{0}")]
    Other(String),
    #[error("worker process crashed: {0}")]
    WorkerCrashed(String),
    #[error("request timed out after {seconds}s")]
    Timeout { seconds: u64 },
}

/// Trait that each inference runtime (Candle, ONNX, llama.cpp) must implement.
pub trait InferenceRuntime: Send + Sync {
    /// Which runtime kind this is.
    fn kind(&self) -> InferenceRuntimeKind;

    /// Whether this runtime is available on the current system.
    fn is_available(&self) -> bool;

    /// Get runtime information and diagnostics.
    fn info(&self) -> RuntimeInfo;

    /// Load a model from a local path into memory.
    fn load_model(&self, model_id: &str, model_path: &Path) -> Result<(), InferenceError>;

    /// Unload a previously loaded model.
    fn unload_model(&self, model_id: &str) -> Result<(), InferenceError>;

    /// Check if a model is currently loaded.
    fn is_model_loaded(&self, model_id: &str) -> bool;

    /// Run inference against a loaded model.
    fn infer(
        &self,
        model_id: &str,
        request: &InferenceRequest,
    ) -> Result<InferenceOutput, InferenceError>;

    /// Compute embeddings for a text input. Returns the embedding vector.
    fn embed(&self, model_id: &str, text: &str) -> Result<Vec<f32>, InferenceError>;

    /// List file extensions this runtime can consume.
    fn supported_formats(&self) -> Vec<String>;
}

// ---------------------------------------------------------------------------
// Real runtime implementations (when features are enabled)
// ---------------------------------------------------------------------------

#[cfg(feature = "candle")]
pub use crate::runtime_candle::CandleRuntime;

#[cfg(feature = "llama-cpp")]
pub use crate::runtime_llama::LlamaCppRuntime;

#[cfg(feature = "onnx")]
pub use crate::runtime_onnx::OnnxRuntime;

// ---------------------------------------------------------------------------
// Stub implementations (when features are disabled)
// ---------------------------------------------------------------------------

#[cfg(not(feature = "candle"))]
pub struct CandleRuntime {
    loaded_models: Mutex<HashMap<String, PathBuf>>,
}

#[cfg(not(feature = "candle"))]
impl CandleRuntime {
    pub fn new() -> Self {
        Self { loaded_models: Mutex::new(HashMap::new()) }
    }
}

#[cfg(not(feature = "candle"))]
impl InferenceRuntime for CandleRuntime {
    fn kind(&self) -> InferenceRuntimeKind {
        InferenceRuntimeKind::Candle
    }
    fn is_available(&self) -> bool {
        true
    }
    fn info(&self) -> RuntimeInfo {
        let loaded = self.loaded_models.lock();
        RuntimeInfo {
            kind: InferenceRuntimeKind::Candle,
            version: "0.9-stub".to_string(),
            supports_gpu: false,
            loaded_model: loaded.keys().next().cloned(),
            memory_used_bytes: 0,
        }
    }
    fn load_model(&self, model_id: &str, model_path: &Path) -> Result<(), InferenceError> {
        if !model_path.exists() {
            return Err(InferenceError::ModelFileNotFound(model_path.display().to_string()));
        }
        self.loaded_models.lock().insert(model_id.to_string(), model_path.to_path_buf());
        tracing::info!(runtime = "candle", model_id, path = %model_path.display(), "model loaded (stub)");
        Ok(())
    }
    fn unload_model(&self, model_id: &str) -> Result<(), InferenceError> {
        self.loaded_models.lock().remove(model_id);
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
        if !self.is_model_loaded(model_id) {
            return Err(InferenceError::ModelNotLoaded { model_id: model_id.to_string() });
        }
        Ok(InferenceOutput {
            text: format!("[candle:{}] {}", model_id, truncate(&request.prompt, 100)),
            tokens_used: request.prompt.split_whitespace().count() as u64,
            finish_reason: FinishReason::Stop,
        })
    }
    fn embed(&self, model_id: &str, text: &str) -> Result<Vec<f32>, InferenceError> {
        if !self.is_model_loaded(model_id) {
            return Err(InferenceError::ModelNotLoaded { model_id: model_id.to_string() });
        }
        let dim = 384;
        let hash = simple_hash(text);
        let mut v: Vec<f32> =
            (0..dim).map(|i| ((hash.wrapping_add(i as u64)) % 1000) as f32 / 1000.0).collect();
        crate::embedding::normalize_l2(&mut v);
        Ok(v)
    }
    fn supported_formats(&self) -> Vec<String> {
        vec!["safetensors".into(), "bin".into(), "pt".into()]
    }
}

#[cfg(not(feature = "onnx"))]
pub struct OnnxRuntime {
    loaded_models: Mutex<HashMap<String, PathBuf>>,
}

#[cfg(not(feature = "onnx"))]
impl OnnxRuntime {
    pub fn new() -> Self {
        Self { loaded_models: Mutex::new(HashMap::new()) }
    }
}

#[cfg(not(feature = "onnx"))]
impl InferenceRuntime for OnnxRuntime {
    fn kind(&self) -> InferenceRuntimeKind {
        InferenceRuntimeKind::Onnx
    }
    fn is_available(&self) -> bool {
        true
    }
    fn info(&self) -> RuntimeInfo {
        let loaded = self.loaded_models.lock();
        RuntimeInfo {
            kind: InferenceRuntimeKind::Onnx,
            version: "1.20-stub".to_string(),
            supports_gpu: true,
            loaded_model: loaded.keys().next().cloned(),
            memory_used_bytes: 0,
        }
    }
    fn load_model(&self, model_id: &str, model_path: &Path) -> Result<(), InferenceError> {
        if !model_path.exists() {
            return Err(InferenceError::ModelFileNotFound(model_path.display().to_string()));
        }
        self.loaded_models.lock().insert(model_id.to_string(), model_path.to_path_buf());
        tracing::info!(runtime = "onnx", model_id, path = %model_path.display(), "model loaded (stub)");
        Ok(())
    }
    fn unload_model(&self, model_id: &str) -> Result<(), InferenceError> {
        self.loaded_models.lock().remove(model_id);
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
        if !self.is_model_loaded(model_id) {
            return Err(InferenceError::ModelNotLoaded { model_id: model_id.to_string() });
        }
        Ok(InferenceOutput {
            text: format!("[onnx:{}] {}", model_id, truncate(&request.prompt, 100)),
            tokens_used: request.prompt.split_whitespace().count() as u64,
            finish_reason: FinishReason::Stop,
        })
    }
    fn embed(&self, model_id: &str, text: &str) -> Result<Vec<f32>, InferenceError> {
        if !self.is_model_loaded(model_id) {
            return Err(InferenceError::ModelNotLoaded { model_id: model_id.to_string() });
        }
        let dim = 384;
        let hash = simple_hash(text);
        let mut v: Vec<f32> =
            (0..dim).map(|i| ((hash.wrapping_add(i as u64)) % 1000) as f32 / 1000.0).collect();
        crate::embedding::normalize_l2(&mut v);
        Ok(v)
    }
    fn supported_formats(&self) -> Vec<String> {
        vec!["onnx".into()]
    }
}

#[cfg(not(feature = "llama-cpp"))]
pub struct LlamaCppRuntime {
    loaded_models: Mutex<HashMap<String, PathBuf>>,
}

#[cfg(not(feature = "llama-cpp"))]
impl LlamaCppRuntime {
    pub fn new() -> Self {
        Self { loaded_models: Mutex::new(HashMap::new()) }
    }
}

#[cfg(not(feature = "llama-cpp"))]
impl InferenceRuntime for LlamaCppRuntime {
    fn kind(&self) -> InferenceRuntimeKind {
        InferenceRuntimeKind::LlamaCpp
    }
    fn is_available(&self) -> bool {
        true
    }
    fn info(&self) -> RuntimeInfo {
        let loaded = self.loaded_models.lock();
        RuntimeInfo {
            kind: InferenceRuntimeKind::LlamaCpp,
            version: "b5000-stub".to_string(),
            supports_gpu: true,
            loaded_model: loaded.keys().next().cloned(),
            memory_used_bytes: 0,
        }
    }
    fn load_model(&self, model_id: &str, model_path: &Path) -> Result<(), InferenceError> {
        if !model_path.exists() {
            return Err(InferenceError::ModelFileNotFound(model_path.display().to_string()));
        }
        self.loaded_models.lock().insert(model_id.to_string(), model_path.to_path_buf());
        tracing::info!(runtime = "llama.cpp", model_id, path = %model_path.display(), "model loaded (stub)");
        Ok(())
    }
    fn unload_model(&self, model_id: &str) -> Result<(), InferenceError> {
        self.loaded_models.lock().remove(model_id);
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
        if !self.is_model_loaded(model_id) {
            return Err(InferenceError::ModelNotLoaded { model_id: model_id.to_string() });
        }
        Ok(InferenceOutput {
            text: format!("[llama.cpp:{}] {}", model_id, truncate(&request.prompt, 100)),
            tokens_used: request.prompt.split_whitespace().count() as u64,
            finish_reason: FinishReason::Stop,
        })
    }
    fn embed(&self, model_id: &str, text: &str) -> Result<Vec<f32>, InferenceError> {
        if !self.is_model_loaded(model_id) {
            return Err(InferenceError::ModelNotLoaded { model_id: model_id.to_string() });
        }
        let dim = 384;
        let hash = simple_hash(text);
        let mut v: Vec<f32> =
            (0..dim).map(|i| ((hash.wrapping_add(i as u64)) % 1000) as f32 / 1000.0).collect();
        crate::embedding::normalize_l2(&mut v);
        Ok(v)
    }
    fn supported_formats(&self) -> Vec<String> {
        vec!["gguf".into(), "ggml".into()]
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

#[allow(dead_code)]
fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max_chars).collect::<String>())
    }
}

#[allow(dead_code)]
fn simple_hash(text: &str) -> u64 {
    text.bytes().fold(0u64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u64))
}

// ---------------------------------------------------------------------------
// Tests — stub-based (run only when runtime features are disabled)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use std::path::Path;
    use tempfile::tempdir;

    // Stub-only tests: these use fake model files that only work with stubs
    #[cfg(not(feature = "candle"))]
    #[test]
    fn stub_candle_runtime_load_and_infer() {
        let dir = tempdir().unwrap();
        let model_path = dir.path().join("model.safetensors");
        fs::write(&model_path, b"fake-candle-model").unwrap();

        let rt = CandleRuntime::new();
        assert!(rt.is_available());
        assert!(!rt.is_model_loaded("test-model"));

        rt.load_model("test-model", &model_path).unwrap();
        assert!(rt.is_model_loaded("test-model"));

        let req = InferenceRequest { prompt: "Hello world".into(), ..Default::default() };
        let output = rt.infer("test-model", &req).unwrap();
        assert!(output.text.contains("candle"));

        rt.unload_model("test-model").unwrap();
        assert!(!rt.is_model_loaded("test-model"));
    }

    #[cfg(not(feature = "onnx"))]
    #[test]
    fn stub_onnx_runtime_load_and_infer() {
        let dir = tempdir().unwrap();
        let model_path = dir.path().join("model.onnx");
        fs::write(&model_path, b"fake-onnx-model").unwrap();

        let rt = OnnxRuntime::new();
        rt.load_model("onnx-test", &model_path).unwrap();

        let req = InferenceRequest { prompt: "Test".into(), ..Default::default() };
        let output = rt.infer("onnx-test", &req).unwrap();
        assert!(output.text.contains("onnx"));
    }

    #[cfg(not(feature = "llama-cpp"))]
    #[test]
    fn stub_llama_cpp_runtime_load_and_infer() {
        let dir = tempdir().unwrap();
        let model_path = dir.path().join("model.gguf");
        fs::write(&model_path, b"fake-gguf-model").unwrap();

        let rt = LlamaCppRuntime::new();
        rt.load_model("gguf-test", &model_path).unwrap();

        let req = InferenceRequest { prompt: "Why is the sky blue?".into(), ..Default::default() };
        let output = rt.infer("gguf-test", &req).unwrap();
        assert!(output.text.contains("llama.cpp"));
    }

    // These tests work with any runtime implementation (real or stub)
    #[test]
    fn runtime_rejects_missing_model_file() {
        let rt = CandleRuntime::new();
        let result = rt.load_model("missing", Path::new("/nonexistent/model.bin"));
        assert!(result.is_err());
    }

    #[test]
    fn infer_on_unloaded_model_fails() {
        let rt = LlamaCppRuntime::new();
        let req = InferenceRequest { prompt: "Hi".into(), ..Default::default() };
        let result = rt.infer("not-loaded", &req);
        assert!(matches!(result, Err(InferenceError::ModelNotLoaded { .. })));
    }

    #[test]
    fn embed_on_unloaded_model_fails() {
        let rt = OnnxRuntime::new();
        let result = rt.embed("not-loaded", "test text");
        assert!(matches!(result, Err(InferenceError::ModelNotLoaded { .. })));
    }

    #[cfg(not(feature = "candle"))]
    #[test]
    fn stub_embed_produces_fixed_dimension_vector() {
        let dir = tempdir().unwrap();
        let model_path = dir.path().join("embed.bin");
        fs::write(&model_path, b"embed-model").unwrap();

        let rt = CandleRuntime::new();
        rt.load_model("embed", &model_path).unwrap();

        let vec = rt.embed("embed", "Hello world").unwrap();
        assert_eq!(vec.len(), 384);
    }
}
