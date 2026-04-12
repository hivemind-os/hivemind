use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Current format version for `.agentkit` archives.
pub const FORMAT_VERSION: u32 = 1;

/// Extension for Agent Kit archive files (without the leading dot).
pub const AGENT_KIT_EXTENSION: &str = "agentkit";

/// Path of the manifest inside the ZIP archive.
pub const MANIFEST_PATH: &str = "manifest.json";

// ── Manifest ────────────────────────────────────────────────────────

/// Top-level manifest stored as `manifest.json` inside an `.agentkit` ZIP.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentKitManifest {
    pub format_version: u32,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hivemind_version: Option<String>,
    #[serde(default)]
    pub personas: Vec<ManifestPersonaEntry>,
    #[serde(default)]
    pub workflows: Vec<ManifestWorkflowEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestPersonaEntry {
    /// Original namespaced ID (e.g. `"acme/sales-bot"`).
    pub id: String,
    /// Relative path inside the archive.
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestWorkflowEntry {
    /// Original namespaced name (e.g. `"acme/lead-qualifier"`).
    pub name: String,
    /// Relative path inside the archive.
    pub path: String,
}

// ── Export ───────────────────────────────────────────────────────────

/// Input for an export operation.
#[derive(Debug, Clone)]
pub struct ExportRequest {
    /// Display name for the kit.
    pub kit_name: String,
    /// Optional description.
    pub description: Option<String>,
    /// Optional author tag.
    pub author: Option<String>,
    /// Personas to include. Each entry is a `PersonaExportData`.
    pub personas: Vec<PersonaExportData>,
    /// Workflows to include. Each entry is a `WorkflowExportData`.
    pub workflows: Vec<WorkflowExportData>,
}

/// All data for one persona, ready to be archived.
#[derive(Debug, Clone)]
pub struct PersonaExportData {
    /// Namespaced ID (e.g. `"acme/sales-bot"`).
    pub id: String,
    /// Raw YAML content of `persona.yaml`.
    pub persona_yaml: Vec<u8>,
    /// Skill files keyed by relative path (e.g. `"skills/my-skill/SKILL.md"` → bytes).
    pub skill_files: HashMap<String, Vec<u8>>,
}

/// All data for one workflow, ready to be archived.
#[derive(Debug, Clone)]
pub struct WorkflowExportData {
    /// Namespaced name (e.g. `"acme/lead-qualifier"`).
    pub name: String,
    /// Raw YAML content of the workflow definition.
    pub workflow_yaml: Vec<u8>,
    /// Attachment files keyed by filename (e.g. `"abc123_readme.pdf"` → bytes).
    pub attachment_files: HashMap<String, Vec<u8>>,
}

// ── Import ──────────────────────────────────────────────────────────

/// Result of previewing an import (no side effects).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportPreview {
    /// The manifest read from the archive.
    pub manifest: AgentKitManifest,
    /// Target namespace that will be applied.
    pub target_namespace: String,
    /// Per-item preview entries.
    pub items: Vec<ImportPreviewItem>,
    /// Validation errors that prevent import (e.g. `system/` namespace).
    pub errors: Vec<String>,
    /// Warnings (e.g. external references that won't be rewritten).
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportPreviewItem {
    /// What kind of item this is.
    pub kind: ImportItemKind,
    /// The original ID/name from the archive.
    pub original_id: String,
    /// The new ID/name after re-namespacing.
    pub new_id: String,
    /// Whether this would overwrite an existing item.
    pub overwrites_existing: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ImportItemKind {
    Persona,
    Workflow,
}

/// Request to apply an import after preview.
#[derive(Debug, Clone)]
pub struct ImportApplyRequest {
    /// Target namespace root (e.g. `"myteam"`).
    pub target_namespace: String,
    /// IDs of items the user has selected to import (uses `new_id` from preview).
    pub selected_items: Vec<String>,
}

/// Result of an applied import.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportResult {
    pub imported_personas: Vec<ImportedItem>,
    pub imported_workflows: Vec<ImportedItem>,
    pub skipped: Vec<String>,
    pub errors: Vec<ImportError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportedItem {
    pub original_id: String,
    pub new_id: String,
    pub overwritten: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportError {
    pub item_id: String,
    pub message: String,
}

// ── Saver traits ────────────────────────────────────────────────────

/// Trait for persisting a persona during import.
pub trait PersonaSaver {
    /// Save persona YAML + skill files under the given (new) ID.
    fn save_persona(
        &self,
        new_id: &str,
        persona_yaml: &[u8],
        skill_files: &HashMap<String, Vec<u8>>,
    ) -> Result<(), anyhow::Error>;

    /// Check whether a persona with the given ID already exists.
    fn persona_exists(&self, id: &str) -> Result<bool, anyhow::Error>;
}

/// Trait for persisting a workflow during import.
pub trait WorkflowSaver {
    /// Save a workflow definition (YAML) + attachment files under the given (new) name.
    fn save_workflow(
        &self,
        new_name: &str,
        workflow_yaml: &[u8],
        attachment_files: &HashMap<String, Vec<u8>>,
    ) -> Result<(), anyhow::Error>;

    /// Check whether a workflow with the given name already exists.
    fn workflow_exists(&self, name: &str) -> Result<bool, anyhow::Error>;
}

// ── Errors ──────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum AgentKitError {
    #[error("invalid archive: {0}")]
    InvalidArchive(String),

    #[error("unsupported format version: {0}")]
    UnsupportedVersion(u32),

    #[error("namespace validation failed: {0}")]
    InvalidNamespace(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("ZIP error: {0}")]
    Zip(#[from] zip::result::ZipError),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("YAML error: {0}")]
    Yaml(#[from] serde_yaml::Error),

    #[error("{0}")]
    Other(String),
}
