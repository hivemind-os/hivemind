//! Managed Python environment for hivemind agents.
//!
//! Downloads and manages `uv` (Astral's Python toolchain manager), uses it to
//! install a pinned Python version, creates virtual environments with curated
//! base packages, and provides environment variables for shell tool integration.

mod download;
mod manager;
mod platform;

pub use manager::{PythonEnvInfo, PythonEnvManager, PythonEnvStatus};

use serde::{Deserialize, Serialize};

/// Configuration for the managed Python environment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PythonEnvConfig {
    /// Whether the managed Python environment is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Python version to install (e.g. "3.12").
    #[serde(default = "default_python_version")]
    pub python_version: String,
    /// Packages to pre-install in the managed environment.
    #[serde(default = "default_base_packages")]
    pub base_packages: Vec<String>,
    /// Whether to auto-detect and install workspace dependencies.
    #[serde(default = "default_true")]
    pub auto_detect_workspace_deps: bool,
    /// Pinned uv version to download.
    #[serde(default = "default_uv_version")]
    pub uv_version: String,
}

impl Default for PythonEnvConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            python_version: default_python_version(),
            base_packages: default_base_packages(),
            auto_detect_workspace_deps: true,
            uv_version: default_uv_version(),
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_python_version() -> String {
    "3.12".to_string()
}

fn default_uv_version() -> String {
    "0.6.14".to_string()
}

fn default_base_packages() -> Vec<String> {
    [
        "requests",
        "beautifulsoup4",
        "pandas",
        "numpy",
        "pyyaml",
        "python-dateutil",
        "Pillow",
        "matplotlib",
        "jinja2",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

#[derive(Debug, thiserror::Error)]
pub enum PythonEnvError {
    #[error("download failed: {0}")]
    Download(String),
    #[error("extraction failed: {0}")]
    Extraction(String),
    #[error("uv command failed: {0}")]
    UvCommand(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("python environment is disabled")]
    Disabled,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_expected_values() {
        let cfg = PythonEnvConfig::default();
        assert!(cfg.enabled);
        assert_eq!(cfg.python_version, "3.12");
        assert!(cfg.base_packages.contains(&"requests".to_string()));
        assert!(cfg.auto_detect_workspace_deps);
    }

    #[test]
    fn config_roundtrips_through_json() {
        let cfg = PythonEnvConfig::default();
        let json = serde_json::to_string(&cfg).unwrap();
        let restored: PythonEnvConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.python_version, cfg.python_version);
        assert_eq!(restored.base_packages, cfg.base_packages);
    }

    #[tokio::test]
    async fn disabled_manager_returns_error() {
        let dir = std::env::temp_dir().join("hive-python-test-disabled");
        let mgr =
            PythonEnvManager::new(dir, PythonEnvConfig { enabled: false, ..Default::default() });
        assert!(matches!(mgr.ensure_uv().await, Err(PythonEnvError::Disabled)));
        assert!(matches!(mgr.ensure_default_env().await, Err(PythonEnvError::Disabled)));
        assert!(matches!(mgr.status().await, PythonEnvStatus::Disabled));
    }

    #[tokio::test]
    async fn shell_env_vars_returns_none_when_not_ready() {
        let dir = std::env::temp_dir().join("hive-python-test-not-ready");
        let mgr = PythonEnvManager::new(dir, PythonEnvConfig::default());
        assert!(mgr.shell_env_vars(None).await.is_none());
    }

    #[test]
    fn platform_helpers_return_valid_values() {
        let name = platform::uv_archive_name("0.6.14");
        assert!(name.ends_with(".tar.gz") || name.ends_with(".zip"));
        assert!(!platform::uv_binary_name().is_empty());
        assert!(!platform::venv_bin_dir().is_empty());
        assert!(!platform::venv_python_relative().is_empty());
        assert!(!platform::path_separator().is_empty());
    }

    #[test]
    fn uv_download_url_contains_version() {
        let url = platform::uv_download_url("0.6.14");
        assert!(url.contains("0.6.14"));
        assert!(url.starts_with("https://"));
    }
}
