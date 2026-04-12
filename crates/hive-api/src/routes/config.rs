use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use serde_json::json;

use crate::{AppState, ValidationResponse};
use hive_contracts::config::Persona;
use hive_core::{
    archive_persona, find_prompt_template, is_bundled_persona, load_personas,
    render_prompt_template, save_config, save_persona, validate_config as validate_hivemind_config,
    HiveMindConfig,
};
use serde::Serialize;
use serde_json::Value;

use crate::ModelRouterSnapshot;

// ── Config get / put / validate ──────────────────────────────────────────

pub(crate) async fn get_config(State(state): State<AppState>) -> Json<HiveMindConfig> {
    Json((**state.config.load()).clone())
}

#[derive(Debug, Serialize)]
pub(crate) struct UpdateConfigResponse {
    saved: bool,
    config_path: String,
    message: String,
}

pub(crate) async fn update_config(
    State(state): State<AppState>,
    Json(new_config): Json<HiveMindConfig>,
) -> Result<Json<UpdateConfigResponse>, (StatusCode, String)> {
    if let Err(e) = validate_hivemind_config(&new_config) {
        return Err((StatusCode::BAD_REQUEST, format!("Config validation failed: {e}")));
    }
    save_config(&new_config, &state.config_path)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to save config: {e}")))?;

    // Apply the new config at runtime (model router, HF token, MCP servers).
    state.apply_config(&new_config).await;

    if let Err(e) = state.event_bus.publish(
        "config.reloaded",
        "hive-api",
        json!({ "config_path": state.config_path.display().to_string() }),
    ) {
        tracing::warn!(error = %e, "failed to publish config.reloaded event");
    }
    Ok(Json(UpdateConfigResponse {
        saved: true,
        config_path: state.config_path.display().to_string(),
        message: "Configuration saved and applied.".into(),
    }))
}

pub(crate) async fn validate_config(State(state): State<AppState>) -> Json<ValidationResponse> {
    let cfg = state.config.load();
    if let Err(e) =
        state.event_bus.publish("config.validated", "hive-api", json!({ "bind": cfg.api.bind }))
    {
        tracing::warn!(error = %e, "failed to publish config.validated event");
    }

    Json(ValidationResponse { valid: true })
}

pub(crate) async fn get_model_router(State(state): State<AppState>) -> Json<ModelRouterSnapshot> {
    Json(state.chat.model_router_snapshot())
}

// ── Personas ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub(crate) struct ListPersonasQuery {
    #[serde(default)]
    include_archived: bool,
}

pub(crate) async fn api_list_personas(
    State(state): State<AppState>,
    Query(query): Query<ListPersonasQuery>,
) -> Result<Json<Vec<Persona>>, (StatusCode, String)> {
    let mut personas = load_personas(&state.personas_dir).map_err(|error| {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to load personas: {error}"))
    })?;
    if !personas.iter().any(|p| p.id == "system/general") {
        personas.insert(0, Persona::default_persona());
    }
    if !query.include_archived {
        personas.retain(|p| !p.archived);
    }
    Ok(Json(personas))
}

pub(crate) async fn api_save_personas(
    State(state): State<AppState>,
    Json(personas): Json<Vec<Persona>>,
) -> Result<StatusCode, (StatusCode, String)> {
    // Determine which non-archived personas to archive (present on disk but not in the new list).
    let existing = load_personas(&state.personas_dir).unwrap_or_default();
    let new_ids: std::collections::HashSet<&str> = personas.iter().map(|p| p.id.as_str()).collect();
    for old in &existing {
        if !old.archived && !new_ids.contains(old.id.as_str()) {
            archive_persona(&state.personas_dir, &old.id).map_err(|error| {
                (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to archive persona: {error}"))
            })?;
        }
    }

    // Save each persona as an individual file.  Ensure the `bundled` flag is
    // preserved for factory-shipped personas even if the client omits it.
    for persona in &personas {
        let mut p = persona.clone();
        if is_bundled_persona(&p.id) {
            p.bundled = true;
        }
        save_persona(&state.personas_dir, &p).map_err(|error| {
            (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to save persona: {error}"))
        })?;
    }

    // Update in-memory state.
    state.chat.update_personas(personas.clone());

    // Reconcile MCP servers so newly-added persona MCP tools become available.
    let all_mcp = crate::collect_mcp_configs(&personas);
    state.reconcile_mcp_servers(all_mcp).await;

    if let Err(e) = state.event_bus.publish(
        "config.reloaded",
        "hive-api",
        json!({ "personas_dir": state.personas_dir.display().to_string() }),
    ) {
        tracing::warn!(error = %e, "failed to publish config.reloaded event");
    }

    Ok(StatusCode::NO_CONTENT)
}

/// Reset a bundled persona to its factory YAML.
pub(crate) async fn api_reset_persona(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Persona>, (StatusCode, String)> {
    let persona = hive_core::reset_bundled_persona(&state.personas_dir, &id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to reset persona: {e}")))?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                format!("Persona '{id}' is not a bundled persona and cannot be reset."),
            )
        })?;

    // Reload all personas and update in-memory state.
    let personas = load_personas(&state.personas_dir).unwrap_or_default();
    state.chat.update_personas(personas.clone());

    // Reconcile MCP servers so reset persona MCP changes take effect.
    let all_mcp = crate::collect_mcp_configs(&personas);
    state.reconcile_mcp_servers(all_mcp).await;

    let _ = state.event_bus.publish(
        "config.reloaded",
        "hive-api",
        json!({ "personas_dir": state.personas_dir.display().to_string() }),
    );

    Ok(Json(persona))
}

// ── Copy persona ─────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub(crate) struct CopyPersonaRequest {
    source_id: String,
    new_id: String,
}

pub(crate) async fn api_copy_persona(
    State(state): State<AppState>,
    Json(request): Json<CopyPersonaRequest>,
) -> Result<Json<Persona>, (StatusCode, String)> {
    // Validate both IDs.
    Persona::validate_id(&request.new_id)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid new_id: {e}")))?;
    Persona::validate_id(&request.source_id)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid source_id: {e}")))?;

    // Load source persona.
    let personas = load_personas(&state.personas_dir).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to load personas: {e}"))
    })?;
    let source = personas.iter().find(|p| p.id == request.source_id).ok_or_else(|| {
        (StatusCode::NOT_FOUND, format!("Source persona '{}' not found", request.source_id))
    })?;

    // Clone and save.
    let mut new_persona = source.clone();
    new_persona.id = request.new_id.clone();
    new_persona.bundled = false;
    save_persona(&state.personas_dir, &new_persona)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to save persona: {e}")))?;

    // Copy skills directory (using platform path separator).
    let source_path = request.source_id.replace('/', std::path::MAIN_SEPARATOR_STR);
    let dest_path = request.new_id.replace('/', std::path::MAIN_SEPARATOR_STR);
    let source_skills = state.personas_dir.join(&source_path).join("skills");
    let dest_skills = state.personas_dir.join(&dest_path).join("skills");
    if source_skills.exists() {
        copy_dir_recursive(&source_skills, &dest_skills).map_err(|e| {
            (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to copy skills: {e}"))
        })?;
    }

    // Update in-memory state.
    let all_personas = load_personas(&state.personas_dir).unwrap_or_default();
    state.chat.update_personas(all_personas.clone());

    // Reconcile MCP servers so copied persona MCP tools become available.
    let all_mcp = crate::collect_mcp_configs(&all_personas);
    state.reconcile_mcp_servers(all_mcp).await;

    Ok(Json(new_persona))
}

fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let dest_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&entry.path(), &dest_path)?;
        } else {
            std::fs::copy(entry.path(), dest_path)?;
        }
    }
    Ok(())
}

// ── Prompt template endpoints ──────────────────────────────────

#[derive(Deserialize)]
pub(crate) struct RenderPromptBody {
    #[serde(default)]
    params: Value,
}

pub(crate) async fn api_render_prompt_template(
    State(state): State<AppState>,
    Path((persona_id, prompt_id)): Path<(String, String)>,
    Json(body): Json<RenderPromptBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let personas = load_personas(&state.personas_dir).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to load personas: {e}"))
    })?;
    let persona = personas
        .iter()
        .find(|p| p.id == persona_id)
        .ok_or_else(|| (StatusCode::NOT_FOUND, format!("Persona '{persona_id}' not found")))?;
    let template = find_prompt_template(&persona.prompts, &prompt_id).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            format!("Prompt template '{prompt_id}' not found on persona '{persona_id}'"),
        )
    })?;
    let rendered = render_prompt_template(template, &body.params)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(Json(json!({ "rendered": rendered })))
}
