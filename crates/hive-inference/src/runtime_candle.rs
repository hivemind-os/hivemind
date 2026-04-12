//! Candle-based inference runtime.
//!
//! Supports safetensors/bin model files. Primary use: BERT-family embeddings.
//! For text generation, prefer the llama.cpp runtime with GGUF models.

use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config as BertConfig};
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

enum CandleModel {
    Bert(BertModel),
}

struct LoadedCandleModel {
    model: CandleModel,
    tokenizer: Tokenizer,
    device: Device,
    memory_bytes: u64,
    #[allow(dead_code)]
    model_path: PathBuf,
}

// SAFETY: LoadedCandleModel is only accessed through &self methods that don't
// mutate internal state. The candle Tensor type is internally reference-counted
// and thread-safe.
unsafe impl Send for LoadedCandleModel {}
unsafe impl Sync for LoadedCandleModel {}

// ---------------------------------------------------------------------------
// CandleRuntime
// ---------------------------------------------------------------------------

pub struct CandleRuntime {
    loaded_models: Mutex<HashMap<String, LoadedCandleModel>>,
}

impl Default for CandleRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl CandleRuntime {
    pub fn new() -> Self {
        Self { loaded_models: Mutex::new(HashMap::new()) }
    }

    /// Try to find tokenizer.json adjacent to the model file.
    fn find_tokenizer(model_path: &Path) -> Result<Tokenizer, InferenceError> {
        let model_dir = model_path.parent().unwrap_or_else(|| Path::new("."));
        let tokenizer_path = model_dir.join("tokenizer.json");
        if !tokenizer_path.exists() {
            return Err(InferenceError::LoadFailed(format!(
                "tokenizer.json not found in {}",
                model_dir.display()
            )));
        }
        Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| InferenceError::LoadFailed(format!("Failed to load tokenizer: {e}")))
    }

    /// Read config.json adjacent to the model file to detect architecture.
    fn read_config(model_path: &Path) -> Result<serde_json::Value, InferenceError> {
        let model_dir = model_path.parent().unwrap_or_else(|| Path::new("."));
        let config_path = model_dir.join("config.json");
        if !config_path.exists() {
            return Err(InferenceError::LoadFailed(format!(
                "config.json not found in {}",
                model_dir.display()
            )));
        }
        let config_str = std::fs::read_to_string(&config_path)
            .map_err(|e| InferenceError::LoadFailed(format!("Failed to read config.json: {e}")))?;
        serde_json::from_str(&config_str)
            .map_err(|e| InferenceError::LoadFailed(format!("Invalid config.json: {e}")))
    }

    fn load_bert(
        model_path: &Path,
        config_str: &str,
        device: &Device,
    ) -> Result<(BertModel, u64), InferenceError> {
        let config: BertConfig = serde_json::from_str(config_str)
            .map_err(|e| InferenceError::LoadFailed(format!("Failed to parse BERT config: {e}")))?;

        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(&[model_path], DType::F32, device).map_err(|e| {
                InferenceError::LoadFailed(format!("Failed to load safetensors: {e}"))
            })?
        };

        let memory_bytes = std::fs::metadata(model_path).map(|m| m.len()).unwrap_or(0);

        let model = BertModel::load(vb, &config)
            .map_err(|e| InferenceError::LoadFailed(format!("Failed to build BERT model: {e}")))?;

        Ok((model, memory_bytes))
    }
}

impl InferenceRuntime for CandleRuntime {
    fn kind(&self) -> InferenceRuntimeKind {
        InferenceRuntimeKind::Candle
    }

    fn is_available(&self) -> bool {
        true // Candle is pure Rust, always available when compiled in
    }

    fn info(&self) -> RuntimeInfo {
        let loaded = self.loaded_models.lock();
        let memory: u64 = loaded.values().map(|m| m.memory_bytes).sum();
        RuntimeInfo {
            kind: InferenceRuntimeKind::Candle,
            version: "0.9".to_string(),
            supports_gpu: cfg!(target_os = "macos"), // Metal on macOS
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
        if !["safetensors", "bin", "pt"].contains(&ext) {
            return Err(InferenceError::UnsupportedFormat {
                runtime: InferenceRuntimeKind::Candle,
                format: ext.to_string(),
            });
        }

        let device = Device::Cpu;
        let tokenizer = Self::find_tokenizer(model_path)?;
        let config_json = Self::read_config(model_path)?;

        let model_dir = model_path.parent().unwrap_or_else(|| Path::new("."));
        let config_str = std::fs::read_to_string(model_dir.join("config.json"))
            .map_err(|e| InferenceError::LoadFailed(e.to_string()))?;

        let model_type = config_json["model_type"].as_str().unwrap_or("unknown");

        let (model, memory_bytes) = match model_type {
            "bert" | "roberta" | "xlm-roberta" => {
                let (bert, mem) = Self::load_bert(model_path, &config_str, &device)?;
                (CandleModel::Bert(bert), mem)
            }
            other => {
                return Err(InferenceError::LoadFailed(format!(
                    "Unsupported model architecture for Candle: '{other}'. Supported: bert, roberta. \
                     For text generation, use the llama-cpp runtime with GGUF models."
                )));
            }
        };

        let loaded = LoadedCandleModel {
            model,
            tokenizer,
            device,
            memory_bytes,
            model_path: model_path.to_path_buf(),
        };

        self.loaded_models.lock().insert(model_id.to_string(), loaded);
        tracing::info!(runtime = "candle", model_id, path = %model_path.display(), "model loaded");
        Ok(())
    }

    fn unload_model(&self, model_id: &str) -> Result<(), InferenceError> {
        self.loaded_models.lock().remove(model_id);
        tracing::info!(runtime = "candle", model_id, "model unloaded");
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
        let model_data = loaded
            .get(model_id)
            .ok_or_else(|| InferenceError::ModelNotLoaded { model_id: model_id.to_string() })?;

        match &model_data.model {
            CandleModel::Bert(_) => {
                // BERT is an encoder model - not designed for text generation.
                // We can do a simple "fill-mask" style forward pass for demonstration,
                // but real generation should use llama.cpp runtime.
                Err(InferenceError::InferenceFailed(
                    "BERT models do not support text generation. Use embed() for embeddings, \
                     or use the llama-cpp runtime for text generation."
                        .to_string(),
                ))
            }
        }
    }

    fn embed(&self, model_id: &str, text: &str) -> Result<Vec<f32>, InferenceError> {
        let loaded = self.loaded_models.lock();
        let model_data = loaded
            .get(model_id)
            .ok_or_else(|| InferenceError::ModelNotLoaded { model_id: model_id.to_string() })?;

        match &model_data.model {
            CandleModel::Bert(bert) => {
                let encoding = model_data.tokenizer.encode(text, true).map_err(|e| {
                    InferenceError::InferenceFailed(format!("Tokenization failed: {e}"))
                })?;

                let ids = encoding.get_ids();
                let type_ids = encoding.get_type_ids();

                let input_ids = Tensor::new(ids, &model_data.device)
                    .and_then(|t| t.unsqueeze(0))
                    .map_err(|e| InferenceError::InferenceFailed(e.to_string()))?;

                let token_type_ids = Tensor::new(type_ids, &model_data.device)
                    .and_then(|t| t.unsqueeze(0))
                    .map_err(|e| InferenceError::InferenceFailed(e.to_string()))?;

                let hidden_states =
                    bert.forward(&input_ids, &token_type_ids, None).map_err(|e| {
                        InferenceError::InferenceFailed(format!("BERT forward pass failed: {e}"))
                    })?;

                // Mean pooling over the sequence dimension
                let (_batch, n_tokens, _hidden) = hidden_states
                    .dims3()
                    .map_err(|e| InferenceError::InferenceFailed(e.to_string()))?;

                let sum = hidden_states
                    .sum(1)
                    .map_err(|e| InferenceError::InferenceFailed(e.to_string()))?;

                let mean = (sum / (n_tokens as f64))
                    .map_err(|e| InferenceError::InferenceFailed(e.to_string()))?;

                let mut embeddings: Vec<f32> = mean
                    .squeeze(0)
                    .and_then(|t| t.to_vec1())
                    .map_err(|e| InferenceError::InferenceFailed(e.to_string()))?;

                crate::embedding::normalize_l2(&mut embeddings);
                Ok(embeddings)
            }
        }
    }

    fn supported_formats(&self) -> Vec<String> {
        vec!["safetensors".into(), "bin".into(), "pt".into()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candle_runtime_reports_correct_kind() {
        let rt = CandleRuntime::new();
        assert_eq!(rt.kind(), InferenceRuntimeKind::Candle);
    }

    #[test]
    fn candle_runtime_is_always_available() {
        let rt = CandleRuntime::new();
        assert!(rt.is_available());
    }

    #[test]
    fn candle_runtime_supported_formats_include_safetensors() {
        let rt = CandleRuntime::new();
        let formats = rt.supported_formats();
        assert!(formats.contains(&"safetensors".to_string()));
        assert!(formats.contains(&"bin".to_string()));
    }
}
