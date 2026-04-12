use crate::config::{InferenceRuntimeKind, ModelTask};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ── Hub search types ────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HubModelInfo {
    /// The model identifier (e.g. "google/gemma-3-1b-it").
    /// HF returns both `id` and `modelId`; we keep `id` to avoid duplicate-field errors.
    pub id: String,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default, rename = "lastModified")]
    pub last_modified: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub downloads: u64,
    #[serde(default)]
    pub likes: u64,
    #[serde(default, rename = "pipeline_tag")]
    pub pipeline_tag: Option<String>,
    #[serde(default)]
    pub library_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HubSearchResult {
    pub models: Vec<HubModelInfo>,
    pub total: usize,
}

// ── Installed model types ───────────────────────────────────────────

/// Per-model inference tuning parameters.
/// All fields are optional — `None` means "use the runtime default".
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct InferenceParams {
    /// Context window size in tokens (e.g. 2048, 4096, 8192).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_length: Option<u32>,
    /// Maximum number of tokens to generate per request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    /// Sampling temperature (0.0 = greedy, 1.0+ = creative).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Nucleus sampling threshold (0.0–1.0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    /// Repetition penalty multiplier (1.0 = no penalty).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat_penalty: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCapabilities {
    pub tasks: Vec<ModelTask>,
    pub can_call_tools: bool,
    pub has_reasoning: bool,
    pub context_length: Option<u32>,
    pub parameter_count: Option<String>,
}

impl Default for ModelCapabilities {
    fn default() -> Self {
        Self {
            tasks: vec![ModelTask::Chat],
            can_call_tools: false,
            has_reasoning: false,
            context_length: None,
            parameter_count: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ModelStatus {
    Available,
    Downloading,
    Error,
    Removed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledModel {
    pub id: String,
    pub hub_repo: String,
    pub filename: String,
    pub runtime: InferenceRuntimeKind,
    pub capabilities: ModelCapabilities,
    pub status: ModelStatus,
    pub size_bytes: u64,
    pub local_path: PathBuf,
    pub sha256: Option<String>,
    pub installed_at: String,
    #[serde(default)]
    pub inference_params: InferenceParams,
}
