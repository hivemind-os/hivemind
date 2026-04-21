//! Plugin manifest — parsed from `package.json` `hivemind` field.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Parsed plugin manifest from package.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    /// npm package name (e.g., "@hivemind/connector-github-issues").
    pub name: String,
    /// Package version.
    pub version: String,
    /// Main entry point (e.g., "dist/index.js").
    pub main: String,
    /// Hivemind-specific metadata.
    pub hivemind: HivemindMeta,
}

/// The `hivemind` field in package.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HivemindMeta {
    /// Plugin type: "connector", "tool-pack", or "integration".
    #[serde(rename = "type")]
    pub plugin_type: String,
    /// Human-readable display name.
    pub display_name: String,
    /// Plugin description.
    pub description: String,
    /// Icon path relative to the package root.
    #[serde(default)]
    pub icon: Option<String>,
    /// Plugin categories for discovery.
    #[serde(default)]
    pub categories: Vec<String>,
    /// Services this plugin provides.
    #[serde(default)]
    pub services: Vec<String>,
    /// Declared permissions.
    #[serde(default)]
    pub permissions: Vec<String>,
    /// Minimum host version required.
    #[serde(default)]
    pub min_host_version: Option<String>,
}

impl PluginManifest {
    /// Parse a manifest from a package.json file.
    pub fn from_package_json(path: &Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        Self::from_json_str(&content)
    }

    /// Parse a manifest from a JSON string.
    pub fn from_json_str(json: &str) -> anyhow::Result<Self> {
        let raw: serde_json::Value = serde_json::from_str(json)?;

        let name = raw["name"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'name' field"))?
            .to_string();
        let version = raw["version"].as_str().unwrap_or("0.0.0").to_string();
        let main = raw["main"].as_str().unwrap_or("dist/index.js").to_string();

        let hivemind_val = raw
            .get("hivemind")
            .ok_or_else(|| anyhow::anyhow!("Missing 'hivemind' field in package.json"))?;
        let hivemind: HivemindMeta = serde_json::from_value(hivemind_val.clone())?;

        Ok(Self { name, version, main, hivemind })
    }

    /// Generate a stable plugin ID from the package name.
    pub fn plugin_id(&self) -> String {
        self.name.trim_start_matches('@').replace('/', ".").replace(' ', "-")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_manifest() {
        let json = r#"{
            "name": "@hivemind/connector-github-issues",
            "version": "0.1.0",
            "main": "dist/index.js",
            "hivemind": {
                "type": "connector",
                "displayName": "GitHub Issues",
                "description": "Track issues",
                "categories": ["dev-tools"],
                "permissions": ["secrets:read", "loop:background"]
            }
        }"#;
        let manifest = PluginManifest::from_json_str(json).unwrap();
        assert_eq!(manifest.name, "@hivemind/connector-github-issues");
        assert_eq!(manifest.hivemind.plugin_type, "connector");
        assert_eq!(manifest.hivemind.display_name, "GitHub Issues");
        assert_eq!(manifest.plugin_id(), "hivemind.connector-github-issues");
        assert_eq!(manifest.hivemind.permissions.len(), 2);
    }

    #[test]
    fn test_missing_hivemind_field() {
        let json = r#"{"name": "test", "version": "1.0.0"}"#;
        assert!(PluginManifest::from_json_str(json).is_err());
    }
}
