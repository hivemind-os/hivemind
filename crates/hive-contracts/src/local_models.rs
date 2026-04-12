use crate::config::InferenceRuntimeKind;
use crate::hardware::{HardwareInfo, RuntimeResourceUsage};
use crate::models::{InstalledModel, ModelCapabilities};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct InstallModelRequest {
    pub hub_repo: String,
    pub filename: String,
    pub runtime: InferenceRuntimeKind,
    pub capabilities: Option<ModelCapabilities>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LocalModelSummary {
    pub installed_count: usize,
    pub total_size_bytes: u64,
    pub models: Vec<InstalledModel>,
}

#[derive(Debug, Deserialize)]
pub struct HubSearchQuery {
    pub query: Option<String>,
    pub task: Option<String>,
    pub runtime: Option<InferenceRuntimeKind>,
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HardwareSummary {
    pub hardware: HardwareInfo,
    pub usage: RuntimeResourceUsage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HubFileInfo {
    pub filename: String,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HubRepoFilesResult {
    pub repo_id: String,
    pub files: Vec<HubFileInfo>,
}
