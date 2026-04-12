mod chunker;
mod extract;
mod watcher;

pub use chunker::{chunk_text, TextChunk};
pub use extract::{
    extract_text, is_binary_file, is_text_extension, is_text_filename, mime_for_extension,
};
pub use watcher::{
    should_ignore as watcher_should_ignore, FileEvent, FileEventKind, WorkspaceWatcher,
};

use glob::Pattern;
use hive_classification::DataClass;
use hive_contracts::WorkspaceClassification;
use hive_knowledge::{KgPool, KnowledgeGraph, NewNode};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex, Semaphore};
use tracing::{debug, info, warn};

/// Normalize a path string to use forward slashes for cross-platform consistency.
fn normalize_path(s: &str) -> String {
    s.replace('\\', "/")
}

/// Resolves a file path to an embedding model ID using ordered glob rules.
/// First matching rule wins. Falls back to the default model.
pub struct EmbeddingModelResolver {
    rules: Vec<(Pattern, String)>,
    default_model: String,
}

impl EmbeddingModelResolver {
    /// Create from (glob_pattern, model_id) pairs and a default model.
    pub fn new(rules: Vec<(String, String)>, default_model: String) -> Self {
        let compiled = rules
            .into_iter()
            .filter_map(|(glob, model)| {
                Pattern::new(&glob)
                    .map(|p| (p, model))
                    .map_err(|e| warn!(glob = %glob, "invalid embedding glob pattern: {e}"))
                    .ok()
            })
            .collect();
        Self { rules: compiled, default_model }
    }

    /// Create a resolver that always returns the default model.
    pub fn default_only(model_id: String) -> Self {
        Self { rules: Vec::new(), default_model: model_id }
    }

    /// Resolve a relative file path to its embedding model ID.
    pub fn resolve(&self, rel_path: &str) -> &str {
        for (pattern, model_id) in &self.rules {
            if pattern.matches(rel_path) {
                return model_id;
            }
        }
        &self.default_model
    }
}

/// Callback trait for generating embeddings after chunk nodes are created.
/// Implemented by the API layer to bridge to the inference runtime.
pub trait EmbeddingCallback: Send + Sync {
    fn embed(&self, node_id: i64, text: String, model_id: String);
}

/// Index status for a single file, emitted as SSE events.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum FileIndexStatus {
    /// File is waiting in the debounce/work queue.
    Queued { path: String },
    /// File has been indexed into the knowledge graph.
    Indexed { path: String },
    /// File has been removed from the index.
    Removed { path: String },
}

/// Cached metadata + content hash for a tracked file.
#[derive(Clone)]
struct FileState {
    content_hash: String,
    mtime_secs: i64,
    size: u64,
}

/// Per-session indexing state.
struct SessionState {
    session_node_id: i64,
    workspace_path: PathBuf,
    kg_path: PathBuf,
    /// Reusable connection pool for the knowledge graph.
    kg_pool: Arc<KgPool>,
    classification: WorkspaceClassification,
    /// Maps relative path → file state (content hash + mtime/size).
    file_state: HashMap<String, FileState>,
    /// Tracks consecutive indexing failures per file path. Files exceeding
    /// `MAX_INDEX_RETRIES` are skipped until manually reindexed or the next
    /// reconciliation cycle.
    failure_counts: HashMap<String, u8>,
    _watcher: WorkspaceWatcher,
    /// Broadcast sender for index status events.
    status_tx: broadcast::Sender<FileIndexStatus>,
}

/// Manages per-session file watchers and indexes workspace files into the
/// knowledge graph.  Events are debounced (coalesced within a configurable
/// window) and processed by a bounded worker pool.
pub struct WorkspaceIndexer {
    sessions: Mutex<HashMap<String, SessionState>>,
    embed_callback: Arc<dyn EmbeddingCallback>,
    embed_resolver: Arc<EmbeddingModelResolver>,
    /// Limits concurrent `spawn_blocking` indexing tasks.
    index_semaphore: Arc<Semaphore>,
    /// Debounce window in milliseconds.
    debounce_ms: u64,
    /// Interval between periodic reconciliation scans (seconds).
    reconciliation_interval_secs: u64,
}

const CHUNK_SIZE: usize = 2000;
const OVERLAP_PCT: f64 = 0.10;
const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024; // 10 MB
const DEFAULT_MAX_CONCURRENT: usize = 4;
const DEFAULT_DEBOUNCE_MS: u64 = 300;
/// Maximum consecutive indexing failures before a file is skipped.
const MAX_INDEX_RETRIES: u8 = 3;
/// Interval between periodic reconciliation scans (seconds).
const DEFAULT_RECONCILIATION_INTERVAL_SECS: u64 = 15 * 60;

impl WorkspaceIndexer {
    pub fn new(embed_callback: Arc<dyn EmbeddingCallback>) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            embed_callback,
            embed_resolver: Arc::new(EmbeddingModelResolver::default_only(
                hive_inference::defaults::DEFAULT_EMBEDDING_MODEL_ID.to_string(),
            )),
            index_semaphore: Arc::new(Semaphore::new(DEFAULT_MAX_CONCURRENT)),
            debounce_ms: DEFAULT_DEBOUNCE_MS,
            reconciliation_interval_secs: DEFAULT_RECONCILIATION_INTERVAL_SECS,
        }
    }

    /// Create an indexer with custom concurrency and debounce settings.
    pub fn with_config(
        embed_callback: Arc<dyn EmbeddingCallback>,
        max_concurrent: usize,
        debounce_ms: u64,
        reconciliation_interval_secs: u64,
    ) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            embed_callback,
            embed_resolver: Arc::new(EmbeddingModelResolver::default_only(
                hive_inference::defaults::DEFAULT_EMBEDDING_MODEL_ID.to_string(),
            )),
            index_semaphore: Arc::new(Semaphore::new(max_concurrent.max(1))),
            debounce_ms,
            reconciliation_interval_secs,
        }
    }

    /// Set the embedding model resolver (glob rules → model ID).
    pub fn set_resolver(&mut self, resolver: EmbeddingModelResolver) {
        self.embed_resolver = Arc::new(resolver);
    }

    /// Start watching a session's workspace. Performs an initial full scan.
    pub async fn start(
        self: &Arc<Self>,
        session_id: String,
        session_node_id: i64,
        workspace_path: PathBuf,
        kg_path: PathBuf,
        classification: WorkspaceClassification,
    ) -> anyhow::Result<()> {
        let (event_rx, session_id) = self
            .start_common(session_id, session_node_id, workspace_path, kg_path, classification)
            .await?;

        let Some(mut event_rx) = event_rx else {
            return Ok(());
        };

        // Perform the initial full scan synchronously (within the async
        // context) so that callers can rely on files being indexed by the
        // time `start()` returns.
        self.full_scan(&session_id).await;
        info!(session_id, "workspace indexer initial scan complete");

        // Spawn background task for the debounced event loop.
        let this = Arc::clone(self);
        let sid = session_id.clone();
        let debounce_ms = self.debounce_ms;
        let reconciliation_interval_secs = self.reconciliation_interval_secs;
        tokio::spawn(async move {
            this.debounced_event_loop(
                &sid,
                &mut event_rx,
                debounce_ms,
                reconciliation_interval_secs,
            )
            .await;
            debug!(session_id = %sid, "workspace watcher event loop exited");
        });

        info!(session_id, "workspace indexer started");
        Ok(())
    }

    /// Like [`start`](Self::start), but spawns the initial full scan in the
    /// background instead of blocking the caller. The file-system watcher is
    /// active immediately so changes are buffered and processed once the scan
    /// completes. Use this during daemon restore when the caller does not need
    /// the index to be fully populated before proceeding.
    pub async fn start_deferred(
        self: &Arc<Self>,
        session_id: String,
        session_node_id: i64,
        workspace_path: PathBuf,
        kg_path: PathBuf,
        classification: WorkspaceClassification,
    ) -> anyhow::Result<()> {
        let (event_rx, session_id) = self
            .start_common(session_id, session_node_id, workspace_path, kg_path, classification)
            .await?;

        let Some(mut event_rx) = event_rx else {
            return Ok(());
        };

        // Spawn a single background task that performs the initial full scan
        // and then enters the debounced event loop.
        let this = Arc::clone(self);
        let sid = session_id.clone();
        let debounce_ms = self.debounce_ms;
        let reconciliation_interval_secs = self.reconciliation_interval_secs;
        tokio::spawn(async move {
            this.full_scan(&sid).await;
            info!(session_id = %sid, "workspace indexer deferred scan complete");
            this.debounced_event_loop(
                &sid,
                &mut event_rx,
                debounce_ms,
                reconciliation_interval_secs,
            )
            .await;
            debug!(session_id = %sid, "workspace watcher event loop exited");
        });

        info!(session_id, "workspace indexer started (deferred scan)");
        Ok(())
    }

    /// Shared setup for [`start`] and [`start_deferred`]. Returns the event
    /// receiver channel and the (possibly unchanged) session id. Returns
    /// `Ok((None, _))` when the workspace path does not exist and the caller
    /// should return early.
    async fn start_common(
        self: &Arc<Self>,
        session_id: String,
        session_node_id: i64,
        workspace_path: PathBuf,
        kg_path: PathBuf,
        classification: WorkspaceClassification,
    ) -> anyhow::Result<(Option<tokio::sync::mpsc::Receiver<crate::watcher::FileEvent>>, String)>
    {
        if !workspace_path.exists() {
            debug!(session_id, "workspace path does not exist yet, skipping indexer start");
            return Ok((None, session_id));
        }

        // Canonicalize so that FSEvents paths (which use real paths like
        // /private/var/…) match the stored workspace_path for strip_prefix.
        let workspace_path = workspace_path.canonicalize().unwrap_or(workspace_path);

        let (event_tx, event_rx) =
            tokio::sync::mpsc::channel(crate::watcher::WATCHER_CHANNEL_CAPACITY);
        let watcher = WorkspaceWatcher::start(&workspace_path, event_tx)?;
        let (status_tx, _) = broadcast::channel::<FileIndexStatus>(4096);

        let state = SessionState {
            session_node_id,
            workspace_path: workspace_path.clone(),
            kg_path: kg_path.clone(),
            kg_pool: Arc::new(KgPool::new(&kg_path)),
            classification: classification.clone(),
            file_state: HashMap::new(),
            failure_counts: HashMap::new(),
            _watcher: watcher,
            status_tx,
        };

        {
            let mut sessions = self.sessions.lock().await;
            sessions.insert(session_id.clone(), state);
        }

        Ok((Some(event_rx), session_id))
    }

    /// Debounced event loop: collects events into a buffer and flushes them
    /// after `debounce_ms` of silence, coalescing duplicate paths.
    /// Also runs a periodic reconciliation scan to recover missed events.
    async fn debounced_event_loop(
        self: &Arc<Self>,
        session_id: &str,
        rx: &mut tokio::sync::mpsc::Receiver<FileEvent>,
        debounce_ms: u64,
        reconciliation_interval_secs: u64,
    ) {
        let mut pending: HashMap<String, FileEventKind> = HashMap::new();
        let debounce = tokio::time::Duration::from_millis(debounce_ms);
        let mut reconcile_interval =
            tokio::time::interval(tokio::time::Duration::from_secs(reconciliation_interval_secs));
        // First tick fires immediately; skip it since we just did a full_scan.
        reconcile_interval.tick().await;

        loop {
            // If the buffer is empty, wait for the first event or reconciliation.
            if pending.is_empty() {
                tokio::select! {
                    event = rx.recv() => {
                        match event {
                            Some(event) => {
                                if let Some(key) = self.event_key(session_id, &event) {
                                    coalesce_event(&mut pending, key, event.kind);
                                }
                            }
                            None => break, // channel closed
                        }
                    }
                    _ = reconcile_interval.tick() => {
                        self.reconciliation_scan(session_id).await;
                        continue;
                    }
                }
            }

            // Drain any additional events that arrive within the debounce window.
            loop {
                tokio::select! {
                    result = tokio::time::timeout(debounce, rx.recv()) => {
                        match result {
                            Ok(Some(event)) => {
                                if let Some(key) = self.event_key(session_id, &event) {
                                    coalesce_event(&mut pending, key, event.kind);
                                }
                            }
                            Ok(None) => return, // channel closed
                            Err(_) => break,    // timeout — debounce window elapsed
                        }
                    }
                    _ = reconcile_interval.tick() => {
                        // Reconciliation fires but we have pending events — flush first, reconcile later.
                        break;
                    }
                }
            }

            // Flush the coalesced batch.
            // Sort: Creates first, then Modified, then Removed.
            // This ordering allows rename detection via content hash —
            // Created files can find matching hashes before Removed files
            // clear them from the session state.
            let mut batch: Vec<(String, FileEventKind)> = pending.drain().collect();
            batch.sort_by_key(|(_, kind)| match kind {
                FileEventKind::Created => 0,
                FileEventKind::Modified => 1,
                FileEventKind::Removed => 2,
            });

            // Emit Queued status for all files about to be processed
            {
                let sessions = self.sessions.lock().await;
                if let Some(state) = sessions.get(session_id) {
                    for (abs_path, _) in &batch {
                        let abs = PathBuf::from(abs_path);
                        if let Ok(rel) = abs.strip_prefix(&state.workspace_path) {
                            let rel_str = normalize_path(&rel.to_string_lossy());
                            let _ = state.status_tx.send(FileIndexStatus::Queued { path: rel_str });
                        }
                    }
                }
            }

            for (rel_path, kind) in batch {
                self.handle_coalesced(session_id, &rel_path, kind).await;
            }
        }
    }

    /// Periodic reconciliation: re-walk the workspace filesystem and compare
    /// against the in-memory file_state to find any files that drifted
    /// (missed watcher events). Also resets failure counters to give files a
    /// fresh retry budget.
    async fn reconciliation_scan(self: &Arc<Self>, session_id: &str) {
        debug!(session_id, "starting periodic reconciliation scan");

        let (workspace_path, kg_path, session_node_id, classification, kg_pool, known_files) = {
            let mut sessions = self.sessions.lock().await;
            let state = match sessions.get_mut(session_id) {
                Some(s) => s,
                None => return,
            };
            // Reset failure counts so files get a fresh retry budget
            state.failure_counts.clear();
            let known: HashSet<String> = state.file_state.keys().cloned().collect();
            (
                state.workspace_path.clone(),
                state.kg_path.clone(),
                state.session_node_id,
                state.classification.clone(),
                Arc::clone(&state.kg_pool),
                known,
            )
        };

        // Walk filesystem on a blocking thread
        let ws = workspace_path.clone();
        let disk_files: HashSet<String> = tokio::task::spawn_blocking(move || {
            let mut files = Vec::new();
            collect_files_recursive(&ws, &ws, &mut files);
            files.into_iter().collect()
        })
        .await
        .unwrap_or_default();

        // Index files that are on disk but not in our hash map (new)
        let mut join_set = tokio::task::JoinSet::new();
        let sid = session_id.to_string();
        for rel_path in &disk_files {
            if !known_files.contains(rel_path) {
                let this = Arc::clone(self);
                let sid = sid.clone();
                let rp = rel_path.clone();
                let ws = workspace_path.clone();
                let kp = kg_path.clone();
                let cls = classification.clone();
                let pool = Arc::clone(&kg_pool);
                join_set.spawn(async move {
                    this.index_file_inner(&sid, &rp, &ws, &kp, session_node_id, &cls, &pool).await;
                });
            }
        }
        while join_set.join_next().await.is_some() {}

        // Check for modifications in files present in both sets (mtime/size)
        let mut modified_join_set = tokio::task::JoinSet::new();
        for rel_path in disk_files.intersection(&known_files) {
            let abs_path = workspace_path.join(rel_path);
            let changed = if let Ok(metadata) = std::fs::metadata(&abs_path) {
                let sessions = self.sessions.lock().await;
                if let Some(state) = sessions.get(session_id) {
                    if let Some(fs) = state.file_state.get(rel_path) {
                        let mtime = metadata
                            .modified()
                            .ok()
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|d| d.as_secs() as i64)
                            .unwrap_or(0);
                        let size = metadata.len();
                        fs.mtime_secs != mtime || fs.size != size
                    } else {
                        true
                    }
                } else {
                    false
                }
            } else {
                false
            };
            if changed {
                let this = Arc::clone(self);
                let sid = sid.clone();
                let rp = rel_path.clone();
                let ws = workspace_path.clone();
                let kp = kg_path.clone();
                let cls = classification.clone();
                let pool = Arc::clone(&kg_pool);
                modified_join_set.spawn(async move {
                    this.index_file_inner(&sid, &rp, &ws, &kp, session_node_id, &cls, &pool).await;
                });
            }
        }
        while modified_join_set.join_next().await.is_some() {}

        // Remove graph entries for files no longer on disk
        for known_path in &known_files {
            if !disk_files.contains(known_path) {
                self.remove_file_inner(session_id, known_path, &kg_path, session_node_id, &kg_pool)
                    .await;
            }
        }

        debug!(session_id, "reconciliation scan complete");
    }

    /// Extract the relative path key from an event, or None if it should be
    /// skipped (e.g., ignored path or outside workspace).
    fn event_key(&self, _session_id: &str, event: &FileEvent) -> Option<String> {
        // We need the workspace path to compute relative, but we don't have
        // it without the lock. We'll store the absolute path as the key and
        // resolve later in handle_coalesced. Actually let's just store the
        // full path as a string since handle_coalesced re-reads session state.
        let path_str = normalize_path(&event.path.to_string_lossy());
        if watcher::should_ignore(&path_str) {
            return None;
        }
        Some(path_str)
    }

    /// Handle a single coalesced event after debouncing.
    async fn handle_coalesced(
        self: &Arc<Self>,
        session_id: &str,
        abs_path_str: &str,
        kind: FileEventKind,
    ) {
        let (workspace_path, kg_path, session_node_id, classification, kg_pool) = {
            let sessions = self.sessions.lock().await;
            let state = match sessions.get(session_id) {
                Some(s) => s,
                None => return,
            };
            (
                state.workspace_path.clone(),
                state.kg_path.clone(),
                state.session_node_id,
                state.classification.clone(),
                Arc::clone(&state.kg_pool),
            )
        };

        let abs_path = PathBuf::from(abs_path_str);
        let rel_path = match abs_path.strip_prefix(&workspace_path) {
            Ok(r) => normalize_path(&r.to_string_lossy()),
            Err(_) => return,
        };

        if watcher::should_ignore(&rel_path) {
            return;
        }

        match kind {
            FileEventKind::Created | FileEventKind::Modified => {
                if abs_path.is_dir() {
                    // Directory created or renamed-to: scan and index all files in it
                    self.scan_directory(
                        session_id,
                        &abs_path,
                        &workspace_path,
                        &kg_path,
                        session_node_id,
                        &classification,
                        &kg_pool,
                    )
                    .await;
                } else if !abs_path.exists() {
                    // File/dir no longer exists (rename-from with ambiguous event kind)
                    self.remove_path_from_graph(
                        session_id,
                        &rel_path,
                        &kg_path,
                        session_node_id,
                        &workspace_path,
                        &classification,
                        &kg_pool,
                    )
                    .await;
                } else {
                    self.index_file_inner(
                        session_id,
                        &rel_path,
                        &workspace_path,
                        &kg_path,
                        session_node_id,
                        &classification,
                        &kg_pool,
                    )
                    .await;
                }
            }
            FileEventKind::Removed => {
                self.remove_path_from_graph(
                    session_id,
                    &rel_path,
                    &kg_path,
                    session_node_id,
                    &workspace_path,
                    &classification,
                    &kg_pool,
                )
                .await;
            }
        }
    }

    /// Scan a directory and index all files in it (used when a directory is
    /// created or renamed-to).
    #[allow(clippy::too_many_arguments)]
    async fn scan_directory(
        self: &Arc<Self>,
        session_id: &str,
        abs_dir: &Path,
        workspace_path: &Path,
        kg_path: &Path,
        session_node_id: i64,
        classification: &WorkspaceClassification,
        kg_pool: &Arc<KgPool>,
    ) {
        let ad = abs_dir.to_path_buf();
        let ws = workspace_path.to_path_buf();
        let files = tokio::task::spawn_blocking(move || {
            let mut files = Vec::new();
            collect_files_recursive(&ad, &ws, &mut files);
            files
        })
        .await
        .unwrap_or_default();

        let mut join_set = tokio::task::JoinSet::new();
        let sid = session_id.to_string();
        for rel_path in files {
            let this = Arc::clone(self);
            let sid = sid.clone();
            let ws = workspace_path.to_path_buf();
            let kp = kg_path.to_path_buf();
            let cls = classification.clone();
            let pool = Arc::clone(kg_pool);
            join_set.spawn(async move {
                this.index_file_inner(&sid, &rel_path, &ws, &kp, session_node_id, &cls, &pool)
                    .await;
            });
        }
        while join_set.join_next().await.is_some() {}
    }

    /// Remove a path from the graph — handles both files and directories.
    /// For files, removes the file node + chunks. For directories, detects
    /// whether files moved elsewhere in the workspace (rename) and updates
    /// node paths instead of re-indexing when possible.
    #[allow(clippy::too_many_arguments)]
    async fn remove_path_from_graph(
        &self,
        session_id: &str,
        rel_path: &str,
        kg_path: &Path,
        session_node_id: i64,
        workspace_path: &Path,
        classification: &WorkspaceClassification,
        kg_pool: &Arc<KgPool>,
    ) {
        // Check if this path is a file node in the graph before attempting removal.
        // This avoids spurious Removed status events for directory paths.
        let is_file_node = {
            let pool = Arc::clone(kg_pool);
            let rel = rel_path.to_string();
            let snid = session_node_id;
            tokio::task::spawn_blocking(move || {
                pool.get()
                    .ok()
                    .and_then(|g| g.find_node_in_workspace_tree(snid, "workspace_file", &rel).ok())
                    .flatten()
                    .is_some()
            })
            .await
            .unwrap_or(false)
        };

        if is_file_node {
            self.remove_file_inner(session_id, rel_path, kg_path, session_node_id, kg_pool).await;
        }

        // Check if there's a directory node with this path
        let rel = rel_path.to_string();

        let files_under_dir: Vec<String> = tokio::task::spawn_blocking({
            let rel = rel.clone();
            let pool = Arc::clone(kg_pool);
            let snid = session_node_id;
            move || {
                let mut files = Vec::new();
                let graph = match pool.get() {
                    Ok(g) => g,
                    Err(_) => return files,
                };
                if let Ok(Some(dir)) =
                    graph.find_node_in_workspace_tree(snid, "workspace_dir", &rel)
                {
                    collect_files_under_dir(&graph, dir.id, &mut files);
                }
                files
            }
        })
        .await
        .unwrap_or_default();

        if files_under_dir.is_empty() {
            return;
        }

        // Collect old hashes for the files under this directory (path → hash)
        // Also snapshot all currently tracked paths so we can skip re-hashing them.
        let (old_hashes, tracked_paths): (HashMap<String, String>, HashSet<String>) = {
            let sessions = self.sessions.lock().await;
            if let Some(state) = sessions.get(session_id) {
                let old = files_under_dir
                    .iter()
                    .filter_map(|p| {
                        state.file_state.get(p).map(|fs| (p.clone(), fs.content_hash.clone()))
                    })
                    .collect();
                let tracked = state.file_state.keys().cloned().collect();
                (old, tracked)
            } else {
                (HashMap::new(), HashSet::new())
            }
        };

        // Scan the workspace to find where these files moved.
        // Build a hash→new_path map, but only hash *untracked* files (new paths
        // that appeared after the rename). Already-tracked files haven't moved and
        // can be skipped, avoiding O(N) extract+hash over the entire workspace.
        let ws = workspace_path.to_path_buf();
        let new_file_hashes: HashMap<String, String> = tokio::task::spawn_blocking({
            let ws = ws.clone();
            move || {
                let mut hash_to_path = HashMap::new();
                let mut all_files = Vec::new();
                collect_files_recursive(&ws, &ws, &mut all_files);
                for rel in all_files {
                    if tracked_paths.contains(&rel) {
                        continue;
                    }
                    let abs = ws.join(&rel);
                    if let Ok(Some(text)) = extract::extract_text(&abs) {
                        let mut hasher = Sha256::new();
                        hasher.update(text.as_bytes());
                        let hash = format!("{:x}", hasher.finalize());
                        hash_to_path.insert(hash, rel);
                    }
                }
                hash_to_path
            }
        })
        .await
        .unwrap_or_default();

        // Match old files to new locations
        let mut truly_removed = Vec::new();

        for old_file_path in &files_under_dir {
            if let Some(old_hash) = old_hashes.get(old_file_path) {
                if let Some(new_path) = new_file_hashes.get(old_hash) {
                    if new_path != old_file_path {
                        // File moved — rename node (no re-embedding)
                        let data_class = classification.resolve(new_path);
                        let old = old_file_path.clone();
                        let new = new_path.clone();
                        let pool = Arc::clone(kg_pool);

                        let permit = self.index_semaphore.clone().acquire_owned().await;
                        let _permit = match permit {
                            Ok(p) => p,
                            Err(_) => continue,
                        };

                        tokio::task::spawn_blocking(move || {
                            let _permit = _permit;
                            if let Ok(graph) = pool.get() {
                                rename_file_in_graph(
                                    &graph,
                                    session_node_id,
                                    &old,
                                    &new,
                                    data_class,
                                );
                            }
                        })
                        .await
                        .ok();

                        // Update file_state
                        {
                            let mut sessions = self.sessions.lock().await;
                            if let Some(state) = sessions.get_mut(session_id) {
                                let old_fs = state.file_state.remove(old_file_path);
                                let new_abs = workspace_path.join(new_path.as_str());
                                let metadata = std::fs::metadata(&new_abs).ok();
                                let mtime_secs = metadata
                                    .as_ref()
                                    .and_then(|m| m.modified().ok())
                                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                                    .map(|d| d.as_secs() as i64)
                                    .unwrap_or_else(|| {
                                        old_fs.as_ref().map(|f| f.mtime_secs).unwrap_or(0)
                                    });
                                let size =
                                    metadata.as_ref().map(|m| m.len()).unwrap_or_else(|| {
                                        old_fs.as_ref().map(|f| f.size).unwrap_or(0)
                                    });
                                state.file_state.insert(
                                    new_path.clone(),
                                    FileState { content_hash: old_hash.clone(), mtime_secs, size },
                                );
                            }
                        }

                        debug!(
                            old = %old_file_path,
                            new = %new_path,
                            "directory rename: relocated file (no re-embedding)"
                        );
                        continue;
                    }
                }
            }
            // File is truly gone
            truly_removed.push(old_file_path.clone());
        }

        // Remove truly deleted files
        for file_rel_path in &truly_removed {
            self.remove_file_inner(session_id, file_rel_path, kg_path, session_node_id, kg_pool)
                .await;
        }

        // Remove the old directory node and prune
        {
            let rel = rel.clone();
            let pool = Arc::clone(kg_pool);
            let snid = session_node_id;
            tokio::task::spawn_blocking(move || {
                if let Ok(graph) = pool.get() {
                    if let Ok(Some(dir)) =
                        graph.find_node_in_workspace_tree(snid, "workspace_dir", &rel)
                    {
                        if let Err(e) = graph.remove_node(dir.id) {
                            warn!(dir = rel, "failed to remove directory node: {e}");
                        }
                    }
                    // Prune any empty parent directories
                    prune_empty_directories(&graph, snid, &rel);
                }
            })
            .await
            .ok();
        }

        // Clean up content hashes for truly removed files
        if !truly_removed.is_empty() {
            let mut sessions = self.sessions.lock().await;
            if let Some(state) = sessions.get_mut(session_id) {
                for p in &truly_removed {
                    state.file_state.remove(p);
                }
            }
        }
    }

    /// Stop watching a session's workspace.
    pub async fn stop(&self, session_id: &str) {
        let mut sessions = self.sessions.lock().await;
        if sessions.remove(session_id).is_some() {
            info!(session_id, "workspace indexer stopped");
        }
    }

    /// Subscribe to index status events for a session.
    pub async fn subscribe_index_status(
        &self,
        session_id: &str,
    ) -> Option<broadcast::Receiver<FileIndexStatus>> {
        let sessions = self.sessions.lock().await;
        sessions.get(session_id).map(|s| s.status_tx.subscribe())
    }

    /// Return the set of currently indexed file paths for a session.
    pub async fn indexed_files(&self, session_id: &str) -> Vec<String> {
        let sessions = self.sessions.lock().await;
        sessions.get(session_id).map(|s| s.file_state.keys().cloned().collect()).unwrap_or_default()
    }

    /// Force reindex of a single file (clears its content hash so the
    /// next indexing pass will re-extract and re-embed it).
    pub async fn reindex_file(self: &Arc<Self>, session_id: &str, rel_path: &str) {
        let (workspace_path, kg_path, session_node_id, classification, kg_pool) = {
            let mut sessions = self.sessions.lock().await;
            let state = match sessions.get_mut(session_id) {
                Some(s) => s,
                None => return,
            };
            // Clear the file state so index_file_inner won't skip it
            state.file_state.remove(rel_path);
            // Reset failure count so the file gets a fresh retry budget
            state.failure_counts.remove(rel_path);
            (
                state.workspace_path.clone(),
                state.kg_path.clone(),
                state.session_node_id,
                state.classification.clone(),
                Arc::clone(&state.kg_pool),
            )
        };

        self.emit_status(session_id, FileIndexStatus::Queued { path: rel_path.to_string() }).await;

        self.index_file_inner(
            session_id,
            rel_path,
            &workspace_path,
            &kg_path,
            session_node_id,
            &classification,
            &kg_pool,
        )
        .await;
    }

    /// Emit a status event for a session (best-effort, never fails).
    async fn emit_status(&self, session_id: &str, status: FileIndexStatus) {
        let sessions = self.sessions.lock().await;
        if let Some(state) = sessions.get(session_id) {
            let _ = state.status_tx.send(status);
        }
    }

    /// Update classification for all file/chunk nodes of a session.
    pub async fn reclass_session(&self, session_id: &str, classification: WorkspaceClassification) {
        let mut sessions = self.sessions.lock().await;
        let state = match sessions.get_mut(session_id) {
            Some(s) => s,
            None => return,
        };
        state.classification = classification.clone();

        let kg_pool = Arc::clone(&state.kg_pool);
        let session_node_id = state.session_node_id;
        let hashes = state.file_state.keys().cloned().collect::<Vec<_>>();
        let class = classification;

        drop(sessions);

        tokio::task::spawn_blocking(move || {
            let graph = match kg_pool.get() {
                Ok(g) => g,
                Err(e) => {
                    warn!("failed to open KG for reclass: {e}");
                    return;
                }
            };
            for rel_path in hashes {
                let data_class = class.resolve(&rel_path);
                if let Err(e) =
                    update_file_classification(&graph, session_node_id, &rel_path, data_class)
                {
                    warn!(path = rel_path, "reclass failed: {e}");
                }
            }
        })
        .await
        .ok();
    }

    /// Full scan of the workspace directory.
    async fn full_scan(self: &Arc<Self>, session_id: &str) {
        let (workspace_path, kg_path, session_node_id, classification, kg_pool) = {
            let sessions = self.sessions.lock().await;
            let state = match sessions.get(session_id) {
                Some(s) => s,
                None => return,
            };
            (
                state.workspace_path.clone(),
                state.kg_path.clone(),
                state.session_node_id,
                state.classification.clone(),
                Arc::clone(&state.kg_pool),
            )
        };

        // Ensure workspace root directory node exists
        let pool = Arc::clone(&kg_pool);
        let snid = session_node_id;
        tokio::task::spawn_blocking(move || {
            if let Ok(graph) = pool.get() {
                ensure_workspace_root(&graph, snid).ok();
            }
        })
        .await
        .ok();

        // Walk the workspace on a blocking thread to avoid blocking the runtime
        let ws = workspace_path.clone();
        let files = tokio::task::spawn_blocking(move || {
            let mut files = Vec::new();
            collect_files_recursive(&ws, &ws, &mut files);
            files
        })
        .await
        .unwrap_or_default();

        // Index files concurrently — the semaphore inside index_file_inner
        // naturally limits parallelism to DEFAULT_MAX_CONCURRENT.
        let mut join_set = tokio::task::JoinSet::new();
        let sid = session_id.to_string();
        for rel_path in files {
            let this = Arc::clone(self);
            let sid = sid.clone();
            let ws = workspace_path.clone();
            let kp = kg_path.clone();
            let cls = classification.clone();
            let pool = Arc::clone(&kg_pool);
            join_set.spawn(async move {
                this.index_file_inner(&sid, &rel_path, &ws, &kp, session_node_id, &cls, &pool)
                    .await;
            });
        }
        while join_set.join_next().await.is_some() {}
    }

    #[allow(clippy::too_many_arguments)]
    async fn index_file_inner(
        &self,
        session_id: &str,
        rel_path: &str,
        workspace_path: &Path,
        _kg_path: &Path,
        session_node_id: i64,
        classification: &WorkspaceClassification,
        kg_pool: &Arc<KgPool>,
    ) {
        let abs_path = workspace_path.join(rel_path);

        // Check retry budget — skip files that have repeatedly failed to index
        {
            let sessions = self.sessions.lock().await;
            if let Some(state) = sessions.get(session_id) {
                if let Some(&count) = state.failure_counts.get(rel_path) {
                    if count >= MAX_INDEX_RETRIES {
                        debug!(
                            path = rel_path,
                            failures = count,
                            "skipped: file exceeded max index retry attempts"
                        );
                        return;
                    }
                }
            }
        }

        // Check file size
        let meta = match std::fs::metadata(&abs_path) {
            Ok(m) => m,
            Err(e) => {
                warn!(path = rel_path, error = %e, "skipped: file metadata unreadable (file may have been deleted)");
                return;
            }
        };
        if !meta.is_file() {
            debug!(path = rel_path, "skipped: not a regular file");
            return;
        }
        if meta.len() > MAX_FILE_SIZE {
            warn!(
                path = rel_path,
                size = meta.len(),
                max = MAX_FILE_SIZE,
                "skipped: file exceeds maximum size"
            );
            return;
        }

        // Extract text
        let text = match extract::extract_text(&abs_path) {
            Ok(Some(t)) => t,
            Ok(None) => {
                debug!(path = rel_path, "skipped: unsupported file format");
                return;
            }
            Err(e) => {
                warn!(path = rel_path, error = %e, "skipped: text extraction failed");
                return;
            }
        };

        // SHA-256 content hash — skip if unchanged
        let hash = {
            let mut hasher = Sha256::new();
            hasher.update(text.as_bytes());
            format!("{:x}", hasher.finalize())
        };

        // Check if unchanged at same path, or if same content exists at a
        // different path (rename vs copy). For renames we update node names
        // in the graph instead of re-extracting/re-embedding.
        //
        // All hash lookups happen under a single lock to avoid a TOCTOU
        // race where the rename source could be modified between two
        // separate lock acquisitions.
        let rename_source: Option<String> = {
            let sessions = self.sessions.lock().await;
            if let Some(state) = sessions.get(session_id) {
                if state.file_state.get(rel_path).map(|s| &s.content_hash) == Some(&hash) {
                    debug!(path = rel_path, "skipped: content unchanged");
                    return;
                }
                // Look for the same hash at a different path (rename detection).
                // Only treat as rename if the old file no longer exists on disk
                // — otherwise it's a copy and both should be in the graph.
                state
                    .file_state
                    .iter()
                    .find(|(path, fs)| {
                        fs.content_hash == hash
                            && path.as_str() != rel_path
                            && !workspace_path.join(path.as_str()).exists()
                    })
                    .map(|(p, _)| p.clone())
            } else {
                None
            }
        };

        // Fast path: rename — just update node names in the graph, skip
        // text extraction, chunking, and embedding.
        if let Some(old_rel_path) = rename_source {
            let new_rel = rel_path.to_string();
            let old_rel = old_rel_path.clone();
            let pool = Arc::clone(kg_pool);
            let data_class = classification.resolve(rel_path);

            let permit = self.index_semaphore.clone().acquire_owned().await;
            let _permit = match permit {
                Ok(p) => p,
                Err(_) => {
                    warn!(path = rel_path, "skipped rename: indexer semaphore closed");
                    return;
                }
            };

            tokio::task::spawn_blocking(move || {
                let _permit = _permit;
                let graph = match pool.get() {
                    Ok(g) => g,
                    Err(e) => {
                        warn!(old = old_rel, new = new_rel, "rename: failed to open KG: {e}");
                        return;
                    }
                };
                rename_file_in_graph(&graph, session_node_id, &old_rel, &new_rel, data_class);
            })
            .await
            .ok();

            // Insert file state only after successful rename
            {
                let new_abs = workspace_path.join(rel_path);
                let metadata = std::fs::metadata(&new_abs).ok();
                let mtime_secs = metadata
                    .as_ref()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                let size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
                let mut sessions = self.sessions.lock().await;
                if let Some(state) = sessions.get_mut(session_id) {
                    state.file_state.insert(
                        rel_path.to_string(),
                        FileState { content_hash: hash.clone(), mtime_secs, size },
                    );
                    state.file_state.remove(&old_rel_path);
                }
            }

            info!(
                old = %old_rel_path,
                new = rel_path,
                "renamed workspace file (no re-embedding)"
            );
            self.emit_status(session_id, FileIndexStatus::Removed { path: old_rel_path }).await;
            self.emit_status(session_id, FileIndexStatus::Indexed { path: rel_path.to_string() })
                .await;
            return;
        }

        let data_class = classification.resolve(rel_path);
        let chunks = chunk_text(&text, CHUNK_SIZE, OVERLAP_PCT);
        let rel_path_owned = rel_path.to_string();
        let pool = Arc::clone(kg_pool);
        let embed_cb = Arc::clone(&self.embed_callback);
        let model_id = self.embed_resolver.resolve(rel_path).to_string();

        // Acquire a semaphore permit to bound concurrent KG writes.
        let permit = self.index_semaphore.clone().acquire_owned().await;
        let _permit = match permit {
            Ok(p) => p,
            Err(_) => {
                warn!(path = rel_path, "skipped: indexer semaphore closed");
                return;
            }
        };

        let indexed_ok = tokio::task::spawn_blocking(move || {
            let _permit = _permit; // hold permit until block completes
            let graph = match pool.get() {
                Ok(g) => g,
                Err(e) => {
                    warn!(path = rel_path_owned, "failed to open KG: {e}");
                    return false;
                }
            };

            // Remove existing file nodes (re-index)
            if let Err(e) = remove_file_from_graph(&graph, session_node_id, &rel_path_owned) {
                warn!(path = rel_path_owned, "failed to remove existing file nodes: {e}");
            }

            // Ensure directory nodes exist
            let dir_node_id = match ensure_directory_chain(
                &graph,
                session_node_id,
                &rel_path_owned,
                data_class,
            ) {
                Ok(id) => id,
                Err(e) => {
                    warn!(path = rel_path_owned, "failed to ensure directory chain: {e}");
                    return false;
                }
            };

            // Insert file node
            let file_meta = serde_json::json!({
                "path": rel_path_owned,
                "chunks": chunks.len(),
            });
            let file_node_id = match graph.insert_node(&NewNode {
                node_type: "workspace_file".to_string(),
                name: rel_path_owned.clone(),
                data_class,
                content: Some(file_meta.to_string()),
            }) {
                Ok(id) => id,
                Err(e) => {
                    warn!(path = rel_path_owned, "failed to insert file node: {e}");
                    return false;
                }
            };
            if let Err(e) = graph.insert_edge(dir_node_id, file_node_id, "contains_file", 1.0) {
                warn!(path = rel_path_owned, "failed to link file to directory: {e}");
            }

            // Insert chunk nodes + trigger embeddings
            for chunk in &chunks {
                let chunk_name = format!("{}#chunk{}", rel_path_owned, chunk.index);
                match graph.insert_node(&NewNode {
                    node_type: "file_chunk".to_string(),
                    name: chunk_name,
                    data_class,
                    content: Some(chunk.text.clone()),
                }) {
                    Ok(chunk_id) => {
                        if let Err(e) = graph.insert_edge(file_node_id, chunk_id, "file_chunk", 1.0)
                        {
                            warn!(path = rel_path_owned, "failed to link chunk node: {e}");
                        }
                        embed_cb.embed(chunk_id, chunk.text.clone(), model_id.clone());
                    }
                    Err(e) => {
                        warn!(path = rel_path_owned, "failed to insert chunk node: {e}");
                    }
                }
            }

            info!(path = rel_path_owned, chunks = chunks.len(), "indexed workspace file");
            true
        })
        .await
        .unwrap_or(false);

        // Only persist file state after successful KG write
        if indexed_ok {
            let metadata = std::fs::metadata(&abs_path).ok();
            let mtime_secs = metadata
                .as_ref()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
            let mut sessions = self.sessions.lock().await;
            if let Some(state) = sessions.get_mut(session_id) {
                state.file_state.insert(
                    rel_path.to_string(),
                    FileState { content_hash: hash, mtime_secs, size },
                );
                state.failure_counts.remove(rel_path);
            }
        } else {
            let mut sessions = self.sessions.lock().await;
            if let Some(state) = sessions.get_mut(session_id) {
                let count = state.failure_counts.entry(rel_path.to_string()).or_insert(0);
                *count = count.saturating_add(1);
                if *count >= MAX_INDEX_RETRIES {
                    warn!(
                        path = rel_path,
                        failures = *count,
                        "file exceeded max index retry attempts, will skip until reindex"
                    );
                }
            }
        }

        self.emit_status(session_id, FileIndexStatus::Indexed { path: rel_path.to_string() }).await;
    }

    async fn remove_file_inner(
        &self,
        session_id: &str,
        rel_path: &str,
        _kg_path: &Path,
        session_node_id: i64,
        kg_pool: &Arc<KgPool>,
    ) {
        {
            let mut sessions = self.sessions.lock().await;
            if let Some(state) = sessions.get_mut(session_id) {
                state.file_state.remove(rel_path);
            }
        }

        let rel = rel_path.to_string();
        let pool = Arc::clone(kg_pool);

        let permit = self.index_semaphore.clone().acquire_owned().await;
        let _permit = match permit {
            Ok(p) => p,
            Err(_) => return,
        };

        tokio::task::spawn_blocking(move || {
            let _permit = _permit;
            if let Ok(graph) = pool.get() {
                if let Err(e) = remove_file_from_graph(&graph, session_node_id, &rel) {
                    warn!(path = rel, "failed to remove file nodes: {e}");
                } else {
                    // Prune empty parent directory nodes.
                    prune_empty_directories(&graph, session_node_id, &rel);
                }
            }
        })
        .await
        .ok();

        self.emit_status(session_id, FileIndexStatus::Removed { path: rel_path.to_string() }).await;
    }
}

// ── Helper functions ──────────────────────────────────────────────────────

/// Coalesce a new event into the pending buffer.  If the same path already
/// has a pending event, the resulting kind is the "most destructive":
/// - Remove always wins (the file is gone).
/// - Create + Modified → Created (net new).
/// - Modified + Modified → Modified.
fn coalesce_event(pending: &mut HashMap<String, FileEventKind>, key: String, kind: FileEventKind) {
    use FileEventKind::*;
    pending
        .entry(key)
        .and_modify(|existing| {
            *existing = match (*existing, kind) {
                // Remove always wins
                (_, Removed) => Removed,
                (Removed, Created) | (Removed, Modified) => Created, // re-appeared
                (Created, Modified) => Created,                      // still net-new
                (Modified, Modified) => Modified,
                (Created, Created) => Created,
                (Modified, Created) => Modified, // unusual, treat as modify
            };
        })
        .or_insert(kind);
}

/// Maximum number of files collected during a workspace scan.
const MAX_WORKSPACE_FILES: usize = 100_000;

fn collect_files_recursive(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let mut visited = HashSet::new();
    collect_files_inner(root, dir, out, &mut visited);
}

fn collect_files_inner(
    root: &Path,
    dir: &Path,
    out: &mut Vec<String>,
    visited: &mut HashSet<PathBuf>,
) {
    if out.len() >= MAX_WORKSPACE_FILES {
        return;
    }

    // Resolve canonical path to detect symlink cycles
    let canonical = match std::fs::canonicalize(dir) {
        Ok(c) => c,
        Err(_) => return,
    };
    if !visited.insert(canonical) {
        debug!(dir = %dir.display(), "skipped: symlink cycle detected");
        return;
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        if out.len() >= MAX_WORKSPACE_FILES {
            warn!("workspace scan stopped: file limit ({MAX_WORKSPACE_FILES}) reached");
            return;
        }
        let path = entry.path();
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if watcher::should_ignore(name) {
                continue;
            }
        }
        if path.is_dir() {
            collect_files_inner(root, &path, out, visited);
        } else if path.is_file() {
            if let Ok(rel) = path.strip_prefix(root) {
                // Normalise to forward slashes for cross-platform consistency.
                let rel_str = rel.to_string_lossy().replace('\\', "/");
                if !watcher::should_ignore(&rel_str) {
                    out.push(rel_str);
                }
            }
        }
    }
}

/// Recursively collect all workspace_file node names under a directory node.
fn collect_files_under_dir(graph: &KnowledgeGraph, dir_id: i64, out: &mut Vec<String>) {
    // Collect direct file children
    if let Ok(files) =
        graph.list_outbound_nodes(dir_id, "contains_file", DataClass::Restricted, 100000)
    {
        for f in &files {
            out.push(f.name.clone());
        }
    }
    // Recurse into subdirectories
    if let Ok(subdirs) =
        graph.list_outbound_nodes(dir_id, "contains_dir", DataClass::Restricted, 100000)
    {
        for d in &subdirs {
            collect_files_under_dir(graph, d.id, out);
        }
    }
}

fn ensure_workspace_root(graph: &KnowledgeGraph, session_node_id: i64) -> anyhow::Result<i64> {
    // Check if root dir node already exists
    if let Some(id) = find_workspace_dir_node(graph, session_node_id, "/") {
        return Ok(id);
    }
    let root_id = graph.insert_node(&NewNode {
        node_type: "workspace_dir".to_string(),
        name: "/".to_string(),
        data_class: DataClass::Internal,
        content: None,
    })?;
    graph.insert_edge(session_node_id, root_id, "session_workspace", 1.0)?;
    Ok(root_id)
}

/// Ensure directory chain exists from workspace root to the file's parent.
/// Returns the node ID of the immediate parent directory.
fn ensure_directory_chain(
    graph: &KnowledgeGraph,
    session_node_id: i64,
    file_rel_path: &str,
    data_class: DataClass,
) -> anyhow::Result<i64> {
    let root_id = ensure_workspace_root(graph, session_node_id)?;

    let parent = match Path::new(file_rel_path).parent() {
        Some(p) if !p.as_os_str().is_empty() => normalize_path(&p.to_string_lossy()),
        _ => return Ok(root_id),
    };

    let mut current = root_id;
    let mut accumulated = String::new();
    for segment in parent.split('/') {
        if segment.is_empty() {
            continue;
        }
        if !accumulated.is_empty() {
            accumulated.push('/');
        }
        accumulated.push_str(segment);

        if let Some(existing) = find_workspace_dir_node(graph, session_node_id, &accumulated) {
            current = existing;
        } else {
            match graph.insert_node(&NewNode {
                node_type: "workspace_dir".to_string(),
                name: accumulated.clone(),
                data_class,
                content: None,
            }) {
                Ok(id) => {
                    graph.insert_edge(current, id, "contains_dir", 1.0)?;
                    current = id;
                }
                Err(e) => return Err(e),
            }
        }
    }
    Ok(current)
}

/// Find a workspace_dir node by name, scoped to the session's workspace tree.
fn find_workspace_dir_node(
    graph: &KnowledgeGraph,
    session_node_id: i64,
    dir_name: &str,
) -> Option<i64> {
    graph
        .find_node_in_workspace_tree(session_node_id, "workspace_dir", dir_name)
        .ok()?
        .map(|n| n.id)
}

/// Remove a file and its chunk nodes from the graph (session-scoped).
fn remove_file_from_graph(
    graph: &KnowledgeGraph,
    session_node_id: i64,
    rel_path: &str,
) -> anyhow::Result<()> {
    let file_node =
        match graph.find_node_in_workspace_tree(session_node_id, "workspace_file", rel_path)? {
            Some(n) => n,
            None => return Ok(()),
        };

    // Find chunk nodes linked from this file
    let chunks =
        graph.list_outbound_nodes(file_node.id, "file_chunk", DataClass::Restricted, 100000)?;
    let mut ids: Vec<i64> = chunks.iter().map(|chunk| chunk.id).collect();
    ids.push(file_node.id);
    graph.remove_nodes_batch(&ids)?;
    Ok(())
}

/// Rename a file node and its chunks in the graph (move to new path).
/// Re-parents the file under the correct directory chain for the new path
/// without re-extracting text or re-embedding.
fn rename_file_in_graph(
    graph: &KnowledgeGraph,
    session_node_id: i64,
    old_rel_path: &str,
    new_rel_path: &str,
    data_class: DataClass,
) {
    let file_node =
        match graph.find_node_in_workspace_tree(session_node_id, "workspace_file", old_rel_path) {
            Ok(Some(n)) => n,
            _ => return,
        };

    // Update file node name
    if graph.update_node_name(file_node.id, new_rel_path).is_err() {
        return;
    }

    // Update data_class in case the new path resolves to a different classification
    if let Err(e) = graph.update_node_data_class(file_node.id, data_class) {
        warn!(old = old_rel_path, new = new_rel_path, "failed to update file data_class: {e}");
    }

    // Update file node content (metadata JSON with new path)
    if let Some(content) = &file_node.content {
        if let Ok(mut meta) = serde_json::from_str::<serde_json::Value>(content) {
            meta["path"] = serde_json::Value::String(new_rel_path.to_string());
            if let Err(e) = graph.update_node_content(file_node.id, &meta.to_string()) {
                warn!(
                    old = old_rel_path,
                    new = new_rel_path,
                    "failed to update file metadata: {e}"
                );
            }
        }
    }

    // Update chunk node names
    let old_prefix = format!("{old_rel_path}#chunk");
    let new_prefix = format!("{new_rel_path}#chunk");
    if let Ok(chunks) =
        graph.list_outbound_nodes(file_node.id, "file_chunk", DataClass::Restricted, 100000)
    {
        for chunk in &chunks {
            if chunk.name.starts_with(&old_prefix) {
                let new_name = chunk.name.replacen(&old_prefix, &new_prefix, 1);
                if let Err(e) = graph.update_node_name(chunk.id, &new_name) {
                    warn!(old = old_rel_path, new = new_rel_path, "failed to rename chunk: {e}");
                }
            }
            // Update chunk data_class to match the new file classification
            if let Err(e) = graph.update_node_data_class(chunk.id, data_class) {
                warn!(
                    old = old_rel_path,
                    new = new_rel_path,
                    "failed to update chunk data_class: {e}"
                );
            }
        }
    }

    // Re-parent: remove old edge from parent dir, ensure new dir chain,
    // add new edge.
    // Remove old parent edge (find old dir node and remove contains_file edge)
    let old_dir_name = Path::new(old_rel_path)
        .parent()
        .map(|p| {
            let s = normalize_path(&p.to_string_lossy());
            if s.is_empty() {
                "/".to_string()
            } else {
                s
            }
        })
        .unwrap_or_else(|| "/".to_string());
    if let Ok(Some(old_dir)) =
        graph.find_node_in_workspace_tree(session_node_id, "workspace_dir", &old_dir_name)
    {
        if let Err(e) = graph.remove_edge_between(old_dir.id, file_node.id, "contains_file") {
            warn!(old = old_rel_path, new = new_rel_path, "failed to remove old dir edge: {e}");
        }
    }

    // Ensure new directory chain exists and add edge
    let new_dir_id = match ensure_directory_chain(graph, session_node_id, new_rel_path, data_class)
    {
        Ok(id) => id,
        Err(e) => {
            warn!(old = old_rel_path, new = new_rel_path, "failed to ensure new directory: {e}");
            return;
        }
    };
    if let Err(e) = graph.insert_edge(new_dir_id, file_node.id, "contains_file", 1.0) {
        warn!(old = old_rel_path, new = new_rel_path, "failed to link file to new dir: {e}");
    }

    // Prune empty old directory chain
    prune_empty_directories(graph, session_node_id, old_rel_path);

    debug!(old = old_rel_path, new = new_rel_path, "renamed file node in graph");
}

/// Walk up from the file's parent directory and prune any empty
/// `workspace_dir` nodes (no `contains_file` or `contains_dir` children).
/// Stops at the workspace root ("/").
fn prune_empty_directories(graph: &KnowledgeGraph, session_node_id: i64, rel_path: &str) {
    let mut current = match Path::new(rel_path).parent() {
        Some(p) if !p.as_os_str().is_empty() => normalize_path(&p.to_string_lossy()),
        _ => return,
    };

    loop {
        let dir_node =
            match graph.find_node_in_workspace_tree(session_node_id, "workspace_dir", &current) {
                Ok(Some(n)) => n,
                _ => break,
            };

        // Check if it still has children
        let files = graph
            .list_outbound_nodes(dir_node.id, "contains_file", DataClass::Restricted, 1)
            .unwrap_or_default();
        let dirs = graph
            .list_outbound_nodes(dir_node.id, "contains_dir", DataClass::Restricted, 1)
            .unwrap_or_default();

        if !files.is_empty() || !dirs.is_empty() {
            break; // directory is not empty
        }

        // Prune this directory node
        if graph.remove_node(dir_node.id).is_err() {
            break;
        }
        debug!(dir = current, "pruned empty directory node");

        // Walk up to parent
        current = match Path::new(&current).parent() {
            Some(p) if !p.as_os_str().is_empty() => normalize_path(&p.to_string_lossy()),
            _ => break,
        };
    }
}

/// Update data_class on a file and all its chunks.
fn update_file_classification(
    graph: &KnowledgeGraph,
    session_node_id: i64,
    rel_path: &str,
    data_class: DataClass,
) -> anyhow::Result<()> {
    let file_node =
        match graph.find_node_in_workspace_tree(session_node_id, "workspace_file", rel_path)? {
            Some(n) => n,
            None => return Ok(()),
        };

    graph.update_node_data_class(file_node.id, data_class)?;

    let chunks =
        graph.list_outbound_nodes(file_node.id, "file_chunk", DataClass::Restricted, 100000)?;
    for chunk in &chunks {
        graph.update_node_data_class(chunk.id, data_class)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_files_skips_ignored_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join(".git/objects")).unwrap();
        std::fs::write(root.join(".git/objects/abc"), "data").unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
        std::fs::write(root.join("README.md"), "# Hello").unwrap();

        let mut files = Vec::new();
        collect_files_recursive(root, root, &mut files);
        files.sort();

        assert!(files.contains(&"README.md".to_string()));
        assert!(files.contains(&"src/main.rs".to_string()));
        assert!(!files.iter().any(|f| f.contains(".git")));
    }

    #[test]
    fn ensure_directory_chain_creates_hierarchy() {
        let graph = KnowledgeGraph::open_in_memory().unwrap();
        let session_id = graph
            .insert_node(&NewNode {
                node_type: "chat_session".to_string(),
                name: "test".to_string(),
                data_class: DataClass::Internal,
                content: None,
            })
            .unwrap();

        let dir_id =
            ensure_directory_chain(&graph, session_id, "src/utils/helpers.rs", DataClass::Internal)
                .unwrap();
        assert!(dir_id > 0);

        // Verify nodes were created
        let dirs = graph.list_nodes_by_type("workspace_dir").unwrap();
        let dir_names: Vec<&str> = dirs.iter().map(|n| n.name.as_str()).collect();
        assert!(dir_names.contains(&"/"));
        assert!(dir_names.contains(&"src"));
        assert!(dir_names.contains(&"src/utils"));
    }

    #[test]
    fn remove_file_from_graph_cleans_up_chunks() {
        let graph = KnowledgeGraph::open_in_memory().unwrap();
        let session_id = graph
            .insert_node(&NewNode {
                node_type: "chat_session".to_string(),
                name: "test".to_string(),
                data_class: DataClass::Internal,
                content: None,
            })
            .unwrap();

        // Build workspace tree so the file is reachable from the session node.
        let root_dir_id =
            ensure_directory_chain(&graph, session_id, "test.rs", DataClass::Internal).unwrap();

        let file_id = graph
            .insert_node(&NewNode {
                node_type: "workspace_file".to_string(),
                name: "test.rs".to_string(),
                data_class: DataClass::Internal,
                content: Some("{}".to_string()),
            })
            .unwrap();
        graph.insert_edge(root_dir_id, file_id, "contains_file", 1.0).unwrap();

        let chunk_id = graph
            .insert_node(&NewNode {
                node_type: "file_chunk".to_string(),
                name: "test.rs#chunk0".to_string(),
                data_class: DataClass::Internal,
                content: Some("chunk content".to_string()),
            })
            .unwrap();
        graph.insert_edge(file_id, chunk_id, "file_chunk", 1.0).unwrap();

        remove_file_from_graph(&graph, session_id, "test.rs").unwrap();

        assert!(graph.get_node(file_id).unwrap().is_none());
        assert!(graph.get_node(chunk_id).unwrap().is_none());
    }

    #[test]
    fn prune_empty_directories_walks_up() {
        let graph = KnowledgeGraph::open_in_memory().unwrap();
        let session_id = graph
            .insert_node(&NewNode {
                node_type: "chat_session".to_string(),
                name: "test".to_string(),
                data_class: DataClass::Internal,
                content: None,
            })
            .unwrap();

        // Build: / → src → utils, with one file in utils
        let _dir_id =
            ensure_directory_chain(&graph, session_id, "src/utils/helper.rs", DataClass::Internal)
                .unwrap();
        let file_id = graph
            .insert_node(&NewNode {
                node_type: "workspace_file".to_string(),
                name: "src/utils/helper.rs".to_string(),
                data_class: DataClass::Internal,
                content: None,
            })
            .unwrap();
        let utils_id = find_workspace_dir_node(&graph, session_id, "src/utils").unwrap();
        graph.insert_edge(utils_id, file_id, "contains_file", 1.0).unwrap();

        // Remove the file, then prune
        remove_file_from_graph(&graph, session_id, "src/utils/helper.rs").unwrap();
        prune_empty_directories(&graph, session_id, "src/utils/helper.rs");

        // Both "src/utils" and "src" should be pruned (empty)
        assert!(find_workspace_dir_node(&graph, session_id, "src/utils").is_none());
        assert!(find_workspace_dir_node(&graph, session_id, "src").is_none());
        // Root "/" should remain
        assert!(find_workspace_dir_node(&graph, session_id, "/").is_some());
    }

    #[test]
    fn prune_stops_at_non_empty_parent() {
        let graph = KnowledgeGraph::open_in_memory().unwrap();
        let session_id = graph
            .insert_node(&NewNode {
                node_type: "chat_session".to_string(),
                name: "test".to_string(),
                data_class: DataClass::Internal,
                content: None,
            })
            .unwrap();

        // Create: / → src → utils (with file) and / → src → lib.rs
        let _utils_dir =
            ensure_directory_chain(&graph, session_id, "src/utils/helper.rs", DataClass::Internal)
                .unwrap();
        let _src_dir =
            ensure_directory_chain(&graph, session_id, "src/lib.rs", DataClass::Internal).unwrap();

        // Add a file in "src" directly (so src has a child)
        let lib_id = graph
            .insert_node(&NewNode {
                node_type: "workspace_file".to_string(),
                name: "src/lib.rs".to_string(),
                data_class: DataClass::Internal,
                content: None,
            })
            .unwrap();
        let src_id = find_workspace_dir_node(&graph, session_id, "src").unwrap();
        graph.insert_edge(src_id, lib_id, "contains_file", 1.0).unwrap();

        // Prune for a file under src/utils — src/utils should go, src should stay
        prune_empty_directories(&graph, session_id, "src/utils/helper.rs");
        assert!(find_workspace_dir_node(&graph, session_id, "src/utils").is_none());
        assert!(find_workspace_dir_node(&graph, session_id, "src").is_some());
    }

    #[test]
    fn coalesce_event_combines_correctly() {
        let mut pending = HashMap::new();

        // First event: Created
        coalesce_event(&mut pending, "a.txt".to_string(), FileEventKind::Created);
        assert_eq!(pending["a.txt"], FileEventKind::Created);

        // Modified after Created → still Created
        coalesce_event(&mut pending, "a.txt".to_string(), FileEventKind::Modified);
        assert_eq!(pending["a.txt"], FileEventKind::Created);

        // Removed always wins
        coalesce_event(&mut pending, "a.txt".to_string(), FileEventKind::Removed);
        assert_eq!(pending["a.txt"], FileEventKind::Removed);

        // Re-created after removed → Created
        coalesce_event(&mut pending, "a.txt".to_string(), FileEventKind::Created);
        assert_eq!(pending["a.txt"], FileEventKind::Created);

        // Modified + Modified → Modified
        let mut p2 = HashMap::new();
        coalesce_event(&mut p2, "b.txt".to_string(), FileEventKind::Modified);
        coalesce_event(&mut p2, "b.txt".to_string(), FileEventKind::Modified);
        assert_eq!(p2["b.txt"], FileEventKind::Modified);
    }
}
