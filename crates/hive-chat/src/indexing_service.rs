use hive_classification::DataClass;
use hive_contracts::{FileAuditRecord, WorkspaceClassification};
use hive_inference::RuntimeManager;
use hive_knowledge::KgPool;
use hive_workspace_index::WorkspaceIndexer;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::chat::{open_graph, ChatServiceError, ReindexProgress};

/// Maximum number of file audit records retained per session.
#[allow(dead_code)]
const MAX_FILE_AUDITS_PER_SESSION: usize = 10_000;

/// Manages workspace indexing, embeddings, file classifications, and file audits.
#[derive(Clone)]
pub(crate) struct IndexingService {
    pub(crate) workspace_indexer: Arc<WorkspaceIndexer>,
    pub(crate) runtime_manager: Arc<Mutex<Option<Arc<RuntimeManager>>>>,
    pub(crate) workspace_classifications: Arc<Mutex<HashMap<String, WorkspaceClassification>>>,
    pub(crate) file_audits: Arc<Mutex<HashMap<String, HashMap<String, FileAuditRecord>>>>,
    pub(crate) knowledge_graph_path: Arc<PathBuf>,
    pub(crate) kg_pool: Arc<KgPool>,
}

impl IndexingService {
    /// Set the inference runtime manager for embedding-based clustering.
    pub fn set_runtime_manager(&self, runtime: Arc<RuntimeManager>) {
        *self.runtime_manager.lock() = Some(runtime);
    }

    /// Reindex all node embeddings with a new model.
    pub async fn reindex_embeddings(
        &self,
        new_model_id: String,
        new_dimensions: usize,
        progress_tx: tokio::sync::broadcast::Sender<ReindexProgress>,
    ) -> Result<(), ChatServiceError> {
        let runtime =
            self.runtime_manager.lock().clone().ok_or_else(|| ChatServiceError::Internal {
                detail: "no inference runtime available for reindexing".into(),
            })?;

        let pool = Arc::clone(&self.kg_pool);
        let model_id = new_model_id.clone();

        tokio::task::spawn(async move {
            // Acquire the write guard (serializes with other writers)
            let mut guard = match pool.write().await {
                Ok(g) => g,
                Err(e) => {
                    tracing::error!(error = %e, "failed to acquire KG write guard for reindex");
                    return;
                }
            };

            let result = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
                guard.prepare_reindex(&model_id, new_dimensions)?;

                const BATCH_SIZE: usize = 100;
                let total = guard.node_count()? as usize;
                let mut done = 0usize;
                let _ = progress_tx.send(ReindexProgress { done, total });

                loop {
                    let batch = guard.nodes_needing_embedding(
                        &model_id,
                        &["file_chunk", "chat_message", "memory"],
                        BATCH_SIZE,
                    )?;
                    if batch.is_empty() {
                        break;
                    }

                    for node_id in batch {
                        let text = guard
                            .get_node(node_id)?
                            .and_then(|n| n.content.or(Some(n.name)))
                            .unwrap_or_default();

                        match runtime.embed(&model_id, &text) {
                            Ok(embedding) => {
                                if let Err(e) = guard.set_embedding(node_id, &embedding, &model_id)
                                {
                                    tracing::warn!(
                                        node_id,
                                        error = %e,
                                        "failed to store embedding during reindex"
                                    );
                                }
                            }
                            Err(e) => {
                                tracing::warn!(
                                    node_id,
                                    error = %e,
                                    "failed to embed node during reindex"
                                );
                            }
                        }

                        done += 1;
                        let _ = progress_tx.send(ReindexProgress { done, total });
                    }
                }

                let _ = progress_tx.send(ReindexProgress { done, total });
                Ok(())
            })
            .await;

            match result {
                Ok(Ok(())) => tracing::info!("embedding reindex completed"),
                Ok(Err(e)) => tracing::error!(error = %e, "embedding reindex failed"),
                Err(e) => tracing::error!(error = %e, "embedding reindex task panicked"),
            }
        });

        Ok(())
    }

    /// Get embedding statistics for the knowledge graph.
    pub async fn embedding_stats(
        &self,
        model_id: &str,
    ) -> Result<hive_knowledge::EmbeddingStats, ChatServiceError> {
        let graph_path = Arc::clone(&self.knowledge_graph_path);
        let model_id = model_id.to_string();
        tokio::task::spawn_blocking(move || {
            let graph = open_graph(&graph_path)?;
            graph.embedding_stats(&model_id, &["file_chunk", "chat_message", "memory"]).map_err(
                |e| ChatServiceError::KnowledgeGraphFailed {
                    operation: "embedding_stats",
                    detail: e.to_string(),
                },
            )
        })
        .await
        .map_err(|e| ChatServiceError::KnowledgeGraphFailed {
            operation: "embedding_stats",
            detail: e.to_string(),
        })?
    }

    // ── Workspace classification ───────────────────────────

    pub fn get_workspace_classification(&self, session_id: &str) -> WorkspaceClassification {
        let classifications = self.workspace_classifications.lock();
        classifications.get(session_id).cloned().unwrap_or_default()
    }

    pub fn set_workspace_classification_default(&self, session_id: &str, default: DataClass) {
        let mut classifications = self.workspace_classifications.lock();
        let wc = classifications.entry(session_id.to_string()).or_default();
        wc.default = default;
        let new_class = wc.clone();
        drop(classifications);
        self.notify_workspace_reclass(session_id, new_class);
    }

    pub fn set_classification_override(&self, session_id: &str, path: &str, class: DataClass) {
        let mut classifications = self.workspace_classifications.lock();
        let wc = classifications.entry(session_id.to_string()).or_default();
        wc.set_override(path, class);
        let new_class = wc.clone();
        drop(classifications);
        self.notify_workspace_reclass(session_id, new_class);
    }

    pub fn clear_classification_override(&self, session_id: &str, path: &str) -> bool {
        let mut classifications = self.workspace_classifications.lock();
        let result = if let Some(wc) = classifications.get_mut(session_id) {
            wc.clear_override(path)
        } else {
            false
        };
        if result {
            if let Some(wc) = classifications.get(session_id) {
                let new_class = wc.clone();
                drop(classifications);
                self.notify_workspace_reclass(session_id, new_class);
            }
        }
        result
    }

    fn notify_workspace_reclass(&self, session_id: &str, classification: WorkspaceClassification) {
        let indexer = Arc::clone(&self.workspace_indexer);
        let sid = session_id.to_string();
        tokio::spawn(async move {
            indexer.reclass_session(&sid, classification).await;
        });
    }

    // ── Workspace index status ─────────────────────────────

    pub async fn subscribe_index_status(
        &self,
        session_id: &str,
    ) -> Option<tokio::sync::broadcast::Receiver<hive_workspace_index::FileIndexStatus>> {
        self.workspace_indexer.subscribe_index_status(session_id).await
    }

    pub async fn indexed_files(&self, session_id: &str) -> Vec<String> {
        self.workspace_indexer.indexed_files(session_id).await
    }

    pub async fn reindex_file(&self, session_id: &str, path: &str) {
        self.workspace_indexer.reindex_file(session_id, path).await;
    }

    pub fn resolve_file_classification(&self, session_id: &str, path: &str) -> DataClass {
        let classifications = self.workspace_classifications.lock();
        classifications.get(session_id).map(|wc| wc.resolve(path)).unwrap_or(DataClass::Internal)
    }

    // ── File audits ────────────────────────────────────────

    #[allow(dead_code)]
    pub fn record_file_audit(&self, session_id: &str, path: &str, record: FileAuditRecord) {
        let mut audits = self.file_audits.lock();
        let session_audits = audits.entry(session_id.to_string()).or_default();
        if session_audits.len() >= MAX_FILE_AUDITS_PER_SESSION {
            let cutoff = MAX_FILE_AUDITS_PER_SESSION / 4;
            let mut entries: Vec<_> =
                session_audits.iter().map(|(k, v)| (k.clone(), v.audited_at_ms)).collect();
            entries.sort_by_key(|(_, ts)| *ts);
            for (key, _) in entries.into_iter().take(cutoff) {
                session_audits.remove(&key);
            }
        }
        session_audits.insert(path.to_string(), record);
    }

    #[allow(dead_code)]
    pub fn get_cached_file_audit(
        &self,
        session_id: &str,
        path: &str,
        content_hash: &str,
    ) -> Option<FileAuditRecord> {
        let audits = self.file_audits.lock();
        audits
            .get(session_id)
            .and_then(|session_audits| session_audits.get(path))
            .cloned()
            .filter(|record| record.content_hash == content_hash)
    }

    #[allow(dead_code)]
    pub fn get_file_audit_record(&self, session_id: &str, path: &str) -> Option<FileAuditRecord> {
        let audits = self.file_audits.lock();
        audits.get(session_id).and_then(|session_audits| session_audits.get(path)).cloned()
    }

    #[allow(dead_code)]
    pub fn get_session_audits(&self, session_id: &str) -> HashMap<String, FileAuditRecord> {
        let audits = self.file_audits.lock();
        audits.get(session_id).cloned().unwrap_or_default()
    }

    /// Fire-and-forget embedding of a knowledge-graph node.
    ///
    /// If `model_id` is `None`, the default embedding model is used.
    #[allow(dead_code)]
    pub fn embed_node_async(&self, node_id: i64, text: String, model_id: Option<String>) {
        let runtime = self.runtime_manager.lock().clone();
        let pool = Arc::clone(&self.kg_pool);
        let model = model_id
            .unwrap_or_else(|| hive_inference::defaults::DEFAULT_EMBEDDING_MODEL_ID.to_string());
        tokio::task::spawn(async move {
            let Some(rt) = runtime else {
                return;
            };
            // Acquire write guard before spawn_blocking
            let guard = match pool.write().await {
                Ok(g) => g,
                Err(e) => {
                    tracing::warn!(node_id, error = %e, "failed to acquire KG write guard for embed");
                    return;
                }
            };
            let embed_model = model.clone();
            let store_model = model;
            let result = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
                let embedding =
                    rt.embed(&embed_model, &text).map_err(|e| anyhow::anyhow!("{e}"))?;
                guard.set_embedding(node_id, &embedding, &store_model)?;
                Ok(())
            })
            .await;

            match result {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    tracing::warn!(
                        node_id,
                        error = %e,
                        "embedding generation failed (will be retried on reindex)"
                    );
                }
                Err(e) => {
                    tracing::warn!(node_id, error = %e, "embedding task panicked");
                }
            }
        });
    }
}
