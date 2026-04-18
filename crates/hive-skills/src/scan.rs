//! Shared directory-scanning helpers for skill discovery.
//!
//! These functions are used by both `GitHubRepoSource` and `LocalDirSource`
//! to find SKILL.md files and collect skill content from a directory tree.

use crate::parser::{self, ParsedSkill};
use hive_contracts::DiscoveredSkill;
use std::path::Path;

const MAX_SCAN_DEPTH: u32 = 6;
const SKIP_DIRS: &[&str] = &[".git", "node_modules", "__pycache__", ".venv", "target"];

/// Recursively scan a directory tree for SKILL.md files and return discovered skills.
///
/// `base` is the root of the source directory (used to compute relative `source_path`).
/// `dir` is the current directory being scanned.
/// If a directory contains a SKILL.md, it is treated as a skill root and not recursed further.
pub async fn scan_directory(
    base: &Path,
    dir: &Path,
    source_id: &str,
    results: &mut Vec<DiscoveredSkill>,
    depth: u32,
) {
    if depth > MAX_SCAN_DEPTH {
        return;
    }

    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(e) => e,
        Err(_) => return,
    };

    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        let file_name = entry.file_name();
        let name_str = file_name.to_string_lossy();

        if !path.is_dir() {
            continue;
        }
        if SKIP_DIRS.contains(&name_str.as_ref()) || name_str.starts_with('.') {
            continue;
        }

        let skill_md_path = path.join("SKILL.md");
        if skill_md_path.exists() {
            match tokio::fs::read_to_string(&skill_md_path).await {
                Ok(content) => match parser::parse_skill_md(&content) {
                    Ok(ParsedSkill { manifest, .. }) => {
                        let source_path =
                            path.strip_prefix(base).unwrap_or(&path).to_string_lossy().to_string();
                        results.push(DiscoveredSkill {
                            manifest,
                            source_id: source_id.to_string(),
                            source_path,
                            installed: false,
                        });
                    }
                    Err(e) => {
                        tracing::warn!("Skipping {}: {}", skill_md_path.display(), e);
                    }
                },
                Err(e) => {
                    tracing::warn!("Failed to read {}: {}", skill_md_path.display(), e);
                }
            }
        } else {
            // Recurse into subdirectories
            Box::pin(scan_directory(base, &path, source_id, results, depth + 1)).await;
        }
    }
}

/// Collect all text files in a skill directory (for audit / install).
pub async fn collect_skill_files(
    base: &Path,
    dir: &Path,
    files: &mut std::collections::BTreeMap<String, String>,
) {
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(e) => e,
        Err(_) => return,
    };

    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();

        if name.starts_with('.') {
            continue;
        }

        if path.is_file() && name != "SKILL.md" {
            // Only include text-like files (skip binaries)
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            let is_text = matches!(
                ext,
                "md" | "txt"
                    | "py"
                    | "js"
                    | "ts"
                    | "sh"
                    | "bash"
                    | "yaml"
                    | "yml"
                    | "json"
                    | "toml"
                    | "rs"
                    | "go"
                    | "rb"
                    | "pl"
                    | "r"
                    | "sql"
                    | "html"
                    | "css"
                    | "xml"
                    | "csv"
                    | "cfg"
                    | "ini"
                    | "conf"
            ) || ext.is_empty();

            if is_text {
                if let Ok(content) = tokio::fs::read_to_string(&path).await {
                    let rel =
                        path.strip_prefix(base).unwrap_or(&path).to_string_lossy().to_string();
                    files.insert(rel, content);
                }
            }
        } else if path.is_dir() && !SKIP_DIRS.contains(&name.as_str()) {
            Box::pin(collect_skill_files(base, &path, files)).await;
        }
    }
}
