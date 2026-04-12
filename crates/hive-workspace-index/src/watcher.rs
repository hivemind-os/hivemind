use notify::event::ModifyKind;
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, warn};

/// Simplified file event for the indexer.
#[derive(Debug, Clone)]
pub struct FileEvent {
    pub path: PathBuf,
    pub kind: FileEventKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileEventKind {
    Created,
    Modified,
    Removed,
}

/// Wrapper around `notify::RecommendedWatcher` with debouncing.
pub struct WorkspaceWatcher {
    _watcher: RecommendedWatcher,
}

/// Channel capacity for file watcher events.
pub(crate) const WATCHER_CHANNEL_CAPACITY: usize = 10_000;

impl WorkspaceWatcher {
    /// Start watching `path` recursively. File events are sent to `tx`.
    pub fn start(path: &Path, tx: mpsc::Sender<FileEvent>) -> anyhow::Result<Self> {
        let mut watcher = RecommendedWatcher::new(
            move |result: Result<Event, notify::Error>| {
                let event = match result {
                    Ok(e) => e,
                    Err(e) => {
                        warn!("watcher error: {e}");
                        return;
                    }
                };

                let kind = match event.kind {
                    EventKind::Create(_) => FileEventKind::Created,
                    // Rename events: "From" is the old path (treat as remove),
                    // "To" / "Both" / "Any" is the new path (treat as create).
                    EventKind::Modify(ModifyKind::Name(rename_mode)) => {
                        use notify::event::RenameMode;
                        match rename_mode {
                            RenameMode::From => FileEventKind::Removed,
                            RenameMode::To | RenameMode::Both => FileEventKind::Created,
                            // Any/Other — we don't know direction; treat as modified
                            // and let handle_coalesced resolve via existence check
                            _ => FileEventKind::Modified,
                        }
                    }
                    EventKind::Modify(_) => FileEventKind::Modified,
                    EventKind::Remove(_) => FileEventKind::Removed,
                    _ => return, // ignore access, other
                };

                for path in event.paths {
                    // Skip ignored paths
                    if let Some(name) = path.to_str() {
                        if should_ignore(name) {
                            continue;
                        }
                    }

                    // Use try_send to avoid blocking; dropped events will be
                    // picked up on the next full scan or debounce cycle.
                    match tx.try_send(FileEvent { path, kind }) {
                        Ok(()) => {}
                        Err(mpsc::error::TrySendError::Closed(_)) => {
                            debug!("watcher event channel closed");
                            return;
                        }
                        Err(mpsc::error::TrySendError::Full(_)) => {
                            debug!("watcher event channel full, dropping event");
                        }
                    }
                }
            },
            Config::default().with_poll_interval(Duration::from_millis(500)),
        )?;

        watcher.watch(path, RecursiveMode::Recursive)?;
        debug!(path = %path.display(), "started workspace watcher");

        Ok(Self { _watcher: watcher })
    }
}

/// Returns true if a path component or relative path should be ignored.
pub fn should_ignore(path: &str) -> bool {
    // Check each path component
    for component in path.split(['/', '\\']) {
        if component.is_empty() {
            continue;
        }
        if matches!(
            component,
            ".git"
                | ".hg"
                | ".svn"
                | "node_modules"
                | "target"
                | "__pycache__"
                | ".tox"
                | ".venv"
                | "venv"
                | ".mypy_cache"
                | ".pytest_cache"
                | "dist"
                | "build"
                | ".next"
                | ".nuxt"
                | ".DS_Store"
                | "Thumbs.db"
                | "vendor"
                | ".gradle"
                | ".idea"
                | ".vscode"
                | ".cache"
                | "coverage"
                | "out"
                | ".turbo"
                | ".cargo"
        ) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_ignore_git() {
        assert!(should_ignore(".git"));
        assert!(should_ignore(".git/objects/abc"));
        assert!(should_ignore("src/.git/config"));
    }

    #[test]
    fn should_ignore_node_modules() {
        assert!(should_ignore("node_modules"));
        assert!(should_ignore("node_modules/package/index.js"));
    }

    #[test]
    fn should_ignore_target() {
        assert!(should_ignore("target/debug/main"));
    }

    #[test]
    fn should_not_ignore_normal_paths() {
        assert!(!should_ignore("src/main.rs"));
        assert!(!should_ignore("docs/README.md"));
        assert!(!should_ignore("package.json"));
    }

    #[test]
    fn should_ignore_ds_store() {
        assert!(should_ignore(".DS_Store"));
        assert!(should_ignore("src/.DS_Store"));
    }
}
