use crate::{BoxFuture, Tool, ToolError, ToolResult};
use hive_classification::{ChannelClass, DataClass};
use hive_connectors::ConnectorRegistry;
use hive_contracts::{ToolAnnotations, ToolApproval, ToolDefinition};
use serde_json::{json, Value};
use std::sync::Arc;

// ===========================================================================
// drive.list_files
// ===========================================================================

pub struct DriveListFilesTool {
    definition: ToolDefinition,
    registry: Arc<ConnectorRegistry>,
}

impl DriveListFilesTool {
    pub fn new(registry: Arc<ConnectorRegistry>) -> Self {
        Self {
            definition: ToolDefinition {
                id: "drive.list_files".to_string(),
                name: "List Drive Files".to_string(),
                description: "List files and folders in a cloud drive. Optionally filter by path. Returns file ID, name, type, size, and modification time.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "connector_id": {
                            "type": "string",
                            "description": "The connector ID for the drive provider (use connector.list to find connectors with has_drive=true)"
                        },
                        "path": {
                            "type": "string",
                            "description": "Optional folder path to list (default: root)"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of files to return (default: 20)",
                            "default": 20
                        }
                    },
                    "required": ["connector_id"]
                }),
                output_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "files": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string" },
                                    "name": { "type": "string" },
                                    "mime_type": { "type": "string" },
                                    "size": { "type": "integer" },
                                    "modified_at": { "type": "string" }
                                }
                            }
                        }
                    }
                })),
                channel_class: ChannelClass::Internal,
                side_effects: false,
                approval: ToolApproval::Auto,
                annotations: ToolAnnotations {
                    title: "List Drive Files".to_string(),
                    read_only_hint: Some(true),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(true),
                    open_world_hint: Some(false),
                },
            },
            registry,
        }
    }
}

impl Tool for DriveListFilesTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let connector_id = input
                .get("connector_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("missing `connector_id`".into()))?;

            let limit = input.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;

            let path = input.get("path").and_then(|v| v.as_str()).map(String::from);

            let connector = self.registry.get(connector_id).ok_or_else(|| {
                ToolError::ExecutionFailed(format!("connector '{connector_id}' not found"))
            })?;

            let drive = connector.drive().ok_or_else(|| {
                ToolError::ExecutionFailed(format!(
                    "connector '{connector_id}' does not support drive"
                ))
            })?;

            let files = drive
                .list_files(path.as_deref(), limit)
                .await
                .map_err(|e| ToolError::ExecutionFailed(format!("drive error: {e}")))?;

            let output = json!({ "files": serde_json::to_value(&files).map_err(|e| ToolError::ExecutionFailed(e.to_string()))? });
            Ok(ToolResult { output, data_class: DataClass::Internal })
        })
    }
}

// ===========================================================================
// drive.read_file
// ===========================================================================

pub struct DriveReadFileTool {
    definition: ToolDefinition,
    registry: Arc<ConnectorRegistry>,
    default_dir: Option<std::path::PathBuf>,
}

impl DriveReadFileTool {
    pub fn new(registry: Arc<ConnectorRegistry>) -> Self {
        Self::with_workspace(registry, None)
    }

    pub fn with_workspace(
        registry: Arc<ConnectorRegistry>,
        default_dir: Option<std::path::PathBuf>,
    ) -> Self {
        Self {
            definition: ToolDefinition {
                id: "drive.read_file".to_string(),
                name: "Download Drive File".to_string(),
                description: "Download a file from a cloud drive to a local path. Returns file metadata and the local path where the file was saved. Does NOT return file content in the response. The local_path must be a workspace-relative path.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "connector_id": {
                            "type": "string",
                            "description": "The connector ID for the drive provider (use connector.list to find connectors with has_drive=true)"
                        },
                        "file_id": {
                            "type": "string",
                            "description": "The ID of the file to download"
                        },
                        "local_path": {
                            "type": "string",
                            "description": "Workspace-relative path where the file should be saved (e.g. 'downloads/report.pdf')"
                        }
                    },
                    "required": ["connector_id", "file_id", "local_path"]
                }),
                output_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "file_id": { "type": "string" },
                        "name": { "type": "string" },
                        "mime_type": { "type": "string" },
                        "size_bytes": { "type": "integer" },
                        "local_path": { "type": "string" }
                    }
                })),
                channel_class: ChannelClass::Internal,
                side_effects: true,
                approval: ToolApproval::Auto,
                annotations: ToolAnnotations {
                    title: "Download Drive File".to_string(),
                    read_only_hint: Some(false),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(true),
                    open_world_hint: Some(true),
                },
            },
            registry,
            default_dir,
        }
    }
}

impl Tool for DriveReadFileTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let connector_id = input
                .get("connector_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("missing `connector_id`".into()))?;

            let file_id = input
                .get("file_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("missing `file_id`".into()))?;

            let local_path = input
                .get("local_path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("missing `local_path`".into()))?;

            // Validate local_path is within the workspace
            let resolved_path = if let Some(ref root) = self.default_dir {
                crate::resolve_relative_path(root, local_path)?
            } else {
                std::path::PathBuf::from(local_path)
            };

            let connector = self.registry.get(connector_id).ok_or_else(|| {
                ToolError::ExecutionFailed(format!("connector '{connector_id}' not found"))
            })?;

            let drive = connector.drive().ok_or_else(|| {
                ToolError::ExecutionFailed(format!(
                    "connector '{connector_id}' does not support drive"
                ))
            })?;

            let file_content = drive
                .get_file(file_id)
                .await
                .map_err(|e| ToolError::ExecutionFailed(format!("drive error: {e}")))?;

            // Create parent directories if needed and write to disk
            if let Some(parent) = resolved_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    ToolError::ExecutionFailed(format!(
                        "failed to create directory '{}': {e}",
                        parent.display()
                    ))
                })?;
            }
            std::fs::write(&resolved_path, &file_content.content).map_err(|e| {
                ToolError::ExecutionFailed(format!("failed to write file '{local_path}': {e}"))
            })?;

            let output = json!({
                "file_id": file_content.item.id,
                "name": file_content.item.name,
                "mime_type": file_content.item.mime_type,
                "size_bytes": file_content.content.len(),
                "local_path": resolved_path.to_string_lossy(),
            });
            Ok(ToolResult { output, data_class: DataClass::Internal })
        })
    }
}

// ===========================================================================
// drive.search_files
// ===========================================================================

pub struct DriveSearchFilesTool {
    definition: ToolDefinition,
    registry: Arc<ConnectorRegistry>,
}

impl DriveSearchFilesTool {
    pub fn new(registry: Arc<ConnectorRegistry>) -> Self {
        Self {
            definition: ToolDefinition {
                id: "drive.search_files".to_string(),
                name: "Search Drive Files".to_string(),
                description: "Search for files in a cloud drive by name or content query. Returns matching files with metadata.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "connector_id": {
                            "type": "string",
                            "description": "The connector ID for the drive provider (use connector.list to find connectors with has_drive=true)"
                        },
                        "query": {
                            "type": "string",
                            "description": "Search query string"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of results to return (default: 20)",
                            "default": 20
                        }
                    },
                    "required": ["connector_id", "query"]
                }),
                output_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "files": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string" },
                                    "name": { "type": "string" },
                                    "mime_type": { "type": "string" },
                                    "size": { "type": "integer" },
                                    "modified_at": { "type": "string" }
                                }
                            }
                        }
                    }
                })),
                channel_class: ChannelClass::Internal,
                side_effects: false,
                approval: ToolApproval::Auto,
                annotations: ToolAnnotations {
                    title: "Search Drive Files".to_string(),
                    read_only_hint: Some(true),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(true),
                    open_world_hint: Some(false),
                },
            },
            registry,
        }
    }
}

impl Tool for DriveSearchFilesTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let connector_id = input
                .get("connector_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("missing `connector_id`".into()))?;

            let query = input
                .get("query")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("missing `query`".into()))?;

            let limit = input.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;

            let connector = self.registry.get(connector_id).ok_or_else(|| {
                ToolError::ExecutionFailed(format!("connector '{connector_id}' not found"))
            })?;

            let drive = connector.drive().ok_or_else(|| {
                ToolError::ExecutionFailed(format!(
                    "connector '{connector_id}' does not support drive"
                ))
            })?;

            let files = drive
                .search_files(query, limit)
                .await
                .map_err(|e| ToolError::ExecutionFailed(format!("drive error: {e}")))?;

            let output = json!({ "files": serde_json::to_value(&files).map_err(|e| ToolError::ExecutionFailed(e.to_string()))? });
            Ok(ToolResult { output, data_class: DataClass::Internal })
        })
    }
}

// ===========================================================================
// drive.upload_file
// ===========================================================================

pub struct DriveUploadFileTool {
    definition: ToolDefinition,
    registry: Arc<ConnectorRegistry>,
    default_dir: Option<std::path::PathBuf>,
}

impl DriveUploadFileTool {
    pub fn new(registry: Arc<ConnectorRegistry>) -> Self {
        Self::with_workspace(registry, None)
    }

    pub fn with_workspace(
        registry: Arc<ConnectorRegistry>,
        default_dir: Option<std::path::PathBuf>,
    ) -> Self {
        Self {
            definition: ToolDefinition {
                id: "drive.upload_file".to_string(),
                name: "Upload Drive File".to_string(),
                description: "Upload a local file to a cloud drive. Reads the file from a workspace-relative local_path and uploads it to the specified drive location.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "connector_id": {
                            "type": "string",
                            "description": "The connector ID for the drive provider (use connector.list to find connectors with has_drive=true)"
                        },
                        "local_path": {
                            "type": "string",
                            "description": "Workspace-relative path of the local file to upload (e.g. 'docs/report.pdf')"
                        },
                        "parent_path": {
                            "type": "string",
                            "description": "The parent folder path on the drive where the file will be uploaded"
                        },
                        "name": {
                            "type": "string",
                            "description": "The file name to create on the drive (defaults to the local file name if omitted)"
                        },
                        "mime_type": {
                            "type": "string",
                            "description": "MIME type of the file (auto-detected from extension if omitted)"
                        }
                    },
                    "required": ["connector_id", "local_path", "parent_path"]
                }),
                output_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "file_id": { "type": "string" },
                        "status": { "type": "string" }
                    }
                })),
                channel_class: ChannelClass::Internal,
                side_effects: true,
                approval: ToolApproval::Ask,
                annotations: ToolAnnotations {
                    title: "Upload Drive File".to_string(),
                    read_only_hint: Some(false),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(false),
                    open_world_hint: Some(true),
                },
            },
            registry,
            default_dir,
        }
    }
}

impl Tool for DriveUploadFileTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let connector_id = input
                .get("connector_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("missing `connector_id`".into()))?;

            let local_path_str = input
                .get("local_path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("missing `local_path`".into()))?;

            let parent_path = input
                .get("parent_path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("missing `parent_path`".into()))?;

            // Validate local_path is within the workspace
            let resolved_path = if let Some(ref root) = self.default_dir {
                crate::resolve_existing_path(root, local_path_str)?
            } else {
                let p = std::path::PathBuf::from(local_path_str);
                if !p.exists() {
                    return Err(ToolError::ExecutionFailed(format!(
                        "file not found: '{local_path_str}'"
                    )));
                }
                p
            };

            // Derive file name: explicit > local file name
            let name = input
                .get("name")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| resolved_path.file_name().map(|n| n.to_string_lossy().to_string()))
                .ok_or_else(|| {
                    ToolError::InvalidInput("cannot determine file name from local_path".into())
                })?;

            // Read file bytes from disk
            let bytes = std::fs::read(&resolved_path).map_err(|e| {
                ToolError::ExecutionFailed(format!(
                    "failed to read file '{}': {e}",
                    resolved_path.display()
                ))
            })?;

            // Determine MIME type: explicit > guess from extension > fallback
            let mime_type = input
                .get("mime_type")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| {
                    mime_guess::from_path(&resolved_path).first_or_octet_stream().to_string()
                });

            let connector = self.registry.get(connector_id).ok_or_else(|| {
                ToolError::ExecutionFailed(format!("connector '{connector_id}' not found"))
            })?;

            let drive = connector.drive().ok_or_else(|| {
                ToolError::ExecutionFailed(format!(
                    "connector '{connector_id}' does not support drive"
                ))
            })?;

            let item = drive
                .upload_file(parent_path, &name, &bytes, &mime_type)
                .await
                .map_err(|e| ToolError::ExecutionFailed(format!("drive error: {e}")))?;

            let output = serde_json::to_value(&item)
                .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
            Ok(ToolResult { output, data_class: DataClass::Internal })
        })
    }
}

// ===========================================================================
// drive.share_file
// ===========================================================================

pub struct DriveShareFileTool {
    definition: ToolDefinition,
    registry: Arc<ConnectorRegistry>,
}

impl DriveShareFileTool {
    pub fn new(registry: Arc<ConnectorRegistry>) -> Self {
        Self {
            definition: ToolDefinition {
                id: "drive.share_file".to_string(),
                name: "Share Drive File".to_string(),
                description: "Share a file from a cloud drive with specified recipients by email. Grants access to the file for each listed email address.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "connector_id": {
                            "type": "string",
                            "description": "The connector ID for the drive provider (use connector.list to find connectors with has_drive=true)"
                        },
                        "file_id": {
                            "type": "string",
                            "description": "The ID of the file to share"
                        },
                        "share_with": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "List of email addresses to share the file with"
                        }
                    },
                    "required": ["connector_id", "file_id", "share_with"]
                }),
                output_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "status": { "type": "string" }
                    }
                })),
                channel_class: ChannelClass::Internal,
                side_effects: true,
                approval: ToolApproval::Ask,
                annotations: ToolAnnotations {
                    title: "Share Drive File".to_string(),
                    read_only_hint: Some(false),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(false),
                    open_world_hint: Some(true),
                },
            },
            registry,
        }
    }
}

impl Tool for DriveShareFileTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let connector_id = input
                .get("connector_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("missing `connector_id`".into()))?;

            let file_id = input
                .get("file_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("missing `file_id`".into()))?;

            let share_with: Vec<String> = input
                .get("share_with")
                .and_then(|v| v.as_array())
                .ok_or_else(|| ToolError::InvalidInput("missing `share_with`".into()))?
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();

            let connector = self.registry.get(connector_id).ok_or_else(|| {
                ToolError::ExecutionFailed(format!("connector '{connector_id}' not found"))
            })?;

            let drive = connector.drive().ok_or_else(|| {
                ToolError::ExecutionFailed(format!(
                    "connector '{connector_id}' does not support drive"
                ))
            })?;

            let link = drive
                .share_file(file_id, &share_with)
                .await
                .map_err(|e| ToolError::ExecutionFailed(format!("drive error: {e}")))?;

            let output = serde_json::to_value(&link)
                .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
            Ok(ToolResult { output, data_class: DataClass::Internal })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Tool;

    fn empty_registry() -> Arc<ConnectorRegistry> {
        Arc::new(ConnectorRegistry::new())
    }

    #[test]
    fn upload_tool_rejects_absolute_local_path() {
        let tool =
            DriveUploadFileTool::with_workspace(empty_registry(), Some(std::env::temp_dir()));
        let input = json!({
            "connector_id": "test",
            "local_path": "/etc/passwd",
            "parent_path": "/uploads"
        });
        let result = tokio::runtime::Runtime::new().unwrap().block_on(tool.execute(input));
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("absolute paths are not allowed"),
            "expected absolute path rejection, got: {err}"
        );
    }

    #[test]
    fn upload_tool_rejects_parent_traversal() {
        let tool =
            DriveUploadFileTool::with_workspace(empty_registry(), Some(std::env::temp_dir()));
        let input = json!({
            "connector_id": "test",
            "local_path": "../../etc/passwd",
            "parent_path": "/uploads"
        });
        let result = tokio::runtime::Runtime::new().unwrap().block_on(tool.execute(input));
        // Should fail because the file doesn't exist under temp_dir or path escapes root
        assert!(result.is_err());
    }

    #[test]
    fn read_tool_rejects_absolute_local_path() {
        let tool = DriveReadFileTool::with_workspace(empty_registry(), Some(std::env::temp_dir()));
        let input = json!({
            "connector_id": "test",
            "file_id": "abc",
            "local_path": "/tmp/evil/output.txt"
        });
        let result = tokio::runtime::Runtime::new().unwrap().block_on(tool.execute(input));
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("absolute paths are not allowed"),
            "expected absolute path rejection, got: {err}"
        );
    }

    #[test]
    fn read_tool_rejects_parent_traversal() {
        let tool = DriveReadFileTool::with_workspace(empty_registry(), Some(std::env::temp_dir()));
        let input = json!({
            "connector_id": "test",
            "file_id": "abc",
            "local_path": "../../../etc/shadow"
        });
        let result = tokio::runtime::Runtime::new().unwrap().block_on(tool.execute(input));
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("parent segments"),
            "expected parent traversal rejection, got: {err}"
        );
    }

    #[test]
    fn upload_tool_definition_uses_local_path() {
        let tool = DriveUploadFileTool::new(empty_registry());
        let def = tool.definition();
        assert_eq!(def.id, "drive.upload_file");
        let schema = &def.input_schema;
        assert!(schema["properties"]["local_path"].is_object(), "should have local_path property");
        assert!(schema["properties"]["content_base64"].is_null(), "should NOT have content_base64");
        let required = schema["required"].as_array().unwrap();
        let req_strs: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
        assert!(req_strs.contains(&"local_path"), "local_path should be required");
        assert!(!req_strs.contains(&"content_base64"), "content_base64 should not be required");
    }

    #[test]
    fn read_tool_definition_has_workspace_description() {
        let tool = DriveReadFileTool::with_workspace(empty_registry(), None);
        let def = tool.definition();
        assert!(
            def.description.contains("workspace-relative"),
            "description should mention workspace-relative"
        );
    }
}
