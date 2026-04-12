use std::path::Path;

use hive_classification::DataClass;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use crate::KnowledgeGraph;

const MEMORY_NODE_TYPE: &str = "memory";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryEntry {
    pub key: String,
    pub content: String,
    pub data_class: DataClass,
}

/// Abstract storage backend for agent memory.
pub trait MemoryStore {
    /// Store a memory. If a memory with the same key exists, update it.
    fn remember(&self, key: &str, content: &str, data_class: DataClass) -> anyhow::Result<()>;

    /// Recall memories matching a text query. Returns up to `limit` results.
    fn recall(&self, query: &str, limit: usize) -> anyhow::Result<Vec<MemoryEntry>>;

    /// Forget (delete) a memory by key. Returns true if a memory was removed.
    fn forget(&self, key: &str) -> anyhow::Result<bool>;

    /// List all stored memories, up to `limit`.
    fn list(&self, limit: usize) -> anyhow::Result<Vec<MemoryEntry>>;

    /// Count stored memories.
    fn count(&self) -> anyhow::Result<usize>;
}

pub struct MemoryManager {
    graph: KnowledgeGraph,
}

impl MemoryManager {
    pub fn new(graph: KnowledgeGraph) -> Self {
        Self { graph }
    }

    pub fn open(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        Ok(Self::new(KnowledgeGraph::open(path)?))
    }

    pub fn open_in_memory() -> anyhow::Result<Self> {
        Ok(Self::new(KnowledgeGraph::open_in_memory()?))
    }

    /// Store a memory and also set its embedding vector.
    /// This combines `remember()` with `set_embedding()` in a single call.
    /// If the embedding fails, the memory is still stored (FTS5 searchable).
    pub fn remember_with_embedding(
        &self,
        key: &str,
        content: &str,
        data_class: DataClass,
        embedding: &[f32],
        model_id: &str,
    ) -> anyhow::Result<()> {
        // Use unchecked_transaction to work with &self reference.
        let tx = self.graph.connection.unchecked_transaction()?;

        let existing: Option<i64> = tx
            .query_row(
                "SELECT id FROM nodes WHERE node_type = ?1 AND name = ?2",
                rusqlite::params![MEMORY_NODE_TYPE, key],
                |row| row.get(0),
            )
            .optional()?;

        if let Some(existing_id) = existing {
            tx.execute(
                "DELETE FROM edges WHERE source_id = ?1 OR target_id = ?1",
                rusqlite::params![existing_id],
            )?;
            tx.execute("DELETE FROM nodes WHERE id = ?1", rusqlite::params![existing_id])?;
        }

        tx.execute(
            "INSERT INTO nodes (node_type, name, data_class, content) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![MEMORY_NODE_TYPE, key, data_class.to_i64(), Some(content)],
        )?;

        let node_id = tx.last_insert_rowid();
        tx.commit()?;

        if let Err(e) = self.graph.set_embedding(node_id, embedding, model_id) {
            // Use remove_node which properly cleans up embedding_meta + vec
            // table rows, unlike raw SQL deletes.
            let _ = self.graph.remove_node(node_id);
            return Err(e);
        }

        Ok(())
    }

    /// Returns a reference to the underlying graph for embedding operations.
    pub fn graph(&self) -> &KnowledgeGraph {
        &self.graph
    }
}

impl MemoryStore for MemoryManager {
    fn remember(&self, key: &str, content: &str, data_class: DataClass) -> anyhow::Result<()> {
        // Use unchecked_transaction to work with &self reference.
        // Safety: MemoryManager is single-writer via the calling code.
        let tx = self.graph.connection.unchecked_transaction()?;

        // Remove existing node with the same key inside the transaction.
        let existing: Option<i64> = tx
            .query_row(
                "SELECT id FROM nodes WHERE node_type = ?1 AND name = ?2",
                rusqlite::params![MEMORY_NODE_TYPE, key],
                |row| row.get(0),
            )
            .optional()?;

        if let Some(existing_id) = existing {
            tx.execute(
                "DELETE FROM edges WHERE source_id = ?1 OR target_id = ?1",
                rusqlite::params![existing_id],
            )?;
            tx.execute("DELETE FROM nodes WHERE id = ?1", rusqlite::params![existing_id])?;
        }

        tx.execute(
            "INSERT INTO nodes (node_type, name, data_class, content) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![MEMORY_NODE_TYPE, key, data_class.to_i64(), Some(content),],
        )?;

        tx.commit()?;
        Ok(())
    }

    fn recall(&self, query: &str, limit: usize) -> anyhow::Result<Vec<MemoryEntry>> {
        let results = self.graph.search_text_filtered(query, DataClass::Restricted, limit)?;

        Ok(results
            .into_iter()
            .filter(|r| r.node_type == MEMORY_NODE_TYPE)
            .map(|r| MemoryEntry {
                key: r.name,
                content: r.content.unwrap_or_default(),
                data_class: r.data_class,
            })
            .collect())
    }

    fn forget(&self, key: &str) -> anyhow::Result<bool> {
        match self.graph.find_node_by_type_and_name(MEMORY_NODE_TYPE, key)? {
            Some(node) => Ok(self.graph.remove_node(node.id)?),
            None => Ok(false),
        }
    }

    fn list(&self, limit: usize) -> anyhow::Result<Vec<MemoryEntry>> {
        let nodes = self.graph.list_nodes(Some(MEMORY_NODE_TYPE), None, limit)?;

        Ok(nodes
            .into_iter()
            .map(|n| MemoryEntry {
                key: n.name,
                content: n.content.unwrap_or_default(),
                data_class: n.data_class,
            })
            .collect())
    }

    fn count(&self) -> anyhow::Result<usize> {
        let counts = self.graph.node_counts_by_type()?;
        Ok(counts
            .iter()
            .find(|(t, _)| t == MEMORY_NODE_TYPE)
            .map(|(_, c)| *c as usize)
            .unwrap_or(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remember_and_recall_roundtrip() {
        let mm = MemoryManager::open_in_memory().expect("open");
        mm.remember("greeting", "Hello, world!", DataClass::Public).expect("remember");

        let results = mm.recall("Hello", 10).expect("recall");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].key, "greeting");
        assert_eq!(results[0].content, "Hello, world!");
        assert_eq!(results[0].data_class, DataClass::Public);
    }

    #[test]
    fn remember_overwrites_existing_key() {
        let mm = MemoryManager::open_in_memory().expect("open");
        mm.remember("note", "first version", DataClass::Internal).expect("remember v1");
        mm.remember("note", "second version", DataClass::Confidential).expect("remember v2");

        assert_eq!(mm.count().expect("count"), 1);

        let entries = mm.list(10).expect("list");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].key, "note");
        assert_eq!(entries[0].content, "second version");
        assert_eq!(entries[0].data_class, DataClass::Confidential);
    }

    #[test]
    fn forget_removes_memory() {
        let mm = MemoryManager::open_in_memory().expect("open");
        mm.remember("temp", "temporary data", DataClass::Public).expect("remember");
        assert_eq!(mm.count().expect("count"), 1);

        let removed = mm.forget("temp").expect("forget");
        assert!(removed);
        assert_eq!(mm.count().expect("count"), 0);
    }

    #[test]
    fn forget_returns_false_for_missing_key() {
        let mm = MemoryManager::open_in_memory().expect("open");
        let removed = mm.forget("nonexistent").expect("forget");
        assert!(!removed);
    }

    #[test]
    fn recall_with_no_matches_returns_empty() {
        let mm = MemoryManager::open_in_memory().expect("open");
        mm.remember("apples", "I like apples", DataClass::Public).expect("remember");

        let results = mm.recall("zebra", 10).expect("recall");
        assert!(results.is_empty());
    }

    #[test]
    fn count_tracks_memories() {
        let mm = MemoryManager::open_in_memory().expect("open");
        assert_eq!(mm.count().expect("count"), 0);

        mm.remember("a", "alpha", DataClass::Public).expect("remember");
        assert_eq!(mm.count().expect("count"), 1);

        mm.remember("b", "beta", DataClass::Public).expect("remember");
        assert_eq!(mm.count().expect("count"), 2);

        mm.forget("a").expect("forget");
        assert_eq!(mm.count().expect("count"), 1);
    }

    #[test]
    fn list_returns_all_memories() {
        let mm = MemoryManager::open_in_memory().expect("open");
        mm.remember("first", "content one", DataClass::Public).expect("remember");
        mm.remember("second", "content two", DataClass::Internal).expect("remember");

        let entries = mm.list(10).expect("list");
        assert_eq!(entries.len(), 2);

        let keys: Vec<&str> = entries.iter().map(|e| e.key.as_str()).collect();
        assert!(keys.contains(&"first"));
        assert!(keys.contains(&"second"));
    }
}
