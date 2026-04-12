use crate::DataClass;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Hierarchical classification for workspace files and directories.
/// The default applies to everything; per-path overrides are more specific.
/// Resolution walks up parent paths to find the nearest override, falling back to default.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceClassification {
    /// Default classification for the entire workspace
    pub default: DataClass,
    /// Per-path overrides (path uses forward slashes, relative to workspace root)
    /// Can be set on directories or individual files
    pub overrides: HashMap<String, DataClass>,
}

impl Default for WorkspaceClassification {
    fn default() -> Self {
        Self { default: DataClass::Internal, overrides: HashMap::new() }
    }
}

impl WorkspaceClassification {
    pub fn new(default: DataClass) -> Self {
        Self { default, overrides: HashMap::new() }
    }

    /// Set an override for a specific path
    pub fn set_override(&mut self, path: &str, class: DataClass) {
        let normalized = normalize_path(path);
        self.overrides.insert(normalized, class);
    }

    /// Clear an override for a specific path (reverts to inheritance)
    pub fn clear_override(&mut self, path: &str) -> bool {
        let normalized = normalize_path(path);
        self.overrides.remove(&normalized).is_some()
    }

    /// Resolve the effective classification for a given path.
    /// Walks up parent directories to find the nearest override,
    /// falling back to the workspace default.
    pub fn resolve(&self, path: &str) -> DataClass {
        let normalized = normalize_path(path);

        // Check exact path match first
        if let Some(&class) = self.overrides.get(&normalized) {
            return class;
        }

        // Walk up parent directories
        let mut current = normalized.as_str();
        while let Some(pos) = current.rfind('/') {
            current = &current[..pos];
            if let Some(&class) = self.overrides.get(current) {
                return class;
            }
        }

        self.default
    }

    /// Check if a path has a direct override (not inherited)
    pub fn has_override(&self, path: &str) -> bool {
        let normalized = normalize_path(path);
        self.overrides.contains_key(&normalized)
    }
}

fn normalize_path(path: &str) -> String {
    use std::path::Component;
    let replaced = path.replace('\\', "/");
    let mut parts: Vec<&str> = Vec::new();
    for component in std::path::Path::new(&replaced).components() {
        match component {
            Component::ParentDir => {
                parts.pop();
            }
            Component::CurDir => {}
            Component::Normal(s) => {
                parts.push(s.to_str().unwrap_or(""));
            }
            Component::RootDir => {
                parts.push("");
            }
            Component::Prefix(_) => {}
        }
    }
    let result = parts.join("/");
    result.trim_matches('/').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_classification() {
        let wc = WorkspaceClassification::default();
        assert_eq!(wc.resolve("any/file.txt"), DataClass::Internal);
    }

    #[test]
    fn exact_path_override() {
        let mut wc = WorkspaceClassification::new(DataClass::Public);
        wc.set_override("secrets/keys.txt", DataClass::Restricted);
        assert_eq!(wc.resolve("secrets/keys.txt"), DataClass::Restricted);
        assert_eq!(wc.resolve("other/file.txt"), DataClass::Public);
    }

    #[test]
    fn directory_inheritance() {
        let mut wc = WorkspaceClassification::new(DataClass::Public);
        wc.set_override("internal/docs", DataClass::Confidential);
        assert_eq!(wc.resolve("internal/docs/report.pdf"), DataClass::Confidential);
        assert_eq!(wc.resolve("internal/docs/sub/deep.txt"), DataClass::Confidential);
        assert_eq!(wc.resolve("internal/other.txt"), DataClass::Public);
    }

    #[test]
    fn nested_overrides_most_specific_wins() {
        let mut wc = WorkspaceClassification::new(DataClass::Public);
        wc.set_override("src", DataClass::Internal);
        wc.set_override("src/auth", DataClass::Confidential);
        wc.set_override("src/auth/keys.rs", DataClass::Restricted);
        assert_eq!(wc.resolve("src/utils.rs"), DataClass::Internal);
        assert_eq!(wc.resolve("src/auth/handler.rs"), DataClass::Confidential);
        assert_eq!(wc.resolve("src/auth/keys.rs"), DataClass::Restricted);
    }

    #[test]
    fn clear_override_reverts_to_parent() {
        let mut wc = WorkspaceClassification::new(DataClass::Public);
        wc.set_override("data", DataClass::Confidential);
        wc.set_override("data/public", DataClass::Public);
        assert_eq!(wc.resolve("data/public/file.txt"), DataClass::Public);
        assert!(wc.clear_override("data/public"));
        assert_eq!(wc.resolve("data/public/file.txt"), DataClass::Confidential);
    }

    #[test]
    fn has_override_check() {
        let mut wc = WorkspaceClassification::new(DataClass::Public);
        wc.set_override("src", DataClass::Internal);
        assert!(wc.has_override("src"));
        assert!(!wc.has_override("src/file.rs"));
    }

    #[test]
    fn backslash_normalization() {
        let mut wc = WorkspaceClassification::new(DataClass::Public);
        wc.set_override("src\\auth", DataClass::Confidential);
        assert_eq!(wc.resolve("src/auth/handler.rs"), DataClass::Confidential);
    }
}
