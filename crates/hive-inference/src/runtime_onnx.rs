//! ONNX Runtime-based inference runtime.
//!
//! Primary use: embedding models (e.g., BGE-small-en-v1.5).
//! Supports any ONNX model for embeddings.

use ort::session::Session;
use ort::value::Tensor;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokenizers::Tokenizer;

use crate::runtime::{
    InferenceError, InferenceOutput, InferenceRequest, InferenceRuntime, RuntimeInfo,
};
use hive_core::InferenceRuntimeKind;

// ---------------------------------------------------------------------------
// Internal model storage
// ---------------------------------------------------------------------------

struct LoadedOnnxModel {
    session: Session,
    tokenizer: Option<Tokenizer>,
    input_count: usize,
    #[allow(dead_code)]
    model_path: PathBuf,
    memory_bytes: u64,
}

// SAFETY: LoadedOnnxModel wraps an ONNX InferenceSession that is thread-safe
// for concurrent inference calls.
unsafe impl Send for LoadedOnnxModel {}
unsafe impl Sync for LoadedOnnxModel {}

// ---------------------------------------------------------------------------
// OnnxRuntime
// ---------------------------------------------------------------------------

pub struct OnnxRuntime {
    loaded_models: Mutex<HashMap<String, LoadedOnnxModel>>,
}

impl Default for OnnxRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl OnnxRuntime {
    pub fn new() -> Self {
        Self { loaded_models: Mutex::new(HashMap::new()) }
    }

    fn find_tokenizer(model_path: &Path) -> Option<Tokenizer> {
        let model_dir = model_path.parent().unwrap_or_else(|| Path::new("."));
        let tokenizer_path = model_dir.join("tokenizer.json");
        if tokenizer_path.exists() {
            Tokenizer::from_file(&tokenizer_path).ok()
        } else {
            None
        }
    }
}

impl InferenceRuntime for OnnxRuntime {
    fn kind(&self) -> InferenceRuntimeKind {
        InferenceRuntimeKind::Onnx
    }

    fn is_available(&self) -> bool {
        true
    }

    fn info(&self) -> RuntimeInfo {
        let loaded = self.loaded_models.lock();
        let memory: u64 = loaded.values().map(|m| m.memory_bytes).sum();
        RuntimeInfo {
            kind: InferenceRuntimeKind::Onnx,
            version: "1.20".to_string(),
            supports_gpu: true,
            loaded_model: loaded.keys().next().cloned(),
            memory_used_bytes: memory,
        }
    }

    fn load_model(&self, model_id: &str, model_path: &Path) -> Result<(), InferenceError> {
        if !model_path.exists() {
            return Err(InferenceError::ModelFileNotFound(model_path.display().to_string()));
        }

        let ext = model_path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ext != "onnx" {
            return Err(InferenceError::UnsupportedFormat {
                runtime: InferenceRuntimeKind::Onnx,
                format: ext.to_string(),
            });
        }

        let session = Session::builder()
            .map_err(|e| {
                InferenceError::LoadFailed(format!("Failed to create session builder: {e}"))
            })?
            .commit_from_file(model_path)
            .map_err(|e| InferenceError::LoadFailed(format!("Failed to load ONNX model: {e}")))?;

        let input_count = session.inputs().len();
        let tokenizer = Self::find_tokenizer(model_path);

        let memory_bytes = std::fs::metadata(model_path).map(|m| m.len()).unwrap_or(0);

        let loaded = LoadedOnnxModel {
            session,
            tokenizer,
            input_count,
            model_path: model_path.to_path_buf(),
            memory_bytes,
        };

        self.loaded_models.lock().insert(model_id.to_string(), loaded);
        tracing::info!(
            runtime = "onnx",
            model_id,
            path = %model_path.display(),
            inputs = input_count,
            "model loaded"
        );
        Ok(())
    }

    fn unload_model(&self, model_id: &str) -> Result<(), InferenceError> {
        self.loaded_models.lock().remove(model_id);
        tracing::info!(runtime = "onnx", model_id, "model unloaded");
        Ok(())
    }

    fn is_model_loaded(&self, model_id: &str) -> bool {
        self.loaded_models.lock().contains_key(model_id)
    }

    fn infer(
        &self,
        model_id: &str,
        _request: &InferenceRequest,
    ) -> Result<InferenceOutput, InferenceError> {
        let loaded = self.loaded_models.lock();
        let _model_data = loaded
            .get(model_id)
            .ok_or_else(|| InferenceError::ModelNotLoaded { model_id: model_id.to_string() })?;

        Err(InferenceError::InferenceFailed(
            "Text generation is not supported by the ONNX runtime for embedding models. \
             Use the llama-cpp runtime for text generation, or use embed() for embeddings."
                .to_string(),
        ))
    }

    fn embed(&self, model_id: &str, text: &str) -> Result<Vec<f32>, InferenceError> {
        let mut loaded = self.loaded_models.lock();
        let model_data = loaded
            .get_mut(model_id)
            .ok_or_else(|| InferenceError::ModelNotLoaded { model_id: model_id.to_string() })?;

        let tokenizer = model_data.tokenizer.as_ref().ok_or_else(|| {
            InferenceError::InferenceFailed(
                "No tokenizer available. Place tokenizer.json alongside the model file."
                    .to_string(),
            )
        })?;

        let encoding = tokenizer
            .encode(text, true)
            .map_err(|e| InferenceError::InferenceFailed(format!("Tokenization failed: {e}")))?;

        // Most ONNX embedding models (e.g. bge-small-en-v1.5) have a 512-token
        // position embedding limit.  Truncate to avoid broadcast errors.
        const MAX_SEQ_LEN: usize = 512;

        let ids: Vec<i64> =
            encoding.get_ids().iter().take(MAX_SEQ_LEN).map(|&id| id as i64).collect();
        let mask: Vec<i64> =
            encoding.get_attention_mask().iter().take(MAX_SEQ_LEN).map(|&m| m as i64).collect();
        let type_ids: Vec<i64> =
            encoding.get_type_ids().iter().take(MAX_SEQ_LEN).map(|&t| t as i64).collect();
        let seq_len = ids.len();

        let map_ort = |e: ort::Error| InferenceError::InferenceFailed(e.to_string());

        // Build tensors using shape tuples (avoids ndarray version compatibility issues)
        let outputs = if model_data.input_count >= 3 {
            let t_ids =
                Tensor::from_array(([1usize, seq_len], ids.into_boxed_slice())).map_err(map_ort)?;
            let t_mask = Tensor::from_array(([1usize, seq_len], mask.into_boxed_slice()))
                .map_err(map_ort)?;
            let t_type = Tensor::from_array(([1usize, seq_len], type_ids.into_boxed_slice()))
                .map_err(map_ort)?;
            model_data.session.run(ort::inputs![t_ids, t_mask, t_type]).map_err(map_ort)?
        } else if model_data.input_count >= 2 {
            let t_ids =
                Tensor::from_array(([1usize, seq_len], ids.into_boxed_slice())).map_err(map_ort)?;
            let t_mask = Tensor::from_array(([1usize, seq_len], mask.into_boxed_slice()))
                .map_err(map_ort)?;
            model_data.session.run(ort::inputs![t_ids, t_mask]).map_err(map_ort)?
        } else {
            let t_ids =
                Tensor::from_array(([1usize, seq_len], ids.into_boxed_slice())).map_err(map_ort)?;
            model_data.session.run(ort::inputs![t_ids]).map_err(map_ort)?
        };

        // Extract embeddings from the first output
        let output = &outputs[0];
        let (_, data) = output.try_extract_tensor::<f32>().map_err(|e| {
            InferenceError::InferenceFailed(format!("Failed to extract embedding tensor: {e}"))
        })?;

        // Infer shape from data length and input seq_len.
        // Embedding models output either (1, hidden) or (1, seq, hidden).
        let total = data.len();
        let hidden_size = if total > seq_len && total % seq_len == 0 {
            total / seq_len // shape is (1, seq_len, hidden_size)
        } else {
            total // shape is (1, hidden_size) — already pooled
        };

        let mut embeddings = if hidden_size == total {
            // Already pooled
            data.to_vec()
        } else {
            // Mean pool over sequence dimension
            let mut pooled = vec![0.0f32; hidden_size];
            for t in 0..seq_len {
                let offset = t * hidden_size;
                for h in 0..hidden_size {
                    pooled[h] += data[offset + h];
                }
            }
            for v in &mut pooled {
                *v /= seq_len as f32;
            }
            pooled
        };

        crate::embedding::normalize_l2(&mut embeddings);
        Ok(embeddings)
    }

    fn supported_formats(&self) -> Vec<String> {
        vec!["onnx".into()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn onnx_runtime_reports_correct_kind() {
        let rt = OnnxRuntime::new();
        assert_eq!(rt.kind(), InferenceRuntimeKind::Onnx);
    }

    #[test]
    fn onnx_runtime_is_available() {
        let rt = OnnxRuntime::new();
        assert!(rt.is_available());
    }

    #[test]
    fn onnx_runtime_supported_formats() {
        let rt = OnnxRuntime::new();
        let formats = rt.supported_formats();
        assert_eq!(formats, vec!["onnx".to_string()]);
    }
}
