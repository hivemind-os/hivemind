use async_trait::async_trait;
use hive_contracts::connectors::{DriveFileContent, DriveItem, ShareLink};

/// Abstract interface for the Drive (file storage) service on a connector.
#[async_trait]
pub trait DriveService: Send + Sync {
    /// Human-readable name (e.g. "Microsoft OneDrive").
    fn name(&self) -> &str;

    /// Test drive connectivity and authentication.
    async fn test_connection(&self) -> anyhow::Result<()>;

    /// List files/folders at an optional path.  `None` means root.
    async fn list_files(&self, path: Option<&str>, limit: usize) -> anyhow::Result<Vec<DriveItem>>;

    /// Get the content of a file by ID.
    async fn get_file(&self, file_id: &str) -> anyhow::Result<DriveFileContent>;

    /// Search for files matching `query`.
    async fn search_files(&self, query: &str, limit: usize) -> anyhow::Result<Vec<DriveItem>>;

    /// Upload a file.
    async fn upload_file(
        &self,
        parent_path: &str,
        name: &str,
        content: &[u8],
        mime_type: &str,
    ) -> anyhow::Result<DriveItem>;

    /// Create a sharing link for a file.
    async fn share_file(&self, file_id: &str, share_with: &[String]) -> anyhow::Result<ShareLink>;

    /// Delete a file.
    async fn delete_file(&self, file_id: &str) -> anyhow::Result<()>;
}
