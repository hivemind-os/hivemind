//! Lightweight connection pool for [`KnowledgeGraph`].
//!
//! `KnowledgeGraph` wraps a `rusqlite::Connection` which is `Send` but not
//! `Sync`. Each `spawn_blocking` task therefore needs its own instance.
//! Without pooling, every task opens a fresh SQLite connection and re-runs
//! schema bootstrapping — this module avoids that overhead by reusing idle
//! connections.
//!
//! # Usage
//!
//! ```ignore
//! let pool = Arc::new(KgPool::new(&path));
//! let p = Arc::clone(&pool);
//! tokio::task::spawn_blocking(move || {
//!     let graph = p.get()?; // checkout (or open new)
//!     graph.insert_node(&node)?;
//!     // `graph` is returned to the pool on drop
//!     Ok(())
//! });
//! ```

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::KnowledgeGraph;

/// Maximum number of idle connections kept in the pool.
/// Keep low to minimise file-descriptor usage — each SQLite connection
/// holds 3 FDs (db + WAL + SHM).
const DEFAULT_MAX_IDLE: usize = 1;

/// A reusable pool of [`KnowledgeGraph`] connections for a single database
/// path. Thread-safe (`Send + Sync`) and designed to be shared via `Arc`.
pub struct KgPool {
    path: PathBuf,
    idle: Mutex<Vec<KnowledgeGraph>>,
    max_idle: usize,
}

impl KgPool {
    /// Create a new pool for the given database path.
    ///
    /// No connections are opened eagerly — the first [`get`](Self::get) call
    /// will open the initial connection.
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            idle: Mutex::new(Vec::new()),
            max_idle: DEFAULT_MAX_IDLE,
        }
    }

    /// Checkout a connection from the pool.
    ///
    /// Returns an idle connection if one is available, otherwise opens a new
    /// one. The returned [`PooledKg`] automatically returns the connection to
    /// the pool when dropped.
    pub fn get(self: &std::sync::Arc<Self>) -> anyhow::Result<PooledKg> {
        let graph = {
            let mut idle = self.idle.lock().expect("KgPool lock poisoned");
            idle.pop()
        };
        let graph = match graph {
            Some(g) => g,
            None => KnowledgeGraph::open(&self.path)?,
        };
        Ok(PooledKg { owner: std::sync::Arc::clone(self), graph: Some(graph) })
    }

    /// Return a connection to the pool. Called automatically by [`PooledKg`]
    /// on drop, but can also be called manually.
    fn checkin(&self, graph: KnowledgeGraph) {
        let mut idle = match self.idle.lock() {
            Ok(g) => g,
            Err(_) => return, // poisoned — just drop the connection
        };
        if idle.len() < self.max_idle {
            idle.push(graph);
        }
        // else: drop the connection to keep the pool bounded
    }
}

/// RAII guard that dereferences to [`KnowledgeGraph`] and returns the
/// connection to the pool on drop.
///
/// This type is `Send` (because both `Arc<KgPool>` and `KnowledgeGraph`
/// are `Send`) so it can be moved into `spawn_blocking` closures.
pub struct PooledKg {
    owner: std::sync::Arc<KgPool>,
    graph: Option<KnowledgeGraph>,
}

impl Drop for PooledKg {
    fn drop(&mut self) {
        if let Some(g) = self.graph.take() {
            self.owner.checkin(g);
        }
    }
}

impl std::ops::Deref for PooledKg {
    type Target = KnowledgeGraph;
    fn deref(&self) -> &KnowledgeGraph {
        self.graph.as_ref().expect("PooledKg used after take")
    }
}

impl std::ops::DerefMut for PooledKg {
    fn deref_mut(&mut self) -> &mut KnowledgeGraph {
        self.graph.as_mut().expect("PooledKg used after take")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NewNode;
    use hive_classification::model::DataClass;
    use std::sync::Arc;

    #[test]
    fn pool_reuses_connections() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let pool = Arc::new(KgPool::new(&db_path));

        // First checkout opens a new connection
        {
            let graph = pool.get().unwrap();
            graph
                .insert_node(&NewNode {
                    node_type: "test".to_string(),
                    name: "n1".to_string(),
                    data_class: DataClass::Public,
                    content: None,
                })
                .unwrap();
        } // returned to pool

        // Second checkout reuses the connection
        {
            let graph = pool.get().unwrap();
            let node = graph.find_node_by_type_and_name("test", "n1").unwrap();
            assert!(node.is_some(), "should find node from reused connection");
        }
    }

    #[test]
    fn pool_handles_concurrent_checkouts() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("concurrent.db");
        let pool = Arc::new(KgPool::new(&db_path));

        // Checkout two connections simultaneously
        let g1 = pool.get().unwrap();
        let g2 = pool.get().unwrap();

        g1.insert_node(&NewNode {
            node_type: "test".to_string(),
            name: "from_g1".to_string(),
            data_class: DataClass::Public,
            content: None,
        })
        .unwrap();

        // g2 can see g1's insert (WAL mode)
        let node = g2.find_node_by_type_and_name("test", "from_g1").unwrap();
        assert!(node.is_some());

        drop(g1);
        drop(g2);
    }

    #[test]
    fn pool_bounds_idle_connections() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("bounded.db");
        let pool = Arc::new(KgPool::new(&db_path));

        // Checkout more than max_idle connections
        let mut guards: Vec<PooledKg> = Vec::new();
        for _ in 0..(DEFAULT_MAX_IDLE + 2) {
            guards.push(pool.get().unwrap());
        }

        // Return all — pool should cap at max_idle
        drop(guards);

        let idle = pool.idle.lock().unwrap();
        assert!(
            idle.len() <= DEFAULT_MAX_IDLE,
            "pool should not exceed max_idle, got {}",
            idle.len()
        );
    }
}
