use arc_swap::ArcSwap;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use hive_contracts::InferenceParams;
pub use hive_contracts::{
    HardwareSummary, HubRepoFilesResult, HubSearchQuery, InstallModelRequest, LocalModelSummary,
};
use hive_inference::{
    current_resource_usage, detect_hardware, DownloadProgress, HardwareInfo, HubClient,
    HubSearchRequest, HubSearchResult, InstalledModel, LocalModelRegistry, ModelRegistryStore,
    ModelStatus, RuntimeResourceUsage,
};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Service
// ---------------------------------------------------------------------------

pub struct LocalModelService {
    registry: LocalModelRegistry,
    hub: ArcSwap<HubClient>,
    storage_path: PathBuf,
    downloads: Arc<Mutex<HashMap<String, DownloadProgress>>>,
    router_rebuilder: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl LocalModelService {
    pub fn new(
        db_path: PathBuf,
        storage_path: PathBuf,
        hf_token: Option<&str>,
    ) -> anyhow::Result<Self> {
        let registry = LocalModelRegistry::open(&db_path)?;
        let mut hub = HubClient::new();
        if let Some(token) = hf_token {
            hub = hub.with_token(token);
        }
        Ok(Self {
            registry,
            hub: ArcSwap::from_pointee(hub),
            storage_path,
            downloads: Arc::new(Mutex::new(HashMap::new())),
            router_rebuilder: Mutex::new(None),
        })
    }

    pub fn with_in_memory_registry(
        storage_path: PathBuf,
        hf_token: Option<&str>,
    ) -> anyhow::Result<Self> {
        let registry = LocalModelRegistry::open_in_memory()?;
        let mut hub = HubClient::new();
        if let Some(token) = hf_token {
            hub = hub.with_token(token);
        }
        Ok(Self {
            registry,
            hub: ArcSwap::from_pointee(hub),
            storage_path,
            downloads: Arc::new(Mutex::new(HashMap::new())),
            router_rebuilder: Mutex::new(None),
        })
    }

    /// Create a service with a custom `HubClient` (useful for testing with mock servers).
    pub fn with_hub_client(storage_path: PathBuf, hub: HubClient) -> anyhow::Result<Self> {
        let registry = LocalModelRegistry::open_in_memory()?;
        Ok(Self {
            registry,
            hub: ArcSwap::from_pointee(hub),
            storage_path,
            downloads: Arc::new(Mutex::new(HashMap::new())),
            router_rebuilder: Mutex::new(None),
        })
    }

    /// Set a callback that will be invoked after a model download completes
    /// to rebuild the model router with the newly available model.
    pub fn set_router_rebuilder(&self, f: Arc<dyn Fn() + Send + Sync>) {
        *self.router_rebuilder.lock() = Some(f);
    }

    /// Replace the HubClient with one carrying the new token.
    pub fn update_hub_token(&self, token: Option<&str>) {
        let mut hub = HubClient::new();
        if let Some(t) = token {
            hub = hub.with_token(t);
        }
        self.hub.store(Arc::new(hub));
    }

    /// Return a reference to the underlying model registry.
    pub fn registry(&self) -> &LocalModelRegistry {
        &self.registry
    }

    pub fn list_models(&self) -> Vec<InstalledModel> {
        self.registry.list().unwrap_or_default()
    }

    pub fn get_model(&self, model_id: &str) -> Result<InstalledModel, LocalModelError> {
        self.registry
            .get(model_id)
            .map_err(|_| LocalModelError::NotFound { model_id: model_id.to_string() })
    }

    pub fn update_inference_params(
        &self,
        model_id: &str,
        params: &InferenceParams,
    ) -> Result<(), LocalModelError> {
        self.registry
            .update_inference_params(model_id, params)
            .map_err(|e| LocalModelError::RegistryError { detail: e.to_string() })
    }

    pub async fn search_hub(
        &self,
        request: &HubSearchRequest,
    ) -> Result<HubSearchResult, LocalModelError> {
        self.hub
            .load()
            .search(request)
            .await
            .map_err(|e| LocalModelError::HubError { detail: format!("{e:#}") })
    }

    pub async fn list_hub_files(
        &self,
        repo_id: &str,
    ) -> Result<Vec<hive_contracts::HubFileInfo>, LocalModelError> {
        let files = self
            .hub
            .load()
            .list_files(repo_id)
            .await
            .map_err(|e| LocalModelError::HubError { detail: e.to_string() })?;
        Ok(files
            .into_iter()
            .map(|f| hive_contracts::HubFileInfo { filename: f.filename, size: f.size })
            .collect())
    }

    pub async fn install_model(
        &self,
        request: &InstallModelRequest,
    ) -> Result<InstalledModel, LocalModelError> {
        // Create a unique id from repo + filename.
        let id = format!(
            "{}/{}",
            request.hub_repo.replace('/', "_"),
            request.filename.replace('/', "_")
        );

        // If a download is already in progress for this model, return the
        // existing record instead of starting a duplicate download.
        {
            let map = self.downloads.lock();
            if let Some(entry) = map.get(&id) {
                if entry.status == "downloading" {
                    return self
                        .registry
                        .get(&id)
                        .map_err(|e| LocalModelError::RegistryError { detail: e.to_string() });
                }
            }
        }

        // Record as downloading (INSERT OR REPLACE handles duplicates).
        let model = InstalledModel {
            id: id.clone(),
            hub_repo: request.hub_repo.clone(),
            filename: request.filename.clone(),
            runtime: request.runtime,
            capabilities: request.capabilities.clone().unwrap_or_default(),
            status: ModelStatus::Downloading,
            size_bytes: 0,
            local_path: self.storage_path.join(&id),
            sha256: None,
            installed_at: chrono_now(),
            inference_params: Default::default(),
        };

        self.registry
            .insert(&model)
            .map_err(|e| LocalModelError::RegistryError { detail: e.to_string() })?;

        // Insert initial download progress.
        {
            let progress = DownloadProgress {
                model_id: id.clone(),
                repo_id: request.hub_repo.clone(),
                filename: request.filename.clone(),
                total_bytes: None,
                downloaded_bytes: 0,
                status: "downloading".to_string(),
                error: None,
            };
            self.downloads.lock().insert(id.clone(), progress);
        }

        // Spawn the download as a background task.
        let dest_dir = self.storage_path.join(request.hub_repo.replace('/', "_"));
        let downloads = Arc::clone(&self.downloads);
        let hub = (*self.hub.load_full()).clone();
        let registry = self.registry.clone();
        let model_id = id.clone();
        let repo_id = request.hub_repo.clone();
        let filename = request.filename.clone();
        let rebuilder = self.router_rebuilder.lock().clone();

        tokio::spawn(async move {
            let progress_id = model_id.clone();
            let progress_downloads = Arc::clone(&downloads);
            let progress_cb: Arc<dyn Fn(u64, Option<u64>) + Send + Sync> =
                Arc::new(move |downloaded, total| {
                    let mut map = progress_downloads.lock();
                    if let Some(entry) = map.get_mut(&progress_id) {
                        entry.downloaded_bytes = downloaded;
                        entry.total_bytes = total;
                    }
                });

            match hub
                .download_file_with_progress(&repo_id, &filename, &dest_dir, Some(progress_cb))
                .await
            {
                Ok((path, sha256)) => {
                    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                    let _ = registry.update_status(&model_id, ModelStatus::Available);
                    let _ = registry.update_details(&model_id, &path, &sha256, size);

                    // Download companion files that the runtime needs but
                    // that live as separate files in the HuggingFace repo.
                    if let Some(dir) = path.parent() {
                        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                        let companions: &[&str] = match ext {
                            // ONNX needs tokenizer.json for embedding.
                            "onnx" => &["tokenizer.json"],
                            // Candle (safetensors) needs tokenizer.json AND config.json.
                            "safetensors" | "bin" => &["tokenizer.json", "config.json"],
                            _ => &[],
                        };
                        for companion in companions {
                            let companion_path = dir.join(companion);
                            if !companion_path.exists() {
                                tracing::info!(
                                    repo = %repo_id,
                                    dest = %companion_path.display(),
                                    "downloading companion {companion} for {ext} model"
                                );
                                if let Err(e) = hub
                                    .download_file_with_progress(&repo_id, companion, dir, None)
                                    .await
                                {
                                    tracing::warn!(
                                        repo = %repo_id,
                                        file = companion,
                                        error = %e,
                                        "failed to download companion file — \
                                         model may not load correctly"
                                    );
                                }
                            }
                        }

                        // For sharded safetensors models, download all
                        // remaining shards automatically.  The user picks one
                        // shard in the install dialog and we detect the
                        // naming pattern (e.g. model-00001-of-00004) to pull
                        // the rest.
                        if ext == "safetensors" {
                            if let Some(fname) = path.file_name().and_then(|f| f.to_str()) {
                                // Match pattern like "model-00001-of-00004.safetensors"
                                let re = regex::Regex::new(r"^(.*)-(\d+)-of-(\d+)\.safetensors$")
                                    .unwrap();
                                if let Some(caps) = re.captures(fname) {
                                    let prefix = &caps[1];
                                    let total: u32 = caps[3].parse().unwrap_or(1);

                                    // Collect shards that still need downloading.
                                    let mut pending_shards = Vec::new();
                                    for idx in 1..=total {
                                        let shard_name = format!(
                                            "{prefix}-{:05}-of-{:05}.safetensors",
                                            idx, total
                                        );
                                        let shard_path = dir.join(&shard_name);
                                        if !shard_path.exists() {
                                            pending_shards.push((idx, shard_name));
                                        }
                                    }

                                    for (_seq, (idx, shard_name)) in
                                        pending_shards.iter().enumerate()
                                    {
                                        // Update progress so frontend shows shard status.
                                        {
                                            let mut map = downloads.lock();
                                            if let Some(entry) = map.get_mut(&model_id) {
                                                entry.status = format!("shard {idx}/{total}");
                                                entry.downloaded_bytes = 0;
                                                entry.total_bytes = None;
                                            }
                                        }

                                        let shard_downloads = Arc::clone(&downloads);
                                        let shard_model_id = model_id.clone();
                                        let shard_progress: Arc<
                                            dyn Fn(u64, Option<u64>) + Send + Sync,
                                        > = Arc::new(move |downloaded, total_size| {
                                            let mut map = shard_downloads.lock();
                                            if let Some(entry) = map.get_mut(&shard_model_id) {
                                                entry.downloaded_bytes = downloaded;
                                                entry.total_bytes = total_size;
                                            }
                                        });

                                        tracing::info!(
                                            repo = %repo_id,
                                            shard = %shard_name,
                                            "downloading shard {idx}/{total}"
                                        );
                                        if let Err(e) = hub
                                            .download_file_with_progress(
                                                &repo_id,
                                                shard_name,
                                                dir,
                                                Some(shard_progress),
                                            )
                                            .await
                                        {
                                            tracing::error!(
                                                repo = %repo_id,
                                                shard = %shard_name,
                                                error = %e,
                                                "failed to download shard"
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Mark as finalizing while the router rebuilds.
                    {
                        let mut map = downloads.lock();
                        if let Some(entry) = map.get_mut(&model_id) {
                            entry.status = "finalizing".to_string();
                        }
                    }

                    // Rebuild the model router so the new model is available.
                    // Run on a blocking thread because the rebuilder may call
                    // synchronous code that needs to block_on async futures
                    // (e.g. loading models via worker proxy IPC).  We also
                    // catch panics so that the download always transitions to
                    // "complete" even if the rebuild fails.
                    if let Some(ref rebuild) = rebuilder {
                        let rebuild = Arc::clone(rebuild);
                        let result = tokio::task::spawn_blocking(move || {
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                rebuild();
                            }))
                        })
                        .await;
                        match result {
                            Ok(Ok(())) => {}
                            Ok(Err(_)) => {
                                tracing::error!(
                                    model = %model_id,
                                    "router rebuild panicked after model download"
                                );
                            }
                            Err(e) => {
                                tracing::error!(
                                    model = %model_id,
                                    error = %e,
                                    "router rebuild task failed after model download"
                                );
                            }
                        }
                    }

                    // Mark download as complete.
                    {
                        let mut map = downloads.lock();
                        if let Some(entry) = map.get_mut(&model_id) {
                            entry.status = "complete".to_string();
                            entry.downloaded_bytes = size;
                        }
                    }

                    // Schedule cleanup of completed entry after 30 seconds.
                    let cleanup_downloads = Arc::clone(&downloads);
                    let cleanup_id = model_id.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                        cleanup_downloads.lock().remove(&cleanup_id);
                    });
                }
                Err(e) => {
                    let _ = registry.update_status(&model_id, ModelStatus::Error);
                    {
                        let mut map = downloads.lock();
                        if let Some(entry) = map.get_mut(&model_id) {
                            entry.status = "error".to_string();
                            entry.error = Some(e.to_string());
                        }
                    }
                }
            }
        });

        Ok(model)
    }

    pub fn get_downloads(&self) -> Vec<DownloadProgress> {
        self.downloads.lock().values().cloned().collect()
    }

    pub fn remove_download(&self, model_id: &str) {
        self.downloads.lock().remove(model_id);
    }

    pub fn remove_model(&self, model_id: &str) -> Result<(), LocalModelError> {
        let model = self
            .registry
            .get(model_id)
            .map_err(|_| LocalModelError::NotFound { model_id: model_id.to_string() })?;

        // Try to delete file from disk.
        if model.local_path.exists() {
            if let Err(e) = std::fs::remove_file(&model.local_path) {
                tracing::warn!("failed to delete model file: {e}");
            }
        }

        self.registry
            .remove(model_id)
            .map_err(|e| LocalModelError::RegistryError { detail: e.to_string() })?;

        // Rebuild the model router so deleted models are no longer routed to.
        if let Some(rebuild) = self.router_rebuilder.lock().clone() {
            tracing::info!(model_id, "rebuilding model router after model removal");
            if let Err(e) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                rebuild();
            })) {
                tracing::error!("router rebuild panicked after model removal: {e:?}");
            }
        }

        Ok(())
    }

    pub fn hardware_info(&self) -> HardwareInfo {
        detect_hardware()
    }

    pub fn resource_usage(&self) -> RuntimeResourceUsage {
        current_resource_usage()
    }

    pub fn total_storage_bytes(&self) -> u64 {
        self.registry.total_size_bytes().unwrap_or(0)
    }
}

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum LocalModelError {
    #[error("model not found: {model_id}")]
    NotFound { model_id: String },
    #[error("registry error: {detail}")]
    RegistryError { detail: String },
    #[error("hub error: {detail}")]
    HubError { detail: String },
    #[error("download failed: {detail}")]
    DownloadFailed { detail: String },
}

pub fn local_model_error(error: LocalModelError) -> (StatusCode, String) {
    match error {
        LocalModelError::NotFound { .. } => (StatusCode::NOT_FOUND, error.to_string()),
        LocalModelError::RegistryError { .. } => {
            (StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
        }
        LocalModelError::HubError { .. } => (StatusCode::BAD_GATEWAY, error.to_string()),
        LocalModelError::DownloadFailed { .. } => (StatusCode::BAD_GATEWAY, error.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

pub async fn list_local_models(
    State(service): State<Arc<LocalModelService>>,
) -> Json<LocalModelSummary> {
    let models = service.list_models();
    Json(LocalModelSummary {
        installed_count: models.len(),
        total_size_bytes: service.total_storage_bytes(),
        models,
    })
}

pub async fn get_local_model(
    State(service): State<Arc<LocalModelService>>,
    Path(model_id): Path<String>,
) -> Result<Json<InstalledModel>, (StatusCode, String)> {
    service.get_model(&model_id).map(Json).map_err(local_model_error)
}

pub async fn install_local_model(
    State(service): State<Arc<LocalModelService>>,
    Json(request): Json<InstallModelRequest>,
) -> Result<Json<InstalledModel>, (StatusCode, String)> {
    tracing::info!(repo = %request.hub_repo, file = %request.filename, "install_local_model: request received");
    let result = service.install_model(&request).await.map(Json).map_err(local_model_error);
    match &result {
        Ok(model) => tracing::info!(id = %model.id, "install_local_model: download started"),
        Err((code, msg)) => {
            tracing::error!(code = %code, msg = %msg, "install_local_model: failed")
        }
    }
    result
}

pub async fn remove_local_model(
    State(service): State<Arc<LocalModelService>>,
    Path(model_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    service.remove_model(&model_id).map_err(local_model_error)?;
    Ok(Json(serde_json::json!({ "removed": model_id })))
}

pub async fn update_model_params(
    State(service): State<Arc<LocalModelService>>,
    Path(model_id): Path<String>,
    Json(params): Json<InferenceParams>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    service.update_inference_params(&model_id, &params).map_err(local_model_error)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn search_hub_models(
    State(service): State<Arc<LocalModelService>>,
    Query(query): Query<HubSearchQuery>,
) -> Result<Json<HubSearchResult>, (StatusCode, String)> {
    let request = HubSearchRequest {
        query: query.query.unwrap_or_default(),
        task: query.task,
        runtime_filter: query.runtime,
        limit: query.limit,
    };
    service.search_hub(&request).await.map(Json).map_err(local_model_error)
}

pub async fn list_hub_repo_files(
    State(service): State<Arc<LocalModelService>>,
    Path(repo_id): Path<String>,
) -> Result<Json<hive_contracts::HubRepoFilesResult>, (StatusCode, String)> {
    let files = service.list_hub_files(&repo_id).await.map_err(local_model_error)?;
    Ok(Json(hive_contracts::HubRepoFilesResult { repo_id, files }))
}

pub async fn get_hardware(State(service): State<Arc<LocalModelService>>) -> Json<HardwareSummary> {
    Json(HardwareSummary { hardware: service.hardware_info(), usage: service.resource_usage() })
}

pub async fn list_downloads(
    State(service): State<Arc<LocalModelService>>,
) -> Json<Vec<DownloadProgress>> {
    let downloads = service.get_downloads();
    tracing::debug!(count = downloads.len(), "list_downloads: returning active downloads");
    Json(downloads)
}

pub async fn remove_download(
    State(service): State<Arc<LocalModelService>>,
    Path(model_id): Path<String>,
) -> StatusCode {
    service.remove_download(&model_id);
    StatusCode::NO_CONTENT
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn chrono_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let duration = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let total_ms = duration.as_millis() as i64;
    let secs = total_ms / 1000;
    let millis = (total_ms % 1000) as u32;

    let days = secs / 86400;
    let day_secs = (secs % 86400) as u32;
    let h = day_secs / 3600;
    let min = (day_secs % 3600) / 60;
    let sec = day_secs % 60;

    // Howard Hinnant's civil_from_days algorithm
    let z = days + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    format!("{y:04}-{m:02}-{d:02}T{h:02}:{min:02}:{sec:02}.{millis:03}Z")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use hive_inference::HubClient;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    fn test_service() -> Arc<LocalModelService> {
        let dir = std::env::temp_dir().join("hive-local-model-test");
        let _ = std::fs::create_dir_all(&dir);
        Arc::new(LocalModelService::with_in_memory_registry(dir, None).expect("in-memory service"))
    }

    /// Start a tiny mock HTTP server that returns the given body for one request.
    fn mock_hf_server(response_body: &str) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let base_url = format!("http://{addr}");
        let body = response_body.to_string();

        let handle = std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                reader.read_line(&mut request_line).ok();
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).ok();
                    if line.trim().is_empty() {
                        break;
                    }
                }
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body,
                );
                stream.write_all(response.as_bytes()).ok();
            }
        });

        (base_url, handle)
    }

    /// Mock server that returns a specific HTTP status code with a body.
    fn mock_hf_error_server(
        status: u16,
        response_body: &str,
    ) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let base_url = format!("http://{addr}");
        let body = response_body.to_string();

        let handle = std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                reader.read_line(&mut request_line).ok();
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).ok();
                    if line.trim().is_empty() {
                        break;
                    }
                }
                let response = format!(
                    "HTTP/1.1 {status} Error\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body,
                );
                stream.write_all(response.as_bytes()).ok();
            }
        });

        (base_url, handle)
    }

    fn service_with_mock(mock_base_url: &str) -> Arc<LocalModelService> {
        let dir = std::env::temp_dir().join("hive-local-model-test-hub");
        let _ = std::fs::create_dir_all(&dir);
        let hub = HubClient::new().with_base_url(format!("{mock_base_url}/api"));
        Arc::new(LocalModelService::with_hub_client(dir, hub).expect("service with hub"))
    }

    #[test]
    fn list_models_empty() {
        let service = test_service();
        let models = service.list_models();
        assert!(models.is_empty());
    }

    #[test]
    fn hardware_info_returns_cpu() {
        let service = test_service();
        let hw = service.hardware_info();
        assert!(!hw.cpu.name.is_empty());
        assert!(hw.cpu.cores_logical >= 1);
    }

    #[test]
    fn resource_usage_defaults() {
        let service = test_service();
        let usage = service.resource_usage();
        assert_eq!(usage.models_loaded, 0);
    }

    // ── Hub search integration tests ────────────────────────────────

    #[tokio::test]
    async fn search_hub_returns_models() {
        let body = r#"[
            {
                "_id": "abc1", "id": "google/gemma-2b", "modelId": "google/gemma-2b",
                "author": "google", "tags": ["gguf", "gemma"],
                "downloads": 100000, "likes": 500, "private": false,
                "pipeline_tag": "text-generation", "createdAt": "2025-01-01T00:00:00.000Z"
            },
            {
                "_id": "abc2", "id": "google/gemma-7b", "modelId": "google/gemma-7b",
                "author": "google", "tags": ["safetensors"],
                "downloads": 50000, "likes": 300, "private": false,
                "pipeline_tag": "text-generation", "createdAt": "2025-01-01T00:00:00.000Z"
            }
        ]"#;

        let (base_url, handle) = mock_hf_server(body);
        let service = service_with_mock(&base_url);

        let result = service
            .search_hub(&HubSearchRequest {
                query: "gemma".to_string(),
                task: None,
                runtime_filter: None,
                limit: Some(20),
            })
            .await
            .unwrap();

        assert_eq!(result.models.len(), 2);
        assert_eq!(result.models[0].id, "google/gemma-2b");
        assert_eq!(result.models[0].author, Some("google".to_string()));
        assert_eq!(result.models[0].downloads, 100000);
        assert_eq!(result.total, 2);

        handle.join().ok();
    }

    #[tokio::test]
    async fn search_hub_empty_query_returns_results() {
        let body = r#"[{"_id":"x","id":"meta/llama-3","modelId":"meta/llama-3","downloads":999,"private":false}]"#;

        let (base_url, handle) = mock_hf_server(body);
        let service = service_with_mock(&base_url);

        let result = service
            .search_hub(&HubSearchRequest {
                query: "".to_string(),
                task: None,
                runtime_filter: None,
                limit: None,
            })
            .await
            .unwrap();

        assert_eq!(result.models.len(), 1);
        handle.join().ok();
    }

    #[tokio::test]
    async fn search_hub_propagates_api_error() {
        let (base_url, handle) = mock_hf_error_server(500, "Internal Server Error");
        let service = service_with_mock(&base_url);

        let err = service
            .search_hub(&HubSearchRequest {
                query: "gemma".to_string(),
                task: None,
                runtime_filter: None,
                limit: Some(5),
            })
            .await
            .unwrap_err();

        match err {
            LocalModelError::HubError { detail } => {
                assert!(detail.contains("500"), "error should contain status code: {detail}");
            }
            other => panic!("expected HubError, got: {other:?}"),
        }

        handle.join().ok();
    }

    #[tokio::test]
    async fn search_hub_result_serialization_roundtrip() {
        let body = r#"[
            {
                "_id": "abc3", "id": "TheBloke/Llama-2-7B-GGUF", "modelId": "TheBloke/Llama-2-7B-GGUF",
                "author": "TheBloke", "tags": ["gguf"],
                "downloads": 50000, "likes": 200, "private": false,
                "pipeline_tag": "text-generation", "library_name": "transformers",
                "createdAt": "2024-01-01T00:00:00.000Z"
            }
        ]"#;

        let (base_url, handle) = mock_hf_server(body);
        let service = service_with_mock(&base_url);

        let result = service
            .search_hub(&HubSearchRequest {
                query: "llama".to_string(),
                task: None,
                runtime_filter: None,
                limit: Some(10),
            })
            .await
            .unwrap();

        // Verify the result can be serialized to JSON and back (as the API handler does)
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["models"][0]["id"], "TheBloke/Llama-2-7B-GGUF");
        assert_eq!(json["models"][0]["author"], "TheBloke");
        assert_eq!(json["models"][0]["downloads"], 50000);
        assert_eq!(json["models"][0]["pipeline_tag"], "text-generation");
        assert_eq!(json["total"], 1);

        // Verify it deserializes back
        let roundtripped: HubSearchResult = serde_json::from_value(json).unwrap();
        assert_eq!(roundtripped.models[0].id, "TheBloke/Llama-2-7B-GGUF");

        handle.join().ok();
    }

    #[tokio::test]
    async fn list_hub_files_returns_compatible_files() {
        let body = r#"[
            { "type": "file", "path": "llama-2-7b.Q4_K_M.gguf", "size": 4000000000 },
            { "type": "file", "path": "README.md", "size": 1024 },
            { "type": "file", "path": "config.json", "size": 512 }
        ]"#;

        let (base_url, handle) = mock_hf_server(body);
        let service = service_with_mock(&base_url);

        let files = service.list_hub_files("TheBloke/Llama-2-7B-GGUF").await.unwrap();
        assert_eq!(files.len(), 3);
        assert_eq!(files[0].filename, "llama-2-7b.Q4_K_M.gguf");
        assert_eq!(files[0].size, Some(4_000_000_000));

        handle.join().ok();
    }

    #[tokio::test]
    async fn hub_error_maps_to_bad_gateway() {
        let err = LocalModelError::HubError { detail: "connection refused".to_string() };
        let (status, body) = local_model_error(err);
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert!(body.contains("connection refused"));
    }
}
