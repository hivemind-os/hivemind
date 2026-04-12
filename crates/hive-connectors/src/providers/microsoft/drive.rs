use std::sync::Arc;

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use hive_classification::DataClass;
use hive_contracts::connectors::*;
use tracing::info;

use super::graph_client::GraphClient;
use crate::services::DriveService;

pub struct MicrosoftDrive {
    graph: Arc<GraphClient>,
    default_class: DataClass,
}

impl MicrosoftDrive {
    pub fn new(graph: Arc<GraphClient>, default_class: DataClass) -> Self {
        Self { graph, default_class }
    }

    fn parse_item(&self, item: &serde_json::Value) -> DriveItem {
        DriveItem {
            id: item["id"].as_str().unwrap_or("").to_string(),
            connector_id: self.graph.connector_id().to_string(),
            name: item["name"].as_str().unwrap_or("").to_string(),
            path: item["parentReference"]["path"].as_str().map(|p| {
                // Graph returns paths like /drive/root:/Documents — strip the prefix
                p.strip_prefix("/drive/root:").unwrap_or(p).to_string()
            }),
            is_folder: item["folder"].is_object(),
            size_bytes: item["size"].as_u64(),
            mime_type: item["file"]["mimeType"].as_str().map(|s| s.to_string()),
            modified: item["lastModifiedDateTime"].as_str().map(|s| s.to_string()),
            web_url: item["webUrl"].as_str().map(|s| s.to_string()),
            data_class: self.default_class,
        }
    }
}

#[async_trait]
impl DriveService for MicrosoftDrive {
    fn name(&self) -> &str {
        "Microsoft OneDrive"
    }

    async fn test_connection(&self) -> Result<()> {
        self.graph.get("/me/drive").await?;
        info!(
            connector = %self.graph.connector_id(),
            "OneDrive connection test OK"
        );
        Ok(())
    }

    async fn list_files(&self, path: Option<&str>, limit: usize) -> Result<Vec<DriveItem>> {
        let api_path = match path {
            Some(p) if !p.is_empty() && p != "/" => {
                format!(
                    "/me/drive/root:{}:/children?$top={limit}\
                     &$select=id,name,size,file,folder,parentReference,\
                     lastModifiedDateTime,webUrl",
                    urlencoding::encode(p)
                )
            }
            _ => format!(
                "/me/drive/root/children?$top={limit}\
                 &$select=id,name,size,file,folder,parentReference,\
                 lastModifiedDateTime,webUrl"
            ),
        };
        let body = self.graph.get(&api_path).await?;
        let items = body["value"].as_array().cloned().unwrap_or_default();
        Ok(items.iter().map(|i| self.parse_item(i)).collect())
    }

    async fn get_file(&self, file_id: &str) -> Result<DriveFileContent> {
        // Get metadata
        let meta_path = format!(
            "/me/drive/items/{file_id}?$select=id,name,size,file,folder,\
             parentReference,lastModifiedDateTime,webUrl"
        );
        let meta = self.graph.get(&meta_path).await?;
        let item = self.parse_item(&meta);

        // Download content using a separate request with the raw token
        let token = self.graph.get_token().await?;
        let url = format!("https://graph.microsoft.com/v1.0/me/drive/items/{file_id}/content");
        let client = reqwest::Client::new();
        let resp = client
            .get(&url)
            .bearer_auth(&token)
            .send()
            .await
            .context("downloading file content")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let err = resp.text().await.unwrap_or_default();
            bail!("Download file failed ({status}): {err}");
        }
        let content = resp.bytes().await.context("reading file bytes")?.to_vec();

        Ok(DriveFileContent { item, content })
    }

    async fn search_files(&self, query: &str, limit: usize) -> Result<Vec<DriveItem>> {
        let path = format!(
            "/me/drive/root/search(q='{}')?$top={limit}\
             &$select=id,name,size,file,folder,parentReference,\
             lastModifiedDateTime,webUrl",
            urlencoding::encode(query)
        );
        let body = self.graph.get(&path).await?;
        let items = body["value"].as_array().cloned().unwrap_or_default();
        Ok(items.iter().map(|i| self.parse_item(i)).collect())
    }

    async fn upload_file(
        &self,
        parent_path: &str,
        name: &str,
        content: &[u8],
        mime_type: &str,
    ) -> Result<DriveItem> {
        let path = format!(
            "/me/drive/root:{}{}:/content",
            if parent_path.ends_with('/') {
                parent_path.to_string()
            } else {
                format!("{parent_path}/")
            },
            urlencoding::encode(name)
        );
        let resp = self.graph.put_bytes(&path, content, mime_type).await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let err = resp.text().await.unwrap_or_default();
            bail!("Upload file failed ({status}): {err}");
        }
        let body: serde_json::Value = resp.json().await.context("parsing upload response")?;
        Ok(self.parse_item(&body))
    }

    async fn share_file(&self, file_id: &str, share_with: &[String]) -> Result<ShareLink> {
        let payload = serde_json::json!({
            "type": "view",
            "scope": "organization"
        });
        let path = format!("/me/drive/items/{file_id}/createLink");
        let resp = self.graph.post(&path, &payload).await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let err = resp.text().await.unwrap_or_default();
            bail!("Share file failed ({status}): {err}");
        }
        let body: serde_json::Value = resp.json().await?;
        Ok(ShareLink {
            url: body["link"]["webUrl"].as_str().unwrap_or("").to_string(),
            shared_with: share_with.to_vec(),
            expires: body["expirationDateTime"].as_str().map(|s| s.to_string()),
        })
    }

    async fn delete_file(&self, file_id: &str) -> Result<()> {
        let path = format!("/me/drive/items/{file_id}");
        let resp = self.graph.delete(&path).await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let err = resp.text().await.unwrap_or_default();
            bail!("Delete file failed ({status}): {err}");
        }
        Ok(())
    }
}
