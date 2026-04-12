use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Cursor;

use hive_agent_kit::{
    apply_import, export_kit, preview_import, ExportRequest, ImportApplyRequest, ImportPreview,
    ImportResult, PersonaExportData, PersonaSaver, WorkflowExportData, WorkflowSaver,
};

use crate::AppState;

// ── Request / response types ────────────────────────────────────────

#[derive(Deserialize)]
pub(crate) struct ExportAgentKitRequest {
    pub kit_name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    /// Persona IDs to include in the export.
    #[serde(default)]
    pub persona_ids: Vec<String>,
    /// Workflow names to include in the export.
    #[serde(default)]
    pub workflow_names: Vec<String>,
}

#[derive(Deserialize)]
pub(crate) struct PreviewAgentKitRequest {
    /// Base64-encoded `.agentkit` archive content.
    pub content: String,
    /// Target namespace root (e.g. `"myteam"`).
    pub target_namespace: String,
}

#[derive(Deserialize)]
pub(crate) struct ImportAgentKitRequest {
    /// Base64-encoded `.agentkit` archive content.
    pub content: String,
    /// Target namespace root.
    pub target_namespace: String,
    /// IDs of items the user selected to import (uses `new_id` from preview).
    pub selected_items: Vec<String>,
}

#[derive(Serialize)]
pub(crate) struct ExportAgentKitResponse {
    /// Base64-encoded `.agentkit` archive.
    pub content: String,
    pub filename: String,
}

// ── Export handler ───────────────────────────────────────────────────

pub(crate) async fn api_export_agent_kit(
    State(state): State<AppState>,
    Json(body): Json<ExportAgentKitRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    use base64::Engine;

    // Collect persona data
    let mut personas = Vec::new();
    for persona_id in &body.persona_ids {
        let data = collect_persona_data(&state, persona_id)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        personas.push(data);
    }

    // Collect workflow data
    let mut workflows = Vec::new();
    for wf_name in &body.workflow_names {
        let data = collect_workflow_data(&state, wf_name)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        workflows.push(data);
    }

    let request = ExportRequest {
        kit_name: body.kit_name.clone(),
        description: body.description.clone(),
        author: body.author.clone(),
        personas,
        workflows,
    };

    let mut buf = Cursor::new(Vec::new());
    export_kit(&request, &mut buf)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let bytes = buf.into_inner();
    let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let filename = format!("{}.agentkit", sanitize_filename(&body.kit_name));

    Ok(Json(ExportAgentKitResponse { content: encoded, filename }))
}

// ── Preview handler ─────────────────────────────────────────────────

pub(crate) async fn api_preview_agent_kit(
    State(state): State<AppState>,
    Json(body): Json<PreviewAgentKitRequest>,
) -> Result<Json<ImportPreview>, (StatusCode, String)> {
    use base64::Engine;

    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&body.content)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("invalid base64: {e}")))?;

    let persona_checker = ApiPersonaSaver::new(state.clone());
    let workflow_checker = ApiWorkflowSaver::new(state.clone());
    let target_ns = body.target_namespace.clone();

    let preview = tokio::task::spawn_blocking(move || {
        preview_import(Cursor::new(bytes), &target_ns, &persona_checker, &workflow_checker)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    Ok(Json(preview))
}

// ── Import handler ──────────────────────────────────────────────────

pub(crate) async fn api_import_agent_kit(
    State(state): State<AppState>,
    Json(body): Json<ImportAgentKitRequest>,
) -> Result<Json<ImportResult>, (StatusCode, String)> {
    use base64::Engine;

    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&body.content)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("invalid base64: {e}")))?;

    let persona_saver = ApiPersonaSaver::new(state.clone());
    let workflow_saver = ApiWorkflowSaver::new(state.clone());

    let request = ImportApplyRequest {
        target_namespace: body.target_namespace.clone(),
        selected_items: body.selected_items.clone(),
    };

    let result = tokio::task::spawn_blocking(move || {
        apply_import(Cursor::new(bytes), &request, &persona_saver, &workflow_saver)
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    // Refresh in-memory state for imported personas
    let personas = hive_core::load_personas(&state.personas_dir).unwrap_or_default();
    state.chat.update_personas(personas.clone());
    let all_mcp = crate::collect_mcp_configs(&personas);
    state.reconcile_mcp_servers(all_mcp).await;

    // Register triggers for imported workflows
    for item in &result.imported_workflows {
        if let Ok((def, _)) = state.workflows.get_latest_definition(&item.new_id).await {
            state.trigger_manager.register_definition(&def).await;
        }
    }

    Ok(Json(result))
}

// ── Data collection helpers ─────────────────────────────────────────

fn collect_persona_data(
    state: &AppState,
    persona_id: &str,
) -> Result<PersonaExportData, anyhow::Error> {
    let persona_dir =
        state.personas_dir.join(persona_id.replace('/', std::path::MAIN_SEPARATOR_STR));

    // Read persona.yaml
    let yaml_path = persona_dir.join("persona.yaml");
    let persona_yaml = std::fs::read(&yaml_path)
        .map_err(|e| anyhow::anyhow!("Failed to read persona '{}': {}", persona_id, e))?;

    // Collect skill files recursively
    let skills_dir = persona_dir.join("skills");
    let mut skill_files = HashMap::new();
    if skills_dir.exists() {
        collect_files_recursive(&skills_dir, &persona_dir, &mut skill_files)?;
    }

    Ok(PersonaExportData { id: persona_id.to_string(), persona_yaml, skill_files })
}

async fn collect_workflow_data(
    state: &AppState,
    wf_name: &str,
) -> Result<WorkflowExportData, anyhow::Error> {
    let (def, yaml) = state
        .workflows
        .get_latest_definition(wf_name)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to get workflow '{}': {}", wf_name, e))?;

    // Collect attachment files
    let mut attachment_files = HashMap::new();
    let att_dir = state.workflows.attachments_dir(&def.id, &def.version);
    if att_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&att_dir) {
            for entry in entries.flatten() {
                if entry.path().is_file() {
                    let filename = entry.file_name().to_string_lossy().to_string();
                    let data = std::fs::read(entry.path())?;
                    attachment_files.insert(filename, data);
                }
            }
        }
    }

    Ok(WorkflowExportData {
        name: wf_name.to_string(),
        workflow_yaml: yaml.into_bytes(),
        attachment_files,
    })
}

fn collect_files_recursive(
    dir: &std::path::Path,
    base: &std::path::Path,
    files: &mut HashMap<String, Vec<u8>>,
) -> Result<(), anyhow::Error> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_files_recursive(&path, base, files)?;
        } else {
            let rel = path.strip_prefix(base).unwrap_or(&path).to_string_lossy().replace('\\', "/");
            let data = std::fs::read(&path)?;
            files.insert(rel, data);
        }
    }
    Ok(())
}

fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect()
}

// ── PersonaSaver / WorkflowSaver implementations ────────────────────

struct ApiPersonaSaver {
    state: AppState,
}

impl ApiPersonaSaver {
    fn new(state: AppState) -> Self {
        Self { state }
    }
}

impl PersonaSaver for ApiPersonaSaver {
    fn save_persona(
        &self,
        new_id: &str,
        persona_yaml: &[u8],
        skill_files: &HashMap<String, Vec<u8>>,
    ) -> Result<(), anyhow::Error> {
        let persona_dir =
            self.state.personas_dir.join(new_id.replace('/', std::path::MAIN_SEPARATOR_STR));
        std::fs::create_dir_all(&persona_dir)?;

        // Write persona.yaml
        std::fs::write(persona_dir.join("persona.yaml"), persona_yaml)?;

        // Write skill files
        for (rel_path, data) in skill_files {
            let full_path = persona_dir.join(rel_path.replace('/', std::path::MAIN_SEPARATOR_STR));
            if let Some(parent) = full_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&full_path, data)?;
        }

        Ok(())
    }

    fn persona_exists(&self, id: &str) -> Result<bool, anyhow::Error> {
        let persona_dir =
            self.state.personas_dir.join(id.replace('/', std::path::MAIN_SEPARATOR_STR));
        Ok(persona_dir.join("persona.yaml").exists())
    }
}

struct ApiWorkflowSaver {
    state: AppState,
}

impl ApiWorkflowSaver {
    fn new(state: AppState) -> Self {
        Self { state }
    }
}

impl WorkflowSaver for ApiWorkflowSaver {
    fn save_workflow(
        &self,
        _new_name: &str,
        workflow_yaml: &[u8],
        attachment_files: &HashMap<String, Vec<u8>>,
    ) -> Result<(), anyhow::Error> {
        let yaml_str = std::str::from_utf8(workflow_yaml)
            .map_err(|e| anyhow::anyhow!("invalid workflow YAML encoding: {}", e))?;

        // Use the blocking save_definition_sync method via a handle
        let rt = tokio::runtime::Handle::current();
        let wf_service = self.state.workflows.clone();
        let yaml_owned = yaml_str.to_string();
        let def = rt
            .block_on(async { wf_service.save_definition(&yaml_owned).await })
            .map_err(|e| anyhow::anyhow!("Failed to save workflow: {}", e))?;

        // Save attachment files
        for (filename, data) in attachment_files {
            let attachment_id = uuid::Uuid::new_v4().to_string();
            self.state
                .workflows
                .upload_attachment(&def.id, &def.version, &attachment_id, filename, data)
                .map_err(|e| anyhow::anyhow!("Failed to save attachment: {}", e))?;
        }

        Ok(())
    }

    fn workflow_exists(&self, name: &str) -> Result<bool, anyhow::Error> {
        let rt = tokio::runtime::Handle::current();
        let wf_service = self.state.workflows.clone();
        let name_owned = name.to_string();
        match rt.block_on(async { wf_service.get_latest_definition(&name_owned).await }) {
            Ok(_) => Ok(true),
            Err(_) => Ok(false),
        }
    }
}
