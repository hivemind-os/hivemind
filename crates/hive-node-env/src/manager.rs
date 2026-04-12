//! Core Node.js environment manager.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::{download, platform, NodeEnvConfig, NodeEnvError};

/// Status of the managed Node.js environment.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum NodeEnvStatus {
    Disabled,
    NotInstalled,
    Installing { progress: String },
    Ready { node_dir: String },
    Failed { error: String },
}

/// Manages the lifecycle of a Node.js runtime for MCP servers and tools.
pub struct NodeEnvManager {
    hivemind_home: PathBuf,
    config: NodeEnvConfig,
    status: Arc<RwLock<NodeEnvStatus>>,
    status_tx: tokio::sync::broadcast::Sender<NodeEnvStatus>,
    /// Serializes install/reinstall operations to prevent concurrent downloads.
    install_lock: tokio::sync::Mutex<()>,
}

impl NodeEnvManager {
    pub fn new(hivemind_home: PathBuf, config: NodeEnvConfig) -> Self {
        let initial_status =
            if config.enabled { NodeEnvStatus::NotInstalled } else { NodeEnvStatus::Disabled };
        let (status_tx, _) = tokio::sync::broadcast::channel(16);
        Self {
            hivemind_home,
            config,
            status: Arc::new(RwLock::new(initial_status)),
            status_tx,
            install_lock: tokio::sync::Mutex::new(()),
        }
    }

    /// Root directory for managed Node.js installations.
    fn node_runtimes_dir(&self) -> PathBuf {
        self.hivemind_home.join("runtimes").join("node")
    }

    /// Current status of the managed Node.js environment.
    pub async fn status(&self) -> NodeEnvStatus {
        self.status.read().await.clone()
    }

    /// Non-async status check (returns `None` if the lock is contended).
    pub fn status_blocking(&self) -> Option<NodeEnvStatus> {
        self.status.try_read().ok().map(|s| s.clone())
    }

    /// Subscribe to status change notifications.
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<NodeEnvStatus> {
        self.status_tx.subscribe()
    }

    /// Set status and broadcast the change.
    async fn set_status(&self, new_status: NodeEnvStatus) {
        *self.status.write().await = new_status.clone();
        let _ = self.status_tx.send(new_status);
    }

    /// The configured Node.js version.
    pub fn node_version(&self) -> &str {
        &self.config.node_version
    }

    /// Whether managed Node.js is enabled.
    pub fn enabled(&self) -> bool {
        self.config.enabled
    }

    /// Ensure the Node.js distribution is downloaded and available.
    ///
    /// Returns the path to the distribution root directory.
    /// Serialized via an internal mutex — concurrent callers will wait.
    pub async fn ensure_node(&self) -> Result<PathBuf, NodeEnvError> {
        if !self.config.enabled {
            return Err(NodeEnvError::Disabled);
        }

        // Validate version before anything else.
        crate::validate_node_version(&self.config.node_version)?;

        // Serialize concurrent install attempts.
        let _guard = self.install_lock.lock().await;

        // Re-check status under the lock — another caller may have finished.
        if matches!(*self.status.read().await, NodeEnvStatus::Ready { .. }) {
            let status = self.status.read().await;
            if let NodeEnvStatus::Ready { node_dir } = &*status {
                return Ok(PathBuf::from(node_dir));
            }
        }

        self.set_status(NodeEnvStatus::Installing { progress: "downloading Node.js".to_string() })
            .await;
        tracing::info!(version = %self.config.node_version, "ensuring Node.js runtime");

        match download::ensure_node_distribution(
            &self.node_runtimes_dir(),
            &self.config.node_version,
        )
        .await
        {
            Ok(dist_dir) => {
                let dir_str = dist_dir.to_string_lossy().to_string();
                tracing::info!(path = %dist_dir.display(), "Node.js runtime ready");
                self.set_status(NodeEnvStatus::Ready { node_dir: dir_str }).await;
                Ok(dist_dir)
            }
            Err(e) => {
                tracing::error!(error = %e, "failed to ensure Node.js runtime");
                self.set_status(NodeEnvStatus::Failed { error: e.to_string() }).await;
                Err(e)
            }
        }
    }

    /// Return environment variables to inject into commands that need Node.js.
    ///
    /// Returns `None` if the environment is not ready.
    pub async fn shell_env_vars(&self) -> Option<HashMap<String, String>> {
        let status = self.status.read().await;
        let node_dir = match &*status {
            NodeEnvStatus::Ready { node_dir } => PathBuf::from(node_dir),
            _ => return None,
        };

        let bin_dir = if platform::node_bin_dir().is_empty() {
            node_dir.clone()
        } else {
            node_dir.join(platform::node_bin_dir())
        };

        let existing_path = std::env::var("PATH").unwrap_or_default();
        let new_path =
            format!("{}{}{}", bin_dir.to_string_lossy(), platform::path_separator(), existing_path);

        let mut vars = HashMap::new();
        vars.insert("PATH".to_string(), new_path);
        Some(vars)
    }

    /// Get the path to the node binary, if ready.
    pub async fn node_binary_path(&self) -> Option<PathBuf> {
        let status = self.status.read().await;
        match &*status {
            NodeEnvStatus::Ready { node_dir } => {
                let dir = PathBuf::from(node_dir);
                let bin = if platform::node_bin_dir().is_empty() {
                    dir.join(platform::node_binary_name())
                } else {
                    dir.join(platform::node_bin_dir()).join(platform::node_binary_name())
                };
                if bin.exists() {
                    Some(bin)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Force re-download the Node.js distribution.
    /// Serialized via the same install lock as `ensure_node`.
    pub async fn reinstall(&self) -> Result<PathBuf, NodeEnvError> {
        crate::validate_node_version(&self.config.node_version)?;

        let _guard = self.install_lock.lock().await;

        let dist_dir = self
            .node_runtimes_dir()
            .join(platform::node_archive_dir_name(&self.config.node_version));
        if dist_dir.exists() {
            std::fs::remove_dir_all(&dist_dir)?;
        }

        // Release lock before calling ensure_node? No — we hold the lock
        // and call the inner download directly to avoid double-locking.
        self.set_status(NodeEnvStatus::Installing { progress: "downloading Node.js".to_string() })
            .await;
        tracing::info!(version = %self.config.node_version, "reinstalling Node.js runtime");

        match download::ensure_node_distribution(
            &self.node_runtimes_dir(),
            &self.config.node_version,
        )
        .await
        {
            Ok(dist_dir) => {
                let dir_str = dist_dir.to_string_lossy().to_string();
                tracing::info!(path = %dist_dir.display(), "Node.js runtime reinstalled");
                self.set_status(NodeEnvStatus::Ready { node_dir: dir_str }).await;
                Ok(dist_dir)
            }
            Err(e) => {
                tracing::error!(error = %e, "failed to reinstall Node.js runtime");
                self.set_status(NodeEnvStatus::Failed { error: e.to_string() }).await;
                Err(e)
            }
        }
    }

    /// Check if the managed distribution is already present on disk
    /// and update status accordingly. Called on startup to detect
    /// a previously-installed Node.js without re-downloading.
    pub async fn detect_existing(&self) {
        if !self.config.enabled {
            return;
        }

        let dist_dir = self
            .node_runtimes_dir()
            .join(platform::node_archive_dir_name(&self.config.node_version));
        let node_binary = if platform::node_bin_dir().is_empty() {
            dist_dir.join(platform::node_binary_name())
        } else {
            dist_dir.join(platform::node_bin_dir()).join(platform::node_binary_name())
        };

        if node_binary.exists() {
            self.set_status(NodeEnvStatus::Ready {
                node_dir: dist_dir.to_string_lossy().to_string(),
            })
            .await;
            tracing::info!(
                path = %dist_dir.display(),
                "detected existing managed Node.js"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn disabled_manager_returns_error() {
        let dir = std::env::temp_dir().join("hive-node-test-disabled");
        let mgr = NodeEnvManager::new(dir, NodeEnvConfig { enabled: false, ..Default::default() });
        assert!(matches!(mgr.ensure_node().await, Err(NodeEnvError::Disabled)));
        assert!(matches!(mgr.status().await, NodeEnvStatus::Disabled));
    }

    #[tokio::test]
    async fn shell_env_vars_returns_none_when_not_ready() {
        let dir = std::env::temp_dir().join("hive-node-test-not-ready");
        let mgr = NodeEnvManager::new(dir, NodeEnvConfig::default());
        assert!(mgr.shell_env_vars().await.is_none());
    }

    #[tokio::test]
    async fn detect_existing_stays_not_installed_when_missing() {
        let dir = std::env::temp_dir().join("hive-node-test-detect-missing");
        let mgr = NodeEnvManager::new(dir, NodeEnvConfig::default());
        mgr.detect_existing().await;
        assert!(matches!(mgr.status().await, NodeEnvStatus::NotInstalled));
    }

    #[test]
    fn node_version_accessor() {
        let mgr = NodeEnvManager::new(PathBuf::from("."), NodeEnvConfig::default());
        assert_eq!(mgr.node_version(), "22.16.0");
    }

    /// Create a fake Node.js distribution directory with a dummy binary.
    fn create_fake_node_dist(hivemind_home: &std::path::Path, version: &str) -> PathBuf {
        let runtimes = hivemind_home.join("runtimes").join("node");
        let archive_dir = runtimes.join(crate::platform::node_archive_dir_name(version));
        let bin_path = if crate::platform::node_bin_dir().is_empty() {
            archive_dir.join(crate::platform::node_binary_name())
        } else {
            archive_dir
                .join(crate::platform::node_bin_dir())
                .join(crate::platform::node_binary_name())
        };
        std::fs::create_dir_all(bin_path.parent().unwrap()).unwrap();
        std::fs::write(&bin_path, b"fake").unwrap();
        archive_dir
    }

    #[tokio::test]
    async fn detect_existing_finds_fake_distribution() {
        let dir = std::env::temp_dir().join("hive-node-test-detect-found");
        let _ = std::fs::remove_dir_all(&dir);
        let version = "22.16.0";
        let expected_dir = create_fake_node_dist(&dir, version);

        let mgr = NodeEnvManager::new(dir.clone(), NodeEnvConfig::default());
        mgr.detect_existing().await;

        match mgr.status().await {
            NodeEnvStatus::Ready { node_dir } => {
                assert_eq!(node_dir, expected_dir.to_string_lossy().to_string());
            }
            other => panic!("expected Ready, got {:?}", other),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn ensure_node_rejects_invalid_version() {
        let dir = std::env::temp_dir().join("hive-node-test-invalid-ver");
        let mgr = NodeEnvManager::new(
            dir,
            NodeEnvConfig { enabled: true, node_version: "../evil".to_string() },
        );

        let result = mgr.ensure_node().await;
        assert!(matches!(result, Err(NodeEnvError::InvalidVersion(_))));
        assert!(matches!(mgr.status().await, NodeEnvStatus::NotInstalled));
    }

    #[tokio::test]
    async fn concurrent_ensure_node_serialized() {
        let dir = std::env::temp_dir().join("hive-node-test-concurrent");
        let _ = std::fs::remove_dir_all(&dir);
        create_fake_node_dist(&dir, "22.16.0");

        let mgr = NodeEnvManager::new(dir.clone(), NodeEnvConfig::default());
        mgr.detect_existing().await;
        assert!(matches!(mgr.status().await, NodeEnvStatus::Ready { .. }));

        let (r1, r2) = tokio::join!(mgr.ensure_node(), mgr.ensure_node());
        assert!(r1.is_ok(), "first concurrent ensure_node failed: {:?}", r1);
        assert!(r2.is_ok(), "second concurrent ensure_node failed: {:?}", r2);
        assert_eq!(r1.unwrap(), r2.unwrap());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn shell_env_vars_path_when_ready() {
        let dir = std::env::temp_dir().join("hive-node-test-shell-env");
        let _ = std::fs::remove_dir_all(&dir);
        let archive_dir = create_fake_node_dist(&dir, "22.16.0");

        let mgr = NodeEnvManager::new(dir.clone(), NodeEnvConfig::default());
        mgr.detect_existing().await;

        let vars = mgr.shell_env_vars().await.expect("expected Some env vars");
        let path_val = vars.get("PATH").expect("expected PATH key");

        let expected_bin_dir = if crate::platform::node_bin_dir().is_empty() {
            archive_dir.to_string_lossy().to_string()
        } else {
            archive_dir.join(crate::platform::node_bin_dir()).to_string_lossy().to_string()
        };
        assert!(
            path_val.starts_with(&expected_bin_dir),
            "PATH should start with node bin dir.\n  PATH: {path_val}\n  expected prefix: {expected_bin_dir}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn reinstall_rejects_invalid_version() {
        let dir = std::env::temp_dir().join("hive-node-test-reinstall-invalid");
        let mgr = NodeEnvManager::new(
            dir,
            NodeEnvConfig { enabled: true, node_version: "../evil".to_string() },
        );

        let result = mgr.reinstall().await;
        assert!(matches!(result, Err(NodeEnvError::InvalidVersion(_))));
    }

    #[tokio::test]
    async fn subscribe_receives_status_changes() {
        let dir = std::env::temp_dir().join("hive-node-test-subscribe");
        let _ = std::fs::remove_dir_all(&dir);
        create_fake_node_dist(&dir, "22.16.0");

        let mgr = NodeEnvManager::new(dir.clone(), NodeEnvConfig::default());
        let mut rx = mgr.subscribe();

        mgr.detect_existing().await;

        let received = rx.recv().await.expect("expected a status broadcast");
        assert!(
            matches!(received, NodeEnvStatus::Ready { .. }),
            "expected Ready status from subscription, got {:?}",
            received
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
