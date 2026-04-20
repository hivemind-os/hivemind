//! Plugin registry — discovers, installs, and manages plugins.

use crate::manifest::PluginManifest;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

/// Metadata about an installed plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledPlugin {
    pub manifest: PluginManifest,
    pub install_path: PathBuf,
    pub enabled: bool,
    /// Plugin config (non-secret values).
    #[serde(default)]
    pub config: serde_json::Value,
    /// Cached config schema (extracted at registration time).
    #[serde(default)]
    pub config_schema: Option<serde_json::Value>,
    /// Persona IDs allowed to use this plugin's tools. Empty = all personas.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_personas: Vec<String>,
}

/// Manages installed plugins.
pub struct PluginRegistry {
    plugins_dir: PathBuf,
    state_file: PathBuf,
    plugins: parking_lot::RwLock<HashMap<String, InstalledPlugin>>,
}

impl PluginRegistry {
    pub fn new(plugins_dir: PathBuf) -> Self {
        let state_file = plugins_dir.join("plugins.json");
        Self {
            plugins_dir,
            state_file,
            plugins: parking_lot::RwLock::new(HashMap::new()),
        }
    }

    /// Load registry state from disk.
    pub fn load(&self) -> anyhow::Result<()> {
        if self.state_file.exists() {
            let content = std::fs::read_to_string(&self.state_file)?;
            let mut plugins: HashMap<String, InstalledPlugin> = serde_json::from_str(&content)?;
            // Refresh schemas for plugins that were registered before schema extraction existed
            let mut updated = false;
            for plugin in plugins.values_mut() {
                if plugin.config_schema.is_none() {
                    plugin.config_schema = Self::read_config_schema(&plugin.install_path);
                    if plugin.config_schema.is_some() {
                        updated = true;
                    }
                }
            }
            *self.plugins.write() = plugins;
            if updated {
                let _ = self.save();
            }
            info!(count = self.plugins.read().len(), "Loaded plugin registry");
        }
        Ok(())
    }

    /// Save registry state to disk.
    pub fn save(&self) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.plugins_dir)?;
        let content = serde_json::to_string_pretty(&*self.plugins.read())?;
        std::fs::write(&self.state_file, content)?;
        Ok(())
    }

    /// Discover plugins in the plugins directory.
    pub fn discover(&self) -> anyhow::Result<Vec<PluginManifest>> {
        let mut discovered = Vec::new();

        if !self.plugins_dir.exists() {
            return Ok(discovered);
        }

        for entry in std::fs::read_dir(&self.plugins_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                let pkg_json = path.join("package.json");
                if pkg_json.exists() {
                    match PluginManifest::from_package_json(&pkg_json) {
                        Ok(manifest) => {
                            discovered.push(manifest);
                        }
                        Err(e) => {
                            warn!(path = %pkg_json.display(), error = %e, "Failed to parse plugin manifest");
                        }
                    }
                }
            }
        }

        Ok(discovered)
    }

    /// Register a plugin from a local directory path.
    pub fn register_local(&self, package_dir: &Path) -> anyhow::Result<String> {
        let pkg_json = package_dir.join("package.json");
        let manifest = PluginManifest::from_package_json(&pkg_json)?;
        let plugin_id = manifest.plugin_id();
        let config_schema = Self::read_config_schema(package_dir);

        let installed = InstalledPlugin {
            manifest,
            install_path: package_dir.to_path_buf(),
            enabled: true,
            config: serde_json::Value::Object(Default::default()),
            config_schema,
            allowed_personas: Vec::new(),
        };

        self.plugins.write().insert(plugin_id.clone(), installed);
        self.save()?;

        info!(plugin_id, path = %package_dir.display(), "Plugin registered (local)");
        Ok(plugin_id)
    }

    /// Uninstall a plugin.
    pub fn uninstall(&self, plugin_id: &str) -> anyhow::Result<()> {
        self.plugins.write().remove(plugin_id);
        self.save()?;
        info!(plugin_id, "Plugin uninstalled");
        Ok(())
    }

    /// Get an installed plugin.
    pub fn get(&self, plugin_id: &str) -> Option<InstalledPlugin> {
        self.plugins.read().get(plugin_id).cloned()
    }

    /// List all installed plugins.
    pub fn list(&self) -> Vec<InstalledPlugin> {
        self.plugins.read().values().cloned().collect()
    }

    /// List plugins accessible to a specific persona.
    ///
    /// A plugin is accessible if its `allowed_personas` is empty (all personas)
    /// or contains the given persona ID.
    pub fn list_for_persona(&self, persona_id: &str) -> Vec<InstalledPlugin> {
        self.plugins
            .read()
            .values()
            .filter(|p| {
                p.allowed_personas.is_empty()
                    || p.allowed_personas.iter().any(|a| a == "*" || a == persona_id)
            })
            .cloned()
            .collect()
    }

    /// Update plugin config.
    pub fn update_config(&self, plugin_id: &str, config: serde_json::Value) -> anyhow::Result<()> {
        let mut plugins = self.plugins.write();
        if let Some(plugin) = plugins.get_mut(plugin_id) {
            plugin.config = config;
            drop(plugins);
            self.save()?;
            Ok(())
        } else {
            anyhow::bail!("Plugin not found: {}", plugin_id)
        }
    }

    /// Enable/disable a plugin.
    pub fn set_enabled(&self, plugin_id: &str, enabled: bool) -> anyhow::Result<()> {
        let mut plugins = self.plugins.write();
        if let Some(plugin) = plugins.get_mut(plugin_id) {
            plugin.enabled = enabled;
            drop(plugins);
            self.save()?;
            Ok(())
        } else {
            anyhow::bail!("Plugin not found: {}", plugin_id)
        }
    }

    /// Update the allowed personas for a plugin.
    pub fn set_allowed_personas(&self, plugin_id: &str, personas: Vec<String>) -> anyhow::Result<()> {
        let mut plugins = self.plugins.write();
        if let Some(plugin) = plugins.get_mut(plugin_id) {
            plugin.allowed_personas = personas;
            drop(plugins);
            self.save()?;
            Ok(())
        } else {
            anyhow::bail!("Plugin not found: {}", plugin_id)
        }
    }

    /// Install a plugin from npm.
    ///
    /// Runs `npm install <package_name>` in the plugins directory and registers
    /// the plugin from the resulting node_modules entry.
    pub fn install_npm(&self, package_name: &str) -> anyhow::Result<String> {
        use std::process::Command;

        std::fs::create_dir_all(&self.plugins_dir)?;

        // Initialize package.json in plugins dir if it doesn't exist
        let pkg_json = self.plugins_dir.join("package.json");
        if !pkg_json.exists() {
            std::fs::write(&pkg_json, r#"{"private":true,"dependencies":{}}"#)?;
        }

        // Run npm install
        info!(package = package_name, "Installing plugin from npm");
        let output = Command::new("npm")
            .arg("install")
            .arg("--save")
            .arg(package_name)
            .current_dir(&self.plugins_dir)
            .output()
            .map_err(|e| anyhow::anyhow!("Failed to run npm: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("npm install failed: {}", stderr);
        }

        // Determine the actual package directory name (strip scope if present)
        let dir_name = if package_name.starts_with('@') {
            // Scoped package: @scope/name → node_modules/@scope/name
            package_name
                .split('/')
                .last()
                .unwrap_or(package_name)
        } else {
            // Strip version specifiers: name@1.0.0 → name
            package_name.split('@').next().unwrap_or(package_name)
        };

        // Look for the package in node_modules
        let nm = self.plugins_dir.join("node_modules");
        let install_path = if package_name.starts_with('@') {
            // Scoped: @scope/name
            let scope_and_name: Vec<&str> = package_name.splitn(2, '/').collect();
            if scope_and_name.len() == 2 {
                let name_part = scope_and_name[1].split('@').next().unwrap_or(scope_and_name[1]);
                nm.join(scope_and_name[0]).join(name_part)
            } else {
                nm.join(dir_name)
            }
        } else {
            nm.join(dir_name)
        };

        let plugin_pkg_json = install_path.join("package.json");
        if !plugin_pkg_json.exists() {
            anyhow::bail!(
                "Installed package not found at {}",
                plugin_pkg_json.display()
            );
        }

        let manifest = PluginManifest::from_package_json(&plugin_pkg_json)?;
        let plugin_id = manifest.plugin_id();
        let config_schema = Self::read_config_schema(&install_path);

        let installed = InstalledPlugin {
            manifest,
            install_path,
            enabled: true,
            config: serde_json::Value::Object(Default::default()),
            config_schema,
            allowed_personas: Vec::new(),
        };

        self.plugins.write().insert(plugin_id.clone(), installed);
        self.save()?;

        info!(plugin_id, package = package_name, "Plugin installed from npm");
        Ok(plugin_id)
    }

    /// Read config-schema.json from a plugin's dist directory.
    fn read_config_schema(package_dir: &Path) -> Option<serde_json::Value> {
        let schema_path = package_dir.join("dist").join("config-schema.json");
        match std::fs::read_to_string(&schema_path) {
            Ok(content) => match serde_json::from_str(&content) {
                Ok(val) => {
                    info!(path = %schema_path.display(), "Loaded plugin config schema");
                    Some(val)
                }
                Err(e) => {
                    warn!(path = %schema_path.display(), error = %e, "Failed to parse config schema");
                    None
                }
            },
            Err(_) => {
                info!(path = %schema_path.display(), "No config-schema.json found");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_lifecycle() {
        let dir = tempfile::tempdir().unwrap();
        let registry = PluginRegistry::new(dir.path().to_path_buf());

        // Create a fake plugin
        let plugin_dir = dir.path().join("test-plugin");
        std::fs::create_dir_all(&plugin_dir).unwrap();
        std::fs::write(
            plugin_dir.join("package.json"),
            r#"{
                "name": "test-plugin",
                "version": "1.0.0",
                "main": "dist/index.js",
                "hivemind": {
                    "type": "connector",
                    "displayName": "Test",
                    "description": "Test plugin"
                }
            }"#,
        )
        .unwrap();

        // Register
        let id = registry.register_local(&plugin_dir).unwrap();
        assert_eq!(id, "test-plugin");

        // Get
        let plugin = registry.get("test-plugin").unwrap();
        assert!(plugin.enabled);

        // List
        assert_eq!(registry.list().len(), 1);

        // Update config
        registry
            .update_config("test-plugin", serde_json::json!({"key": "val"}))
            .unwrap();
        let plugin = registry.get("test-plugin").unwrap();
        assert_eq!(plugin.config["key"], "val");

        // Disable
        registry.set_enabled("test-plugin", false).unwrap();
        let plugin = registry.get("test-plugin").unwrap();
        assert!(!plugin.enabled);

        // Uninstall
        registry.uninstall("test-plugin").unwrap();
        assert!(registry.get("test-plugin").is_none());
    }
}
