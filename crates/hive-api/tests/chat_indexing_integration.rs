//! Integration tests for the chat–knowledge-graph indexing system.
//!
//! Tests range from simple (session CRUD, message storage) to complex
//! (KB scrub, knowledge query tool, workspace indexer lifecycle, recall,
//! classification propagation).

use hive_api::canvas_ws::CanvasSessionRegistry;
use hive_api::{ChatRuntimeConfig, ChatService};
use hive_classification::DataClass;
use hive_contracts::{SendMessageRequest, SendMessageResponse, SessionModality};
use hive_core::{AuditLogger, EventBus, HiveMindConfig};
use hive_knowledge::{KnowledgeGraph, NewNode};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

// ── Helpers ──────────────────────────────────────────────────────────────

fn test_chat_service(tempdir: &TempDir) -> Arc<ChatService> {
    let graph_path = tempdir.path().join("knowledge.db");
    let root = tempdir.path().to_path_buf();
    Arc::new(ChatService::new(
        AuditLogger::new(root.join("audit.log")).expect("audit logger"),
        EventBus::new(32),
        ChatRuntimeConfig { step_delay: Duration::from_millis(1), ..ChatRuntimeConfig::default() },
        root.clone(),
        graph_path,
        HiveMindConfig::default().security.prompt_injection.clone(),
        root.join("risk-ledger.db"),
        CanvasSessionRegistry::new(),
    ))
}

fn test_chat_service_with_graph_path(graph_path: PathBuf) -> Arc<ChatService> {
    let root = graph_path.parent().expect("parent").to_path_buf();
    Arc::new(ChatService::new(
        AuditLogger::new(root.join("audit.log")).expect("audit logger"),
        EventBus::new(32),
        ChatRuntimeConfig { step_delay: Duration::from_millis(1), ..ChatRuntimeConfig::default() },
        root.clone(),
        graph_path,
        HiveMindConfig::default().security.prompt_injection.clone(),
        root.join("risk-ledger.db"),
        CanvasSessionRegistry::new(),
    ))
}

// ── 1. Simple: creating a session creates a KG node ─────────────────────

#[tokio::test]
async fn t01_create_session_creates_kg_node() {
    let tempdir = tempfile::tempdir().unwrap();
    let service = test_chat_service(&tempdir);

    let session = service
        .create_session(SessionModality::Linear, Some("Test Session".into()), None)
        .await
        .unwrap();

    let graph = KnowledgeGraph::open(tempdir.path().join("knowledge.db")).unwrap();
    let node = graph.find_node_by_type_and_name("chat_session", &session.id).unwrap();
    assert!(node.is_some(), "session node should exist in KG");
}

// ── 2. Simple: get_session returns the correct session ──────────────────

#[tokio::test]
async fn t02_get_session_returns_correct_data() {
    let tempdir = tempfile::tempdir().unwrap();
    let service = test_chat_service(&tempdir);

    let session = service
        .create_session(SessionModality::Linear, Some("My Chat".into()), None)
        .await
        .unwrap();

    let fetched = service.get_session(&session.id).await.unwrap();
    assert_eq!(fetched.id, session.id);
    assert_eq!(fetched.title, "My Chat");
    assert_eq!(fetched.modality, SessionModality::Linear);
}

// ── 3. Simple: delete session removes KG node ───────────────────────────

#[tokio::test]
async fn t03_delete_session_removes_kg_node() {
    let tempdir = tempfile::tempdir().unwrap();
    let service = test_chat_service(&tempdir);

    let session = service.create_session(SessionModality::Linear, None, None).await.unwrap();
    let sid = session.id.clone();

    service.delete_session(&sid, false).await.unwrap();

    let graph = KnowledgeGraph::open(tempdir.path().join("knowledge.db")).unwrap();
    let node = graph.find_node_by_type_and_name("chat_session", &sid).unwrap();
    assert!(node.is_none(), "session node should be removed from KG");
}

// ── 4. Simple: enqueue message stores message in session ────────────────

#[tokio::test]
async fn t04_enqueue_message_stores_in_session() {
    let tempdir = tempfile::tempdir().unwrap();
    let service = test_chat_service(&tempdir);

    let session = service.create_session(SessionModality::Linear, None, None).await.unwrap();

    let resp = service
        .enqueue_message(
            &session.id,
            SendMessageRequest {
                content: "Hello from the user".into(),
                scan_decision: None,
                preferred_models: None,
                data_class_override: None,
                agent_id: None,
                role: Default::default(),
                canvas_position: None,
                excluded_tools: None,
                excluded_skills: None,
                attachments: vec![],
                skip_preempt: None,
            },
        )
        .await
        .unwrap();

    let snap = match resp {
        SendMessageResponse::Queued { session } => session,
        other => panic!("expected Queued, got {other:?}"),
    };
    assert_eq!(snap.messages.len(), 1);
    assert_eq!(snap.messages[0].content, "Hello from the user");
}

// ── 5. Simple: enqueue creates message node in KG ───────────────────────

#[tokio::test]
async fn t05_enqueue_creates_message_node_in_kg() {
    let tempdir = tempfile::tempdir().unwrap();
    let service = test_chat_service(&tempdir);

    let session = service.create_session(SessionModality::Linear, None, None).await.unwrap();

    service
        .enqueue_message(
            &session.id,
            SendMessageRequest {
                content: "KG indexed message".into(),
                scan_decision: None,
                preferred_models: None,
                data_class_override: None,
                agent_id: None,
                role: Default::default(),
                canvas_position: None,
                excluded_tools: None,
                excluded_skills: None,
                attachments: vec![],
                skip_preempt: None,
            },
        )
        .await
        .unwrap();

    let graph = KnowledgeGraph::open(tempdir.path().join("knowledge.db")).unwrap();
    let messages = graph.list_nodes_by_type("chat_message").unwrap();
    assert!(!messages.is_empty(), "message node should exist in KG");

    // The message content should be FTS-searchable
    let search = graph.search_text("indexed message", 10).unwrap();
    assert!(!search.is_empty(), "message should be discoverable via FTS5 search");
}

// ── 6. Simple: delete session without scrub keeps messages ──────────────

#[tokio::test]
async fn t06_delete_without_scrub_keeps_messages_in_kg() {
    let tempdir = tempfile::tempdir().unwrap();
    let service = test_chat_service(&tempdir);

    let session = service.create_session(SessionModality::Linear, None, None).await.unwrap();
    service
        .enqueue_message(
            &session.id,
            SendMessageRequest {
                content: "Persisted memory data".into(),
                scan_decision: None,
                preferred_models: None,
                data_class_override: None,
                agent_id: None,
                role: Default::default(),
                canvas_position: None,
                excluded_tools: None,
                excluded_skills: None,
                attachments: vec![],
                skip_preempt: None,
            },
        )
        .await
        .unwrap();

    // Delete WITHOUT scrub — messages should remain (orphaned, but searchable)
    service.delete_session(&session.id, false).await.unwrap();

    let graph = KnowledgeGraph::open(tempdir.path().join("knowledge.db")).unwrap();
    let messages = graph.list_nodes_by_type("chat_message").unwrap();
    assert!(!messages.is_empty(), "messages should still exist when scrub_kb=false");
}

// ── 7. Moderate: delete session with scrub removes messages ─────────────

#[tokio::test]
async fn t07_delete_with_scrub_removes_messages_from_kg() {
    let tempdir = tempfile::tempdir().unwrap();
    let service = test_chat_service(&tempdir);

    let session = service.create_session(SessionModality::Linear, None, None).await.unwrap();
    service
        .enqueue_message(
            &session.id,
            SendMessageRequest {
                content: "Scrubable message content".into(),
                scan_decision: None,
                preferred_models: None,
                data_class_override: None,
                agent_id: None,
                role: Default::default(),
                canvas_position: None,
                excluded_tools: None,
                excluded_skills: None,
                attachments: vec![],
                skip_preempt: None,
            },
        )
        .await
        .unwrap();

    // Delete WITH scrub — messages should be removed
    service.delete_session(&session.id, true).await.unwrap();

    let graph = KnowledgeGraph::open(tempdir.path().join("knowledge.db")).unwrap();
    let messages = graph.list_nodes_by_type("chat_message").unwrap();
    assert!(
        messages.is_empty(),
        "messages should be removed when scrub_kb=true, found {}",
        messages.len()
    );

    // FTS search should also return nothing
    let search = graph.search_text("Scrubable", 10).unwrap();
    assert!(search.is_empty(), "scrubbed messages should not appear in search");
}

// ── 8. Moderate: multiple sessions are independent ──────────────────────

#[tokio::test]
async fn t08_multiple_sessions_independent() {
    let tempdir = tempfile::tempdir().unwrap();
    let service = test_chat_service(&tempdir);

    let s1 = service
        .create_session(SessionModality::Linear, Some("Session 1".into()), None)
        .await
        .unwrap();
    let s2 = service
        .create_session(SessionModality::Linear, Some("Session 2".into()), None)
        .await
        .unwrap();

    service
        .enqueue_message(
            &s1.id,
            SendMessageRequest {
                content: "Message for session one".into(),
                scan_decision: None,
                preferred_models: None,
                data_class_override: None,
                agent_id: None,
                role: Default::default(),
                canvas_position: None,
                excluded_tools: None,
                excluded_skills: None,
                attachments: vec![],
                skip_preempt: None,
            },
        )
        .await
        .unwrap();

    // Session 2 should have no messages
    let s2_snap = service.get_session(&s2.id).await.unwrap();
    assert!(s2_snap.messages.is_empty(), "session 2 should have no messages");

    // Deleting session 1 should not affect session 2
    service.delete_session(&s1.id, true).await.unwrap();
    let s2_snap = service.get_session(&s2.id).await.unwrap();
    assert_eq!(s2_snap.title, "Session 2");
}

// ── 9. Moderate: workspace classification default ───────────────────────

#[tokio::test]
async fn t09_workspace_classification_default() {
    let tempdir = tempfile::tempdir().unwrap();
    let service = test_chat_service(&tempdir);

    let session = service.create_session(SessionModality::Linear, None, None).await.unwrap();

    // Default classification should be Internal
    let class = service.get_workspace_classification(&session.id);
    assert_eq!(class.default, DataClass::Internal);

    // Change default and verify
    service.set_workspace_classification_default(&session.id, DataClass::Public);
    let class = service.get_workspace_classification(&session.id);
    assert_eq!(class.default, DataClass::Public);
}

// ── 10. Moderate: classification override and resolution ────────────────

#[tokio::test]
async fn t10_classification_override_and_resolve() {
    let tempdir = tempfile::tempdir().unwrap();
    let service = test_chat_service(&tempdir);

    let session = service.create_session(SessionModality::Linear, None, None).await.unwrap();

    service.set_workspace_classification_default(&session.id, DataClass::Public);
    service.set_classification_override(&session.id, "secrets", DataClass::Restricted);

    assert_eq!(service.resolve_file_classification(&session.id, "readme.md"), DataClass::Public);
    assert_eq!(
        service.resolve_file_classification(&session.id, "secrets/keys.txt"),
        DataClass::Restricted
    );
}

// ── 11. Moderate: clear classification override ─────────────────────────

#[tokio::test]
async fn t11_clear_classification_override() {
    let tempdir = tempfile::tempdir().unwrap();
    let service = test_chat_service(&tempdir);

    let session = service.create_session(SessionModality::Linear, None, None).await.unwrap();

    service.set_workspace_classification_default(&session.id, DataClass::Public);
    service.set_classification_override(&session.id, "src", DataClass::Confidential);

    assert_eq!(
        service.resolve_file_classification(&session.id, "src/main.rs"),
        DataClass::Confidential
    );

    assert!(service.clear_classification_override(&session.id, "src"));
    assert_eq!(service.resolve_file_classification(&session.id, "src/main.rs"), DataClass::Public);
}

// ── 12. Moderate: session restore from KG ───────────────────────────────

#[tokio::test]
async fn t12_session_restore_from_kg() {
    let tempdir = tempfile::tempdir().unwrap();
    let graph_path = tempdir.path().join("knowledge.db");

    // Seed a session in the KG directly
    let graph = KnowledgeGraph::open(&graph_path).unwrap();
    let metadata = serde_json::json!({
        "title": "Restored Session",
        "modality": "linear",
        "workspace_path": "",
        "workspace_linked": false,
        "created_at_ms": 100,
        "updated_at_ms": 200,
        "permissions": []
    });
    graph
        .insert_node(&NewNode {
            node_type: "chat_session".to_string(),
            name: "session-42".to_string(),
            data_class: DataClass::Internal,
            content: Some(metadata.to_string()),
        })
        .unwrap();
    drop(graph);

    let service = test_chat_service_with_graph_path(graph_path);
    service.restore_sessions().await.unwrap();

    let restored = service.get_session("session-42").await.unwrap();
    assert_eq!(restored.title, "Restored Session");
    assert_eq!(restored.modality, SessionModality::Linear);
}

// ── 13. Moderate: enqueue updates session title ─────────────────────────

#[tokio::test]
async fn t13_enqueue_message_updates_session_title() {
    let tempdir = tempfile::tempdir().unwrap();
    let service = test_chat_service(&tempdir);

    let session = service.create_session(SessionModality::Linear, None, None).await.unwrap();
    assert_eq!(session.title, "New session");

    let resp = service
        .enqueue_message(
            &session.id,
            SendMessageRequest {
                content: "Implement the parser module".into(),
                scan_decision: None,
                preferred_models: None,
                data_class_override: None,
                agent_id: None,
                role: Default::default(),
                canvas_position: None,
                excluded_tools: None,
                excluded_skills: None,
                attachments: vec![],
                skip_preempt: None,
            },
        )
        .await
        .unwrap();

    let snap = match resp {
        SendMessageResponse::Queued { session } => session,
        other => panic!("expected Queued, got {other:?}"),
    };
    // Title should be updated from the first message content
    assert_ne!(snap.title, "New session", "title should change after first message");
}

// ── 14. Moderate: session workspace directory created ────────────────────

#[tokio::test]
async fn t14_workspace_directory_created() {
    let tempdir = tempfile::tempdir().unwrap();
    let service = test_chat_service(&tempdir);

    let session = service.create_session(SessionModality::Linear, None, None).await.unwrap();

    let workspace = PathBuf::from(&session.workspace_path);
    assert!(workspace.exists(), "workspace directory should be created");
    assert!(workspace.is_dir(), "workspace should be a directory");
}

// ── 15. Complex: deleting session removes unlinked workspace ────────────

#[tokio::test]
async fn t15_delete_removes_unlinked_workspace() {
    let tempdir = tempfile::tempdir().unwrap();
    let service = test_chat_service(&tempdir);

    let session = service.create_session(SessionModality::Linear, None, None).await.unwrap();
    let workspace = PathBuf::from(&session.workspace_path);
    assert!(workspace.exists());

    // Create a file in the workspace
    std::fs::write(workspace.join("note.txt"), "test data").unwrap();

    service.delete_session(&session.id, false).await.unwrap();
    assert!(!workspace.exists(), "unlinked workspace should be deleted with session");
}

// ── 16. Complex: scrub with multiple messages ───────────────────────────

#[tokio::test]
async fn t16_scrub_multiple_messages() {
    let tempdir = tempfile::tempdir().unwrap();
    let service = test_chat_service(&tempdir);

    let session = service.create_session(SessionModality::Linear, None, None).await.unwrap();

    for i in 0..5 {
        service
            .enqueue_message(
                &session.id,
                SendMessageRequest {
                    content: format!("Message number {i} for bulk scrub"),
                    scan_decision: None,
                    preferred_models: None,
                    data_class_override: None,
                    agent_id: None,
                    role: Default::default(),
                    canvas_position: None,
                    excluded_tools: None,
                    excluded_skills: None,
                    attachments: vec![],
                    skip_preempt: None,
                },
            )
            .await
            .unwrap();
    }

    let graph = KnowledgeGraph::open(tempdir.path().join("knowledge.db")).unwrap();
    let before_count = graph.list_nodes_by_type("chat_message").unwrap().len();
    assert!(before_count >= 5, "should have at least 5 message nodes");
    drop(graph);

    service.delete_session(&session.id, true).await.unwrap();

    let graph = KnowledgeGraph::open(tempdir.path().join("knowledge.db")).unwrap();
    let after_count = graph.list_nodes_by_type("chat_message").unwrap().len();
    assert_eq!(after_count, 0, "all messages should be scrubbed");
}

// ── 17. Complex: scrub one session preserves another ────────────────────

#[tokio::test]
async fn t17_scrub_preserves_other_sessions() {
    let tempdir = tempfile::tempdir().unwrap();
    let service = test_chat_service(&tempdir);

    let s1 = service
        .create_session(SessionModality::Linear, Some("To Scrub".into()), None)
        .await
        .unwrap();
    let s2 = service
        .create_session(SessionModality::Linear, Some("To Keep".into()), None)
        .await
        .unwrap();

    service
        .enqueue_message(
            &s1.id,
            SendMessageRequest {
                content: "Scrub me".into(),
                scan_decision: None,
                preferred_models: None,
                data_class_override: None,
                agent_id: None,
                role: Default::default(),
                canvas_position: None,
                excluded_tools: None,
                excluded_skills: None,
                attachments: vec![],
                skip_preempt: None,
            },
        )
        .await
        .unwrap();

    service
        .enqueue_message(
            &s2.id,
            SendMessageRequest {
                content: "Keep me in knowledge base".into(),
                scan_decision: None,
                preferred_models: None,
                data_class_override: None,
                agent_id: None,
                role: Default::default(),
                canvas_position: None,
                excluded_tools: None,
                excluded_skills: None,
                attachments: vec![],
                skip_preempt: None,
            },
        )
        .await
        .unwrap();

    // Scrub session 1
    service.delete_session(&s1.id, true).await.unwrap();

    // Session 2's messages should still exist
    let graph = KnowledgeGraph::open(tempdir.path().join("knowledge.db")).unwrap();
    let messages = graph.list_nodes_by_type("chat_message").unwrap();
    assert!(!messages.is_empty(), "session 2 messages should survive scrub of session 1");

    // FTS search should still find session 2's content
    let search = graph.search_text("Keep knowledge", 10).unwrap();
    assert!(!search.is_empty(), "session 2 messages should be searchable after scrub of session 1");
}

// ── 18. Complex: workspace indexer starts for new session ───────────────

#[tokio::test]
async fn t18_workspace_indexer_starts_on_create() {
    let tempdir = tempfile::tempdir().unwrap();
    let service = test_chat_service(&tempdir);

    let session = service.create_session(SessionModality::Linear, None, None).await.unwrap();

    // Write a file to the workspace
    let workspace = PathBuf::from(&session.workspace_path);
    std::fs::write(workspace.join("notes.md"), "# Integration test notes\nThis is a test.")
        .unwrap();

    // Explicitly trigger reindex so we don't depend on OS file-watcher timing
    service.reindex_file(&session.id, "notes.md").await;

    let graph = KnowledgeGraph::open(tempdir.path().join("knowledge.db")).unwrap();

    // The workspace root should exist (created by initial full_scan)
    let root = graph.find_node_by_type_and_name("workspace_dir", "/").unwrap();
    assert!(root.is_some(), "workspace root node should be created by indexer");

    // The file should be indexed after explicit reindex
    let file = graph.find_node_by_type_and_name("workspace_file", "notes.md").unwrap();
    assert!(file.is_some(), "workspace file should be indexed");
}

// ── 19. Complex: workspace indexer stops on session delete ──────────────

#[tokio::test]
async fn t19_workspace_indexer_stops_on_delete() {
    let tempdir = tempfile::tempdir().unwrap();
    let service = test_chat_service(&tempdir);

    let session = service.create_session(SessionModality::Linear, None, None).await.unwrap();
    let workspace = PathBuf::from(&session.workspace_path);

    // Give indexer time to start
    tokio::time::sleep(Duration::from_millis(200)).await;

    service.delete_session(&session.id, false).await.unwrap();

    // After deletion, the workspace should not exist (cleaned up)
    assert!(!workspace.exists(), "workspace should be cleaned up after delete");
}

// ── 20. Complex: KG search after multiple sessions and messages ─────────

#[tokio::test]
async fn t20_kg_search_across_sessions() {
    let tempdir = tempfile::tempdir().unwrap();
    let service = test_chat_service(&tempdir);

    let s1 = service
        .create_session(SessionModality::Linear, Some("Rust Project".into()), None)
        .await
        .unwrap();
    let s2 = service
        .create_session(SessionModality::Linear, Some("Python Project".into()), None)
        .await
        .unwrap();

    service
        .enqueue_message(
            &s1.id,
            SendMessageRequest {
                content: "Implement async trait bounds for the parser".into(),
                scan_decision: None,
                preferred_models: None,
                data_class_override: None,
                agent_id: None,
                role: Default::default(),
                canvas_position: None,
                excluded_tools: None,
                excluded_skills: None,
                attachments: vec![],
                skip_preempt: None,
            },
        )
        .await
        .unwrap();

    service
        .enqueue_message(
            &s2.id,
            SendMessageRequest {
                content: "Create a Django middleware for authentication".into(),
                scan_decision: None,
                preferred_models: None,
                data_class_override: None,
                agent_id: None,
                role: Default::default(),
                canvas_position: None,
                excluded_tools: None,
                excluded_skills: None,
                attachments: vec![],
                skip_preempt: None,
            },
        )
        .await
        .unwrap();

    let graph = KnowledgeGraph::open(tempdir.path().join("knowledge.db")).unwrap();

    // Search for Rust-related content
    let rust_results = graph.search_text("async trait parser", 10).unwrap();
    assert!(!rust_results.is_empty(), "should find Rust session messages");

    // Search for Python-related content
    let python_results = graph.search_text("Django middleware", 10).unwrap();
    assert!(!python_results.is_empty(), "should find Python session messages");

    // Scrub session 1, Python results should remain
    service.delete_session(&s1.id, true).await.unwrap();
    let python_after = graph.search_text("Django middleware", 10).unwrap();
    assert!(!python_after.is_empty(), "Python messages should survive Rust session scrub");
}
