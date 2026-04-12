use std::sync::Arc;

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use hive_classification::DataClass;
use hive_contracts::connectors::*;
use tracing::info;

use super::google_client::GoogleClient;
use crate::services::DriveService;

const DRIVE_API: &str = "https://www.googleapis.com/drive/v3";
const DRIVE_UPLOAD_API: &str = "https://www.googleapis.com/upload/drive/v3";

/// Standard fields requested from the Drive files API.
const FILE_FIELDS: &str = "files(id,name,mimeType,size,modifiedTime,parents,webViewLink)";
/// Fields for a single file metadata request.
const SINGLE_FILE_FIELDS: &str = "id,name,mimeType,size,modifiedTime,parents,webViewLink";

pub struct GoogleDrive {
    google: Arc<GoogleClient>,
    default_class: DataClass,
}

impl GoogleDrive {
    pub fn new(google: Arc<GoogleClient>, default_class: DataClass) -> Self {
        Self { google, default_class }
    }

    fn parse_item(&self, item: &serde_json::Value) -> DriveItem {
        let parent_id =
            item["parents"].as_array().and_then(|arr| arr.first()).and_then(|v| v.as_str());
        let name = item["name"].as_str().unwrap_or("");

        DriveItem {
            id: item["id"].as_str().unwrap_or("").to_string(),
            connector_id: self.google.connector_id().to_string(),
            name: name.to_string(),
            path: parent_id.map(|pid| format!("{pid}/{name}")),
            is_folder: item["mimeType"].as_str() == Some("application/vnd.google-apps.folder"),
            size_bytes: item["size"]
                .as_str()
                .and_then(|s| s.parse::<u64>().ok())
                .or_else(|| item["size"].as_u64()),
            mime_type: item["mimeType"].as_str().map(|s| s.to_string()),
            modified: item["modifiedTime"].as_str().map(|s| s.to_string()),
            web_url: item["webViewLink"].as_str().map(|s| s.to_string()),
            data_class: self.default_class,
        }
    }

    /// Resolve a path to a parent folder ID.
    ///
    /// If the path looks like a Drive file ID (no slashes, no spaces) it is
    /// returned directly; otherwise a name-based folder search is performed.
    async fn resolve_folder_id(&self, path: &str) -> Result<String> {
        // Heuristic: Drive IDs are typically alphanumeric + dashes/underscores
        // with no spaces or slashes.
        let looks_like_id = !path.contains('/') && !path.contains(' ');
        if looks_like_id {
            return Ok(path.to_string());
        }

        // Take the last segment of the path as the folder name.
        let folder_name = path.rsplit('/').next().unwrap_or(path);
        let q = format!(
            "name='{}' and mimeType='application/vnd.google-apps.folder' and trashed=false",
            folder_name.replace('\'', "\\'")
        );
        let url =
            format!("{DRIVE_API}/files?q={}&pageSize=1&fields=files(id)", urlencoding::encode(&q));
        let body = self.google.get(&url).await?;
        let files = body["files"].as_array().cloned().unwrap_or_default();
        match files.first() {
            Some(f) => Ok(f["id"].as_str().unwrap_or("").to_string()),
            None => bail!("folder not found: {path}"),
        }
    }
}

#[async_trait]
impl DriveService for GoogleDrive {
    fn name(&self) -> &str {
        "Google Drive"
    }

    async fn test_connection(&self) -> Result<()> {
        let url = format!("{DRIVE_API}/about?fields=user");
        self.google.get(&url).await?;
        info!(
            connector = %self.google.connector_id(),
            "Google Drive connection test OK"
        );
        Ok(())
    }

    async fn list_files(&self, path: Option<&str>, limit: usize) -> Result<Vec<DriveItem>> {
        let parent_id = match path {
            Some(p) if !p.is_empty() && p != "/" => self.resolve_folder_id(p).await?,
            _ => "root".to_string(),
        };

        let q = format!("'{parent_id}' in parents and trashed=false",);
        let url = format!(
            "{DRIVE_API}/files?q={}&pageSize={limit}&fields={FILE_FIELDS}",
            urlencoding::encode(&q),
        );
        let body = self.google.get(&url).await?;
        let items = body["files"].as_array().cloned().unwrap_or_default();
        Ok(items.iter().map(|i| self.parse_item(i)).collect())
    }

    async fn get_file(&self, file_id: &str) -> Result<DriveFileContent> {
        // Get metadata
        let meta_url = format!("{DRIVE_API}/files/{file_id}?fields={SINGLE_FILE_FIELDS}",);
        let meta = self.google.get(&meta_url).await?;
        let item = self.parse_item(&meta);

        // Google Workspace documents (Docs, Sheets, Slides …) cannot be
        // downloaded directly; they must be exported. For v1 we return empty
        // content for these types and note the limitation.
        let is_google_doc = item
            .mime_type
            .as_deref()
            .map(|m| m.starts_with("application/vnd.google-apps."))
            .unwrap_or(false);

        let content = if is_google_doc {
            Vec::new()
        } else {
            let download_url = format!("{DRIVE_API}/files/{file_id}?alt=media");
            self.google.get_bytes(&download_url).await?
        };

        Ok(DriveFileContent { item, content })
    }

    async fn search_files(&self, query: &str, limit: usize) -> Result<Vec<DriveItem>> {
        let q = format!("name contains '{}' and trashed=false", query.replace('\'', "\\'"));
        let url = format!(
            "{DRIVE_API}/files?q={}&pageSize={limit}&fields={FILE_FIELDS}",
            urlencoding::encode(&q),
        );
        let body = self.google.get(&url).await?;
        let items = body["files"].as_array().cloned().unwrap_or_default();
        Ok(items.iter().map(|i| self.parse_item(i)).collect())
    }

    async fn upload_file(
        &self,
        parent_path: &str,
        name: &str,
        content: &[u8],
        mime_type: &str,
    ) -> Result<DriveItem> {
        let parent_id = match parent_path {
            p if p.is_empty() || p == "/" => "root".to_string(),
            p => self.resolve_folder_id(p).await?,
        };

        let metadata = serde_json::json!({
            "name": name,
            "parents": [parent_id],
        });

        let meta_part = reqwest::multipart::Part::text(metadata.to_string())
            .mime_str("application/json; charset=UTF-8")
            .context("building metadata part")?;

        let content_part = reqwest::multipart::Part::bytes(content.to_vec())
            .mime_str(mime_type)
            .context("building content part")?;

        let form =
            reqwest::multipart::Form::new().part("metadata", meta_part).part("file", content_part);

        let url =
            format!("{DRIVE_UPLOAD_API}/files?uploadType=multipart&fields={SINGLE_FILE_FIELDS}");
        let resp = self.google.post_multipart(&url, form).await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let err = resp.text().await.unwrap_or_default();
            bail!("Upload file failed ({status}): {err}");
        }
        let body: serde_json::Value = resp.json().await.context("parsing upload response")?;
        Ok(self.parse_item(&body))
    }

    async fn share_file(&self, file_id: &str, share_with: &[String]) -> Result<ShareLink> {
        for email in share_with {
            let payload = serde_json::json!({
                "type": "user",
                "role": "reader",
                "emailAddress": email,
            });
            let url = format!("{DRIVE_API}/files/{file_id}/permissions");
            let resp = self.google.post(&url, &payload).await?;
            if !resp.status().is_success() {
                let status = resp.status();
                let err = resp.text().await.unwrap_or_default();
                bail!("Share file with {email} failed ({status}): {err}");
            }
        }

        // Retrieve the web link from file metadata
        let meta_url = format!("{DRIVE_API}/files/{file_id}?fields=webViewLink");
        let meta = self.google.get(&meta_url).await?;
        let web_url = meta["webViewLink"].as_str().unwrap_or("").to_string();

        Ok(ShareLink { url: web_url, shared_with: share_with.to_vec(), expires: None })
    }

    async fn delete_file(&self, file_id: &str) -> Result<()> {
        let url = format!("{DRIVE_API}/files/{file_id}");
        let resp = self.google.delete(&url).await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let err = resp.text().await.unwrap_or_default();
            bail!("Delete file failed ({status}): {err}");
        }
        Ok(())
    }
}
