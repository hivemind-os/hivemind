//! Integration tests for hive-workspace-index.
//!
//! Tests range from simple (extraction, chunking) to complex (full indexer
//! lifecycle with live KG, reclassification, deduplication).

use hive_classification::DataClass;
use hive_contracts::WorkspaceClassification;
use hive_knowledge::{KnowledgeGraph, NewNode};
use hive_workspace_index::{chunk_text, extract_text, EmbeddingCallback, WorkspaceIndexer};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

// ── Helpers ──────────────────────────────────────────────────────────────

/// Dummy embedding callback that records node IDs and text.
struct TestEmbedCallback {
    calls: Mutex<Vec<(i64, String)>>,
    call_count: AtomicUsize,
}

impl TestEmbedCallback {
    fn new() -> Self {
        Self { calls: Mutex::new(Vec::new()), call_count: AtomicUsize::new(0) }
    }

    fn count(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
    }
}

impl EmbeddingCallback for TestEmbedCallback {
    fn embed(&self, node_id: i64, text: String, _model_id: String) {
        self.calls.lock().unwrap().push((node_id, text));
        self.call_count.fetch_add(1, Ordering::SeqCst);
    }
}

fn temp_graph() -> (tempfile::TempDir, PathBuf, KnowledgeGraph) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("knowledge.db");
    let graph = KnowledgeGraph::open(&path).expect("open graph");
    (dir, path, graph)
}

fn session_node(graph: &KnowledgeGraph, name: &str) -> i64 {
    graph
        .insert_node(&NewNode {
            node_type: "chat_session".to_string(),
            name: name.to_string(),
            data_class: DataClass::Internal,
            content: None,
        })
        .expect("insert session node")
}

/// Poll `check` every 200ms for up to `timeout`. Returns `true` if the
/// check succeeded within the deadline. This accommodates FSEvents' higher
/// latency compared to kqueue.
async fn wait_for(timeout: std::time::Duration, check: impl Fn() -> bool) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if check() {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    false
}

// ── 1. Simple: text extraction for known extension ──────────────────────

#[test]
fn t01_extract_text_plain_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hello.txt");
    std::fs::write(&path, "Hello, world!").unwrap();

    let result = extract_text(&path).unwrap();
    assert_eq!(result, Some("Hello, world!".to_string()));
}

// ── 2. Simple: extraction returns None for binary ───────────────────────

#[test]
fn t02_extract_text_returns_none_for_png() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("image.png");
    std::fs::write(&path, [0x89, 0x50, 0x4E, 0x47]).unwrap();

    let result = extract_text(&path).unwrap();
    assert!(result.is_none());
}

// ── 3. Simple: chunking short text ──────────────────────────────────────

#[test]
fn t03_chunk_short_text_single_chunk() {
    let text = "Short text";
    let chunks = chunk_text(text, 2000, 0.10);
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].text, text);
    assert_eq!(chunks[0].index, 0);
}

// ── 4. Simple: chunking produces correct overlap ────────────────────────

#[test]
fn t04_chunk_overlap_correctness() {
    let text = "a ".repeat(2000); // 4000 chars
    let chunks = chunk_text(&text, 2000, 0.10);
    assert!(chunks.len() >= 2, "should produce at least 2 chunks");

    // Verify overlap: end of chunk 0 should appear in start of chunk 1
    let overlap_size = (2000.0 * 0.10) as usize;
    let tail_0 = &chunks[0].text[chunks[0].text.len().saturating_sub(overlap_size)..];
    assert!(chunks[1].text.starts_with(tail_0), "chunk 1 should start with the tail of chunk 0");
}

// ── 5. Simple: chunk indices are sequential ─────────────────────────────

#[test]
fn t05_chunk_indices_sequential() {
    let text = "word ".repeat(3000);
    let chunks = chunk_text(&text, 2000, 0.10);
    for (i, chunk) in chunks.iter().enumerate() {
        assert_eq!(chunk.index, i, "chunk index should match position");
    }
}

// ── 6. Simple: chunks cover entire text ─────────────────────────────────

#[test]
fn t06_chunks_cover_entire_text() {
    let text = "The quick brown fox jumps over the lazy dog. ".repeat(100);
    let chunks = chunk_text(&text, 500, 0.10);
    assert!(!chunks.is_empty());

    // First chunk should start at the beginning of the text
    assert!(
        text.starts_with(&chunks[0].text[..50]),
        "first chunk should start with the beginning of the text"
    );

    // Last chunk should end at the end of the text
    let last = &chunks[chunks.len() - 1];
    assert!(
        text.ends_with(&last.text[last.text.len().saturating_sub(50)..]),
        "last chunk should end with the end of the text"
    );

    // Total chars across all chunks (with overlap) should be >= text length
    let total_chars: usize = chunks.iter().map(|c| c.text.len()).sum();
    assert!(
        total_chars >= text.len(),
        "total chunk characters ({total_chars}) should be >= text length ({})",
        text.len()
    );
}

// ── 7. Moderate: extract text from various extensions ───────────────────

#[test]
fn t07_extract_text_various_extensions() {
    let dir = tempfile::tempdir().unwrap();
    let content = "fn main() { println!(\"hello\"); }";

    for ext in &["rs", "py", "js", "ts", "md", "json", "toml", "yaml", "html", "css"] {
        let path = dir.path().join(format!("file.{ext}"));
        std::fs::write(&path, content).unwrap();
        let result = extract_text(&path).unwrap();
        assert_eq!(
            result,
            Some(content.to_string()),
            "extension .{ext} should be recognized as text"
        );
    }
}

// ── 8. Moderate: watcher ignores standard directories ───────────────────

#[test]
fn t08_watcher_ignore_patterns_comprehensive() {
    use hive_workspace_index::watcher_should_ignore;

    // These should all be ignored
    let ignored = [
        ".git",
        "node_modules",
        "target",
        "__pycache__",
        ".venv",
        "dist",
        ".DS_Store",
        ".next",
        "vendor",
        ".gradle",
        ".idea",
        ".vscode",
        ".cache",
        "coverage",
        "out",
        ".turbo",
        ".cargo",
    ];
    for path in &ignored {
        assert!(watcher_should_ignore(path), "{path} should be ignored");
    }

    // These should NOT be ignored
    let allowed = ["src/main.rs", "docs/readme.md", "Cargo.toml", "package.json"];
    for path in &allowed {
        assert!(!watcher_should_ignore(path), "{path} should NOT be ignored");
    }
}

// ── 9. Moderate: indexer creates workspace root node ────────────────────

#[tokio::test]
async fn t09_indexer_creates_workspace_root() {
    let (_dir, kg_path, graph) = temp_graph();
    let sid = session_node(&graph, "test-session");
    drop(graph);

    let cb = Arc::new(TestEmbedCallback::new());
    let indexer = Arc::new(WorkspaceIndexer::new(cb as Arc<dyn EmbeddingCallback>));
    let ws = tempfile::tempdir().unwrap();

    indexer
        .start(
            "s1".to_string(),
            sid,
            ws.path().to_path_buf(),
            kg_path.clone(),
            WorkspaceClassification::default(),
        )
        .await
        .unwrap();

    let graph = KnowledgeGraph::open(&kg_path).unwrap();
    let root = graph.find_node_by_type_and_name("workspace_dir", "/").unwrap();
    assert!(root.is_some(), "workspace root directory node should exist");

    indexer.stop("s1").await;
}

// ── 10. Moderate: indexer creates file and chunk nodes ──────────────────

#[tokio::test]
async fn t10_indexer_creates_file_and_chunk_nodes() {
    let (_dir, kg_path, graph) = temp_graph();
    let sid = session_node(&graph, "test-session");
    drop(graph);

    let cb = Arc::new(TestEmbedCallback::new());
    let indexer = Arc::new(WorkspaceIndexer::new(Arc::clone(&cb) as Arc<dyn EmbeddingCallback>));
    let ws = tempfile::tempdir().unwrap();
    std::fs::write(ws.path().join("hello.txt"), "Hello workspace file").unwrap();

    indexer
        .start(
            "s1".to_string(),
            sid,
            ws.path().to_path_buf(),
            kg_path.clone(),
            WorkspaceClassification::default(),
        )
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let graph = KnowledgeGraph::open(&kg_path).unwrap();

    let file_node = graph.find_node_by_type_and_name("workspace_file", "hello.txt").unwrap();
    assert!(file_node.is_some(), "file node should be created");

    let chunks = graph.list_nodes_by_type("file_chunk").unwrap();
    assert!(!chunks.is_empty(), "at least one chunk node should exist");

    // Embedding callback should have been called for each chunk
    assert!(cb.count() > 0, "embedding callback should be invoked");

    indexer.stop("s1").await;
}

// ── 11. Moderate: indexer handles nested directories ────────────────────

#[tokio::test]
async fn t11_indexer_creates_directory_hierarchy() {
    let (_dir, kg_path, graph) = temp_graph();
    let sid = session_node(&graph, "test-session");
    drop(graph);

    let cb = Arc::new(TestEmbedCallback::new());
    let indexer = Arc::new(WorkspaceIndexer::new(cb as Arc<dyn EmbeddingCallback>));
    let ws = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(ws.path().join("src/utils")).unwrap();
    std::fs::write(ws.path().join("src/utils/helpers.rs"), "pub fn help() {}").unwrap();

    indexer
        .start(
            "s1".to_string(),
            sid,
            ws.path().to_path_buf(),
            kg_path.clone(),
            WorkspaceClassification::default(),
        )
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let graph = KnowledgeGraph::open(&kg_path).unwrap();
    let dirs = graph.list_nodes_by_type("workspace_dir").unwrap();
    let dir_names: Vec<&str> = dirs.iter().map(|n| n.name.as_str()).collect();

    assert!(dir_names.contains(&"/"), "root dir should exist");
    assert!(dir_names.contains(&"src"), "src dir should exist");
    assert!(dir_names.contains(&"src/utils"), "src/utils dir should exist");

    indexer.stop("s1").await;
}

// ── 12. Moderate: SHA dedup skips unchanged files within session ────────

#[tokio::test]
async fn t12_sha_dedup_skips_unchanged_files() {
    let (_dir, kg_path, graph) = temp_graph();
    let sid = session_node(&graph, "test-session");
    drop(graph);

    let cb = Arc::new(TestEmbedCallback::new());
    let indexer = Arc::new(WorkspaceIndexer::new(Arc::clone(&cb) as Arc<dyn EmbeddingCallback>));
    let ws = tempfile::tempdir().unwrap();
    std::fs::write(ws.path().join("data.txt"), "initial content").unwrap();

    indexer
        .start(
            "s1".to_string(),
            sid,
            ws.path().to_path_buf(),
            kg_path.clone(),
            WorkspaceClassification::default(),
        )
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let first_count = cb.count();
    assert!(first_count > 0, "first scan should trigger embeddings");

    indexer.stop("s1").await;
}

// ── 13. Moderate: indexer stops and removes session state ───────────────

#[tokio::test]
async fn t13_stop_removes_session_state() {
    let (_dir, kg_path, graph) = temp_graph();
    let sid = session_node(&graph, "test-session");
    drop(graph);

    let cb = Arc::new(TestEmbedCallback::new());
    let indexer = Arc::new(WorkspaceIndexer::new(cb as Arc<dyn EmbeddingCallback>));
    let ws = tempfile::tempdir().unwrap();

    indexer
        .start(
            "s1".to_string(),
            sid,
            ws.path().to_path_buf(),
            kg_path.clone(),
            WorkspaceClassification::default(),
        )
        .await
        .unwrap();

    indexer.stop("s1").await;

    // Stopping again should be a no-op (no panic)
    indexer.stop("s1").await;
}

// ── 14. Moderate: large file chunked into multiple vectors ──────────────

#[tokio::test]
async fn t14_large_file_creates_multiple_chunks() {
    let (_dir, kg_path, graph) = temp_graph();
    let sid = session_node(&graph, "test-session");
    drop(graph);

    let cb = Arc::new(TestEmbedCallback::new());
    let indexer = Arc::new(WorkspaceIndexer::new(Arc::clone(&cb) as Arc<dyn EmbeddingCallback>));
    let ws = tempfile::tempdir().unwrap();

    // 10,000 chars → ~5 chunks at 2000 chars each
    let big_text = "word ".repeat(2000);
    std::fs::write(ws.path().join("big.txt"), &big_text).unwrap();

    indexer
        .start(
            "s1".to_string(),
            sid,
            ws.path().to_path_buf(),
            kg_path.clone(),
            WorkspaceClassification::default(),
        )
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let graph = KnowledgeGraph::open(&kg_path).unwrap();
    let chunks = graph.list_nodes_by_type("file_chunk").unwrap();
    assert!(chunks.len() >= 4, "10k chars should create at least 4 chunks, got {}", chunks.len());

    // Each chunk should have triggered an embedding callback
    assert_eq!(cb.count(), chunks.len());

    indexer.stop("s1").await;
}

// ── 15. Complex: reclassification updates file and chunk nodes ──────────

#[tokio::test]
async fn t15_reclass_updates_all_nodes() {
    let (_dir, kg_path, graph) = temp_graph();
    let sid = session_node(&graph, "test-session");
    drop(graph);

    let cb = Arc::new(TestEmbedCallback::new());
    let indexer = Arc::new(WorkspaceIndexer::new(cb as Arc<dyn EmbeddingCallback>));
    let ws = tempfile::tempdir().unwrap();
    std::fs::write(ws.path().join("secret.txt"), "classified data here").unwrap();

    let class = WorkspaceClassification::default();
    indexer
        .start("s1".to_string(), sid, ws.path().to_path_buf(), kg_path.clone(), class)
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Verify initial classification
    let graph = KnowledgeGraph::open(&kg_path).unwrap();
    let file_node = graph
        .find_node_by_type_and_name("workspace_file", "secret.txt")
        .unwrap()
        .expect("file node exists");
    assert_eq!(file_node.data_class, DataClass::Internal);
    drop(graph);

    // Reclassify
    let mut new_class = WorkspaceClassification::default();
    new_class.default = DataClass::Confidential;
    indexer.reclass_session("s1", new_class).await;

    let graph = KnowledgeGraph::open(&kg_path).unwrap();
    let file_node = graph
        .find_node_by_type_and_name("workspace_file", "secret.txt")
        .unwrap()
        .expect("file node still exists");
    assert_eq!(file_node.data_class, DataClass::Confidential, "file node should be reclassified");

    // Chunks should also be reclassified
    let chunks =
        graph.list_outbound_nodes(file_node.id, "file_chunk", DataClass::Restricted, 100).unwrap();
    for chunk in &chunks {
        assert_eq!(
            chunk.data_class,
            DataClass::Confidential,
            "chunk {} should be reclassified",
            chunk.name
        );
    }

    indexer.stop("s1").await;
}

// ── 16. Complex: file with classification override ──────────────────────

#[tokio::test]
async fn t16_classification_overrides_applied_to_nodes() {
    let (_dir, kg_path, graph) = temp_graph();
    let sid = session_node(&graph, "test-session");
    drop(graph);

    let cb = Arc::new(TestEmbedCallback::new());
    let indexer = Arc::new(WorkspaceIndexer::new(cb as Arc<dyn EmbeddingCallback>));
    let ws = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(ws.path().join("public")).unwrap();
    std::fs::create_dir_all(ws.path().join("secret")).unwrap();
    std::fs::write(ws.path().join("public/readme.md"), "# Public docs").unwrap();
    std::fs::write(ws.path().join("secret/keys.txt"), "password=hunter2").unwrap();

    let mut class = WorkspaceClassification::new(DataClass::Public);
    class.set_override("secret", DataClass::Restricted);

    indexer
        .start("s1".to_string(), sid, ws.path().to_path_buf(), kg_path.clone(), class)
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let graph = KnowledgeGraph::open(&kg_path).unwrap();

    let public_file = graph
        .find_node_by_type_and_name("workspace_file", "public/readme.md")
        .unwrap()
        .expect("public file");
    assert_eq!(public_file.data_class, DataClass::Public);

    let secret_file = graph
        .find_node_by_type_and_name("workspace_file", "secret/keys.txt")
        .unwrap()
        .expect("secret file");
    assert_eq!(secret_file.data_class, DataClass::Restricted);

    indexer.stop("s1").await;
}

// ── 17. Complex: multiple sessions share the same KG ────────────────────

#[tokio::test]
async fn t17_multiple_sessions_share_kg() {
    let (_dir, kg_path, graph) = temp_graph();
    let sid1 = session_node(&graph, "session-1");
    let sid2 = session_node(&graph, "session-2");
    drop(graph);

    let cb = Arc::new(TestEmbedCallback::new());
    let indexer = Arc::new(WorkspaceIndexer::new(cb as Arc<dyn EmbeddingCallback>));
    let ws1 = tempfile::tempdir().unwrap();
    let ws2 = tempfile::tempdir().unwrap();
    std::fs::write(ws1.path().join("a.txt"), "session one file").unwrap();
    std::fs::write(ws2.path().join("b.txt"), "session two file").unwrap();

    indexer
        .start(
            "s1".to_string(),
            sid1,
            ws1.path().to_path_buf(),
            kg_path.clone(),
            WorkspaceClassification::default(),
        )
        .await
        .unwrap();

    indexer
        .start(
            "s2".to_string(),
            sid2,
            ws2.path().to_path_buf(),
            kg_path.clone(),
            WorkspaceClassification::default(),
        )
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let graph = KnowledgeGraph::open(&kg_path).unwrap();
    let files = graph.list_nodes_by_type("workspace_file").unwrap();
    let file_names: Vec<&str> = files.iter().map(|n| n.name.as_str()).collect();
    assert!(file_names.contains(&"a.txt"), "session 1 file should exist");
    assert!(file_names.contains(&"b.txt"), "session 2 file should exist");

    indexer.stop("s1").await;
    indexer.stop("s2").await;
}

// ── 18. Complex: indexer detects file creation via watcher ──────────────

#[tokio::test]
async fn t18_watcher_detects_new_file() {
    let (_dir, kg_path, graph) = temp_graph();
    let sid = session_node(&graph, "test-session");
    drop(graph);

    let cb = Arc::new(TestEmbedCallback::new());
    let indexer = Arc::new(WorkspaceIndexer::new(Arc::clone(&cb) as Arc<dyn EmbeddingCallback>));
    let ws = tempfile::tempdir().unwrap();

    indexer
        .start(
            "s1".to_string(),
            sid,
            ws.path().to_path_buf(),
            kg_path.clone(),
            WorkspaceClassification::default(),
        )
        .await
        .unwrap();

    // Write a file AFTER the indexer has started
    std::fs::write(ws.path().join("new_file.rs"), "fn new_function() {}").unwrap();

    // Wait for the watcher + indexer to pick it up (FSEvents may need several seconds)
    let kg = kg_path.clone();
    let found = wait_for(std::time::Duration::from_secs(10), move || {
        let graph = KnowledgeGraph::open(&kg).unwrap();
        graph.find_node_by_type_and_name("workspace_file", "new_file.rs").unwrap().is_some()
    })
    .await;
    assert!(found, "watcher should detect and index new file");

    indexer.stop("s1").await;
}

// ── 19. Complex: indexer detects file modification via watcher ──────────

#[tokio::test]
async fn t19_watcher_detects_file_modification() {
    let (_dir, kg_path, graph) = temp_graph();
    let sid = session_node(&graph, "test-session");
    drop(graph);

    let cb = Arc::new(TestEmbedCallback::new());
    let indexer = Arc::new(WorkspaceIndexer::new(Arc::clone(&cb) as Arc<dyn EmbeddingCallback>));
    let ws = tempfile::tempdir().unwrap();
    std::fs::write(ws.path().join("data.txt"), "version 1").unwrap();

    indexer
        .start(
            "s1".to_string(),
            sid,
            ws.path().to_path_buf(),
            kg_path.clone(),
            WorkspaceClassification::default(),
        )
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let initial_embed_count = cb.count();

    // Modify the file
    std::fs::write(ws.path().join("data.txt"), "version 2 — updated content").unwrap();

    // Wait for watcher + indexer (FSEvents may need several seconds)
    let cb2 = Arc::clone(&cb);
    let found =
        wait_for(std::time::Duration::from_secs(10), move || cb2.count() > initial_embed_count)
            .await;
    assert!(
        found,
        "modification should trigger re-indexing: initial={initial_embed_count}, after={}",
        cb.count()
    );

    // Verify the content was updated
    let graph = KnowledgeGraph::open(&kg_path).unwrap();
    let chunks = graph.list_nodes_by_type("file_chunk").unwrap();
    let has_v2 =
        chunks.iter().any(|c| c.content.as_ref().is_some_and(|text| text.contains("version 2")));
    assert!(has_v2, "updated content should be present in chunks");

    indexer.stop("s1").await;
}

// ── 20. Complex: indexer ignores .git and node_modules ──────────────────

#[tokio::test]
async fn t20_indexer_ignores_git_and_node_modules() {
    let (_dir, kg_path, graph) = temp_graph();
    let sid = session_node(&graph, "test-session");
    drop(graph);

    let cb = Arc::new(TestEmbedCallback::new());
    let indexer = Arc::new(WorkspaceIndexer::new(cb as Arc<dyn EmbeddingCallback>));
    let ws = tempfile::tempdir().unwrap();

    // Create files in both normal and ignored directories
    std::fs::create_dir_all(ws.path().join(".git/objects")).unwrap();
    std::fs::write(ws.path().join(".git/objects/abc123"), "git internal data").unwrap();
    std::fs::create_dir_all(ws.path().join("node_modules/lodash")).unwrap();
    std::fs::write(ws.path().join("node_modules/lodash/index.js"), "module.exports = {}").unwrap();
    std::fs::write(ws.path().join("src.rs"), "fn main() {}").unwrap();

    indexer
        .start(
            "s1".to_string(),
            sid,
            ws.path().to_path_buf(),
            kg_path.clone(),
            WorkspaceClassification::default(),
        )
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let graph = KnowledgeGraph::open(&kg_path).unwrap();
    let files = graph.list_nodes_by_type("workspace_file").unwrap();
    let file_names: Vec<&str> = files.iter().map(|n| n.name.as_str()).collect();

    assert!(file_names.contains(&"src.rs"), "normal file should be indexed");
    assert!(!file_names.iter().any(|n| n.contains(".git")), ".git files should not be indexed");
    assert!(
        !file_names.iter().any(|n| n.contains("node_modules")),
        "node_modules files should not be indexed"
    );

    indexer.stop("s1").await;
}

// ── 21. Debounce: rapid writes produce single index ─────────────────────

#[tokio::test]
async fn t21_debounce_coalesces_rapid_writes() {
    let (_dir, kg_path, graph) = temp_graph();
    let sid = session_node(&graph, "test-session");
    drop(graph);

    let cb = Arc::new(TestEmbedCallback::new());
    // Use a short debounce (100ms) for test speed
    let indexer = Arc::new(WorkspaceIndexer::with_config(
        Arc::clone(&cb) as Arc<dyn EmbeddingCallback>,
        4,
        100,
        60,
    ));
    let ws = tempfile::tempdir().unwrap();

    indexer
        .start(
            "s1".to_string(),
            sid,
            ws.path().to_path_buf(),
            kg_path.clone(),
            WorkspaceClassification::default(),
        )
        .await
        .unwrap();

    // Rapidly overwrite the same file 10 times
    for i in 0..10 {
        std::fs::write(ws.path().join("rapid.txt"), format!("version {i}")).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    // Wait for debounce to flush + indexing to complete
    let kg = kg_path.clone();
    let found = wait_for(std::time::Duration::from_secs(10), move || {
        let graph = KnowledgeGraph::open(&kg).unwrap();
        let files = graph.list_nodes_by_type("workspace_file").unwrap();
        let rapid_files: Vec<_> = files.iter().filter(|f| f.name == "rapid.txt").collect();
        if rapid_files.len() != 1 {
            return false;
        }
        let chunks = graph.list_nodes_by_type("file_chunk").unwrap();
        chunks.iter().any(|c| c.content.as_ref().is_some_and(|t| t.contains("version 9")))
    })
    .await;
    assert!(found, "debounce should coalesce rapid writes into one file node with latest version");

    indexer.stop("s1").await;
}

// ── 22. Debounce: Remove after Create coalesces to nothing ──────────────

#[tokio::test]
async fn t22_debounce_create_then_remove() {
    let (_dir, kg_path, graph) = temp_graph();
    let sid = session_node(&graph, "test-session");
    drop(graph);

    let cb = Arc::new(TestEmbedCallback::new());
    let indexer = Arc::new(WorkspaceIndexer::with_config(
        Arc::clone(&cb) as Arc<dyn EmbeddingCallback>,
        4,
        100,
        60,
    ));
    let ws = tempfile::tempdir().unwrap();

    indexer
        .start(
            "s1".to_string(),
            sid,
            ws.path().to_path_buf(),
            kg_path.clone(),
            WorkspaceClassification::default(),
        )
        .await
        .unwrap();

    // Create then immediately delete a file
    std::fs::write(ws.path().join("ephemeral.txt"), "temporary").unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    std::fs::remove_file(ws.path().join("ephemeral.txt")).unwrap();

    // Wait for debounce + processing
    tokio::time::sleep(std::time::Duration::from_millis(2000)).await;

    let graph = KnowledgeGraph::open(&kg_path).unwrap();
    let file = graph.find_node_by_type_and_name("workspace_file", "ephemeral.txt").unwrap();
    assert!(file.is_none(), "file created and quickly deleted should not remain indexed");

    indexer.stop("s1").await;
}

// ── 23. Move file: old removed, new created ─────────────────────────────

#[tokio::test]
async fn t23_move_file_reindexes_at_new_path() {
    let (_dir, kg_path, graph) = temp_graph();
    let sid = session_node(&graph, "test-session");
    drop(graph);

    let cb = Arc::new(TestEmbedCallback::new());
    let indexer = Arc::new(WorkspaceIndexer::with_config(
        Arc::clone(&cb) as Arc<dyn EmbeddingCallback>,
        4,
        100,
        60,
    ));
    let ws = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(ws.path().join("old_dir")).unwrap();
    std::fs::create_dir_all(ws.path().join("new_dir")).unwrap();
    std::fs::write(ws.path().join("old_dir/moved.txt"), "moveable content").unwrap();

    indexer
        .start(
            "s1".to_string(),
            sid,
            ws.path().to_path_buf(),
            kg_path.clone(),
            WorkspaceClassification::default(),
        )
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(2000)).await;

    // Move the file
    std::fs::rename(ws.path().join("old_dir/moved.txt"), ws.path().join("new_dir/moved.txt"))
        .unwrap();

    // Wait for watcher to detect remove + create
    let kg = kg_path.clone();
    let found = wait_for(std::time::Duration::from_secs(10), move || {
        let graph = KnowledgeGraph::open(&kg).unwrap();
        let old_gone = graph
            .find_node_by_type_and_name("workspace_file", "old_dir/moved.txt")
            .unwrap()
            .is_none();
        let new_exists = graph
            .find_node_by_type_and_name("workspace_file", "new_dir/moved.txt")
            .unwrap()
            .is_some();
        old_gone && new_exists
    })
    .await;
    assert!(found, "watcher should detect move: old path removed and new path indexed");

    indexer.stop("s1").await;
}

// ── 24. Orphan dir cleanup after last file removed ──────────────────────

#[tokio::test]
async fn t24_orphan_directory_cleanup_on_remove() {
    let (_dir, kg_path, graph) = temp_graph();
    let sid = session_node(&graph, "test-session");
    drop(graph);

    let cb = Arc::new(TestEmbedCallback::new());
    let indexer = Arc::new(WorkspaceIndexer::with_config(
        Arc::clone(&cb) as Arc<dyn EmbeddingCallback>,
        4,
        100,
        60,
    ));
    let ws = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(ws.path().join("deep/nested/dir")).unwrap();
    std::fs::write(ws.path().join("deep/nested/dir/only_file.txt"), "lonely content").unwrap();

    indexer
        .start(
            "s1".to_string(),
            sid,
            ws.path().to_path_buf(),
            kg_path.clone(),
            WorkspaceClassification::default(),
        )
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(2000)).await;

    // Remove the only file
    std::fs::remove_file(ws.path().join("deep/nested/dir/only_file.txt")).unwrap();

    // Wait for watcher + cleanup
    let kg = kg_path.clone();
    let found = wait_for(std::time::Duration::from_secs(10), move || {
        let graph = KnowledgeGraph::open(&kg).unwrap();
        let file_gone = graph
            .find_node_by_type_and_name("workspace_file", "deep/nested/dir/only_file.txt")
            .unwrap()
            .is_none();
        let dir_pruned =
            graph.find_node_by_type_and_name("workspace_dir", "deep/nested/dir").unwrap().is_none();
        let nested_pruned =
            graph.find_node_by_type_and_name("workspace_dir", "deep/nested").unwrap().is_none();
        let deep_pruned =
            graph.find_node_by_type_and_name("workspace_dir", "deep").unwrap().is_none();
        file_gone && dir_pruned && nested_pruned && deep_pruned
    })
    .await;
    assert!(found, "file should be removed and empty directory chain should be pruned");

    let graph = KnowledgeGraph::open(&kg_path).unwrap();

    // Root "/" should remain
    assert!(
        graph.find_node_by_type_and_name("workspace_dir", "/").unwrap().is_some(),
        "root dir should survive pruning"
    );

    indexer.stop("s1").await;
}

// ── 25. Orphan cleanup stops at non-empty parent ────────────────────────

#[tokio::test]
async fn t25_orphan_cleanup_stops_at_nonempty_parent() {
    let (_dir, kg_path, graph) = temp_graph();
    let sid = session_node(&graph, "test-session");
    drop(graph);

    let cb = Arc::new(TestEmbedCallback::new());
    let indexer = Arc::new(WorkspaceIndexer::with_config(
        Arc::clone(&cb) as Arc<dyn EmbeddingCallback>,
        4,
        100,
        60,
    ));
    let ws = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(ws.path().join("src/tests")).unwrap();
    std::fs::write(ws.path().join("src/lib.rs"), "pub mod tests;").unwrap();
    std::fs::write(ws.path().join("src/tests/unit.rs"), "fn test() {}").unwrap();

    indexer
        .start(
            "s1".to_string(),
            sid,
            ws.path().to_path_buf(),
            kg_path.clone(),
            WorkspaceClassification::default(),
        )
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(2000)).await;

    // Remove only the file in tests/, not lib.rs
    std::fs::remove_file(ws.path().join("src/tests/unit.rs")).unwrap();

    let kg = kg_path.clone();
    let found = wait_for(std::time::Duration::from_secs(10), move || {
        let graph = KnowledgeGraph::open(&kg).unwrap();
        let tests_pruned =
            graph.find_node_by_type_and_name("workspace_dir", "src/tests").unwrap().is_none();
        let src_exists =
            graph.find_node_by_type_and_name("workspace_dir", "src").unwrap().is_some();
        let lib_exists =
            graph.find_node_by_type_and_name("workspace_file", "src/lib.rs").unwrap().is_some();
        tests_pruned && src_exists && lib_exists
    })
    .await;
    assert!(found, "src/tests should be pruned but src and lib.rs should remain");

    indexer.stop("s1").await;
}

// ── 26. Concurrent indexing with semaphore ──────────────────────────────

#[tokio::test]
async fn t26_concurrent_indexing_bounded() {
    let (_dir, kg_path, graph) = temp_graph();
    let sid = session_node(&graph, "test-session");
    drop(graph);

    let cb = Arc::new(TestEmbedCallback::new());
    // Only 2 concurrent workers
    let indexer = Arc::new(WorkspaceIndexer::with_config(
        Arc::clone(&cb) as Arc<dyn EmbeddingCallback>,
        2,
        50,
        60,
    ));
    let ws = tempfile::tempdir().unwrap();

    // Create 20 files to index concurrently
    for i in 0..20 {
        std::fs::write(ws.path().join(format!("file_{i:02}.txt")), format!("content {i}")).unwrap();
    }

    indexer
        .start(
            "s1".to_string(),
            sid,
            ws.path().to_path_buf(),
            kg_path.clone(),
            WorkspaceClassification::default(),
        )
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let graph = KnowledgeGraph::open(&kg_path).unwrap();
    let files = graph.list_nodes_by_type("workspace_file").unwrap();
    assert_eq!(
        files.len(),
        20,
        "all 20 files should be indexed even with limited concurrency, got {}",
        files.len()
    );

    // All 20 embeddings should have fired
    assert_eq!(cb.count(), 20, "each file should get one embedding call");

    indexer.stop("s1").await;
}

// ── 27. Move directory: all files re-indexed at new paths ───────────────

#[tokio::test]
async fn t27_move_directory_reindexes_contents() {
    let (_dir, kg_path, graph) = temp_graph();
    let sid = session_node(&graph, "test-session");
    drop(graph);

    let cb = Arc::new(TestEmbedCallback::new());
    let indexer = Arc::new(WorkspaceIndexer::with_config(
        Arc::clone(&cb) as Arc<dyn EmbeddingCallback>,
        4,
        100,
        60,
    ));
    let ws = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(ws.path().join("before")).unwrap();
    std::fs::write(ws.path().join("before/a.txt"), "file a").unwrap();
    std::fs::write(ws.path().join("before/b.txt"), "file b").unwrap();

    indexer
        .start(
            "s1".to_string(),
            sid,
            ws.path().to_path_buf(),
            kg_path.clone(),
            WorkspaceClassification::default(),
        )
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(2000)).await;

    // Move the entire directory
    std::fs::rename(ws.path().join("before"), ws.path().join("after")).unwrap();

    let kg = kg_path.clone();
    let found = wait_for(std::time::Duration::from_secs(10), move || {
        let graph = KnowledgeGraph::open(&kg).unwrap();
        let old_gone =
            graph.find_node_by_type_and_name("workspace_file", "before/a.txt").unwrap().is_none();
        let new_a =
            graph.find_node_by_type_and_name("workspace_file", "after/a.txt").unwrap().is_some();
        let new_b =
            graph.find_node_by_type_and_name("workspace_file", "after/b.txt").unwrap().is_some();
        old_gone && new_a && new_b
    })
    .await;
    assert!(found, "directory move should remove old paths and index new paths");

    indexer.stop("s1").await;
}

// ── 28. Custom config constructor ───────────────────────────────────────

#[tokio::test]
async fn t28_with_config_custom_params() {
    let (_dir, kg_path, graph) = temp_graph();
    let sid = session_node(&graph, "test-session");
    drop(graph);

    let cb = Arc::new(TestEmbedCallback::new());
    let indexer = Arc::new(WorkspaceIndexer::with_config(
        Arc::clone(&cb) as Arc<dyn EmbeddingCallback>,
        1,  // single worker
        50, // fast debounce
        60,
    ));
    let ws = tempfile::tempdir().unwrap();
    std::fs::write(ws.path().join("config_test.txt"), "custom config").unwrap();

    indexer
        .start(
            "s1".to_string(),
            sid,
            ws.path().to_path_buf(),
            kg_path.clone(),
            WorkspaceClassification::default(),
        )
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let graph = KnowledgeGraph::open(&kg_path).unwrap();
    let file = graph.find_node_by_type_and_name("workspace_file", "config_test.txt").unwrap();
    assert!(file.is_some(), "custom config indexer should still work");

    indexer.stop("s1").await;
}

// ── 29. Burst of files via initial scan (simulated git checkout) ─────────
// Tests that the initial full_scan correctly indexes a large number of
// pre-existing files (e.g., after a git checkout or clone).

#[tokio::test]
async fn t29_burst_of_files_all_indexed() {
    let (_dir, kg_path, graph) = temp_graph();
    let sid = session_node(&graph, "test-session");
    drop(graph);

    let cb = Arc::new(TestEmbedCallback::new());
    let indexer = Arc::new(WorkspaceIndexer::with_config(
        Arc::clone(&cb) as Arc<dyn EmbeddingCallback>,
        4,
        200,
        60,
    ));
    let ws = tempfile::tempdir().unwrap();

    // Create 50 files BEFORE starting the indexer (simulates git checkout)
    for i in 0..50 {
        std::fs::write(ws.path().join(format!("burst_{i:03}.rs")), format!("fn f{i}() {{}}"))
            .unwrap();
    }

    indexer
        .start(
            "s1".to_string(),
            sid,
            ws.path().to_path_buf(),
            kg_path.clone(),
            WorkspaceClassification::default(),
        )
        .await
        .unwrap();

    // Initial scan should have indexed everything synchronously
    let graph = KnowledgeGraph::open(&kg_path).unwrap();
    let files = graph.list_nodes_by_type("workspace_file").unwrap();
    assert_eq!(
        files.len(),
        50,
        "all 50 burst files should be indexed by initial scan, got {}",
        files.len()
    );
    assert_eq!(cb.count(), 50, "each file should get one embedding call");

    indexer.stop("s1").await;
}

// ── 30. File removed, re-created with different content ─────────────────

#[tokio::test]
async fn t30_remove_and_recreate_file() {
    let (_dir, kg_path, graph) = temp_graph();
    let sid = session_node(&graph, "test-session");
    drop(graph);

    let cb = Arc::new(TestEmbedCallback::new());
    let indexer = Arc::new(WorkspaceIndexer::with_config(
        Arc::clone(&cb) as Arc<dyn EmbeddingCallback>,
        4,
        100,
        60,
    ));
    let ws = tempfile::tempdir().unwrap();
    std::fs::write(ws.path().join("cycle.txt"), "original content").unwrap();

    indexer
        .start(
            "s1".to_string(),
            sid,
            ws.path().to_path_buf(),
            kg_path.clone(),
            WorkspaceClassification::default(),
        )
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(2000)).await;

    // Remove then recreate with different content
    std::fs::remove_file(ws.path().join("cycle.txt")).unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    std::fs::write(ws.path().join("cycle.txt"), "completely new content").unwrap();

    let kg = kg_path.clone();
    let found = wait_for(std::time::Duration::from_secs(10), move || {
        let graph = KnowledgeGraph::open(&kg).unwrap();
        let files = graph.list_nodes_by_type("workspace_file").unwrap();
        let cycle_files: Vec<_> = files.iter().filter(|f| f.name == "cycle.txt").collect();
        if cycle_files.len() != 1 {
            return false;
        }
        let chunks = graph.list_nodes_by_type("file_chunk").unwrap();
        let has_new =
            chunks.iter().any(|c| c.content.as_ref().is_some_and(|t| t.contains("completely new")));
        let has_old = chunks
            .iter()
            .any(|c| c.content.as_ref().is_some_and(|t| t.contains("original content")));
        has_new && !has_old
    })
    .await;
    assert!(found, "cycle.txt should exist with new content and old content should be gone");

    indexer.stop("s1").await;
}

// ── 31. Copy file: both original and copy should be in the graph ────────

#[tokio::test]
async fn t31_copy_file_keeps_both_in_graph() {
    let (_dir, kg_path, graph) = temp_graph();
    let sid = session_node(&graph, "test-session");
    drop(graph);

    let cb = Arc::new(TestEmbedCallback::new());
    let indexer = Arc::new(WorkspaceIndexer::with_config(
        Arc::clone(&cb) as Arc<dyn EmbeddingCallback>,
        4,
        100,
        60,
    ));
    let ws = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(ws.path().join("src")).unwrap();
    std::fs::create_dir_all(ws.path().join("backup")).unwrap();
    std::fs::write(ws.path().join("src/original.txt"), "shared content").unwrap();

    indexer
        .start(
            "s1".to_string(),
            sid,
            ws.path().to_path_buf(),
            kg_path.clone(),
            WorkspaceClassification::default(),
        )
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(2000)).await;

    // Copy (not move) the file to another directory
    std::fs::copy(ws.path().join("src/original.txt"), ws.path().join("backup/original.txt"))
        .unwrap();

    // Wait for watcher to detect the copy
    let kg = kg_path.clone();
    let found = wait_for(std::time::Duration::from_secs(10), move || {
        let graph = KnowledgeGraph::open(&kg).unwrap();
        let orig_exists = graph
            .find_node_by_type_and_name("workspace_file", "src/original.txt")
            .unwrap()
            .is_some();
        let copy_exists = graph
            .find_node_by_type_and_name("workspace_file", "backup/original.txt")
            .unwrap()
            .is_some();
        orig_exists && copy_exists
    })
    .await;
    assert!(found, "both original and copied file should be indexed");

    let graph = KnowledgeGraph::open(&kg_path).unwrap();

    // Both should have chunks
    let all_files = graph.list_nodes_by_type("workspace_file").unwrap();
    assert_eq!(all_files.len(), 2, "should have exactly 2 file nodes");

    indexer.stop("s1").await;
}
