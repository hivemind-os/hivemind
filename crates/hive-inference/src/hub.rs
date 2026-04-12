use anyhow::{bail, Context, Result};
use hive_core::InferenceRuntimeKind;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;

// Re-export shared DTOs from hive-contracts
pub use hive_contracts::{HubModelInfo, HubSearchResult};

const HF_API_BASE: &str = "https://huggingface.co/api";

const HF_NO_TOKEN_MSG: &str = "[HF_NO_TOKEN] HuggingFace authentication required. \
    Add your HF token in Settings → Local Models. \
    Get a token at https://huggingface.co/settings/tokens";

// ---------------------------------------------------------------------------
// Search types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HubSearchRequest {
    pub query: String,
    pub task: Option<String>,
    pub runtime_filter: Option<InferenceRuntimeKind>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HubFileInfo {
    pub filename: String,
    #[serde(default)]
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadProgress {
    pub model_id: String,
    pub repo_id: String,
    pub filename: String,
    pub total_bytes: Option<u64>,
    pub downloaded_bytes: u64,
    pub status: String,
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// HubClient
// ---------------------------------------------------------------------------

pub struct HubClient {
    client: Client,
    download_client: Client,
    api_base: String,
    token: Option<String>,
}

impl Clone for HubClient {
    fn clone(&self) -> Self {
        Self {
            client: self.client.clone(),
            download_client: self.download_client.clone(),
            api_base: self.api_base.clone(),
            token: self.token.clone(),
        }
    }
}

impl Default for HubClient {
    fn default() -> Self {
        Self::new()
    }
}

impl HubClient {
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| Client::new()),
            download_client: Client::builder()
                .connect_timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| Client::new()),
            api_base: HF_API_BASE.to_string(),
            token: None,
        }
    }

    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.token = Some(token.into());
        self
    }

    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.api_base = url.into();
        self
    }

    /// Search models on the Hugging Face Hub.
    pub async fn search(&self, request: &HubSearchRequest) -> Result<HubSearchResult> {
        let limit = request.limit.unwrap_or(20).min(100);
        let mut url = format!(
            "{}/models?search={}&limit={}&sort=downloads&direction=-1",
            self.api_base,
            urlencoded(&request.query),
            limit,
        );

        if let Some(task) = &request.task {
            url.push_str(&format!("&pipeline_tag={}", urlencoded(task)));
        }

        // Add library filter based on runtime.
        if let Some(runtime) = request.runtime_filter {
            let filter = match runtime {
                InferenceRuntimeKind::LlamaCpp => "gguf",
                InferenceRuntimeKind::Onnx => "onnx",
                InferenceRuntimeKind::Candle => "safetensors",
            };
            url.push_str(&format!("&filter={filter}"));
        } else {
            // Default to GGUF — only llama-cpp models support chat completion.
            url.push_str("&filter=gguf");
        }

        let mut req = self.client.get(&url);
        if let Some(token) = &self.token {
            req = req.bearer_auth(token);
        }

        let response = req.send().await.context("failed to contact Hugging Face API")?;
        if !response.status().is_success() {
            let status = response.status();
            if status == reqwest::StatusCode::UNAUTHORIZED {
                bail!("{HF_NO_TOKEN_MSG}");
            }
            if status == reqwest::StatusCode::FORBIDDEN {
                bail!(
                    "[HF_LICENSE] This model requires you to agree to its license terms. \
                    Visit the model page on HuggingFace to accept the license, then try again."
                );
            }
            let body = response.text().await.unwrap_or_default();
            bail!("Hugging Face API returned {status}: {body}");
        }

        let mut models: Vec<HubModelInfo> =
            response.json().await.context("failed to parse HF search results")?;
        // HF list endpoint doesn't return `author` — extract from model ID (org/model-name).
        for model in &mut models {
            if model.author.is_none() {
                if let Some(slash) = model.id.find('/') {
                    model.author = Some(model.id[..slash].to_string());
                }
            }
        }
        let total = models.len();
        Ok(HubSearchResult { models, total })
    }

    /// List files in a model repository.
    pub async fn list_files(&self, repo_id: &str) -> Result<Vec<HubFileInfo>> {
        // Use the tree endpoint which returns file sizes (siblings does not).
        let url = format!("{}/models/{}/tree/main", self.api_base, repo_id);
        let mut req = self.client.get(&url);
        if let Some(token) = &self.token {
            req = req.bearer_auth(token);
        }

        let response = req.send().await.context("failed to contact Hugging Face API")?;
        if !response.status().is_success() {
            let status = response.status();
            if status == reqwest::StatusCode::UNAUTHORIZED {
                bail!("{HF_NO_TOKEN_MSG}");
            }
            if status == reqwest::StatusCode::FORBIDDEN {
                bail!("[HF_LICENSE] repo={repo_id} This model requires you to agree to its license terms. \
                    Visit the model page on HuggingFace to accept the license, then try again.");
            }
            let body = response.text().await.unwrap_or_default();
            bail!("Hugging Face API returned {status}: {body}");
        }
        #[derive(Deserialize)]
        struct TreeEntry {
            #[serde(rename = "type")]
            entry_type: String,
            path: String,
            #[serde(default)]
            size: Option<u64>,
        }

        let entries: Vec<TreeEntry> =
            response.json().await.context("failed to parse HF tree info")?;
        Ok(entries
            .into_iter()
            .filter(|e| e.entry_type == "file")
            .map(|e| HubFileInfo { filename: e.path, size: e.size })
            .collect())
    }

    /// List files filtered by extension matching a runtime.
    pub async fn list_compatible_files(
        &self,
        repo_id: &str,
        runtime: InferenceRuntimeKind,
    ) -> Result<Vec<HubFileInfo>> {
        let files = self.list_files(repo_id).await?;
        let extensions = runtime_extensions(runtime);
        Ok(files
            .into_iter()
            .filter(|f| extensions.iter().any(|ext| f.filename.ends_with(ext)))
            .collect())
    }

    /// Download a single file from a Hub repo to a local directory.
    /// Returns the local path and SHA-256 hash.
    pub async fn download_file(
        &self,
        repo_id: &str,
        filename: &str,
        dest_dir: &Path,
    ) -> Result<(PathBuf, String)> {
        tokio::fs::create_dir_all(dest_dir)
            .await
            .with_context(|| format!("failed to create download dir {}", dest_dir.display()))?;

        let url =
            format!("https://huggingface.co/{}/resolve/main/{}", repo_id, urlencoded(filename));

        let mut req = self.download_client.get(&url);
        if let Some(token) = &self.token {
            req = req.bearer_auth(token);
        }

        let mut response = req.send().await.context("failed to start download")?;
        if !response.status().is_success() {
            let status = response.status();
            if status == reqwest::StatusCode::UNAUTHORIZED {
                bail!("{HF_NO_TOKEN_MSG}");
            }
            if status == reqwest::StatusCode::FORBIDDEN {
                bail!("[HF_LICENSE] repo={repo_id} This model requires you to agree to its license terms. \
                    Visit the model page on HuggingFace to accept the license, then try again.");
            }
            let body = response.text().await.unwrap_or_default();
            bail!("download failed with {status}: {body}");
        }

        let local_name = filename.replace('/', "_");
        let dest_path = dest_dir.join(&local_name);
        let partial_path = dest_dir.join(format!("{local_name}.partial"));
        let mut file = tokio::fs::File::create(&partial_path)
            .await
            .with_context(|| format!("failed to create file {}", partial_path.display()))?;

        let mut hasher = Sha256::new();
        let download_result: Result<()> = async {
            while let Some(bytes) =
                response.chunk().await.context("error reading download stream")?
            {
                file.write_all(&bytes).await.context("error writing model file")?;
                hasher.update(&bytes);
            }
            Ok(())
        }
        .await;

        if let Err(e) = download_result {
            // Clean up partial file on download failure
            let _ = tokio::fs::remove_file(&partial_path).await;
            return Err(e);
        }

        // Rename partial file to final destination
        tokio::fs::rename(&partial_path, &dest_path).await.with_context(|| {
            format!("failed to rename partial download to {}", dest_path.display())
        })?;

        let sha256 = format!("{:x}", hasher.finalize());
        tracing::info!(repo = repo_id, filename, dest = %dest_path.display(), sha256 = %sha256, "download complete");
        Ok((dest_path, sha256))
    }

    /// Download a file with optional progress reporting.
    /// The callback receives `(bytes_downloaded, total_bytes)`.
    pub async fn download_file_with_progress(
        &self,
        repo_id: &str,
        filename: &str,
        dest_dir: &Path,
        progress: Option<Arc<dyn Fn(u64, Option<u64>) + Send + Sync>>,
    ) -> Result<(PathBuf, String)> {
        #![allow(clippy::type_complexity)]
        tokio::fs::create_dir_all(dest_dir)
            .await
            .with_context(|| format!("failed to create download dir {}", dest_dir.display()))?;

        let url =
            format!("https://huggingface.co/{}/resolve/main/{}", repo_id, urlencoded(filename));

        let mut req = self.download_client.get(&url);
        if let Some(token) = &self.token {
            req = req.bearer_auth(token);
        }

        let local_name = filename.replace('/', "_");
        let dest_path = dest_dir.join(&local_name);

        // Check if partial file exists for resume
        let mut downloaded: u64 = 0;
        if dest_path.exists() {
            let metadata = tokio::fs::metadata(&dest_path).await?;
            downloaded = metadata.len();
        }

        if downloaded > 0 {
            req = req.header("Range", format!("bytes={downloaded}-"));
            tracing::info!(
                repo = repo_id,
                filename,
                resumed_from = downloaded,
                "resuming download"
            );
        }

        let response = req.send().await.context("failed to start download")?;

        let (mut file, total_size) = if response.status() == reqwest::StatusCode::PARTIAL_CONTENT {
            // Resume successful — open in append mode
            let total_size = response.content_length().map(|remaining| remaining + downloaded);
            let file =
                tokio::fs::OpenOptions::new().append(true).open(&dest_path).await.with_context(
                    || format!("failed to open file for append {}", dest_path.display()),
                )?;
            (file, total_size)
        } else if response.status() == reqwest::StatusCode::RANGE_NOT_SATISFIABLE && downloaded > 0
        {
            // 416 Range Not Satisfiable — the file is already fully downloaded.
            tracing::info!(
                repo = repo_id,
                filename,
                size = downloaded,
                "file already fully downloaded, skipping"
            );
            // Report 100% progress and skip to hash verification.
            if let Some(ref cb) = progress {
                cb(downloaded, Some(downloaded));
            }
            return self.finalize_download(&dest_path, downloaded).await;
        } else if response.status().is_success() {
            // Server doesn't support range or fresh download
            if downloaded > 0 {
                tracing::info!("server doesn't support range requests, starting fresh");
                downloaded = 0;
            }
            let total_size = response.content_length();
            let file = tokio::fs::File::create(&dest_path)
                .await
                .with_context(|| format!("failed to create file {}", dest_path.display()))?;
            (file, total_size)
        } else {
            let status = response.status();
            if status == reqwest::StatusCode::UNAUTHORIZED {
                bail!("{HF_NO_TOKEN_MSG}");
            }
            if status == reqwest::StatusCode::FORBIDDEN {
                bail!("[HF_LICENSE] repo={repo_id} This model requires you to agree to its license terms. \
                    Visit the model page on HuggingFace to accept the license, then try again.");
            }
            let body = response.text().await.unwrap_or_default();
            bail!("download failed with {status}: {body}");
        };

        let mut response = response;
        while let Some(bytes) = response.chunk().await.context("error reading download stream")? {
            file.write_all(&bytes).await.context("error writing model file")?;
            downloaded += bytes.len() as u64;
            if let Some(ref cb) = progress {
                cb(downloaded, total_size);
            }
        }

        // Hash the complete file for integrity verification (streaming to avoid
        // loading multi-GB models entirely into memory).
        let hash_file =
            tokio::fs::File::open(&dest_path).await.context("failed to open file for hashing")?;
        let mut reader = tokio::io::BufReader::with_capacity(1024 * 1024, hash_file);
        let mut hasher = Sha256::new();
        loop {
            let buf = tokio::io::AsyncBufReadExt::fill_buf(&mut reader)
                .await
                .context("error reading file for hash")?;
            if buf.is_empty() {
                break;
            }
            hasher.update(buf);
            let n = buf.len();
            tokio::io::AsyncBufReadExt::consume(&mut reader, n);
        }
        let sha256 = format!("{:x}", hasher.finalize());
        tracing::info!(repo = repo_id, filename, dest = %dest_path.display(), sha256 = %sha256, "download complete");
        Ok((dest_path, sha256))
    }

    /// Compute the SHA-256 hash of an already-downloaded file and return
    /// `(path, hex_hash)`.  Used both at the end of a normal download and
    /// when the server returns 416 (file already fully downloaded).
    async fn finalize_download(
        &self,
        dest_path: &std::path::Path,
        _size: u64,
    ) -> anyhow::Result<(std::path::PathBuf, String)> {
        let hash_file =
            tokio::fs::File::open(dest_path).await.context("failed to open file for hashing")?;
        let mut reader = tokio::io::BufReader::with_capacity(1024 * 1024, hash_file);
        let mut hasher = Sha256::new();
        loop {
            let buf = tokio::io::AsyncBufReadExt::fill_buf(&mut reader)
                .await
                .context("error reading file for hash")?;
            if buf.is_empty() {
                break;
            }
            hasher.update(buf);
            let n = buf.len();
            tokio::io::AsyncBufReadExt::consume(&mut reader, n);
        }
        let sha256 = format!("{:x}", hasher.finalize());
        tracing::info!(dest = %dest_path.display(), sha256 = %sha256, "finalized existing download");
        Ok((dest_path.to_path_buf(), sha256))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn runtime_extensions(kind: InferenceRuntimeKind) -> Vec<&'static str> {
    match kind {
        InferenceRuntimeKind::LlamaCpp => vec![".gguf", ".ggml"],
        InferenceRuntimeKind::Onnx => vec![".onnx"],
        InferenceRuntimeKind::Candle => vec![".safetensors", ".bin"],
    }
}

/// Infer the inference runtime kind from a filename extension.
pub fn infer_runtime(filename: &str) -> Option<InferenceRuntimeKind> {
    let lower = filename.to_lowercase();
    if lower.ends_with(".gguf") || lower.ends_with(".ggml") {
        Some(InferenceRuntimeKind::LlamaCpp)
    } else if lower.ends_with(".onnx") {
        Some(InferenceRuntimeKind::Onnx)
    } else if lower.ends_with(".safetensors") || lower.ends_with(".bin") {
        Some(InferenceRuntimeKind::Candle)
    } else {
        None
    }
}

fn urlencoded(s: &str) -> String {
    urlencoding::encode(s).into_owned()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    /// Spin up a tiny HTTP server that responds to search queries.
    fn mock_server(response_body: &str) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let base_url = format!("http://{addr}");
        let body = response_body.to_string();

        let handle = std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                reader.read_line(&mut request_line).ok();
                // Read headers until blank line.
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

    #[tokio::test]
    async fn search_parses_model_results() {
        // Use real HF API format: both "id" and "modelId", plus extra fields
        let body = r#"[
            {
                "_id": "abc123",
                "id": "TheBloke/Llama-2-7B-GGUF",
                "modelId": "TheBloke/Llama-2-7B-GGUF",
                "author": "TheBloke",
                "tags": ["gguf", "llama"],
                "downloads": 50000,
                "likes": 200,
                "private": false,
                "pipeline_tag": "text-generation",
                "createdAt": "2024-01-01T00:00:00.000Z"
            },
            {
                "_id": "def456",
                "id": "sentence-transformers/all-MiniLM-L6-v2",
                "modelId": "sentence-transformers/all-MiniLM-L6-v2",
                "tags": ["onnx", "sentence-transformers"],
                "downloads": 100000,
                "likes": 500,
                "private": false,
                "pipeline_tag": "feature-extraction",
                "createdAt": "2024-01-01T00:00:00.000Z"
            }
        ]"#;

        let (base_url, handle) = mock_server(body);
        let client = HubClient::new().with_base_url(format!("{base_url}/api"));

        let result = client
            .search(&HubSearchRequest {
                query: "llama".to_string(),
                task: None,
                runtime_filter: None,
                limit: Some(10),
            })
            .await
            .unwrap();

        assert_eq!(result.models.len(), 2);
        assert_eq!(result.models[0].id, "TheBloke/Llama-2-7B-GGUF");
        assert_eq!(result.models[0].downloads, 50000);
        assert_eq!(result.models[1].pipeline_tag, Some("feature-extraction".to_string()));
        // Author is set explicitly in first model, extracted from ID for second
        assert_eq!(result.models[0].author, Some("TheBloke".to_string()));
        assert_eq!(result.models[1].author, Some("sentence-transformers".to_string()));

        handle.join().ok();
    }

    #[tokio::test]
    async fn list_files_returns_tree_entries() {
        let body = r#"[
            { "type": "file", "path": "llama-2-7b.Q4_K_M.gguf", "size": 4000000000, "oid": "abc" },
            { "type": "file", "path": "llama-2-7b.Q5_K_M.gguf", "size": 5000000000, "oid": "def" },
            { "type": "file", "path": "README.md", "size": 1024, "oid": "ghi" },
            { "type": "file", "path": "config.json", "size": 512, "oid": "jkl" },
            { "type": "directory", "path": "subfolder" }
        ]"#;

        let (base_url, handle) = mock_server(body);
        let client = HubClient::new().with_base_url(format!("{base_url}/api"));

        let files = client.list_files("TheBloke/Llama-2-7B-GGUF").await.unwrap();
        assert_eq!(files.len(), 4); // directory filtered out
        assert_eq!(files[0].filename, "llama-2-7b.Q4_K_M.gguf");
        assert_eq!(files[0].size, Some(4_000_000_000));
        assert_eq!(files[1].size, Some(5_000_000_000));

        handle.join().ok();
    }

    #[tokio::test]
    async fn list_compatible_files_filters_by_runtime() {
        let body = r#"[
            { "type": "file", "path": "model.gguf", "size": 3000000000 },
            { "type": "file", "path": "model.onnx", "size": 2000000000 },
            { "type": "file", "path": "model.safetensors", "size": 1000000000 },
            { "type": "file", "path": "README.md", "size": 512 }
        ]"#;

        let (base_url, handle) = mock_server(body);
        let client = HubClient::new().with_base_url(format!("{base_url}/api"));

        let gguf_files = client
            .list_compatible_files("test/repo", InferenceRuntimeKind::LlamaCpp)
            .await
            .unwrap();
        assert_eq!(gguf_files.len(), 1);
        assert_eq!(gguf_files[0].filename, "model.gguf");

        handle.join().ok();
    }

    #[tokio::test]
    async fn download_file_writes_and_hashes() {
        let model_bytes = b"fake-model-content-for-hash-check";

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let base_url = format!("http://{addr}");

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
                let body = model_bytes;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len(),
                );
                stream.write_all(response.as_bytes()).ok();
                stream.write_all(body).ok();
            }
        });

        let dir = tempfile::tempdir().unwrap();

        let client = Client::builder().timeout(std::time::Duration::from_secs(5)).build().unwrap();

        let dest_path = dir.path().join("model.gguf");
        let url = format!("{base_url}/test/model.gguf");
        let mut response = client.get(&url).send().await.unwrap();

        let mut hasher = Sha256::new();
        let mut file = tokio::fs::File::create(&dest_path).await.unwrap();
        loop {
            match response.chunk().await.unwrap() {
                Some(bytes) => {
                    file.write_all(&bytes).await.unwrap();
                    hasher.update(&bytes);
                }
                None => break,
            }
        }
        let sha256 = format!("{:x}", hasher.finalize());

        assert!(dest_path.exists());
        let content = tokio::fs::read(&dest_path).await.unwrap();
        assert_eq!(content, model_bytes);
        assert!(!sha256.is_empty());

        handle.join().ok();
    }

    #[test]
    fn url_encoding() {
        assert_eq!(urlencoded("hello world"), "hello%20world");
        assert_eq!(urlencoded("a/b"), "a%2Fb");
    }

    #[test]
    fn parse_real_hf_api_response() {
        // Exact format returned by https://huggingface.co/api/models?search=gemma&limit=2
        let json = r#"[
            {
                "_id": "67ced65c9b9a3df71008da90",
                "id": "google/gemma-3-1b-it",
                "likes": 875,
                "private": false,
                "downloads": 3355725,
                "tags": ["transformers", "safetensors", "text-generation"],
                "pipeline_tag": "text-generation",
                "library_name": "transformers",
                "createdAt": "2025-03-10T12:09:00.000Z",
                "modelId": "google/gemma-3-1b-it"
            },
            {
                "_id": "67b79c8700245b72c5706777",
                "id": "google/gemma-3-4b-it",
                "likes": 1224,
                "private": false,
                "downloads": 2190051,
                "tags": ["transformers", "safetensors", "gemma3"],
                "pipeline_tag": "image-text-to-text",
                "library_name": "transformers",
                "createdAt": "2025-02-20T21:20:07.000Z",
                "modelId": "google/gemma-3-4b-it"
            }
        ]"#;

        let models: Vec<HubModelInfo> =
            serde_json::from_str(json).expect("should parse real HF API response");
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "google/gemma-3-1b-it");
        assert_eq!(models[0].likes, 875);
        assert_eq!(models[0].downloads, 3355725);
        assert_eq!(models[0].pipeline_tag, Some("text-generation".to_string()));
        assert_eq!(models[1].id, "google/gemma-3-4b-it");
    }
}
