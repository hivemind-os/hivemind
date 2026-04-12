use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::sse::{Event, KeepAlive, Sse},
    Json,
};
use serde::Deserialize;
use std::convert::Infallible;

use crate::{skills_error, AppState};
use hive_contracts::{
    DiscoveredSkill, InstalledSkill, Persona, SkillAuditResult, SkillContent, SkillRiskSeverity,
    SkillSourceConfig,
};

pub(crate) async fn api_discover_skills(
    State(state): State<AppState>,
) -> Result<Json<Vec<DiscoveredSkill>>, (StatusCode, String)> {
    state.skills.discover().await.map(Json).map_err(skills_error)
}

pub(crate) async fn api_get_skill_sources(
    State(state): State<AppState>,
) -> Json<Vec<SkillSourceConfig>> {
    Json(state.skills.get_sources().await)
}

pub(crate) async fn api_set_skill_sources(
    State(state): State<AppState>,
    Json(sources): Json<Vec<SkillSourceConfig>>,
) -> Result<Json<Vec<SkillSourceConfig>>, (StatusCode, String)> {
    state.skills.set_sources(sources).await.map(Json).map_err(skills_error)
}

pub(crate) async fn api_rebuild_skill_index(
    State(state): State<AppState>,
) -> Result<Json<Vec<DiscoveredSkill>>, (StatusCode, String)> {
    state.skills.rebuild_index().await.map(Json).map_err(skills_error)
}

pub(crate) async fn api_audit_skill(
    State(state): State<AppState>,
    Path(_name): Path<String>,
    Json(request): Json<AuditSkillRequest>,
) -> Result<Json<SkillAuditResult>, (StatusCode, String)> {
    let skill_content: SkillContent = state
        .skills
        .fetch_skill_content(&request.source_id, &request.source_path)
        .await
        .map_err(skills_error)?;
    state
        .skills
        .audit_skill(&request.source_id, &request.source_path, &skill_content, &request.model)
        .await
        .map(Json)
        .map_err(skills_error)
}

/// SSE-streaming version of the audit endpoint. Emits progress events so
/// the desktop client can show real-time status without timing out.
///
/// Events (JSON in `data:` field):
///   `{"phase":"fetching","message":"Fetching skill content…"}`
///   `{"phase":"auditing","message":"Running security audit with <model>…"}`
///   `{"phase":"done","result":{…}}`
///   `{"phase":"error","message":"…"}`
pub(crate) async fn api_audit_skill_stream(
    State(state): State<AppState>,
    Path(_name): Path<String>,
    Json(request): Json<AuditSkillRequest>,
) -> Sse<impl futures_core::Stream<Item = Result<Event, Infallible>>> {
    let source_id = request.source_id;
    let source_path = request.source_path;
    let model = request.model;

    let stream = async_stream::stream! {
        yield Ok(Event::default().data(
            serde_json::json!({"phase":"fetching","message":"Fetching skill content…"}).to_string()
        ));

        let content = match state.skills
            .fetch_skill_content(&source_id, &source_path)
            .await
        {
            Ok(c) => c,
            Err(e) => {
                yield Ok(Event::default().data(
                    serde_json::json!({"phase":"error","message":format!("Failed to fetch content: {e}")}).to_string()
                ));
                return;
            }
        };

        yield Ok(Event::default().data(
            serde_json::json!({"phase":"auditing","message":format!("Running security audit with {model}…")}).to_string()
        ));

        match state.skills
            .audit_skill(&source_id, &source_path, &content, &model)
            .await
        {
            Ok(result) => {
                yield Ok(Event::default().data(
                    serde_json::json!({"phase":"done","result":result}).to_string()
                ));
            }
            Err(e) => {
                yield Ok(Event::default().data(
                    serde_json::json!({"phase":"error","message":e.to_string()}).to_string()
                ));
            }
        }
    };

    Sse::new(stream).keep_alive(
        KeepAlive::new().interval(std::time::Duration::from_secs(15)).text("keep-alive"),
    )
}

// ── Per-persona skill endpoints ──────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub(crate) struct AuditSkillRequest {
    model: String,
    source_id: String,
    source_path: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct InstallSkillRequest {
    source_id: String,
    source_path: String,
    model: Option<String>,
    /// Pre-computed audit result from the client. When provided, the server
    /// skips re-running the (slow) LLM audit and uses this directly.
    audit: Option<SkillAuditResult>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SetSkillEnabledRequest {
    enabled: bool,
}

pub(crate) async fn api_list_persona_skills(
    State(state): State<AppState>,
    Path(persona_id): Path<String>,
) -> Result<Json<Vec<InstalledSkill>>, (StatusCode, String)> {
    Persona::validate_id(&persona_id)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid persona_id: {e}")))?;
    state.skills.list_installed(&persona_id).await.map(Json).map_err(skills_error)
}

pub(crate) async fn api_install_persona_skill(
    State(state): State<AppState>,
    Path((persona_id, name)): Path<(String, String)>,
    Json(request): Json<InstallSkillRequest>,
) -> Result<Json<InstalledSkill>, (StatusCode, String)> {
    Persona::validate_id(&persona_id)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid persona_id: {e}")))?;

    let content = state
        .skills
        .fetch_skill_content(&request.source_id, &request.source_path)
        .await
        .map_err(skills_error)?;

    // Use client-supplied audit when available; otherwise run server-side.
    let audit = if let Some(a) = request.audit {
        a
    } else {
        let model = request.model.unwrap_or_else(|| "default".to_string());
        state
            .skills
            .audit_skill(&request.source_id, &request.source_path, &content, &model)
            .await
            .map_err(skills_error)?
    };

    // Block installation if any critical risks were detected.
    let has_critical = audit.risks.iter().any(|r| r.severity == SkillRiskSeverity::Critical);
    if has_critical {
        return Err((
            StatusCode::FORBIDDEN,
            format!("Installation blocked: critical security risks detected. {}", audit.summary),
        ));
    }

    state
        .skills
        .install_skill(&name, &request.source_id, &request.source_path, &persona_id, audit)
        .await
        .map(Json)
        .map_err(skills_error)
}

pub(crate) async fn api_uninstall_persona_skill(
    State(state): State<AppState>,
    Path((persona_id, name)): Path<(String, String)>,
) -> Result<StatusCode, (StatusCode, String)> {
    Persona::validate_id(&persona_id)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid persona_id: {e}")))?;
    if state.skills.uninstall_skill(&name, &persona_id).await.map_err(skills_error)? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err((StatusCode::NOT_FOUND, format!("skill `{name}` not found")))
    }
}

pub(crate) async fn api_set_persona_skill_enabled(
    State(state): State<AppState>,
    Path((persona_id, name)): Path<(String, String)>,
    Json(request): Json<SetSkillEnabledRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    Persona::validate_id(&persona_id)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid persona_id: {e}")))?;
    if state
        .skills
        .set_skill_enabled(&name, &persona_id, request.enabled)
        .await
        .map_err(skills_error)?
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err((StatusCode::NOT_FOUND, format!("skill `{name}` not found")))
    }
}
