use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde_json::json;

use crate::AppState;

/// GET /api/v1/plugins — list installed plugins.
pub(crate) async fn api_list_plugins(State(state): State<AppState>) -> Json<serde_json::Value> {
    let registry = &state.plugin_registry;
    let plugins: Vec<serde_json::Value> = registry
        .list()
        .into_iter()
        .map(|p| {
            let host = &state.plugin_host;
            let status = host.get(&p.manifest.plugin_id()).map(|proc| {
                let s = proc.status();
                json!({ "state": s.state, "message": s.message })
            });
            json!({
                "plugin_id": p.manifest.plugin_id(),
                "name": p.manifest.name,
                "version": p.manifest.version,
                "display_name": p.manifest.hivemind.display_name,
                "description": p.manifest.hivemind.description,
                "plugin_type": p.manifest.hivemind.plugin_type,
                "enabled": p.enabled,
                "config": p.config,
                "status": status,
                "permissions": p.manifest.hivemind.permissions,
            })
        })
        .collect();
    Json(json!(plugins))
}

/// GET /api/v1/plugins/:id/config-schema — get plugin config schema.
pub(crate) async fn api_get_config_schema(
    State(state): State<AppState>,
    Path(plugin_id): Path<String>,
) -> impl IntoResponse {
    let host = state.plugin_host.clone();
    match tokio::spawn(async move { host.get_config_schema(&plugin_id).await }).await {
        Ok(Ok(schema)) => Json(schema).into_response(),
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// POST /api/v1/plugins/:id/config — save plugin configuration.
pub(crate) async fn api_save_config(
    State(state): State<AppState>,
    Path(plugin_id): Path<String>,
    Json(config): Json<serde_json::Value>,
) -> impl IntoResponse {
    match state.plugin_registry.update_config(&plugin_id, config) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// POST /api/v1/plugins/:id/enabled — enable/disable a plugin.
pub(crate) async fn api_set_enabled(
    State(state): State<AppState>,
    Path(plugin_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let enabled = body
        .get("enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    match state.plugin_registry.set_enabled(&plugin_id, enabled) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// DELETE /api/v1/plugins/:id — uninstall a plugin.
pub(crate) async fn api_uninstall(
    State(state): State<AppState>,
    Path(plugin_id): Path<String>,
) -> impl IntoResponse {
    // Stop the plugin process if running (best-effort, ignore errors)
    let host = state.plugin_host.clone();
    let pid = plugin_id.clone();
    let _ = tokio::spawn(async move { host.stop(&pid).await }).await;
    match state.plugin_registry.uninstall(&plugin_id) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// POST /api/v1/plugins/link — register a local development plugin.
pub(crate) async fn api_link_local(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let path = match body.get("path").and_then(|v| v.as_str()) {
        Some(p) => std::path::PathBuf::from(p),
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "missing `path` in request body" })),
            )
                .into_response()
        }
    };

    match state.plugin_registry.register_local(&path) {
        Ok(plugin_id) => Json(json!({ "plugin_id": plugin_id })).into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// POST /api/v1/plugins/install — install a plugin from npm.
pub(crate) async fn api_install_npm(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let package_name = match body.get("package").and_then(|v| v.as_str()) {
        Some(p) => p.to_string(),
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "missing `package` in request body" })),
            )
                .into_response()
        }
    };

    match state.plugin_registry.install_npm(&package_name) {
        Ok(plugin_id) => Json(json!({ "plugin_id": plugin_id })).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}
