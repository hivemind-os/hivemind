use std::sync::Arc;

use anyhow::bail;
use async_trait::async_trait;
use serde_json::Value;

use hive_classification::ChannelClass;
use hive_contracts::ToolApproval;

use crate::service_registry::{DynService, OperationSchema, ServiceDescriptor};
use crate::services::DriveService;

/// Wraps a [`DriveService`] into a [`DynService`].
pub struct DriveServiceAdapter {
    inner: Arc<dyn DriveService>,
}

impl DriveServiceAdapter {
    pub fn new(inner: Arc<dyn DriveService>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl DynService for DriveServiceAdapter {
    fn descriptor(&self) -> ServiceDescriptor {
        ServiceDescriptor {
            service_type: "drive".into(),
            display_name: self.inner.name().into(),
            description: "Cloud file storage and sharing".into(),
            is_standard: true,
        }
    }

    fn operations(&self) -> Vec<OperationSchema> {
        vec![
            OperationSchema {
                name: "list_files".into(),
                description: "List files and folders at a path".into(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "limit": { "type": "integer", "default": 50 }
                    }
                }),
                output_schema: None,
                side_effects: false,
                approval: ToolApproval::Auto,
                channel_class: ChannelClass::Private,
            },
            OperationSchema {
                name: "get_file".into(),
                description: "Get the content of a file by ID".into(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "file_id": { "type": "string" }
                    },
                    "required": ["file_id"]
                }),
                output_schema: None,
                side_effects: false,
                approval: ToolApproval::Auto,
                channel_class: ChannelClass::Private,
            },
            OperationSchema {
                name: "search_files".into(),
                description: "Search for files matching a query".into(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string" },
                        "limit": { "type": "integer", "default": 20 }
                    },
                    "required": ["query"]
                }),
                output_schema: None,
                side_effects: false,
                approval: ToolApproval::Auto,
                channel_class: ChannelClass::Private,
            },
            OperationSchema {
                name: "upload_file".into(),
                description: "Upload a file to the drive".into(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "parent_path": { "type": "string" },
                        "name": { "type": "string" },
                        "content_base64": { "type": "string" },
                        "mime_type": { "type": "string" }
                    },
                    "required": ["parent_path", "name", "content_base64", "mime_type"]
                }),
                output_schema: None,
                side_effects: true,
                approval: ToolApproval::Ask,
                channel_class: ChannelClass::Private,
            },
            OperationSchema {
                name: "share_file".into(),
                description: "Share a file with others".into(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "file_id": { "type": "string" },
                        "share_with": { "type": "array", "items": { "type": "string" } }
                    },
                    "required": ["file_id", "share_with"]
                }),
                output_schema: None,
                side_effects: true,
                approval: ToolApproval::Ask,
                channel_class: ChannelClass::Public,
            },
            OperationSchema {
                name: "delete_file".into(),
                description: "Delete a file from the drive".into(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "file_id": { "type": "string" }
                    },
                    "required": ["file_id"]
                }),
                output_schema: None,
                side_effects: true,
                approval: ToolApproval::Ask,
                channel_class: ChannelClass::Private,
            },
        ]
    }

    async fn execute(&self, operation: &str, input: Value) -> anyhow::Result<Value> {
        match operation {
            "list_files" => {
                let path = input.get("path").and_then(|v| v.as_str());
                let limit = input.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;
                let files = self.inner.list_files(path, limit).await?;
                Ok(serde_json::to_value(files)?)
            }
            "get_file" => {
                let file_id = input.get("file_id").and_then(|v| v.as_str()).unwrap_or_default();
                let content = self.inner.get_file(file_id).await?;
                Ok(serde_json::to_value(content)?)
            }
            "search_files" => {
                let query = input.get("query").and_then(|v| v.as_str()).unwrap_or_default();
                let limit = input.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;
                let files = self.inner.search_files(query, limit).await?;
                Ok(serde_json::to_value(files)?)
            }
            "upload_file" => {
                let parent_path =
                    input.get("parent_path").and_then(|v| v.as_str()).unwrap_or_default();
                let name = input.get("name").and_then(|v| v.as_str()).unwrap_or_default();
                let content_b64 =
                    input.get("content_base64").and_then(|v| v.as_str()).unwrap_or_default();
                let content = base64::Engine::decode(
                    &base64::engine::general_purpose::STANDARD,
                    content_b64,
                )?;
                let mime_type = input
                    .get("mime_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("application/octet-stream");
                let item = self.inner.upload_file(parent_path, name, &content, mime_type).await?;
                Ok(serde_json::to_value(item)?)
            }
            "share_file" => {
                let file_id = input.get("file_id").and_then(|v| v.as_str()).unwrap_or_default();
                let share_with: Vec<String> = serde_json::from_value(
                    input.get("share_with").cloned().unwrap_or(Value::Array(vec![])),
                )?;
                let link = self.inner.share_file(file_id, &share_with).await?;
                Ok(serde_json::to_value(link)?)
            }
            "delete_file" => {
                let file_id = input.get("file_id").and_then(|v| v.as_str()).unwrap_or_default();
                self.inner.delete_file(file_id).await?;
                Ok(serde_json::json!({ "status": "ok" }))
            }
            other => bail!("unknown drive operation: {other}"),
        }
    }

    async fn test_connection(&self) -> anyhow::Result<()> {
        self.inner.test_connection().await
    }

    fn as_drive(&self) -> Option<&dyn DriveService> {
        Some(self.inner.as_ref())
    }
}
