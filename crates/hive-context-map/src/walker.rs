use std::path::{Path, PathBuf};

/// A file discovered during workspace traversal.
#[derive(Debug, Clone)]
pub struct FileEntry {
    /// Path relative to the workspace root, using `/` separators.
    pub relative_path: String,
    /// Absolute path on disk.
    pub absolute_path: PathBuf,
}

/// Recursively walk the workspace directory, returning files sorted
/// alphabetically by relative path.
///
/// Directories matching the ignore list (`.git`, `node_modules`, `target`,
/// etc.) are skipped entirely.
pub fn walk_workspace(workspace_path: &str) -> Vec<FileEntry> {
    let base = Path::new(workspace_path);
    if !base.is_dir() {
        return vec![];
    }

    let mut entries = Vec::new();
    walk_recursive(base, base, &mut entries);
    entries.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    entries
}

fn walk_recursive(base: &Path, current: &Path, entries: &mut Vec<FileEntry>) {
    let read_dir = match std::fs::read_dir(current) {
        Ok(rd) => rd,
        Err(_) => return,
    };

    let mut items: Vec<std::fs::DirEntry> = read_dir.filter_map(|e| e.ok()).collect();
    items.sort_by_key(|e| e.file_name());

    for item in items {
        let path = item.path();
        let rel = path.strip_prefix(base).unwrap_or(&path).to_string_lossy().replace('\\', "/");

        // Apply ignore rules
        if hive_workspace_index::watcher_should_ignore(&rel) {
            continue;
        }

        // Skip hidden files/directories (starting with `.`)
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if name.starts_with('.') {
                continue;
            }
        }

        let metadata = match item.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };

        if metadata.is_dir() {
            walk_recursive(base, &path, entries);
        } else if metadata.is_file() {
            entries.push(FileEntry { relative_path: rel, absolute_path: path });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walk_empty_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let entries = walk_workspace(tmp.path().to_str().unwrap());
        assert!(entries.is_empty());
    }

    #[test]
    fn walk_flat_dir() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("b.txt"), "b").unwrap();
        std::fs::write(tmp.path().join("a.txt"), "a").unwrap();

        let entries = walk_workspace(tmp.path().to_str().unwrap());
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].relative_path, "a.txt");
        assert_eq!(entries[1].relative_path, "b.txt");
    }

    #[test]
    fn walk_ignores_git() {
        let tmp = tempfile::tempdir().unwrap();
        let git_dir = tmp.path().join(".git");
        std::fs::create_dir(&git_dir).unwrap();
        std::fs::write(git_dir.join("config"), "data").unwrap();
        std::fs::write(tmp.path().join("file.txt"), "content").unwrap();

        let entries = walk_workspace(tmp.path().to_str().unwrap());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].relative_path, "file.txt");
    }

    #[test]
    fn walk_ignores_node_modules() {
        let tmp = tempfile::tempdir().unwrap();
        let nm = tmp.path().join("node_modules");
        std::fs::create_dir(&nm).unwrap();
        std::fs::write(nm.join("package.json"), "{}").unwrap();
        std::fs::write(tmp.path().join("index.js"), "code").unwrap();

        let entries = walk_workspace(tmp.path().to_str().unwrap());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].relative_path, "index.js");
    }

    #[test]
    fn walk_ignores_hidden_files() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(".hidden"), "secret").unwrap();
        std::fs::write(tmp.path().join("visible.txt"), "hello").unwrap();

        let entries = walk_workspace(tmp.path().to_str().unwrap());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].relative_path, "visible.txt");
    }

    #[test]
    fn walk_nested_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let sub = tmp.path().join("src").join("utils");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("helper.rs"), "fn help() {}").unwrap();
        std::fs::write(tmp.path().join("Cargo.toml"), "[package]").unwrap();

        let entries = walk_workspace(tmp.path().to_str().unwrap());
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].relative_path, "Cargo.toml");
        assert_eq!(entries[1].relative_path, "src/utils/helper.rs");
    }

    #[test]
    fn walk_nonexistent_dir() {
        let entries = walk_workspace("/nonexistent/path/12345");
        assert!(entries.is_empty());
    }
}
