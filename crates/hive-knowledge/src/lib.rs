pub mod memory;
pub mod pool;

use hive_classification::DataClass;
use rusqlite::{ffi::sqlite3_auto_extension, params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sqlite_vec::sqlite3_vec_init;
use std::path::Path;
use std::sync::Once;

pub use memory::{MemoryEntry, MemoryManager, MemoryStore};
pub use pool::{KgPool, PooledKg};

const DEFAULT_EMBEDDING_DIMENSIONS: usize = 384;

/// Default maximum L2 distance for vector-similarity search results.
///
/// Because embeddings are L2-normalised, L2 distance relates to cosine
/// similarity as: `cos_sim = 1 − distance² / 2`.  A threshold of **1.0**
/// keeps results with cosine similarity ≥ 0.5, filtering out noise that
/// would otherwise appear for unrelated queries.
pub const DEFAULT_VECTOR_SEARCH_MAX_DISTANCE: f64 = 1.0;

static SQLITE_VEC_INIT: Once = Once::new();

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Node {
    pub id: i64,
    pub node_type: String,
    pub name: String,
    pub data_class: DataClass,
    pub content: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewNode {
    pub node_type: String,
    pub name: String,
    pub data_class: DataClass,
    pub content: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchResult {
    pub id: i64,
    pub node_type: String,
    pub name: String,
    pub data_class: DataClass,
    pub content: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VectorSearchResult {
    pub id: i64,
    pub distance: f64,
    pub data_class: DataClass,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Edge {
    pub id: i64,
    pub source_id: i64,
    pub target_id: i64,
    pub edge_type: String,
    pub weight: f64,
}

/// Statistics about embedding coverage in the knowledge graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingStats {
    /// Total number of nodes in the graph.
    pub total_nodes: usize,
    /// Number of nodes that have an embedding from the current model.
    pub embedded: usize,
    /// Number of nodes with an embedding from a different (stale) model.
    pub stale: usize,
    /// Number of nodes with no embedding at all.
    pub missing: usize,
}

/// A node together with its edges and immediate neighbors (1-hop).
#[derive(Debug, Clone)]
pub struct NodeNeighborhood {
    pub node: Node,
    pub edges: Vec<Edge>,
    pub neighbors: Vec<Node>,
}

pub struct KnowledgeGraph {
    connection: Connection,
    /// Active embedding dimension for this graph (read from graph_meta on open).
    /// Kept for backward compatibility but multi-model tables have their own dimensions.
    embedding_dimensions: usize,
}

/// Sanitize a model ID into a valid SQLite table name suffix.
/// Replaces non-alphanumeric characters with underscores.
///
/// # SQL Injection Safety
///
/// The returned name is safe for interpolation into SQL statements because:
/// 1. All non-ASCII-alphanumeric characters are replaced with underscores,
///    preventing any special SQL characters from appearing in the name.
/// 2. Callers wrap the result in quoted identifiers (`"…"`) which further
///    guard against reserved-word collisions.
fn vec_table_name(model_id: &str) -> String {
    let sanitized: String =
        model_id.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    format!("vec_{sanitized}")
}

impl KnowledgeGraph {
    pub fn open(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        init_sqlite_vec();
        let connection = Connection::open(path)?;
        bootstrap_connection(&connection)?;
        let embedding_dimensions = read_or_init_embedding_dimensions(&connection)?;
        Ok(Self { connection, embedding_dimensions })
    }

    pub fn open_in_memory() -> anyhow::Result<Self> {
        init_sqlite_vec();
        let connection = Connection::open_in_memory()?;
        bootstrap_connection(&connection)?;
        let embedding_dimensions = read_or_init_embedding_dimensions(&connection)?;
        Ok(Self { connection, embedding_dimensions })
    }

    pub fn insert_node(&self, node: &NewNode) -> anyhow::Result<i64> {
        self.connection.execute(
            "INSERT INTO nodes (node_type, name, data_class, content) VALUES (?1, ?2, ?3, ?4)",
            params![node.node_type, node.name, node.data_class.to_i64(), node.content],
        )?;
        Ok(self.connection.last_insert_rowid())
    }

    /// Insert a node and link it to a parent node via an edge, atomically.
    ///
    /// Both the node insert and the edge insert happen inside a single
    /// transaction so that a concurrent `remove_node` on the parent cannot
    /// leave an orphan child node.
    pub fn insert_node_linked(
        &self,
        node: &NewNode,
        parent_id: i64,
        edge_type: &str,
        weight: f64,
    ) -> anyhow::Result<i64> {
        let tx = self.connection.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO nodes (node_type, name, data_class, content) VALUES (?1, ?2, ?3, ?4)",
            params![node.node_type, node.name, node.data_class.to_i64(), node.content],
        )?;
        let node_id = self.connection.last_insert_rowid();
        tx.execute(
            "INSERT INTO edges (source_id, target_id, edge_type, weight) VALUES (?1, ?2, ?3, ?4)",
            params![parent_id, node_id, edge_type, weight],
        )?;
        tx.commit()?;
        Ok(node_id)
    }

    pub fn get_node(&self, id: i64) -> anyhow::Result<Option<Node>> {
        Ok(self.connection
            .query_row(
                "SELECT id, node_type, name, data_class, content, created_at, updated_at FROM nodes WHERE id = ?1",
                params![id],
                map_node,
            )
            .optional()?)
    }

    pub fn update_node_content(&self, node_id: i64, content: &str) -> anyhow::Result<()> {
        self.connection.execute(
            "UPDATE nodes SET content = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![content, node_id],
        )?;
        Ok(())
    }

    pub fn update_node_data_class(
        &self,
        node_id: i64,
        data_class: DataClass,
    ) -> anyhow::Result<()> {
        self.connection.execute(
            "UPDATE nodes SET data_class = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![data_class.to_i64(), node_id],
        )?;
        Ok(())
    }

    pub fn update_node_name(&self, node_id: i64, name: &str) -> anyhow::Result<()> {
        self.connection.execute(
            "UPDATE nodes SET name = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![name, node_id],
        )?;
        Ok(())
    }

    /// Find a single node by type and name.
    ///
    /// **Note:** There is no UNIQUE constraint on `(node_type, name)` because
    /// multiple sessions may legitimately create nodes with the same type and
    /// name (e.g. the same workspace file indexed in different sessions).
    /// This method returns the *first* matching row (LIMIT 1). Callers that
    /// need session-scoped lookups should additionally filter by edge
    /// connectivity to the owning session/workspace root node.
    pub fn find_node_by_type_and_name(
        &self,
        node_type: &str,
        name: &str,
    ) -> anyhow::Result<Option<Node>> {
        Ok(self
            .connection
            .query_row(
                "
                SELECT id, node_type, name, data_class, content, created_at, updated_at
                FROM nodes
                WHERE node_type = ?1 AND name = ?2
                LIMIT 1
                ",
                params![node_type, name],
                map_node,
            )
            .optional()?)
    }

    /// Find a node by type and name that is reachable from a root node through
    /// workspace tree edges (`session_workspace`, `contains_dir`,
    /// `contains_file`).  This provides **session-scoped** lookups for
    /// workspace directory and file nodes so that multiple sessions sharing
    /// the same database do not interfere with each other.
    pub fn find_node_in_workspace_tree(
        &self,
        root_node_id: i64,
        node_type: &str,
        name: &str,
    ) -> anyhow::Result<Option<Node>> {
        Ok(self
            .connection
            .query_row(
                "
                WITH RECURSIVE tree(id) AS (
                    VALUES(?1)
                    UNION ALL
                    SELECT e.target_id FROM edges e
                    JOIN tree t ON e.source_id = t.id
                    WHERE e.edge_type IN ('session_workspace', 'contains_dir', 'contains_file')
                )
                SELECT n.id, n.node_type, n.name, n.data_class, n.content, n.created_at, n.updated_at
                FROM nodes n
                JOIN tree t ON n.id = t.id
                WHERE n.node_type = ?2 AND n.name = ?3
                LIMIT 1
                ",
                params![root_node_id, node_type, name],
                map_node,
            )
            .optional()?)
    }

    pub fn list_nodes_by_type(&self, node_type: &str) -> anyhow::Result<Vec<Node>> {
        let mut stmt = self.connection.prepare(
            "SELECT id, node_type, name, data_class, content, created_at, updated_at FROM nodes WHERE node_type = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map(params![node_type], map_node)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn insert_edge(
        &self,
        source_id: i64,
        target_id: i64,
        edge_type: &str,
        weight: f64,
    ) -> anyhow::Result<i64> {
        self.connection.execute(
            "INSERT INTO edges (source_id, target_id, edge_type, weight) VALUES (?1, ?2, ?3, ?4)",
            params![source_id, target_id, edge_type, weight],
        )?;
        Ok(self.connection.last_insert_rowid())
    }

    pub fn set_embedding(
        &self,
        node_id: i64,
        embedding: &[f32],
        model_id: &str,
    ) -> anyhow::Result<()> {
        let dims = embedding.len();
        let table = self.resolve_or_create_vec_table(model_id, dims)?;
        let blob = embedding_to_bytes(embedding);

        // Retry with exponential backoff on transient lock errors.
        // busy_timeout handles most contention, but under very heavy load
        // the transaction itself may still fail.
        const MAX_RETRIES: u32 = 3;
        const BACKOFF_MS: [u64; 3] = [50, 200, 1000];

        for attempt in 0..MAX_RETRIES {
            match self.try_write_embedding(node_id, &table, &blob, model_id, dims) {
                Ok(()) => return Ok(()),
                Err(e) if attempt < MAX_RETRIES - 1 && is_busy_error(&e) => {
                    std::thread::sleep(std::time::Duration::from_millis(
                        BACKOFF_MS[attempt as usize],
                    ));
                }
                Err(e) => return Err(e),
            }
        }
        Err(anyhow::anyhow!("embedding write retries exhausted for node {node_id}"))
    }

    fn try_write_embedding(
        &self,
        node_id: i64,
        table: &str,
        blob: &[u8],
        model_id: &str,
        dims: usize,
    ) -> anyhow::Result<()> {
        // SAFETY: KnowledgeGraph does not support concurrent access;
        // callers must ensure single-writer serialization.
        let tx = self.connection.unchecked_transaction()?;

        // Remove from any *other* vec tables for this node (model switch).
        let other_tables = self.list_vec_tables_excluding(table)?;
        for other in &other_tables {
            tx.execute(&format!("DELETE FROM \"{other}\" WHERE rowid = ?1"), params![node_id])?;
        }

        tx.execute(
            &format!("INSERT OR REPLACE INTO \"{table}\" (rowid, embedding) VALUES (?1, ?2)"),
            params![node_id, blob],
        )?;
        tx.execute(
            "INSERT OR REPLACE INTO embedding_meta (node_id, model_id, dimensions) VALUES (?1, ?2, ?3)",
            params![node_id, model_id, dims as i64],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Find the vec table name for a model by looking up the `vec_tables`
    /// registry.  If no table exists yet, create one.
    fn resolve_or_create_vec_table(
        &self,
        model_id: &str,
        dimensions: usize,
    ) -> anyhow::Result<String> {
        // Check if a table is already registered for this model.
        let existing: Option<(String, i64)> = self
            .connection
            .query_row(
                "SELECT table_name, dimensions FROM vec_tables WHERE model_id = ?1",
                params![model_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;

        if let Some((name, dim)) = existing {
            if dim as usize == dimensions {
                return Ok(name);
            }
            // Dimension changed — recreate.
            self.connection.execute_batch(&format!("DROP TABLE IF EXISTS \"{name}\";"))?;
            self.connection
                .execute("DELETE FROM vec_tables WHERE table_name = ?1", params![name])?;
        }

        // Create a new table.
        let table = vec_table_name(model_id);
        self.connection.execute_batch(&format!(
            "CREATE VIRTUAL TABLE IF NOT EXISTS \"{table}\" USING vec0(embedding float[{dimensions}]);"
        ))?;
        self.connection.execute(
            "INSERT OR REPLACE INTO vec_tables (table_name, model_id, dimensions) VALUES (?1, ?2, ?3)",
            params![table, model_id, dimensions as i64],
        )?;
        Ok(table)
    }

    /// List all vec table names except the given one.
    fn list_vec_tables_excluding(&self, exclude: &str) -> anyhow::Result<Vec<String>> {
        let mut stmt =
            self.connection.prepare("SELECT table_name FROM vec_tables WHERE table_name != ?1")?;
        let rows = stmt.query_map(params![exclude], |row| row.get(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<String>>>()?)
    }

    pub fn effective_class(&self, node_id: i64) -> anyhow::Result<DataClass> {
        let class: i64 = self.connection.query_row(
            "
            WITH RECURSIVE ancestors(id, data_class) AS (
                SELECT n.id, n.data_class FROM nodes n WHERE n.id = ?1
                UNION ALL
                SELECT n.id, n.data_class
                FROM ancestors a
                JOIN edges e ON e.source_id = a.id AND e.edge_type = 'child_of'
                JOIN nodes n ON n.id = e.target_id
            )
            SELECT MAX(data_class) FROM ancestors
            ",
            params![node_id],
            |row| row.get(0),
        )?;

        Ok(DataClass::from_i64(class).unwrap_or_else(|| {
            tracing::warn!(
                data_class = class,
                "invalid data_class in effective_class query, defaulting to Internal"
            );
            DataClass::Internal
        }))
    }

    pub fn remove_node(&self, id: i64) -> anyhow::Result<bool> {
        let tables = self.all_vec_table_names().unwrap_or_default();
        // Wrap all deletes in a single transaction so that a concurrent
        // writer cannot insert a new edge between the edge-delete and the
        // node-delete, which would cause a FOREIGN KEY constraint failure.
        let tx = self.connection.unchecked_transaction()?;
        tx.execute("DELETE FROM embedding_meta WHERE node_id = ?1", params![id])
            .unwrap_or_else(|e| {
                tracing::warn!("failed to delete embedding_meta for node {id}: {e}");
                0
            });
        for tbl in &tables {
            tx.execute(&format!("DELETE FROM \"{tbl}\" WHERE rowid = ?1"), params![id])
                .unwrap_or_else(|e| {
                    tracing::warn!(
                        "failed to delete embedding row from {tbl} for node {id}: {e}"
                    );
                    0
                });
        }
        tx.execute("DELETE FROM edges WHERE source_id = ?1 OR target_id = ?1", params![id])?;
        let deleted = tx.execute("DELETE FROM nodes WHERE id = ?1", params![id])?;
        tx.commit()?;
        Ok(deleted > 0)
    }

    pub fn remove_nodes_batch(&self, ids: &[i64]) -> anyhow::Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let tables = self.all_vec_table_names().unwrap_or_default();
        let tx = self.connection.unchecked_transaction()?;
        for id in ids {
            tx.execute("DELETE FROM embedding_meta WHERE node_id = ?1", params![id])?;
            for tbl in &tables {
                tx.execute(&format!("DELETE FROM \"{tbl}\" WHERE rowid = ?1"), params![id])?;
            }
            tx.execute("DELETE FROM edges WHERE source_id = ?1 OR target_id = ?1", params![id])?;
            tx.execute("DELETE FROM nodes WHERE id = ?1", params![id])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn remove_edge(&self, id: i64) -> anyhow::Result<bool> {
        let deleted = self.connection.execute("DELETE FROM edges WHERE id = ?1", params![id])?;
        Ok(deleted > 0)
    }

    pub fn remove_edge_between(
        &self,
        source_id: i64,
        target_id: i64,
        edge_type: &str,
    ) -> anyhow::Result<bool> {
        let deleted = self.connection.execute(
            "DELETE FROM edges WHERE source_id = ?1 AND target_id = ?2 AND edge_type = ?3",
            params![source_id, target_id, edge_type],
        )?;
        Ok(deleted > 0)
    }

    pub fn list_nodes(
        &self,
        node_type: Option<&str>,
        data_class: Option<DataClass>,
        limit: usize,
    ) -> anyhow::Result<Vec<Node>> {
        let sql = match (node_type, data_class) {
            (Some(_), Some(_)) => {
                "SELECT id, node_type, name, data_class, content, created_at, updated_at FROM nodes WHERE node_type = ?1 AND data_class = ?2 ORDER BY id DESC LIMIT ?3"
            }
            (Some(_), None) => {
                "SELECT id, node_type, name, data_class, content, created_at, updated_at FROM nodes WHERE node_type = ?1 ORDER BY id DESC LIMIT ?2"
            }
            (None, Some(_)) => {
                "SELECT id, node_type, name, data_class, content, created_at, updated_at FROM nodes WHERE data_class = ?1 ORDER BY id DESC LIMIT ?2"
            }
            (None, None) => {
                "SELECT id, node_type, name, data_class, content, created_at, updated_at FROM nodes ORDER BY id DESC LIMIT ?1"
            }
        };

        let result: rusqlite::Result<Vec<Node>> = match (node_type, data_class) {
            (Some(nt), Some(dc)) => {
                let mut stmt = self.connection.prepare(sql)?;
                let rows = stmt.query_map(params![nt, dc.to_i64(), limit as i64], map_node)?;
                rows.collect()
            }
            (Some(nt), None) => {
                let mut stmt = self.connection.prepare(sql)?;
                let rows = stmt.query_map(params![nt, limit as i64], map_node)?;
                rows.collect()
            }
            (None, Some(dc)) => {
                let mut stmt = self.connection.prepare(sql)?;
                let rows = stmt.query_map(params![dc.to_i64(), limit as i64], map_node)?;
                rows.collect()
            }
            (None, None) => {
                let mut stmt = self.connection.prepare(sql)?;
                let rows = stmt.query_map(params![limit as i64], map_node)?;
                rows.collect()
            }
        };
        Ok(result?)
    }

    pub fn get_edges_for_node(&self, node_id: i64) -> anyhow::Result<Vec<Edge>> {
        let mut stmt = self.connection.prepare(
            "SELECT id, source_id, target_id, edge_type, weight FROM edges WHERE source_id = ?1 OR target_id = ?1 ORDER BY id DESC",
        )?;
        let rows = stmt.query_map(params![node_id], |row| {
            Ok(Edge {
                id: row.get(0)?,
                source_id: row.get(1)?,
                target_id: row.get(2)?,
                edge_type: row.get(3)?,
                weight: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn node_count(&self) -> anyhow::Result<i64> {
        Ok(self.connection.query_row("SELECT COUNT(*) FROM nodes", [], |row| row.get(0))?)
    }

    pub fn edge_count(&self) -> anyhow::Result<i64> {
        Ok(self.connection.query_row("SELECT COUNT(*) FROM edges", [], |row| row.get(0))?)
    }

    pub fn node_counts_by_type(&self) -> anyhow::Result<Vec<(String, i64)>> {
        let mut stmt = self.connection.prepare(
            "SELECT node_type, COUNT(*) FROM nodes GROUP BY node_type ORDER BY COUNT(*) DESC",
        )?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn edge_counts_by_type(&self) -> anyhow::Result<Vec<(String, i64)>> {
        let mut stmt = self.connection.prepare(
            "SELECT edge_type, COUNT(*) FROM edges GROUP BY edge_type ORDER BY COUNT(*) DESC",
        )?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn search_text(&self, query: &str, limit: usize) -> anyhow::Result<Vec<SearchResult>> {
        self.search_text_filtered(query, DataClass::Restricted, limit)
    }

    /// Collect all node IDs transitively reachable from a session node.
    /// Traverses session_message, session_workspace, session_agent edges,
    /// then follows contains_dir, contains_file, and file_chunk edges to
    /// capture the full workspace tree + message nodes + agent nodes.
    pub fn collect_session_node_ids(
        &self,
        session_node_id: i64,
    ) -> anyhow::Result<std::collections::HashSet<i64>> {
        let mut ids = std::collections::HashSet::new();
        ids.insert(session_node_id);

        let mut stmt = self.connection.prepare(
            "
            WITH RECURSIVE session_tree(id) AS (
                SELECT ?1
                UNION
                SELECT e.target_id
                FROM session_tree st
                JOIN edges e ON e.source_id = st.id
                WHERE e.edge_type IN (
                    'session_message', 'session_workspace', 'session_agent',
                    'contains_dir', 'contains_file', 'file_chunk'
                )
            )
            SELECT id FROM session_tree
            ",
        )?;

        let rows = stmt.query_map(params![session_node_id], |row| row.get::<_, i64>(0))?;
        for row in rows {
            ids.insert(row?);
        }
        Ok(ids)
    }

    pub fn search_text_filtered(
        &self,
        query: &str,
        max_class: DataClass,
        limit: usize,
    ) -> anyhow::Result<Vec<SearchResult>> {
        self.search_text_filtered_by_type(query, max_class, None, limit)
    }

    /// Full-text search with optional node-type filtering.
    ///
    /// When `node_types` is `Some`, only nodes whose `node_type` is in the
    /// given slice are returned.
    pub fn search_text_filtered_by_type(
        &self,
        query: &str,
        max_class: DataClass,
        node_types: Option<&[&str]>,
        limit: usize,
    ) -> anyhow::Result<Vec<SearchResult>> {
        let (type_clause, _) = build_node_type_clause(node_types);
        let sql = format!(
            "
                SELECT n.id, n.node_type, n.name, n.data_class, n.content, n.created_at, n.updated_at
                FROM nodes_fts f
                JOIN nodes n ON n.id = f.rowid
                WHERE nodes_fts MATCH ?1
                  AND n.data_class <= ?2
                  {type_clause}
                LIMIT ?3
                "
        );
        let mut stmt = self.connection.prepare(&sql)?;

        let rows = stmt.query_map(params![query, max_class.to_i64(), limit as i64], |row| {
            let class: i64 = row.get(3)?;
            Ok(SearchResult {
                id: row.get(0)?,
                node_type: row.get(1)?,
                name: row.get(2)?,
                data_class: DataClass::from_i64(class).unwrap_or_else(|| {
                    tracing::warn!(data_class = class, "invalid data_class in search result");
                    DataClass::Internal
                }),
                content: row.get(4)?,
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
            })
        })?;

        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn list_outbound_nodes(
        &self,
        source_id: i64,
        edge_type: &str,
        max_class: DataClass,
        limit: usize,
    ) -> anyhow::Result<Vec<Node>> {
        let mut stmt = self.connection.prepare(
            "
            SELECT n.id, n.node_type, n.name, n.data_class, n.content, n.created_at, n.updated_at
            FROM edges e
            JOIN nodes n ON n.id = e.target_id
            WHERE e.source_id = ?1
              AND e.edge_type = ?2
              AND n.data_class <= ?3
            ORDER BY e.id DESC
            LIMIT ?4
            ",
        )?;

        let rows = stmt
            .query_map(params![source_id, edge_type, max_class.to_i64(), limit as i64], map_node)?;

        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn search_similar(
        &self,
        query_vector: &[f32],
        model_id: &str,
        max_class: DataClass,
        limit: usize,
    ) -> anyhow::Result<Vec<VectorSearchResult>> {
        self.search_similar_filtered(
            query_vector,
            model_id,
            max_class,
            None,
            Some(DEFAULT_VECTOR_SEARCH_MAX_DISTANCE),
            limit,
        )
    }

    /// Vector-similarity search with optional node-type filtering.
    ///
    /// Uses the default distance threshold to discard irrelevant results.
    pub fn search_similar_by_type(
        &self,
        query_vector: &[f32],
        model_id: &str,
        max_class: DataClass,
        node_types: Option<&[&str]>,
        limit: usize,
    ) -> anyhow::Result<Vec<VectorSearchResult>> {
        self.search_similar_filtered(
            query_vector,
            model_id,
            max_class,
            node_types,
            Some(DEFAULT_VECTOR_SEARCH_MAX_DISTANCE),
            limit,
        )
    }

    /// Vector-similarity search with optional node-type filtering and distance threshold.
    ///
    /// When `max_distance` is `Some(d)`, results whose L2 distance exceeds `d`
    /// are discarded.  Pass `None` to return the top-k results regardless of
    /// distance.
    pub fn search_similar_filtered(
        &self,
        query_vector: &[f32],
        model_id: &str,
        max_class: DataClass,
        node_types: Option<&[&str]>,
        max_distance: Option<f64>,
        limit: usize,
    ) -> anyhow::Result<Vec<VectorSearchResult>> {
        // Look up the table for this model.
        let table: Option<String> = self
            .connection
            .query_row(
                "SELECT table_name FROM vec_tables WHERE model_id = ?1",
                params![model_id],
                |row| row.get(0),
            )
            .optional()?;

        let table = match table {
            Some(t) => t,
            None => return Ok(Vec::new()),
        };

        let (type_clause, _) = build_node_type_clause(node_types);
        let query_blob = embedding_to_bytes(query_vector);
        let k_overfetch = (limit * 3).max(limit) as i64;
        let sql = format!(
            "
            SELECT v.rowid, v.distance, n.data_class
            FROM \"{table}\" v
            JOIN nodes n ON n.id = v.rowid
            WHERE v.embedding MATCH ?1
              AND k = ?3
              AND n.data_class <= ?2
              {type_clause}
            ORDER BY v.distance
            LIMIT ?4
            "
        );
        let mut stmt = self.connection.prepare(&sql)?;

        let rows = stmt.query_map(
            params![query_blob, max_class.to_i64(), k_overfetch, limit as i64],
            |row| {
                let class: i64 = row.get(2)?;
                Ok(VectorSearchResult {
                    id: row.get(0)?,
                    distance: row.get(1)?,
                    data_class: DataClass::from_i64(class).unwrap_or_else(|| {
                        tracing::warn!(
                            data_class = class,
                            "invalid data_class in vector search result"
                        );
                        DataClass::Internal
                    }),
                })
            },
        )?;

        let mut results: Vec<VectorSearchResult> = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        if let Some(max_dist) = max_distance {
            results.retain(|r| r.distance <= max_dist);
        }
        Ok(results)
    }

    /// Returns the current embedding model ID and dimension, if configured.
    pub fn embedding_model(&self) -> anyhow::Result<Option<(String, usize)>> {
        let model_id: Option<String> = self
            .connection
            .query_row("SELECT value FROM graph_meta WHERE key = 'embedding_model_id'", [], |row| {
                row.get(0)
            })
            .optional()?;

        match model_id {
            Some(id) => Ok(Some((id, self.embedding_dimensions))),
            None => Ok(None),
        }
    }

    /// Returns the active embedding dimension for this graph.
    pub fn embedding_dimensions(&self) -> usize {
        self.embedding_dimensions
    }

    /// Returns nodes that need embedding: either missing embeddings or produced
    /// by a different model than `current_model`.
    ///
    /// When `node_types` is non-empty, only nodes matching one of the given
    /// types are considered (e.g. `["file_chunk", "chat_message"]`). When
    /// empty, all node types are included.
    pub fn nodes_needing_embedding(
        &self,
        current_model: &str,
        node_types: &[&str],
        limit: usize,
    ) -> anyhow::Result<Vec<i64>> {
        if node_types.is_empty() {
            let mut stmt = self.connection.prepare(
                "
                SELECT n.id FROM nodes n
                LEFT JOIN embedding_meta em ON em.node_id = n.id
                WHERE em.node_id IS NULL OR em.model_id != ?1
                ORDER BY n.id
                LIMIT ?2
                ",
            )?;
            let rows = stmt.query_map(params![current_model, limit as i64], |row| row.get(0))?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        } else {
            let placeholders: String = node_types
                .iter()
                .enumerate()
                .map(|(i, _)| format!("?{}", i + 3))
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "
                SELECT n.id FROM nodes n
                LEFT JOIN embedding_meta em ON em.node_id = n.id
                WHERE (em.node_id IS NULL OR em.model_id != ?1)
                  AND n.node_type IN ({placeholders})
                ORDER BY n.id
                LIMIT ?2
                "
            );
            let mut stmt = self.connection.prepare(&sql)?;
            let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
            param_values.push(Box::new(current_model.to_string()));
            param_values.push(Box::new(limit as i64));
            for nt in node_types {
                param_values.push(Box::new(nt.to_string()));
            }
            let params_ref: Vec<&dyn rusqlite::types::ToSql> =
                param_values.iter().map(|p| p.as_ref()).collect();
            let rows = stmt.query_map(params_ref.as_slice(), |row| row.get(0))?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        }
    }

    /// Returns statistics about embedding coverage.
    ///
    /// When `node_types` is non-empty, only nodes matching one of the given
    /// types are counted. When empty, all node types are included.
    pub fn embedding_stats(
        &self,
        current_model: &str,
        node_types: &[&str],
    ) -> anyhow::Result<EmbeddingStats> {
        let total_nodes: usize = if node_types.is_empty() {
            self.connection.query_row("SELECT COUNT(*) FROM nodes", [], |row| row.get(0))?
        } else {
            let placeholders: String = node_types
                .iter()
                .enumerate()
                .map(|(i, _)| format!("?{}", i + 1))
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!("SELECT COUNT(*) FROM nodes WHERE node_type IN ({placeholders})");
            let mut stmt = self.connection.prepare(&sql)?;
            let params: Vec<Box<dyn rusqlite::types::ToSql>> = node_types
                .iter()
                .map(|nt| Box::new(nt.to_string()) as Box<dyn rusqlite::types::ToSql>)
                .collect();
            let params_ref: Vec<&dyn rusqlite::types::ToSql> =
                params.iter().map(|p| p.as_ref()).collect();
            stmt.query_row(params_ref.as_slice(), |row| row.get(0))?
        };

        let (embedded, stale) = if node_types.is_empty() {
            let embedded: usize = self.connection.query_row(
                "SELECT COUNT(*) FROM embedding_meta WHERE model_id = ?1",
                params![current_model],
                |row| row.get(0),
            )?;
            let stale: usize = self.connection.query_row(
                "SELECT COUNT(*) FROM embedding_meta WHERE model_id != ?1",
                params![current_model],
                |row| row.get(0),
            )?;
            (embedded, stale)
        } else {
            let placeholders: String = node_types
                .iter()
                .enumerate()
                .map(|(i, _)| format!("?{}", i + 2))
                .collect::<Vec<_>>()
                .join(", ");
            let sql_embedded = format!(
                "SELECT COUNT(*) FROM embedding_meta em JOIN nodes n ON n.id = em.node_id WHERE em.model_id = ?1 AND n.node_type IN ({placeholders})"
            );
            let sql_stale = format!(
                "SELECT COUNT(*) FROM embedding_meta em JOIN nodes n ON n.id = em.node_id WHERE em.model_id != ?1 AND n.node_type IN ({placeholders})"
            );

            let mut params_embedded: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
            params_embedded.push(Box::new(current_model.to_string()));
            for nt in node_types {
                params_embedded.push(Box::new(nt.to_string()));
            }
            let params_ref: Vec<&dyn rusqlite::types::ToSql> =
                params_embedded.iter().map(|p| p.as_ref()).collect();

            let mut stmt_e = self.connection.prepare(&sql_embedded)?;
            let embedded: usize = stmt_e.query_row(params_ref.as_slice(), |row| row.get(0))?;

            let mut stmt_s = self.connection.prepare(&sql_stale)?;
            let stale: usize = stmt_s.query_row(params_ref.as_slice(), |row| row.get(0))?;

            (embedded, stale)
        };

        let missing = total_nodes.saturating_sub(embedded + stale);

        Ok(EmbeddingStats { total_nodes, embedded, stale, missing })
    }

    /// Prepare the graph for reindexing with a specific embedding model.
    ///
    /// Drops and recreates the vec table for this model.  Clears
    /// `embedding_meta` rows for this model and updates `graph_meta`.
    pub fn prepare_reindex(
        &mut self,
        new_model_id: &str,
        new_dimensions: usize,
    ) -> anyhow::Result<()> {
        let table = vec_table_name(new_model_id);

        // SAFETY: KnowledgeGraph does not support concurrent access;
        // callers must ensure single-writer serialization.
        let tx = self.connection.unchecked_transaction()?;

        // Drop and recreate the vec table for this specific model.
        tx.execute_batch(&format!("DROP TABLE IF EXISTS \"{table}\";"))?;
        tx.execute_batch(&format!(
            "CREATE VIRTUAL TABLE \"{table}\" USING vec0(embedding float[{new_dimensions}]);"
        ))?;
        tx.execute(
            "INSERT OR REPLACE INTO vec_tables (table_name, model_id, dimensions) VALUES (?1, ?2, ?3)",
            params![table, new_model_id, new_dimensions as i64],
        )?;

        // Clear embedding_meta only for this model.
        tx.execute("DELETE FROM embedding_meta WHERE model_id = ?1", params![new_model_id])?;

        tx.execute(
            "INSERT OR REPLACE INTO graph_meta (key, value) VALUES ('embedding_model_id', ?1)",
            params![new_model_id],
        )?;
        tx.execute(
            "INSERT OR REPLACE INTO graph_meta (key, value) VALUES ('embedding_dimensions', ?1)",
            params![new_dimensions.to_string()],
        )?;

        tx.commit()?;
        self.embedding_dimensions = new_dimensions;
        Ok(())
    }

    /// Returns a list of all registered embedding models and their dimensions.
    pub fn list_embedding_models(&self) -> anyhow::Result<Vec<(String, usize)>> {
        let mut stmt = self
            .connection
            .prepare("SELECT model_id, dimensions FROM vec_tables ORDER BY model_id")?;
        let rows = stmt.query_map([], |row| {
            let model_id: String = row.get(0)?;
            let dims: i64 = row.get(1)?;
            Ok((model_id, dims as usize))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Delete all message nodes linked to a session via `session_message` edges.
    ///
    /// This also removes each message's embedding metadata and vector rows.
    /// Must be called **before** `remove_node(session_node_id)` (while edges
    /// still exist). Returns the number of message nodes removed.
    pub fn scrub_session_messages(&self, session_node_id: i64) -> anyhow::Result<usize> {
        // Collect message node IDs linked via outbound `session_message` edges.
        let mut stmt = self.connection.prepare(
            "SELECT target_id FROM edges WHERE source_id = ?1 AND edge_type = 'session_message'",
        )?;
        let msg_ids: Vec<i64> = stmt
            .query_map(params![session_node_id], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        if msg_ids.is_empty() {
            return Ok(0);
        }

        let vec_tables = self.all_vec_table_names()?;
        // SAFETY: KnowledgeGraph does not support concurrent access;
        // callers must ensure single-writer serialization.
        let tx = self.connection.unchecked_transaction()?;
        for &node_id in &msg_ids {
            tx.execute("DELETE FROM embedding_meta WHERE node_id = ?1", params![node_id])?;
            for tbl in &vec_tables {
                tx.execute(&format!("DELETE FROM \"{tbl}\" WHERE rowid = ?1"), params![node_id])
                    .unwrap_or_else(|e| {
                        tracing::warn!(
                            "failed to delete embedding row from {tbl} for node {node_id}: {e}"
                        );
                        0
                    });
            }
            tx.execute(
                "DELETE FROM edges WHERE source_id = ?1 OR target_id = ?1",
                params![node_id],
            )?;
            tx.execute("DELETE FROM nodes WHERE id = ?1", params![node_id])?;
        }
        tx.commit()?;
        Ok(msg_ids.len())
    }

    /// Returns the names of all registered vec tables.
    fn all_vec_table_names(&self) -> anyhow::Result<Vec<String>> {
        let mut stmt = self.connection.prepare("SELECT table_name FROM vec_tables")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<String>>>()?)
    }

    /// Returns a node together with all its edges and 1-hop neighbors.
    pub fn get_node_with_neighbors(
        &self,
        node_id: i64,
        limit: usize,
    ) -> anyhow::Result<NodeNeighborhood> {
        let node =
            self.get_node(node_id)?.ok_or_else(|| anyhow::anyhow!("node {node_id} not found"))?;

        // Edges where this node is source or target.
        let mut stmt = self.connection.prepare(
            "SELECT id, source_id, target_id, edge_type, weight
             FROM edges
             WHERE source_id = ?1 OR target_id = ?1",
        )?;
        let edges: Vec<Edge> = stmt
            .query_map(params![node_id], |row| {
                Ok(Edge {
                    id: row.get(0)?,
                    source_id: row.get(1)?,
                    target_id: row.get(2)?,
                    edge_type: row.get(3)?,
                    weight: row.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        // Collect unique neighbor IDs (the "other end" of each edge).
        let mut neighbor_ids = std::collections::HashSet::new();
        for e in &edges {
            if e.source_id != node_id {
                neighbor_ids.insert(e.source_id);
            }
            if e.target_id != node_id {
                neighbor_ids.insert(e.target_id);
            }
        }

        // Fetch neighbor nodes (respecting limit).
        let mut neighbors = Vec::new();
        for &nid in neighbor_ids.iter().take(limit) {
            if let Some(n) = self.get_node(nid)? {
                neighbors.push(n);
            }
        }

        Ok(NodeNeighborhood { node, edges, neighbors })
    }
}

/// Sets the active embedding model in graph_meta without changing dimensions or
/// clearing data. Used when first configuring a model on an empty graph.
pub fn set_graph_embedding_model(connection: &Connection, model_id: &str) -> anyhow::Result<()> {
    connection.execute(
        "INSERT OR REPLACE INTO graph_meta (key, value) VALUES ('embedding_model_id', ?1)",
        params![model_id],
    )?;
    Ok(())
}

fn bootstrap_connection(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA busy_timeout=5000; PRAGMA foreign_keys=ON;",
    )?;
    create_schema(connection)?;
    migrate_legacy_vec_nodes(connection);
    Ok(())
}

/// If the legacy `vec_nodes` table exists but is not yet registered in
/// `vec_tables`, register it under the default embedding model name.
fn migrate_legacy_vec_nodes(connection: &Connection) {
    let has_legacy: bool = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='vec_nodes'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map(|c| c > 0)
        .unwrap_or(false);

    if !has_legacy {
        return;
    }

    // Check if it's already tracked in vec_tables.
    let already_tracked: bool = connection
        .query_row("SELECT COUNT(*) FROM vec_tables WHERE table_name = 'vec_nodes'", [], |row| {
            row.get::<_, i64>(0)
        })
        .map(|c| c > 0)
        .unwrap_or(false);

    if already_tracked {
        return;
    }

    // Read the stored model ID, or use a sensible default.
    let model_id: String = connection
        .query_row("SELECT value FROM graph_meta WHERE key = 'embedding_model_id'", [], |row| {
            row.get(0)
        })
        .unwrap_or_else(|_| "bge-small-en-v1.5".to_string());

    let dims: i64 = connection
        .query_row("SELECT value FROM graph_meta WHERE key = 'embedding_dimensions'", [], |row| {
            let v: String = row.get(0)?;
            Ok(v.parse::<i64>().unwrap_or(DEFAULT_EMBEDDING_DIMENSIONS as i64))
        })
        .unwrap_or(DEFAULT_EMBEDDING_DIMENSIONS as i64);

    // Register the legacy table.
    connection
        .execute(
            "INSERT OR IGNORE INTO vec_tables (table_name, model_id, dimensions) VALUES ('vec_nodes', ?1, ?2)",
            params![model_id, dims],
        )
        .unwrap_or_else(|e| { tracing::warn!("failed to register legacy vec_nodes table: {e}"); 0 });

    tracing::info!(
        model_id = %model_id,
        dimensions = dims,
        "migrated legacy vec_nodes table to vec_tables registry"
    );
}

fn create_schema(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS nodes (
            id INTEGER PRIMARY KEY,
            node_type TEXT NOT NULL,
            name TEXT NOT NULL,
            data_class INTEGER NOT NULL DEFAULT 1,
            content TEXT,
            created_at TEXT DEFAULT (datetime('now')),
            updated_at TEXT DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS edges (
            id INTEGER PRIMARY KEY,
            source_id INTEGER NOT NULL REFERENCES nodes(id),
            target_id INTEGER NOT NULL REFERENCES nodes(id),
            edge_type TEXT NOT NULL,
            weight REAL NOT NULL DEFAULT 1.0,
            created_at TEXT DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_edges_source ON edges(source_id);
        CREATE INDEX IF NOT EXISTS idx_edges_target ON edges(target_id);
        CREATE INDEX IF NOT EXISTS idx_edges_type ON edges(edge_type);
        CREATE INDEX IF NOT EXISTS idx_edges_source_type ON edges(source_id, edge_type);
        CREATE INDEX IF NOT EXISTS idx_nodes_type ON nodes(node_type);
        CREATE INDEX IF NOT EXISTS idx_nodes_class ON nodes(data_class);
        CREATE INDEX IF NOT EXISTS idx_nodes_type_name ON nodes(node_type, name);
        CREATE INDEX IF NOT EXISTS idx_nodes_type_class ON nodes(node_type, data_class);

        CREATE VIRTUAL TABLE IF NOT EXISTS nodes_fts USING fts5(
            name,
            content,
            content='nodes',
            content_rowid='id'
        );

        CREATE TRIGGER IF NOT EXISTS nodes_ai AFTER INSERT ON nodes BEGIN
            INSERT INTO nodes_fts(rowid, name, content) VALUES (new.id, new.name, new.content);
        END;

        CREATE TRIGGER IF NOT EXISTS nodes_ad AFTER DELETE ON nodes BEGIN
            INSERT INTO nodes_fts(nodes_fts, rowid, name, content) VALUES('delete', old.id, old.name, old.content);
        END;

        CREATE TRIGGER IF NOT EXISTS nodes_au AFTER UPDATE ON nodes BEGIN
            INSERT INTO nodes_fts(nodes_fts, rowid, name, content) VALUES('delete', old.id, old.name, old.content);
            INSERT INTO nodes_fts(rowid, name, content) VALUES (new.id, new.name, new.content);
        END;

        CREATE VIRTUAL TABLE IF NOT EXISTS vec_nodes USING vec0(
            embedding float[384]
        );

        -- Legacy: vec_nodes is kept for backward compatibility with
        -- databases created before per-model vec tables were introduced.
        -- New databases create per-model tables via ensure_vec_table().
        -- TODO: Remove vec_nodes creation once migration has run everywhere.

        CREATE TABLE IF NOT EXISTS vec_tables (
            table_name TEXT PRIMARY KEY,
            model_id TEXT NOT NULL,
            dimensions INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS embedding_meta (
            node_id INTEGER PRIMARY KEY REFERENCES nodes(id),
            model_id TEXT NOT NULL,
            dimensions INTEGER NOT NULL,
            created_at TEXT DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_embedding_meta_model ON embedding_meta(model_id);

        CREATE TABLE IF NOT EXISTS graph_meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        ",
    )
}

/// Read the active embedding dimension from `graph_meta`, defaulting to 384
/// for fresh or pre-migration databases.
fn read_or_init_embedding_dimensions(connection: &Connection) -> anyhow::Result<usize> {
    let stored: Option<String> = connection
        .query_row("SELECT value FROM graph_meta WHERE key = 'embedding_dimensions'", [], |row| {
            row.get(0)
        })
        .optional()?;

    match stored {
        Some(val) => Ok(val.parse::<usize>().unwrap_or(DEFAULT_EMBEDDING_DIMENSIONS)),
        None => {
            connection.execute(
                "INSERT OR REPLACE INTO graph_meta (key, value) VALUES ('embedding_dimensions', ?1)",
                params![DEFAULT_EMBEDDING_DIMENSIONS.to_string()],
            )?;
            Ok(DEFAULT_EMBEDDING_DIMENSIONS)
        }
    }
}

fn map_node(row: &rusqlite::Row<'_>) -> rusqlite::Result<Node> {
    let class: i64 = row.get(3)?;
    Ok(Node {
        id: row.get(0)?,
        node_type: row.get(1)?,
        name: row.get(2)?,
        data_class: DataClass::from_i64(class).unwrap_or_else(|| {
            tracing::warn!(
                data_class = class,
                "invalid data_class value in database, defaulting to Internal"
            );
            DataClass::Internal
        }),
        content: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

fn embedding_to_bytes(embedding: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(std::mem::size_of_val(embedding));
    for value in embedding {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

/// Check if an error is a transient SQLite busy/locked error.
fn is_busy_error(e: &anyhow::Error) -> bool {
    let msg = e.to_string();
    msg.contains("database is locked") || msg.contains("SQLITE_BUSY")
}

fn init_sqlite_vec() {
    SQLITE_VEC_INIT.call_once(|| unsafe {
        // SAFETY: `sqlite3_vec_init` has the exact signature expected by
        // `sqlite3_auto_extension` (extern "C" fn(*mut sqlite3, …) -> c_int).
        // The transmute is required because `sqlite3_auto_extension` expects
        // `Option<unsafe extern "C" fn()>` as an erased function pointer.
        // The sqlite-vec crate guarantees ABI compatibility.
        #[allow(clippy::missing_transmute_annotations)]
        sqlite3_auto_extension(Some(std::mem::transmute(sqlite3_vec_init as *const ())));
    });
}

/// Build an optional `AND n.node_type IN (...)` SQL clause.
/// Returns the SQL fragment and the parameter values to append.
fn build_node_type_clause(node_types: Option<&[&str]>) -> (String, Vec<String>) {
    match node_types {
        None => (String::new(), Vec::new()),
        Some([]) => (String::new(), Vec::new()),
        Some(types) => {
            // Use literal quoted strings to avoid positional placeholder conflicts.
            // Values are safe (node_type strings are internal constants, not user input).
            let quoted: Vec<String> =
                types.iter().map(|t| format!("'{}'", t.replace('\'', "''"))).collect();
            let clause = format!("AND n.node_type IN ({})", quoted.join(", "));
            (clause, Vec::new())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn embedding(seed: f32) -> Vec<f32> {
        (0..DEFAULT_EMBEDDING_DIMENSIONS).map(|index| seed + index as f32).collect()
    }

    #[test]
    fn schema_bootstraps_and_vec_extension_loads() {
        let graph = KnowledgeGraph::open_in_memory().expect("graph");
        // Verify that the graph bootstrapped and the vec extension loaded by
        // checking that node_count works (requires schema) and that vector
        // search doesn't panic (requires sqlite-vec extension).
        assert_eq!(graph.node_count().expect("node_count"), 0);
    }

    #[test]
    fn inserts_and_fetches_nodes() {
        let graph = KnowledgeGraph::open_in_memory().expect("graph");
        let node_id = graph
            .insert_node(&NewNode {
                node_type: "concept".to_string(),
                name: "HiveMind OS".to_string(),
                data_class: DataClass::Internal,
                content: Some("HiveMind OS is a daemon-first desktop AI agent app.".to_string()),
            })
            .expect("insert node");

        let node = graph.get_node(node_id).expect("get node").expect("node exists");
        assert_eq!(node.name, "HiveMind OS");
        assert_eq!(node.data_class, DataClass::Internal);
    }

    #[test]
    fn full_text_search_returns_inserted_node() {
        let graph = KnowledgeGraph::open_in_memory().expect("graph");
        graph
            .insert_node(&NewNode {
                node_type: "fact".to_string(),
                name: "Rust fact".to_string(),
                data_class: DataClass::Public,
                content: Some("Rust powers the HiveMind OS daemon.".to_string()),
            })
            .expect("insert node");

        let matches = graph.search_text("HiveMind OS daemon", 10).expect("fts search");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].name, "Rust fact");
    }

    #[test]
    fn effective_class_propagates_from_ancestors() {
        let graph = KnowledgeGraph::open_in_memory().expect("graph");
        let grandparent = graph
            .insert_node(&NewNode {
                node_type: "entity".to_string(),
                name: "Grandparent".to_string(),
                data_class: DataClass::Restricted,
                content: None,
            })
            .expect("insert grandparent");
        let parent = graph
            .insert_node(&NewNode {
                node_type: "entity".to_string(),
                name: "Parent".to_string(),
                data_class: DataClass::Public,
                content: None,
            })
            .expect("insert parent");
        let child = graph
            .insert_node(&NewNode {
                node_type: "entity".to_string(),
                name: "Child".to_string(),
                data_class: DataClass::Public,
                content: None,
            })
            .expect("insert child");

        graph.insert_edge(child, parent, "child_of", 1.0).expect("child->parent");
        graph.insert_edge(parent, grandparent, "child_of", 1.0).expect("parent->grandparent");

        assert_eq!(graph.effective_class(child).expect("effective class"), DataClass::Restricted);
    }

    #[test]
    fn filtered_vector_search_respects_classification() {
        let graph = KnowledgeGraph::open_in_memory().expect("graph");

        let public_id = graph
            .insert_node(&NewNode {
                node_type: "concept".to_string(),
                name: "Public node".to_string(),
                data_class: DataClass::Public,
                content: Some("public".to_string()),
            })
            .expect("insert public");
        let restricted_id = graph
            .insert_node(&NewNode {
                node_type: "concept".to_string(),
                name: "Restricted node".to_string(),
                data_class: DataClass::Restricted,
                content: Some("restricted".to_string()),
            })
            .expect("insert restricted");

        graph
            .set_embedding(public_id, &embedding(0.1), "test-model")
            .expect("set public embedding");
        graph
            .set_embedding(restricted_id, &embedding(0.2), "test-model")
            .expect("set restricted embedding");

        let results = graph
            .search_similar(&embedding(0.1), "test-model", DataClass::Public, 10)
            .expect("vector search");

        assert!(results.iter().all(|result| result.data_class == DataClass::Public));
        assert!(results.iter().any(|result| result.id == public_id));
        assert!(!results.iter().any(|result| result.id == restricted_id));
    }

    #[test]
    fn search_similar_filtered_respects_max_distance() {
        let graph = KnowledgeGraph::open_in_memory().expect("graph");

        let close_id = graph
            .insert_node(&NewNode {
                node_type: "concept".to_string(),
                name: "close".to_string(),
                data_class: DataClass::Public,
                content: None,
            })
            .expect("insert close");
        let far_id = graph
            .insert_node(&NewNode {
                node_type: "concept".to_string(),
                name: "far".to_string(),
                data_class: DataClass::Public,
                content: None,
            })
            .expect("insert far");

        // Embeddings: close node is near the query, far node is distant.
        graph.set_embedding(close_id, &embedding(1.0), "test-model").expect("set close embedding");
        graph.set_embedding(far_id, &embedding(100.0), "test-model").expect("set far embedding");

        // Without threshold: both returned.
        let all = graph
            .search_similar_filtered(
                &embedding(1.0),
                "test-model",
                DataClass::Public,
                None,
                None,
                10,
            )
            .expect("search no threshold");
        assert_eq!(all.len(), 2);

        // The close node should have distance ~0, the far node much larger.
        let close_result = all.iter().find(|r| r.id == close_id).expect("close present");
        let far_result = all.iter().find(|r| r.id == far_id).expect("far present");
        assert!(close_result.distance < far_result.distance);

        // With a tight threshold: only close node returned.
        let tight = graph
            .search_similar_filtered(
                &embedding(1.0),
                "test-model",
                DataClass::Public,
                None,
                Some(close_result.distance + 0.1),
                10,
            )
            .expect("search tight threshold");
        assert_eq!(tight.len(), 1);
        assert_eq!(tight[0].id, close_id);
    }

    #[test]
    fn remove_node_deletes_node_and_edges() {
        let graph = KnowledgeGraph::open_in_memory().expect("graph");
        let a = graph
            .insert_node(&NewNode {
                node_type: "concept".to_string(),
                name: "A".to_string(),
                data_class: DataClass::Public,
                content: None,
            })
            .expect("insert A");
        let b = graph
            .insert_node(&NewNode {
                node_type: "concept".to_string(),
                name: "B".to_string(),
                data_class: DataClass::Public,
                content: None,
            })
            .expect("insert B");
        graph.insert_edge(a, b, "related_to", 1.0).expect("edge");
        assert!(graph.remove_node(a).expect("remove A"));
        assert!(graph.get_node(a).expect("get A").is_none());
        assert!(graph.get_edges_for_node(b).expect("edges for B").is_empty());
    }

    #[test]
    fn remove_edge_deletes_edge() {
        let graph = KnowledgeGraph::open_in_memory().expect("graph");
        let a = graph
            .insert_node(&NewNode {
                node_type: "concept".to_string(),
                name: "A".to_string(),
                data_class: DataClass::Public,
                content: None,
            })
            .expect("insert A");
        let b = graph
            .insert_node(&NewNode {
                node_type: "concept".to_string(),
                name: "B".to_string(),
                data_class: DataClass::Public,
                content: None,
            })
            .expect("insert B");
        let edge_id = graph.insert_edge(a, b, "related_to", 1.0).expect("edge");
        assert!(graph.remove_edge(edge_id).expect("remove edge"));
        assert!(graph.get_edges_for_node(a).expect("edges").is_empty());
    }

    #[test]
    fn list_nodes_filters_by_type() {
        let graph = KnowledgeGraph::open_in_memory().expect("graph");
        graph
            .insert_node(&NewNode {
                node_type: "person".to_string(),
                name: "Alice".to_string(),
                data_class: DataClass::Public,
                content: None,
            })
            .expect("insert");
        graph
            .insert_node(&NewNode {
                node_type: "concept".to_string(),
                name: "Rust".to_string(),
                data_class: DataClass::Public,
                content: None,
            })
            .expect("insert");

        let persons = graph.list_nodes(Some("person"), None, 50).expect("list");
        assert_eq!(persons.len(), 1);
        assert_eq!(persons[0].name, "Alice");

        let all = graph.list_nodes(None, None, 50).expect("list all");
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn update_node_content_replaces_content() {
        let graph = KnowledgeGraph::open_in_memory().expect("graph");
        let node_id = graph
            .insert_node(&NewNode {
                node_type: "note".to_string(),
                name: "session-1".to_string(),
                data_class: DataClass::Internal,
                content: Some("before".to_string()),
            })
            .expect("insert node");

        graph.update_node_content(node_id, "after").expect("update node content");

        let node = graph.get_node(node_id).expect("get node").expect("node exists");
        assert_eq!(node.content.as_deref(), Some("after"));
        assert!(!node.updated_at.is_empty());
    }

    #[test]
    fn list_nodes_by_type_returns_matching_nodes_in_id_order() {
        let graph = KnowledgeGraph::open_in_memory().expect("graph");
        graph
            .insert_node(&NewNode {
                node_type: "chat_session".to_string(),
                name: "session-1".to_string(),
                data_class: DataClass::Internal,
                content: None,
            })
            .expect("insert first session");
        graph
            .insert_node(&NewNode {
                node_type: "chat_message".to_string(),
                name: "msg-1".to_string(),
                data_class: DataClass::Internal,
                content: None,
            })
            .expect("insert message");
        graph
            .insert_node(&NewNode {
                node_type: "chat_session".to_string(),
                name: "session-2".to_string(),
                data_class: DataClass::Internal,
                content: None,
            })
            .expect("insert second session");

        let sessions = graph.list_nodes_by_type("chat_session").expect("list nodes by type");

        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].name, "session-1");
        assert_eq!(sessions[1].name, "session-2");
        assert!(!sessions[0].created_at.is_empty());
    }

    #[test]
    fn stats_methods_return_correct_counts() {
        let graph = KnowledgeGraph::open_in_memory().expect("graph");
        let a = graph
            .insert_node(&NewNode {
                node_type: "person".to_string(),
                name: "A".to_string(),
                data_class: DataClass::Public,
                content: None,
            })
            .expect("insert");
        let b = graph
            .insert_node(&NewNode {
                node_type: "concept".to_string(),
                name: "B".to_string(),
                data_class: DataClass::Public,
                content: None,
            })
            .expect("insert");
        graph.insert_edge(a, b, "related_to", 1.0).expect("edge");

        assert_eq!(graph.node_count().expect("count"), 2);
        assert_eq!(graph.edge_count().expect("count"), 1);

        let by_type = graph.node_counts_by_type().expect("counts");
        assert_eq!(by_type.len(), 2);
    }

    #[test]
    fn embedding_meta_tracks_model_id() {
        let graph = KnowledgeGraph::open_in_memory().expect("graph");
        let node_id = graph
            .insert_node(&NewNode {
                node_type: "concept".to_string(),
                name: "Test".to_string(),
                data_class: DataClass::Public,
                content: Some("test content".to_string()),
            })
            .expect("insert");

        graph.set_embedding(node_id, &embedding(1.0), "bge-small-en-v1.5").expect("set embedding");

        let model: String = graph
            .connection
            .query_row(
                "SELECT model_id FROM embedding_meta WHERE node_id = ?1",
                params![node_id],
                |row| row.get(0),
            )
            .expect("query embedding_meta");
        assert_eq!(model, "bge-small-en-v1.5");
    }

    #[test]
    fn nodes_needing_embedding_finds_missing_and_stale() {
        let graph = KnowledgeGraph::open_in_memory().expect("graph");
        let n1 = graph
            .insert_node(&NewNode {
                node_type: "concept".to_string(),
                name: "N1".to_string(),
                data_class: DataClass::Public,
                content: None,
            })
            .expect("insert");
        let n2 = graph
            .insert_node(&NewNode {
                node_type: "concept".to_string(),
                name: "N2".to_string(),
                data_class: DataClass::Public,
                content: None,
            })
            .expect("insert");
        let n3 = graph
            .insert_node(&NewNode {
                node_type: "concept".to_string(),
                name: "N3".to_string(),
                data_class: DataClass::Public,
                content: None,
            })
            .expect("insert");

        graph.set_embedding(n1, &embedding(1.0), "current-model").expect("embed n1");
        graph.set_embedding(n2, &embedding(2.0), "old-model").expect("embed n2");

        let needing = graph.nodes_needing_embedding("current-model", &[], 100).expect("needing");
        assert_eq!(needing.len(), 2);
        assert!(needing.contains(&n2));
        assert!(needing.contains(&n3));
        assert!(!needing.contains(&n1));
    }

    #[test]
    fn embedding_stats_reports_correctly() {
        let graph = KnowledgeGraph::open_in_memory().expect("graph");
        let n1 = graph
            .insert_node(&NewNode {
                node_type: "concept".to_string(),
                name: "N1".to_string(),
                data_class: DataClass::Public,
                content: None,
            })
            .expect("insert");
        let n2 = graph
            .insert_node(&NewNode {
                node_type: "concept".to_string(),
                name: "N2".to_string(),
                data_class: DataClass::Public,
                content: None,
            })
            .expect("insert");
        let _n3 = graph
            .insert_node(&NewNode {
                node_type: "concept".to_string(),
                name: "N3".to_string(),
                data_class: DataClass::Public,
                content: None,
            })
            .expect("insert");

        graph.set_embedding(n1, &embedding(1.0), "model-v2").expect("embed n1");
        graph.set_embedding(n2, &embedding(2.0), "model-v1").expect("embed n2");

        let stats = graph.embedding_stats("model-v2", &[]).expect("stats");
        assert_eq!(stats.total_nodes, 3);
        assert_eq!(stats.embedded, 1);
        assert_eq!(stats.stale, 1);
        assert_eq!(stats.missing, 1);
    }

    #[test]
    fn prepare_reindex_clears_meta_and_updates_dims() {
        let mut graph = KnowledgeGraph::open_in_memory().expect("graph");
        let n1 = graph
            .insert_node(&NewNode {
                node_type: "concept".to_string(),
                name: "N1".to_string(),
                data_class: DataClass::Public,
                content: None,
            })
            .expect("insert");

        graph.set_embedding(n1, &embedding(1.0), "old-model").expect("embed");
        assert_eq!(graph.embedding_dimensions(), 384);

        graph.prepare_reindex("new-model", 384).expect("prepare reindex same dim");
        assert_eq!(graph.embedding_dimensions(), 384);
        let stats = graph.embedding_stats("new-model", &[]).expect("stats");
        assert_eq!(stats.embedded, 0);
        // Node still has embedding from "old-model" → stale from new-model's perspective
        assert_eq!(stats.stale, 1);

        let (model_id, dims) = graph.embedding_model().expect("model").expect("model set");
        assert_eq!(model_id, "new-model");
        assert_eq!(dims, 384);
    }

    #[test]
    fn prepare_reindex_with_new_dimensions_recreates_vec_table() {
        let mut graph = KnowledgeGraph::open_in_memory().expect("graph");
        let n1 = graph
            .insert_node(&NewNode {
                node_type: "concept".to_string(),
                name: "N1".to_string(),
                data_class: DataClass::Public,
                content: None,
            })
            .expect("insert");

        graph.set_embedding(n1, &embedding(1.0), "old-model").expect("embed");

        graph.prepare_reindex("large-model", 768).expect("prepare reindex new dim");
        assert_eq!(graph.embedding_dimensions(), 768);

        let stats = graph.embedding_stats("large-model", &[]).expect("stats");
        assert_eq!(stats.embedded, 0);
        // old-model embedding still present → stale
        assert_eq!(stats.stale, 1);
        assert_eq!(stats.missing, 0);

        let emb_768: Vec<f32> = (0..768).map(|i| i as f32).collect();
        graph.set_embedding(n1, &emb_768, "large-model").expect("embed 768");
        let stats = graph.embedding_stats("large-model", &[]).expect("stats after embed");
        assert_eq!(stats.embedded, 1);
    }

    #[test]
    fn embedding_model_returns_none_when_unconfigured() {
        let graph = KnowledgeGraph::open_in_memory().expect("graph");
        assert!(graph.embedding_model().expect("model").is_none());
    }

    #[test]
    fn default_embedding_dimensions_is_384() {
        let graph = KnowledgeGraph::open_in_memory().expect("graph");
        assert_eq!(graph.embedding_dimensions(), 384);
    }

    #[test]
    fn scrub_session_messages_deletes_linked_nodes() {
        let graph = KnowledgeGraph::open_in_memory().expect("graph");

        // Create a session node
        let session_id = graph
            .insert_node(&NewNode {
                node_type: "chat_session".to_string(),
                name: "test session".to_string(),
                data_class: DataClass::Internal,
                content: None,
            })
            .expect("insert session");

        // Create message nodes
        let msg1 = graph
            .insert_node(&NewNode {
                node_type: "chat_message".to_string(),
                name: "msg1".to_string(),
                data_class: DataClass::Internal,
                content: Some("hello world".to_string()),
            })
            .expect("insert msg1");
        let msg2 = graph
            .insert_node(&NewNode {
                node_type: "chat_message".to_string(),
                name: "msg2".to_string(),
                data_class: DataClass::Internal,
                content: Some("goodbye world".to_string()),
            })
            .expect("insert msg2");

        // Link them via session_message edges
        graph.insert_edge(session_id, msg1, "session_message", 1.0).expect("edge1");
        graph.insert_edge(session_id, msg2, "session_message", 1.0).expect("edge2");

        assert_eq!(graph.node_count().unwrap(), 3);

        // Scrub
        let removed = graph.scrub_session_messages(session_id).expect("scrub");
        assert_eq!(removed, 2);

        // Message nodes are gone, session node remains
        assert!(graph.get_node(msg1).expect("get").is_none());
        assert!(graph.get_node(msg2).expect("get").is_none());
        assert!(graph.get_node(session_id).expect("get").is_some());
        assert_eq!(graph.node_count().unwrap(), 1);
    }

    #[test]
    fn scrub_session_messages_returns_zero_when_no_messages() {
        let graph = KnowledgeGraph::open_in_memory().expect("graph");
        let session_id = graph
            .insert_node(&NewNode {
                node_type: "chat_session".to_string(),
                name: "empty session".to_string(),
                data_class: DataClass::Internal,
                content: None,
            })
            .expect("insert session");

        let removed = graph.scrub_session_messages(session_id).expect("scrub");
        assert_eq!(removed, 0);
    }

    #[test]
    fn get_node_with_neighbors_returns_full_neighborhood() {
        let graph = KnowledgeGraph::open_in_memory().expect("graph");

        let center = graph
            .insert_node(&NewNode {
                node_type: "concept".to_string(),
                name: "center".to_string(),
                data_class: DataClass::Internal,
                content: Some("center node".to_string()),
            })
            .expect("insert center");
        let left = graph
            .insert_node(&NewNode {
                node_type: "concept".to_string(),
                name: "left".to_string(),
                data_class: DataClass::Internal,
                content: Some("left node".to_string()),
            })
            .expect("insert left");
        let right = graph
            .insert_node(&NewNode {
                node_type: "concept".to_string(),
                name: "right".to_string(),
                data_class: DataClass::Internal,
                content: Some("right node".to_string()),
            })
            .expect("insert right");

        graph.insert_edge(center, left, "related", 1.0).expect("edge1");
        graph.insert_edge(right, center, "depends_on", 0.5).expect("edge2");

        let hood = graph.get_node_with_neighbors(center, 10).expect("neighborhood");
        assert_eq!(hood.node.name, "center");
        assert_eq!(hood.edges.len(), 2);
        assert_eq!(hood.neighbors.len(), 2);

        let neighbor_names: std::collections::HashSet<&str> =
            hood.neighbors.iter().map(|n| n.name.as_str()).collect();
        assert!(neighbor_names.contains("left"));
        assert!(neighbor_names.contains("right"));
    }

    #[test]
    fn get_node_with_neighbors_respects_limit() {
        let graph = KnowledgeGraph::open_in_memory().expect("graph");

        let center = graph
            .insert_node(&NewNode {
                node_type: "hub".to_string(),
                name: "hub".to_string(),
                data_class: DataClass::Internal,
                content: None,
            })
            .expect("insert hub");

        for i in 0..5 {
            let n = graph
                .insert_node(&NewNode {
                    node_type: "spoke".to_string(),
                    name: format!("spoke-{i}"),
                    data_class: DataClass::Internal,
                    content: None,
                })
                .expect("insert spoke");
            graph.insert_edge(center, n, "spoke", 1.0).expect("edge");
        }

        let hood = graph.get_node_with_neighbors(center, 3).expect("neighborhood");
        assert_eq!(hood.edges.len(), 5); // all edges returned
        assert_eq!(hood.neighbors.len(), 3); // but neighbors capped at limit
    }

    #[test]
    fn collect_session_node_ids_traverses_tree() {
        let graph = KnowledgeGraph::open_in_memory().expect("graph");

        // Build a session → workspace → dir → file → chunk hierarchy
        let session = graph
            .insert_node(&NewNode {
                node_type: "session".into(),
                name: "s1".into(),
                data_class: DataClass::Internal,
                content: None,
            })
            .unwrap();

        let msg = graph
            .insert_node(&NewNode {
                node_type: "chat_message".into(),
                name: "msg1".into(),
                data_class: DataClass::Internal,
                content: Some("hello".into()),
            })
            .unwrap();
        graph.insert_edge(session, msg, "session_message", 1.0).unwrap();

        let ws_root = graph
            .insert_node(&NewNode {
                node_type: "workspace_dir".into(),
                name: "/".into(),
                data_class: DataClass::Internal,
                content: None,
            })
            .unwrap();
        graph.insert_edge(session, ws_root, "session_workspace", 1.0).unwrap();

        let dir = graph
            .insert_node(&NewNode {
                node_type: "workspace_dir".into(),
                name: "src".into(),
                data_class: DataClass::Internal,
                content: None,
            })
            .unwrap();
        graph.insert_edge(ws_root, dir, "contains_dir", 1.0).unwrap();

        let file = graph
            .insert_node(&NewNode {
                node_type: "workspace_file".into(),
                name: "src/main.rs".into(),
                data_class: DataClass::Internal,
                content: None,
            })
            .unwrap();
        graph.insert_edge(dir, file, "contains_file", 1.0).unwrap();

        let chunk = graph
            .insert_node(&NewNode {
                node_type: "file_chunk".into(),
                name: "src/main.rs#chunk0".into(),
                data_class: DataClass::Internal,
                content: Some("fn main() {}".into()),
            })
            .unwrap();
        graph.insert_edge(file, chunk, "file_chunk", 1.0).unwrap();

        // Unrelated node (different session) — should NOT be included
        let other = graph
            .insert_node(&NewNode {
                node_type: "chat_message".into(),
                name: "other_msg".into(),
                data_class: DataClass::Internal,
                content: Some("other".into()),
            })
            .unwrap();

        let ids = graph.collect_session_node_ids(session).unwrap();

        assert!(ids.contains(&session), "should include session node itself");
        assert!(ids.contains(&msg), "should include session message");
        assert!(ids.contains(&ws_root), "should include workspace root");
        assert!(ids.contains(&dir), "should include workspace dir");
        assert!(ids.contains(&file), "should include workspace file");
        assert!(ids.contains(&chunk), "should include file chunk");
        assert!(!ids.contains(&other), "should NOT include unrelated node");
        assert_eq!(ids.len(), 6);
    }
}
