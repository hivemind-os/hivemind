//! Managed Node.js environment for MCP servers and agent tools.
//!
//! Downloads a pinned Node.js LTS distribution (including `npm` and `npx`)
//! and manages it under `~/.hivemind/runtimes/node/`. This allows MCP servers
//! that require `npx` or `node` to work even when the user does not have
//! Node.js installed system-wide.

mod download;
mod manager;
mod platform;

pub use manager::{NodeEnvManager, NodeEnvStatus};

use serde::{Deserialize, Serialize};

/// Configuration for the managed Node.js environment.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct NodeEnvConfig {
    /// Whether the managed Node.js environment is enabled.
    pub enabled: bool,
    /// Node.js version to install (e.g. "22.16.0" — current LTS).
    pub node_version: String,
}

impl Default for NodeEnvConfig {
    fn default() -> Self {
        Self { enabled: true, node_version: default_node_version() }
    }
}

fn default_node_version() -> String {
    "22.16.0".to_string()
}

/// Validate that a node version string is a safe semver-like value.
/// Rejects path traversal attempts (`..`, `/`, `\`) and any characters
/// outside `[0-9.]`.
pub fn validate_node_version(version: &str) -> Result<(), NodeEnvError> {
    if version.is_empty() {
        return Err(NodeEnvError::InvalidVersion("version string is empty".into()));
    }
    // Only allow digits and dots (e.g. "22.16.0").
    if !version.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return Err(NodeEnvError::InvalidVersion(format!(
            "version contains invalid characters: {version}"
        )));
    }
    // Must have at least major.minor.patch form.
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() != 3 || parts.iter().any(|p| p.is_empty()) {
        return Err(NodeEnvError::InvalidVersion(format!(
            "version must be in major.minor.patch format: {version}"
        )));
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum NodeEnvError {
    #[error("download failed: {0}")]
    Download(String),
    #[error("extraction failed: {0}")]
    Extraction(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Node.js environment is disabled")]
    Disabled,
    #[error("invalid node version: {0}")]
    InvalidVersion(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_expected_values() {
        let cfg = NodeEnvConfig::default();
        assert!(cfg.enabled);
        assert_eq!(cfg.node_version, "22.16.0");
    }

    #[test]
    fn config_roundtrips_through_json() {
        let cfg = NodeEnvConfig::default();
        let json = serde_json::to_string(&cfg).unwrap();
        let restored: NodeEnvConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.node_version, cfg.node_version);
        assert_eq!(restored.enabled, cfg.enabled);
    }

    #[test]
    fn platform_helpers_return_valid_values() {
        let name = platform::node_archive_name("22.16.0");
        assert!(name.ends_with(".tar.gz") || name.ends_with(".tar.xz") || name.ends_with(".zip"));
        assert!(!platform::node_binary_name().is_empty());
        assert!(!platform::npm_binary_name().is_empty());
        assert!(!platform::npx_binary_name().is_empty());
        assert!(!platform::path_separator().is_empty());
    }

    #[test]
    fn download_url_contains_version() {
        let url = platform::node_download_url("22.16.0");
        assert!(url.contains("22.16.0"));
        assert!(url.starts_with("https://"));
    }

    #[test]
    fn validate_version_accepts_valid_semver() {
        assert!(validate_node_version("22.16.0").is_ok());
        assert!(validate_node_version("20.0.0").is_ok());
        assert!(validate_node_version("18.19.1").is_ok());
    }

    #[test]
    fn validate_version_rejects_path_traversal() {
        assert!(validate_node_version("../../../evil").is_err());
        assert!(validate_node_version("22.16.0/../../etc").is_err());
        assert!(validate_node_version("22.16.0\\..\\..").is_err());
    }

    #[test]
    fn validate_version_rejects_invalid_formats() {
        assert!(validate_node_version("").is_err());
        assert!(validate_node_version("abc").is_err());
        assert!(validate_node_version("22").is_err());
        assert!(validate_node_version("22.16").is_err());
        assert!(validate_node_version("22.16.0.1").is_err());
        assert!(validate_node_version("v22.16.0").is_err());
    }
}
