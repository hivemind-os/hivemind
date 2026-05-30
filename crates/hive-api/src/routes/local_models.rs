use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};

use crate::AppState;
use hive_inference::ModelRegistryStore;
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

/// Response for the GPU layer recommendation endpoint.
#[derive(serde::Serialize)]
pub(crate) struct GpuRecommendation {
    pub model_id: String,
    pub recommended_layers: u32,
    pub estimated_layer_count: u32,
    pub model_size_bytes: u64,
    pub vram_bytes: u64,
    pub gpu_supported: bool,
}

pub(crate) async fn api_gpu_recommendation(
    State(state): State<AppState>,
    Path(model_id): Path<String>,
) -> Result<Json<GpuRecommendation>, (StatusCode, String)> {
    let svc = state.local_models.ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "local model service not initialised".to_string(),
    ))?;

    let model = svc
        .registry()
        .get(&model_id)
        .map_err(|e| (StatusCode::NOT_FOUND, format!("model not found: {e}")))?;

    let hardware = hive_inference::detect_hardware();
    let vram = hardware.gpus.first().and_then(|g| g.vram_bytes).unwrap_or(0);
    let model_size = std::fs::metadata(&model.local_path).map(|m| m.len()).unwrap_or(0);
    let estimated_layers = hive_inference::estimate_layer_count(model_size);
    let recommended = hive_inference::recommend_gpu_layers(model_size, estimated_layers, vram);
    let gpu_supported = !hardware.gpus.is_empty();

    Ok(Json(GpuRecommendation {
        model_id,
        recommended_layers: recommended,
        estimated_layer_count: estimated_layers,
        model_size_bytes: model_size,
        vram_bytes: vram,
        gpu_supported,
    }))
}
