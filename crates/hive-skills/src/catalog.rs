//! Runtime skill catalog — generates the prompt injection block.

use hive_contracts::prompt_sanitize::escape_prompt_tags;
use hive_contracts::InstalledSkill;
use parking_lot::Mutex;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Manages the runtime skill catalog for prompt injection and activation.
pub struct SkillCatalog {
    /// Enabled installed skills.
    skills: Vec<InstalledSkill>,
    /// Skills activated in the current session (for dedup).
    activated: Mutex<HashSet<String>>,
}

impl SkillCatalog {
    pub fn new(skills: Vec<InstalledSkill>) -> Self {
        Self { skills, activated: Mutex::new(HashSet::new()) }
    }

    /// Return a new catalog with the named skills removed.
    pub fn exclude(&self, excluded_names: &[String]) -> Self {
        let skills = self
            .skills
            .iter()
            .filter(|s| !excluded_names.contains(&s.manifest.name))
            .cloned()
            .collect();
        Self { skills, activated: Mutex::new(HashSet::new()) }
    }

    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }

    /// Generate the Tier 1 catalog block for system prompt injection.
    /// This includes just name + description for each enabled skill (~50-100 tokens each).
    pub fn catalog_prompt(&self) -> String {
        if self.skills.is_empty() {
            return String::new();
        }

        let mut lines = Vec::new();
        lines.push("<available_skills>".to_string());
        for skill in &self.skills {
            lines.push("  <skill>".to_string());
            lines.push(format!("    <name>{}</name>", escape_prompt_tags(&skill.manifest.name)));
            lines.push(format!(
                "    <description>{}</description>",
                escape_prompt_tags(&skill.manifest.description)
            ));
            lines.push("  </skill>".to_string());
        }
        lines.push("</available_skills>".to_string());

        format!(
            "The following skills provide specialized instructions for specific tasks.\n\
             When a task matches a skill's description, call the core.activate_skill tool \
             with the skill's name to load its full instructions before proceeding.\n\n\
             {}\n",
            lines.join("\n")
        )
    }

    /// Activate a skill: returns the full SKILL.md body + resource listing.
    /// Returns None if the skill doesn't exist.
    /// Returns a dedup notice if already activated.
    pub fn activate(&self, name: &str) -> Option<ActivationResult> {
        let skill = self.skills.iter().find(|s| s.manifest.name == name)?;

        let mut activated = self.activated.lock();
        if activated.contains(name) {
            return Some(ActivationResult {
                content: format!(
                    "Skill '{name}' is already active in this session. \
                     Its instructions are already in your context."
                ),
                already_active: true,
                source_dir: None,
            });
        }
        activated.insert(name.to_string());

        // Read the SKILL.md body
        let skill_dir = Path::new(&skill.local_path);
        if !skill_dir.is_absolute() {
            return Some(ActivationResult {
                content: format!("Failed to load skill '{name}': invalid local path"),
                already_active: false,
                source_dir: None,
            });
        }
        let canonical_dir = match std::fs::canonicalize(skill_dir) {
            Ok(path) => path,
            Err(e) => {
                return Some(ActivationResult {
                    content: format!("Failed to load skill '{name}': {e}"),
                    already_active: false,
                    source_dir: None,
                });
            }
        };
        let skill_md_path = canonical_dir.join("SKILL.md");
        let skill_md_canon = match std::fs::canonicalize(&skill_md_path) {
            Ok(path) => path,
            Err(e) => {
                return Some(ActivationResult {
                    content: format!("Failed to load skill '{name}': {e}"),
                    already_active: false,
                    source_dir: None,
                });
            }
        };
        if !skill_md_canon.starts_with(&canonical_dir) {
            return Some(ActivationResult {
                content: format!("Failed to load skill '{name}': invalid SKILL.md path"),
                already_active: false,
                source_dir: None,
            });
        }
        let body = match std::fs::read_to_string(&skill_md_canon) {
            Ok(content) => {
                // Strip frontmatter, return body only
                match crate::parser::parse_skill_md(&content) {
                    Ok(parsed) => parsed.body,
                    Err(_) => content,
                }
            }
            Err(e) => {
                return Some(ActivationResult {
                    content: format!("Failed to load skill '{name}': {e}"),
                    already_active: false,
                    source_dir: None,
                });
            }
        };

        // List bundled resources
        let resources = list_skill_resources(&canonical_dir);

        let safe_body = escape_prompt_tags(&body);
        let safe_name = escape_prompt_tags(name);
        let mut output = format!("<skill_content name=\"{safe_name}\">\n{safe_body}\n");
        if !resources.is_empty() {
            output.push_str(
                "\nSkill directory: <skill_dir>\n\
                 Relative paths in this skill are relative to the skill directory.\n\n\
                 <skill_resources>\n",
            );
            for res in &resources {
                output.push_str(&format!("  <file>{res}</file>\n"));
            }
            output.push_str("</skill_resources>\n");
        }
        output.push_str("</skill_content>");

        Some(ActivationResult {
            content: output,
            already_active: false,
            source_dir: Some(canonical_dir),
        })
    }

    /// Reset activation tracking (e.g. for a new session).
    pub fn reset_activated(&self) {
        self.activated.lock().clear();
    }
}

pub struct ActivationResult {
    pub content: String,
    pub already_active: bool,
    /// The canonical source directory of the skill on disk.
    /// The handler can use this to stage resources into the workspace.
    pub source_dir: Option<PathBuf>,
}

/// Returns an error if `name` cannot be used as a single workspace folder
/// under `.skills/`.
pub fn validate_skill_staging_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("skill name must not be empty".to_string());
    }
    if name.len() > 64 {
        return Err("skill name is too long".to_string());
    }
    if name.contains("..")
        || name.contains('/')
        || name.contains('\\')
        || name.starts_with('.')
        || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!("invalid skill name '{name}'"));
    }
    Ok(())
}

/// Rewrite skill body paths after staging into the session workspace.
///
/// Replaces the `<skill_dir>` token and on-disk source paths (raw, canonical,
/// and macOS `/private` variants) with the workspace-relative staging path.
pub fn rewrite_staged_skill_paths(content: &str, source_dir: &Path, relative_dir: &str) -> String {
    let mut out = content.replace("<skill_dir>", relative_dir);
    for abs in skill_source_path_strings(source_dir) {
        out = out.replace(&abs, relative_dir);
    }
    out
}

fn skill_source_path_strings(source_dir: &Path) -> Vec<String> {
    let mut paths = Vec::new();
    let mut push = |p: &Path| {
        let s = p.to_string_lossy().into_owned();
        if !s.is_empty() && !paths.contains(&s) {
            paths.push(s);
        }
    };
    push(source_dir);
    if let Ok(canon) = source_dir.canonicalize() {
        push(&canon);
    }
    let extras: Vec<String> = paths
        .iter()
        .flat_map(|existing| {
            let mut more = Vec::new();
            if let Some(stripped) = existing.strip_prefix("/private") {
                more.push(stripped.to_string());
            } else if existing.starts_with("/var/") || existing.starts_with("/tmp/") {
                more.push(format!("/private{existing}"));
            }
            more
        })
        .collect();
    for s in extras {
        if !s.is_empty() && !paths.contains(&s) {
            paths.push(s);
        }
    }
    paths.sort_by(|a, b| b.len().cmp(&a.len()));
    paths
}

/// Stage skill resource files into a target directory (typically inside the
/// session workspace). Copies all non-hidden, non-SKILL.md files from
/// `source_dir` into `target_dir`, preserving directory structure.
///
/// Symlinks are skipped for safety. If `target_dir` already exists it is
/// removed first to avoid stale files.
///
/// Returns the list of staged relative paths on success.
pub fn stage_skill_resources(source_dir: &Path, target_dir: &Path) -> std::io::Result<Vec<String>> {
    let canonical_source = source_dir.canonicalize()?;

    // Clean slate — remove any previous staging for this skill.
    if target_dir.exists() {
        std::fs::remove_dir_all(target_dir)?;
    }
    std::fs::create_dir_all(target_dir)?;

    let mut staged = Vec::new();
    copy_dir_recursive(&canonical_source, target_dir, &canonical_source, &mut staged)?;
    staged.sort();
    Ok(staged)
}

fn copy_dir_recursive(
    current: &Path,
    target_base: &Path,
    source_root: &Path,
    staged: &mut Vec<String>,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(current)? {
        let entry = entry?;
        let file_type = entry.file_type()?;

        // Skip symlinks entirely.
        if file_type.is_symlink() {
            continue;
        }

        let name = entry.file_name().to_string_lossy().to_string();

        // Skip hidden files and SKILL.md.
        if name.starts_with('.') || name == "SKILL.md" {
            continue;
        }

        // Verify the entry is still under the source root (symlink safety belt).
        if let Ok(canon) = entry.path().canonicalize() {
            if !canon.starts_with(source_root) {
                continue;
            }
        }

        let entry_path = entry.path();
        let rel = entry_path.strip_prefix(source_root).unwrap_or(&entry_path).to_path_buf();
        let dest = target_base.join(&rel);

        if file_type.is_dir() {
            std::fs::create_dir_all(&dest)?;
            copy_dir_recursive(&entry.path(), target_base, source_root, staged)?;
        } else if file_type.is_file() {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(entry.path(), &dest)?;
            staged.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
    Ok(())
}

/// List files in a skill directory (non-recursive, first level only).
fn list_skill_resources(dir: &Path) -> Vec<String> {
    let mut files = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return files,
    };

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name == "SKILL.md" || name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        if path.is_file() {
            files.push(name);
        } else if path.is_dir() {
            // List contents of immediate subdirectories
            if let Ok(sub_entries) = std::fs::read_dir(&path) {
                for sub in sub_entries.flatten() {
                    let sub_name = sub.file_name().to_string_lossy().to_string();
                    if !sub_name.starts_with('.') && sub.path().is_file() {
                        files.push(format!("{name}/{sub_name}"));
                    }
                }
            }
        }
    }
    files.sort();
    files
}

#[cfg(test)]
mod tests {
    use super::*;
    use hive_contracts::{SkillAuditResult, SkillManifest};

    fn test_skill(name: &str) -> InstalledSkill {
        InstalledSkill {
            manifest: SkillManifest {
                name: name.to_string(),
                description: format!("Does {name} things."),
                license: None,
                compatibility: None,
                metadata: Default::default(),
                allowed_tools: None,
            },
            local_path: format!("/tmp/skills/{name}"),
            source_id: "github:test/repo".to_string(),
            source_path: format!("skills/{name}"),
            persona_id: String::new(),
            audit: SkillAuditResult {
                model_used: "test".to_string(),
                risks: vec![],
                summary: "Clean.".to_string(),
                audited_at_ms: 1000,
            },
            enabled: true,
            installed_at_ms: 1000,
            content_hash: String::new(),
            pinned_commit: String::new(),
        }
    }

    #[test]
    fn catalog_prompt_includes_all_skills() {
        let catalog = SkillCatalog::new(vec![test_skill("alpha"), test_skill("beta")]);
        let prompt = catalog.catalog_prompt();
        assert!(prompt.contains("<name>alpha</name>"));
        assert!(prompt.contains("<name>beta</name>"));
        assert!(prompt.contains("core.activate_skill"));
    }

    #[test]
    fn empty_catalog_returns_empty() {
        let catalog = SkillCatalog::new(vec![]);
        assert!(catalog.catalog_prompt().is_empty());
    }

    #[test]
    fn activation_deduplicates() {
        let catalog = SkillCatalog::new(vec![test_skill("test")]);
        // First activation should work (even if path doesn't exist, it returns an error message)
        let result1 = catalog.activate("test").unwrap();
        assert!(!result1.already_active);

        // Second activation should be deduped
        let result2 = catalog.activate("test").unwrap();
        assert!(result2.already_active);
    }

    #[test]
    fn activation_nonexistent_returns_none() {
        let catalog = SkillCatalog::new(vec![]);
        assert!(catalog.activate("nonexistent").is_none());
    }

    #[test]
    fn stage_skill_resources_copies_files() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("skill-src");
        std::fs::create_dir_all(source.join("scripts")).unwrap();
        std::fs::write(source.join("SKILL.md"), "# Skill").unwrap();
        std::fs::write(source.join(".hidden"), "secret").unwrap();
        std::fs::write(source.join("requirements.txt"), "cadquery").unwrap();
        std::fs::write(source.join("scripts/render.py"), "print('hi')").unwrap();

        let target = tmp.path().join("workspace/.skills/test-skill");
        let staged = stage_skill_resources(&source, &target).unwrap();

        assert!(staged.contains(&"requirements.txt".to_string()));
        assert!(staged.contains(&"scripts/render.py".to_string()));
        // SKILL.md and hidden files should not be copied
        assert!(!staged.iter().any(|s| s.contains("SKILL.md")));
        assert!(!staged.iter().any(|s| s.contains(".hidden")));
        // Files should exist at target
        assert!(target.join("requirements.txt").exists());
        assert!(target.join("scripts/render.py").exists());
    }

    #[test]
    fn rewrite_staged_skill_paths_replaces_token_and_absolute_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("cadquery-modeling");
        std::fs::create_dir_all(&source).unwrap();
        let canon = source.canonicalize().unwrap();
        let content = format!(
            "python \"<skill_dir>/scripts/render_model.py\"\nSkill directory: {}\nCanon: {}",
            source.display(),
            canon.display()
        );
        let rewritten = rewrite_staged_skill_paths(&content, &source, ".skills/cadquery-modeling");
        assert!(
            rewritten.contains("python \".skills/cadquery-modeling/scripts/render_model.py\""),
            "token must be rewritten: {rewritten}"
        );
        assert!(
            rewritten.contains("Skill directory: .skills/cadquery-modeling"),
            "absolute source path must be rewritten: {rewritten}"
        );
        assert!(
            rewritten.contains("Canon: .skills/cadquery-modeling"),
            "canonical source path must be rewritten: {rewritten}"
        );
        assert!(!rewritten.contains("<skill_dir>"), "placeholder must not remain");
        assert!(
            !rewritten.contains(&source.display().to_string()),
            "raw install path must not remain: {rewritten}"
        );
        assert!(
            !rewritten.contains(&canon.display().to_string()),
            "canonical install path must not remain: {rewritten}"
        );
    }

    #[test]
    fn rewrite_staged_skill_paths_rewrites_macos_private_prefix() {
        let source = Path::new("/var/folders/xx/cadquery-modeling");
        let content = "Skill directory: /var/folders/xx/cadquery-modeling\n\
             Also: /private/var/folders/xx/cadquery-modeling";
        let rewritten = rewrite_staged_skill_paths(content, source, ".skills/cadquery-modeling");
        assert!(
            !rewritten.contains("/var/folders/xx/cadquery-modeling"),
            "raw /var path must be rewritten: {rewritten}"
        );
        assert!(
            !rewritten.contains("/private/var/folders"),
            "canonical /private path must be rewritten: {rewritten}"
        );
        assert!(rewritten.contains("Skill directory: .skills/cadquery-modeling"));
    }

    #[test]
    fn validate_skill_staging_name_rejects_path_escape() {
        assert!(validate_skill_staging_name("cadquery-modeling").is_ok());
        assert!(validate_skill_staging_name("../etc").is_err());
        assert!(validate_skill_staging_name("foo/bar").is_err());
        assert!(validate_skill_staging_name("foo\\bar").is_err());
        assert!(validate_skill_staging_name("").is_err());
        assert!(validate_skill_staging_name(".hidden").is_err());
    }

    #[test]
    fn stage_skill_resources_replaces_existing() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("skill-src");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("a.txt"), "version1").unwrap();

        let target = tmp.path().join("staged");
        stage_skill_resources(&source, &target).unwrap();
        assert!(target.join("a.txt").exists());

        // Remove a.txt from source and add b.txt
        std::fs::remove_file(source.join("a.txt")).unwrap();
        std::fs::write(source.join("b.txt"), "version2").unwrap();

        let staged = stage_skill_resources(&source, &target).unwrap();
        // Old file should be gone, new file should be present
        assert!(!target.join("a.txt").exists());
        assert!(target.join("b.txt").exists());
        assert!(staged.contains(&"b.txt".to_string()));
    }
}
