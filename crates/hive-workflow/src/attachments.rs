use std::fs;
use std::path::PathBuf;

use crate::error::WorkflowError;

/// Manages the on-disk storage for workflow file attachments.
///
/// Layout:
/// ```text
/// <base_dir>/
///   <workflow_id>/
///     <version>/
///       <attachment_id>_<filename>
/// ```
pub struct AttachmentStore {
    base_dir: PathBuf,
}

impl AttachmentStore {
    /// Create a new store rooted at `base_dir`.  The directory is created
    /// lazily on the first write.
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self { base_dir: base_dir.into() }
    }

    /// Directory for a specific workflow version's attachments.
    pub fn version_dir(&self, workflow_id: &str, version: &str) -> PathBuf {
        self.base_dir.join(sanitize_component(workflow_id)).join(sanitize_component(version))
    }

    /// Full path for a single attachment file.
    pub fn attachment_path(
        &self,
        workflow_id: &str,
        version: &str,
        attachment_id: &str,
        filename: &str,
    ) -> PathBuf {
        self.version_dir(workflow_id, version).join(format!(
            "{}_{}",
            sanitize_component(attachment_id),
            sanitize_filename(filename)
        ))
    }

    /// Store an attachment on disk.  Returns the path written to.
    pub fn store(
        &self,
        workflow_id: &str,
        version: &str,
        attachment_id: &str,
        filename: &str,
        data: &[u8],
    ) -> Result<PathBuf, WorkflowError> {
        let dir = self.version_dir(workflow_id, version);
        fs::create_dir_all(&dir).map_err(|e| {
            WorkflowError::Other(format!(
                "failed to create attachment directory {}: {e}",
                dir.display()
            ))
        })?;

        let path = self.attachment_path(workflow_id, version, attachment_id, filename);
        fs::write(&path, data).map_err(|e| {
            WorkflowError::Other(format!("failed to write attachment {}: {e}", path.display()))
        })?;

        Ok(path)
    }

    /// Delete a single attachment file.
    pub fn delete(
        &self,
        workflow_id: &str,
        version: &str,
        attachment_id: &str,
        filename: &str,
    ) -> Result<(), WorkflowError> {
        let path = self.attachment_path(workflow_id, version, attachment_id, filename);
        if path.exists() {
            fs::remove_file(&path).map_err(|e| {
                WorkflowError::Other(format!("failed to delete attachment {}: {e}", path.display()))
            })?;
        }
        Ok(())
    }

    /// Check whether an attachment file exists on disk.
    pub fn exists(
        &self,
        workflow_id: &str,
        version: &str,
        attachment_id: &str,
        filename: &str,
    ) -> bool {
        self.attachment_path(workflow_id, version, attachment_id, filename).exists()
    }

    /// Copy all attachment files from one version to another.
    pub fn copy_version(
        &self,
        workflow_id: &str,
        from_version: &str,
        to_version: &str,
    ) -> Result<(), WorkflowError> {
        let src = self.version_dir(workflow_id, from_version);
        let dst = self.version_dir(workflow_id, to_version);

        if !src.exists() {
            return Ok(());
        }

        fs::create_dir_all(&dst).map_err(|e| {
            WorkflowError::Other(format!(
                "failed to create target attachment directory {}: {e}",
                dst.display()
            ))
        })?;

        for entry in fs::read_dir(&src).map_err(|e| {
            WorkflowError::Other(format!(
                "failed to read attachment directory {}: {e}",
                src.display()
            ))
        })? {
            let entry = entry.map_err(|e| WorkflowError::Other(e.to_string()))?;
            let file_type = entry.file_type().map_err(|e| WorkflowError::Other(e.to_string()))?;
            if file_type.is_file() {
                let dest = dst.join(entry.file_name());
                if !dest.exists() {
                    fs::copy(entry.path(), &dest).map_err(|e| {
                        WorkflowError::Other(format!(
                            "failed to copy attachment {}: {e}",
                            entry.path().display()
                        ))
                    })?;
                }
            }
        }
        Ok(())
    }

    /// Delete the entire version folder and all its attachments.
    pub fn delete_version(&self, workflow_id: &str, version: &str) -> Result<(), WorkflowError> {
        let dir = self.version_dir(workflow_id, version);
        if dir.exists() {
            fs::remove_dir_all(&dir).map_err(|e| {
                WorkflowError::Other(format!(
                    "failed to delete attachment directory {}: {e}",
                    dir.display()
                ))
            })?;
        }
        Ok(())
    }
}

/// Sanitize a path component to prevent directory traversal.
fn sanitize_component(s: &str) -> String {
    let filtered: String =
        s.chars().filter(|c| c.is_alphanumeric() || matches!(c, '-' | '_')).collect();
    if filtered.is_empty() {
        "_".to_string()
    } else {
        filtered
    }
}

/// Sanitize a filename, keeping the extension but removing path separators.
fn sanitize_filename(s: &str) -> String {
    let name: String = s
        .replace(['/', '\\'], "_")
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | ' '))
        .collect();
    if name.is_empty() {
        "unnamed".to_string()
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_store_and_read() {
        let tmp = tempfile::tempdir().unwrap();
        let store = AttachmentStore::new(tmp.path());

        let data = b"hello world";
        let path = store.store("wf1", "1.0", "att1", "readme.txt", data).unwrap();
        assert!(path.exists());
        assert_eq!(fs::read(&path).unwrap(), data);
        assert!(store.exists("wf1", "1.0", "att1", "readme.txt"));
    }

    #[test]
    fn test_delete() {
        let tmp = tempfile::tempdir().unwrap();
        let store = AttachmentStore::new(tmp.path());

        store.store("wf1", "1.0", "att1", "f.txt", b"data").unwrap();
        assert!(store.exists("wf1", "1.0", "att1", "f.txt"));

        store.delete("wf1", "1.0", "att1", "f.txt").unwrap();
        assert!(!store.exists("wf1", "1.0", "att1", "f.txt"));
    }

    #[test]
    fn test_copy_version() {
        let tmp = tempfile::tempdir().unwrap();
        let store = AttachmentStore::new(tmp.path());

        store.store("wf1", "1.0", "att1", "a.txt", b"aaa").unwrap();
        store.store("wf1", "1.0", "att2", "b.txt", b"bbb").unwrap();

        store.copy_version("wf1", "1.0", "2.0").unwrap();

        assert!(store.exists("wf1", "2.0", "att1", "a.txt"));
        assert!(store.exists("wf1", "2.0", "att2", "b.txt"));

        let content = fs::read(store.attachment_path("wf1", "2.0", "att1", "a.txt")).unwrap();
        assert_eq!(content, b"aaa");
    }

    #[test]
    fn test_delete_version() {
        let tmp = tempfile::tempdir().unwrap();
        let store = AttachmentStore::new(tmp.path());

        store.store("wf1", "1.0", "att1", "f.txt", b"data").unwrap();
        store.delete_version("wf1", "1.0").unwrap();

        assert!(!store.version_dir("wf1", "1.0").exists());
    }

    #[test]
    fn test_sanitize_component() {
        assert_eq!(sanitize_component("../etc/passwd"), "etcpasswd");
        assert_eq!(sanitize_component("my-workflow_v1"), "my-workflow_v1");
    }

    #[test]
    fn test_copy_version_no_source() {
        let tmp = tempfile::tempdir().unwrap();
        let store = AttachmentStore::new(tmp.path());
        // Should succeed silently when source doesn't exist
        store.copy_version("wf1", "1.0", "2.0").unwrap();
    }
}
