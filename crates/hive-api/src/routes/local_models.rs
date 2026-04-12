use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};

use crate::AppState;
use hive_local_models as local_models;

pub(crate) async fn api_list_local_models(
    State(state): State<AppState>,
) -> Result<Json<local_models::LocalModelSummary>, (StatusCode, String)> {
    let svc = state.local_models.ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "local model service not initialised".to_string(),
    ))?;
    Ok(local_models::list_local_models(State(svc)).await)
}

pub(crate) async fn api_get_local_model(
    State(state): State<AppState>,
    path: Path<String>,
) -> Result<Json<hive_inference::InstalledModel>, (StatusCode, String)> {
    let svc = state.local_models.ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "local model service not initialised".to_string(),
    ))?;
    local_models::get_local_model(State(svc), path).await
}

pub(crate) async fn api_install_local_model(
    State(state): State<AppState>,
    body: Json<local_models::InstallModelRequest>,
) -> Result<Json<hive_inference::InstalledModel>, (StatusCode, String)> {
    let svc = state.local_models.ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "local model service not initialised".to_string(),
    ))?;
    local_models::install_local_model(State(svc), body).await
}

pub(crate) async fn api_remove_local_model(
    State(state): State<AppState>,
    path: Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let svc = state.local_models.ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "local model service not initialised".to_string(),
    ))?;
    local_models::remove_local_model(State(svc), path).await
}

pub(crate) async fn api_update_model_params(
    State(state): State<AppState>,
    path: Path<String>,
    body: Json<hive_contracts::InferenceParams>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let svc = state.local_models.ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "local model service not initialised".to_string(),
    ))?;
    local_models::update_model_params(State(svc), path, body).await
}

pub(crate) async fn api_search_hub_models(
    State(state): State<AppState>,
    query: Query<local_models::HubSearchQuery>,
) -> Result<Json<hive_inference::HubSearchResult>, (StatusCode, String)> {
    let svc = state.local_models.ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "local model service not initialised".to_string(),
    ))?;
    local_models::search_hub_models(State(svc), query).await
}

pub(crate) async fn api_list_hub_repo_files(
    State(state): State<AppState>,
    path: Path<String>,
) -> Result<Json<hive_contracts::HubRepoFilesResult>, (StatusCode, String)> {
    let svc = state.local_models.ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "local model service not initialised".to_string(),
    ))?;
    local_models::list_hub_repo_files(State(svc), path).await
}

pub(crate) async fn api_get_hardware(
    State(state): State<AppState>,
) -> Result<Json<local_models::HardwareSummary>, (StatusCode, String)> {
    let svc = state.local_models.ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "local model service not initialised".to_string(),
    ))?;
    Ok(local_models::get_hardware(State(svc)).await)
}

pub(crate) async fn api_list_downloads(
    State(state): State<AppState>,
) -> Result<Json<Vec<hive_inference::DownloadProgress>>, (StatusCode, String)> {
    let svc = state.local_models.ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "local model service not initialised".to_string(),
    ))?;
    Ok(local_models::list_downloads(State(svc)).await)
}

pub(crate) async fn api_remove_download(
    State(state): State<AppState>,
    path: Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    let svc = state.local_models.ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "local model service not initialised".to_string(),
    ))?;
    Ok(local_models::remove_download(State(svc), path).await)
}
