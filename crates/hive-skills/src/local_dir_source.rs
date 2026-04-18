//! Local directory skill source — discovers skills from a local folder.

use crate::parser;
use crate::scan;
use hive_contracts::{DiscoveredSkill, SkillSourceConfig};
use std::path::{Path, PathBuf};

/// A local directory skill source that scans a folder on disk.
pub struct LocalDirSource {
    path: PathBuf,
}

impl LocalDirSource {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Create from a `SkillSourceConfig::LocalDirectory` variant.
    /// Returns `None` if the config is not a local directory or is disabled.
    pub fn from_config(config: &SkillSourceConfig) -> Option<Self> {
        match config {
            SkillSourceConfig::LocalDirectory { path, enabled } if *enabled => {
                let p = PathBuf::from(path);
                if p.is_absolute() && p.is_dir() {
                    Some(Self::new(p))
                } else {
                    tracing::warn!(
                        "Local skill source path is not an absolute directory: {}",
                        path
                    );
                    None
                }
            }
            _ => None,
        }
    }

    pub fn source_id(&self) -> String {
        format!("local:{}", self.path.display())
    }

    /// Discover all skills by scanning for SKILL.md files.
    ///
    /// Supports two layouts:
    /// 1. The directory itself contains a SKILL.md (single skill).
    /// 2. The directory contains subdirectories, each with a SKILL.md.
    pub async fn discover(&self) -> Result<Vec<DiscoveredSkill>, LocalSourceError> {
        if !self.path.is_dir() {
            return Err(LocalSourceError::IoFailed(format!(
                "path does not exist or is not a directory: {}",
                self.path.display()
            )));
        }

        let mut skills = Vec::new();
        let source_id = self.source_id();

        // Check if the root directory itself is a skill
        let root_skill_md = self.path.join("SKILL.md");
        if root_skill_md.exists() {
            if let Ok(content) = tokio::fs::read_to_string(&root_skill_md).await {
                if let Ok(parsed) = parser::parse_skill_md(&content) {
                    skills.push(DiscoveredSkill {
                        manifest: parsed.manifest,
                        source_id: source_id.clone(),
                        source_path: ".".to_string(),
                        installed: false,
                    });
                }
            }
        }

        // Scan subdirectories for additional skills
        scan::scan_directory(&self.path, &self.path, &source_id, &mut skills, 0).await;
        Ok(skills)
    }

    /// Fetch the full content of a specific skill by its source path.
    pub async fn fetch_skill_content(
        &self,
        source_path: &str,
    ) -> Result<hive_contracts::SkillContent, LocalSourceError> {
        let skill_dir = if source_path == "." {
            self.path.clone()
        } else {
            self.path.join(source_path)
        };

        // Containment check: ensure the resolved path stays under the source root
        let canonical_root = self
            .path
            .canonicalize()
            .map_err(|e| LocalSourceError::IoFailed(format!("failed to canonicalize root: {e}")))?;
        let canonical_skill = skill_dir
            .canonicalize()
            .map_err(|e| LocalSourceError::IoFailed(format!("failed to canonicalize skill dir: {e}")))?;
        if !canonical_skill.starts_with(&canonical_root) {
            return Err(LocalSourceError::IoFailed(format!(
                "skill path escapes source root: {}",
                source_path
            )));
        }

        let skill_md_path = skill_dir.join("SKILL.md");
        let skill_md = tokio::fs::read_to_string(&skill_md_path)
            .await
            .map_err(|e| LocalSourceError::IoFailed(format!("failed to read SKILL.md: {e}")))?;

        let parsed = parser::parse_skill_md(&skill_md)
            .map_err(|e| LocalSourceError::ParseFailed(e.to_string()))?;

        let mut files = std::collections::BTreeMap::new();
        scan::collect_skill_files(&skill_dir, &skill_dir, &mut files).await;

        Ok(hive_contracts::SkillContent { skill_md, body: parsed.body, files })
    }

    /// Local sources have no git commit to pin.
    pub fn head_commit_sha(&self) -> Option<String> {
        None
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LocalSourceError {
    #[error("I/O error: {0}")]
    IoFailed(String),
    #[error("parse error: {0}")]
    ParseFailed(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_skill_md(dir: &Path, name: &str, description: &str) {
        let skill_md = format!(
            "---\nname: {name}\ndescription: {description}\n---\nSkill body for {name}.\n"
        );
        fs::write(dir.join("SKILL.md"), skill_md).unwrap();
    }

    #[tokio::test]
    async fn discover_single_skill_at_root() {
        let tmp = TempDir::new().unwrap();
        write_skill_md(tmp.path(), "root-skill", "A skill at the root level");

        let source = LocalDirSource::new(tmp.path().to_path_buf());
        let skills = source.discover().await.unwrap();

        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].manifest.name, "root-skill");
        assert_eq!(skills[0].source_path, ".");
    }

    #[tokio::test]
    async fn discover_nested_skills() {
        let tmp = TempDir::new().unwrap();
        let skill_a = tmp.path().join("skill-a");
        let skill_b = tmp.path().join("subdir").join("skill-b");
        fs::create_dir_all(&skill_a).unwrap();
        fs::create_dir_all(&skill_b).unwrap();

        write_skill_md(&skill_a, "skill-a", "First skill");
        write_skill_md(&skill_b, "skill-b", "Second skill");

        let source = LocalDirSource::new(tmp.path().to_path_buf());
        let skills = source.discover().await.unwrap();

        let names: Vec<&str> = skills.iter().map(|s| s.manifest.name.as_str()).collect();
        assert!(names.contains(&"skill-a"));
        assert!(names.contains(&"skill-b"));
        assert_eq!(skills.len(), 2);
    }

    #[tokio::test]
    async fn fetch_skill_content_root() {
        let tmp = TempDir::new().unwrap();
        write_skill_md(tmp.path(), "my-skill", "Test skill");
        fs::write(tmp.path().join("helper.py"), "print('hello')").unwrap();

        let source = LocalDirSource::new(tmp.path().to_path_buf());
        let content = source.fetch_skill_content(".").await.unwrap();

        assert!(content.skill_md.contains("my-skill"));
        assert!(content.files.contains_key("helper.py"));
        assert_eq!(content.files["helper.py"], "print('hello')");
    }

    #[tokio::test]
    async fn fetch_skill_content_nested() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join("skills").join("my-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        write_skill_md(&skill_dir, "my-skill", "Nested skill");
        fs::write(skill_dir.join("run.sh"), "#!/bin/bash\necho hi").unwrap();

        let source = LocalDirSource::new(tmp.path().to_path_buf());
        let content = source.fetch_skill_content("skills/my-skill").await.unwrap();

        assert!(content.skill_md.contains("my-skill"));
        assert!(content.files.contains_key("run.sh"));
    }

    #[tokio::test]
    async fn fetch_rejects_path_traversal() {
        let tmp = TempDir::new().unwrap();
        write_skill_md(tmp.path(), "legit", "legit skill");

        let source = LocalDirSource::new(tmp.path().to_path_buf());
        let result = source.fetch_skill_content("../../../etc").await;

        assert!(result.is_err());
    }

    #[test]
    fn head_commit_sha_is_none() {
        let tmp = TempDir::new().unwrap();
        let source = LocalDirSource::new(tmp.path().to_path_buf());
        assert_eq!(source.head_commit_sha(), None);
    }

    #[test]
    fn source_id_format() {
        let source = LocalDirSource::new(PathBuf::from("/home/user/skills"));
        assert_eq!(source.source_id(), "local:/home/user/skills");
    }

    #[test]
    fn from_config_disabled() {
        let config = SkillSourceConfig::LocalDirectory {
            path: "/tmp".to_string(),
            enabled: false,
        };
        assert!(LocalDirSource::from_config(&config).is_none());
    }

    #[test]
    fn from_config_github_returns_none() {
        let config = SkillSourceConfig::GitHub {
            owner: "test".to_string(),
            repo: "repo".to_string(),
            enabled: true,
        };
        assert!(LocalDirSource::from_config(&config).is_none());
    }
}
