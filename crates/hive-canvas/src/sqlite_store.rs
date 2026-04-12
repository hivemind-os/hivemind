use std::collections::{HashSet, VecDeque};
use std::path::Path;

use parking_lot::Mutex;
use rstar::RTree;
use rusqlite::{params, Connection};

use crate::error::CanvasError;
use crate::store::CanvasStore;
use crate::types::*;

#[derive(Clone, Debug)]
struct SpatialEntry {
    id: String,
    canvas_id: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl rstar::RTreeObject for SpatialEntry {
    type Envelope = rstar::AABB<[f64; 2]>;

    fn envelope(&self) -> Self::Envelope {
        rstar::AABB::from_corners([self.x, self.y], [self.x + self.width, self.y + self.height])
    }
}

impl rstar::PointDistance for SpatialEntry {
    fn distance_2(&self, point: &[f64; 2]) -> f64 {
        // Distance from point to the center of the node
        let cx = self.x + self.width / 2.0;
        let cy = self.y + self.height / 2.0;
        let dx = cx - point[0];
        let dy = cy - point[1];
        dx * dx + dy * dy
    }
}

impl PartialEq for SpatialEntry {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for SpatialEntry {}

pub struct SqliteCanvasStore {
    conn: Mutex<Connection>,
    rtree: Mutex<RTree<SpatialEntry>>,
}

impl SqliteCanvasStore {
    /// Open or create a canvas store backed by a file.
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self, CanvasError> {
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    /// Create an in-memory canvas store (useful for tests).
    pub fn in_memory() -> Result<Self, CanvasError> {
        let conn = Connection::open_in_memory()?;
        Self::init(conn)
    }

    fn init(conn: Connection) -> Result<Self, CanvasError> {
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        Self::create_tables(&conn)?;
        let entries = Self::load_spatial_entries(&conn)?;
        let rtree = RTree::bulk_load(entries);
        Ok(Self { conn: Mutex::new(conn), rtree: Mutex::new(rtree) })
    }

    fn create_tables(conn: &Connection) -> Result<(), CanvasError> {
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS canvas_nodes (
                id TEXT PRIMARY KEY,
                canvas_id TEXT NOT NULL,
                card_type TEXT NOT NULL,
                x REAL NOT NULL,
                y REAL NOT NULL,
                width REAL NOT NULL DEFAULT 280.0,
                height REAL NOT NULL DEFAULT 120.0,
                content TEXT NOT NULL DEFAULT '{}',
                status TEXT NOT NULL DEFAULT 'active',
                created_by TEXT NOT NULL DEFAULT '',
                created_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_nodes_canvas ON canvas_nodes(canvas_id);

            CREATE TABLE IF NOT EXISTS canvas_edges (
                id TEXT PRIMARY KEY,
                canvas_id TEXT NOT NULL,
                source_id TEXT NOT NULL REFERENCES canvas_nodes(id) ON DELETE CASCADE,
                target_id TEXT NOT NULL REFERENCES canvas_nodes(id) ON DELETE CASCADE,
                edge_type TEXT NOT NULL,
                metadata TEXT NOT NULL DEFAULT '{}',
                created_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_edges_canvas ON canvas_edges(canvas_id);
            CREATE INDEX IF NOT EXISTS idx_edges_source ON canvas_edges(source_id);
            CREATE INDEX IF NOT EXISTS idx_edges_target ON canvas_edges(target_id);
            ",
        )?;
        Ok(())
    }

    fn load_spatial_entries(conn: &Connection) -> Result<Vec<SpatialEntry>, CanvasError> {
        let mut stmt =
            conn.prepare("SELECT id, canvas_id, x, y, width, height FROM canvas_nodes")?;
        let entries = stmt
            .query_map([], |row| {
                Ok(SpatialEntry {
                    id: row.get(0)?,
                    canvas_id: row.get(1)?,
                    x: row.get(2)?,
                    y: row.get(3)?,
                    width: row.get(4)?,
                    height: row.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(entries)
    }

    fn row_to_node(row: &rusqlite::Row<'_>) -> rusqlite::Result<CanvasNode> {
        let card_type_str: String = row.get(2)?;
        let content_str: String = row.get(7)?;
        let status_str: String = row.get(8)?;

        Ok(CanvasNode {
            id: row.get(0)?,
            canvas_id: row.get(1)?,
            card_type: CardType::from_str(&card_type_str).unwrap_or_else(|| {
                tracing::warn!("invalid card_type '{}', defaulting to Prompt", card_type_str);
                CardType::Prompt
            }),
            x: row.get(3)?,
            y: row.get(4)?,
            width: row.get(5)?,
            height: row.get(6)?,
            content: serde_json::from_str(&content_str).unwrap_or_else(|e| {
                tracing::warn!("invalid node content JSON: {e}");
                serde_json::Value::Null
            }),
            status: CardStatus::from_str(&status_str).unwrap_or_else(|| {
                tracing::warn!("invalid card status '{}', defaulting to Active", status_str);
                CardStatus::Active
            }),
            created_by: row.get(9)?,
            created_at: row.get(10)?,
        })
    }

    fn row_to_edge(row: &rusqlite::Row<'_>) -> rusqlite::Result<CanvasEdge> {
        let edge_type_str: String = row.get(4)?;
        let metadata_str: String = row.get(5)?;

        Ok(CanvasEdge {
            id: row.get(0)?,
            canvas_id: row.get(1)?,
            source_id: row.get(2)?,
            target_id: row.get(3)?,
            edge_type: EdgeType::from_str(&edge_type_str).unwrap_or_else(|| {
                tracing::warn!("invalid edge_type '{}', defaulting to ReplyTo", edge_type_str);
                EdgeType::ReplyTo
            }),
            metadata: serde_json::from_str(&metadata_str).unwrap_or_else(|e| {
                tracing::warn!("invalid edge metadata JSON: {e}");
                serde_json::Value::Null
            }),
            created_at: row.get(6)?,
        })
    }

    fn fetch_node(conn: &Connection, node_id: &str) -> Result<Option<CanvasNode>, CanvasError> {
        let mut stmt = conn.prepare_cached(
            "SELECT id, canvas_id, card_type, x, y, width, height, content, status, created_by, created_at
             FROM canvas_nodes WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(params![node_id], Self::row_to_node)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// Collect neighbor node IDs (both directions) from edges in SQLite.
    fn neighbor_ids(conn: &Connection, node_id: &str) -> Result<Vec<String>, CanvasError> {
        let mut stmt = conn.prepare_cached(
            "SELECT target_id FROM canvas_edges WHERE source_id = ?1
             UNION
             SELECT source_id FROM canvas_edges WHERE target_id = ?1",
        )?;
        let ids = stmt
            .query_map(params![node_id], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ids)
    }
}

impl CanvasStore for SqliteCanvasStore {
    fn insert_node(&self, node: &CanvasNode) -> Result<(), CanvasError> {
        let conn = self.conn.lock();
        let content_str = serde_json::to_string(&node.content)?;
        conn.execute(
            "INSERT INTO canvas_nodes (id, canvas_id, card_type, x, y, width, height, content, status, created_by, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                node.id,
                node.canvas_id,
                node.card_type.as_str(),
                node.x,
                node.y,
                node.width,
                node.height,
                content_str,
                node.status.as_str(),
                node.created_by,
                node.created_at,
            ],
        )?;
        drop(conn);

        let mut rtree = self.rtree.lock();
        rtree.insert(SpatialEntry {
            id: node.id.clone(),
            canvas_id: node.canvas_id.clone(),
            x: node.x,
            y: node.y,
            width: node.width,
            height: node.height,
        });
        Ok(())
    }

    fn insert_edge(&self, edge: &CanvasEdge) -> Result<(), CanvasError> {
        let conn = self.conn.lock();
        let metadata_str = serde_json::to_string(&edge.metadata)?;
        conn.execute(
            "INSERT INTO canvas_edges (id, canvas_id, source_id, target_id, edge_type, metadata, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                edge.id,
                edge.canvas_id,
                edge.source_id,
                edge.target_id,
                edge.edge_type.as_str(),
                metadata_str,
                edge.created_at,
            ],
        )?;
        Ok(())
    }

    fn update_node_position(&self, node_id: &str, x: f64, y: f64) -> Result<(), CanvasError> {
        let conn = self.conn.lock();
        let changed = conn.execute(
            "UPDATE canvas_nodes SET x = ?1, y = ?2 WHERE id = ?3",
            params![x, y, node_id],
        )?;
        if changed == 0 {
            return Err(CanvasError::NodeNotFound(node_id.to_string()));
        }

        // Read current width/height and canvas_id for R-tree update
        let mut stmt =
            conn.prepare_cached("SELECT canvas_id, width, height FROM canvas_nodes WHERE id = ?1")?;
        let (canvas_id, width, height): (String, f64, f64) =
            stmt.query_row(params![node_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        drop(stmt);
        drop(conn);

        let mut rtree = self.rtree.lock();
        // Remove old entry by finding and removing it
        let old_entry = rtree.iter().find(|e| e.id == node_id).cloned();
        if let Some(entry) = old_entry {
            rtree.remove(&entry);
        }
        rtree.insert(SpatialEntry { id: node_id.to_string(), canvas_id, x, y, width, height });
        Ok(())
    }

    fn update_node_content(
        &self,
        node_id: &str,
        content: &serde_json::Value,
    ) -> Result<(), CanvasError> {
        let conn = self.conn.lock();
        let content_str = serde_json::to_string(content)?;
        let changed = conn.execute(
            "UPDATE canvas_nodes SET content = ?1 WHERE id = ?2",
            params![content_str, node_id],
        )?;
        if changed == 0 {
            return Err(CanvasError::NodeNotFound(node_id.to_string()));
        }
        Ok(())
    }

    fn update_node_status(&self, node_id: &str, status: &CardStatus) -> Result<(), CanvasError> {
        let conn = self.conn.lock();
        let changed = conn.execute(
            "UPDATE canvas_nodes SET status = ?1 WHERE id = ?2",
            params![status.as_str(), node_id],
        )?;
        if changed == 0 {
            return Err(CanvasError::NodeNotFound(node_id.to_string()));
        }
        Ok(())
    }

    fn delete_node(&self, node_id: &str) -> Result<(), CanvasError> {
        let conn = self.conn.lock();
        // Delete edges first (foreign keys with CASCADE should handle this,
        // but be explicit for clarity and in case FK enforcement is off).
        conn.execute(
            "DELETE FROM canvas_edges WHERE source_id = ?1 OR target_id = ?1",
            params![node_id],
        )?;
        let changed = conn.execute("DELETE FROM canvas_nodes WHERE id = ?1", params![node_id])?;
        if changed == 0 {
            return Err(CanvasError::NodeNotFound(node_id.to_string()));
        }
        drop(conn);

        let mut rtree = self.rtree.lock();
        let old_entry = rtree.iter().find(|e| e.id == node_id).cloned();
        if let Some(entry) = old_entry {
            rtree.remove(&entry);
        }
        Ok(())
    }

    fn get_node(&self, node_id: &str) -> Result<Option<CanvasNode>, CanvasError> {
        let conn = self.conn.lock();
        Self::fetch_node(&conn, node_id)
    }

    fn get_edges_from(&self, node_id: &str) -> Result<Vec<CanvasEdge>, CanvasError> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare_cached(
            "SELECT id, canvas_id, source_id, target_id, edge_type, metadata, created_at
             FROM canvas_edges WHERE source_id = ?1",
        )?;
        let edges =
            stmt.query_map(params![node_id], Self::row_to_edge)?.collect::<Result<Vec<_>, _>>()?;
        Ok(edges)
    }

    fn get_edges_to(&self, node_id: &str) -> Result<Vec<CanvasEdge>, CanvasError> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare_cached(
            "SELECT id, canvas_id, source_id, target_id, edge_type, metadata, created_at
             FROM canvas_edges WHERE target_id = ?1",
        )?;
        let edges =
            stmt.query_map(params![node_id], Self::row_to_edge)?.collect::<Result<Vec<_>, _>>()?;
        Ok(edges)
    }

    fn query_viewport(
        &self,
        canvas_id: &str,
        min_x: f64,
        min_y: f64,
        max_x: f64,
        max_y: f64,
    ) -> Result<Vec<CanvasNode>, CanvasError> {
        let rtree = self.rtree.lock();
        let envelope = rstar::AABB::from_corners([min_x, min_y], [max_x, max_y]);
        let ids: Vec<String> = rtree
            .locate_in_envelope_intersecting(&envelope)
            .filter(|e| e.canvas_id == canvas_id)
            .map(|e| e.id.clone())
            .collect();
        drop(rtree);

        let conn = self.conn.lock();
        let mut nodes = Vec::with_capacity(ids.len());
        for id in &ids {
            if let Some(node) = Self::fetch_node(&conn, id)? {
                nodes.push(node);
            }
        }
        Ok(nodes)
    }

    fn query_radius(
        &self,
        canvas_id: &str,
        cx: f64,
        cy: f64,
        radius: f64,
    ) -> Result<Vec<CanvasNode>, CanvasError> {
        let rtree = self.rtree.lock();
        let point = [cx, cy];
        let radius_sq = radius * radius;
        let ids: Vec<String> = rtree
            .locate_within_distance(point, radius_sq)
            .filter(|e| e.canvas_id == canvas_id)
            .map(|e| e.id.clone())
            .collect();
        drop(rtree);

        let conn = self.conn.lock();
        let mut nodes = Vec::with_capacity(ids.len());
        for id in &ids {
            if let Some(node) = Self::fetch_node(&conn, id)? {
                nodes.push(node);
            }
        }
        Ok(nodes)
    }

    fn query_nearest(
        &self,
        canvas_id: &str,
        cx: f64,
        cy: f64,
        k: usize,
    ) -> Result<Vec<CanvasNode>, CanvasError> {
        let rtree = self.rtree.lock();
        let point = [cx, cy];
        // nearest_neighbor_iter returns all entries sorted by distance;
        // filter by canvas_id and take k.
        let ids: Vec<String> = rtree
            .nearest_neighbor_iter(&point)
            .filter(|e| e.canvas_id == canvas_id)
            .take(k)
            .map(|e| e.id.clone())
            .collect();
        drop(rtree);

        let conn = self.conn.lock();
        let mut nodes = Vec::with_capacity(ids.len());
        for id in &ids {
            if let Some(node) = Self::fetch_node(&conn, id)? {
                nodes.push(node);
            }
        }
        Ok(nodes)
    }

    fn bfs(&self, start_id: &str, max_depth: usize) -> Result<Vec<CanvasNode>, CanvasError> {
        let conn = self.conn.lock();

        // Verify start node exists
        if Self::fetch_node(&conn, start_id)?.is_none() {
            return Err(CanvasError::NodeNotFound(start_id.to_string()));
        }

        let mut visited = HashSet::new();
        let mut queue: VecDeque<(String, usize)> = VecDeque::new();
        visited.insert(start_id.to_string());
        queue.push_back((start_id.to_string(), 0));

        let mut result_ids = Vec::new();

        while let Some((current_id, depth)) = queue.pop_front() {
            result_ids.push(current_id.clone());
            if depth >= max_depth {
                continue;
            }
            let neighbors = Self::neighbor_ids(&conn, &current_id)?;
            for neighbor_id in neighbors {
                if visited.insert(neighbor_id.clone()) {
                    queue.push_back((neighbor_id, depth + 1));
                }
            }
        }

        let mut nodes = Vec::with_capacity(result_ids.len());
        for id in &result_ids {
            if let Some(node) = Self::fetch_node(&conn, id)? {
                nodes.push(node);
            }
        }
        Ok(nodes)
    }

    fn connected_component(&self, node_id: &str) -> Result<Vec<CanvasNode>, CanvasError> {
        let conn = self.conn.lock();

        if Self::fetch_node(&conn, node_id)?.is_none() {
            return Err(CanvasError::NodeNotFound(node_id.to_string()));
        }

        let mut visited = HashSet::new();
        let mut queue: VecDeque<String> = VecDeque::new();
        visited.insert(node_id.to_string());
        queue.push_back(node_id.to_string());

        while let Some(current_id) = queue.pop_front() {
            let neighbors = Self::neighbor_ids(&conn, &current_id)?;
            for neighbor_id in neighbors {
                if visited.insert(neighbor_id.clone()) {
                    queue.push_back(neighbor_id);
                }
            }
        }

        let mut nodes = Vec::with_capacity(visited.len());
        for id in &visited {
            if let Some(node) = Self::fetch_node(&conn, id)? {
                nodes.push(node);
            }
        }
        Ok(nodes)
    }

    fn get_all_nodes(&self, canvas_id: &str) -> Result<Vec<CanvasNode>, CanvasError> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare_cached(
            "SELECT id, canvas_id, card_type, x, y, width, height, content, status, created_by, created_at
             FROM canvas_nodes WHERE canvas_id = ?1",
        )?;
        let nodes = stmt
            .query_map(params![canvas_id], Self::row_to_node)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(nodes)
    }

    fn get_all_edges(&self, canvas_id: &str) -> Result<Vec<CanvasEdge>, CanvasError> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare_cached(
            "SELECT id, canvas_id, source_id, target_id, edge_type, metadata, created_at
             FROM canvas_edges WHERE canvas_id = ?1",
        )?;
        let edges = stmt
            .query_map(params![canvas_id], Self::row_to_edge)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(edges)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_node(id: &str, canvas_id: &str, x: f64, y: f64) -> CanvasNode {
        CanvasNode {
            id: id.to_string(),
            canvas_id: canvas_id.to_string(),
            card_type: CardType::Prompt,
            x,
            y,
            width: 100.0,
            height: 50.0,
            content: serde_json::json!({"text": "hello"}),
            status: CardStatus::Active,
            created_by: "test".to_string(),
            created_at: 1000,
        }
    }

    fn make_edge(
        id: &str,
        canvas_id: &str,
        source: &str,
        target: &str,
        edge_type: EdgeType,
    ) -> CanvasEdge {
        CanvasEdge {
            id: id.to_string(),
            canvas_id: canvas_id.to_string(),
            source_id: source.to_string(),
            target_id: target.to_string(),
            edge_type,
            metadata: serde_json::json!({}),
            created_at: 1000,
        }
    }

    #[test]
    fn test_insert_and_get_node() {
        let store = SqliteCanvasStore::in_memory().unwrap();
        let node = make_node("n1", "c1", 10.0, 20.0);
        store.insert_node(&node).unwrap();

        let fetched = store.get_node("n1").unwrap().unwrap();
        assert_eq!(fetched.id, "n1");
        assert_eq!(fetched.canvas_id, "c1");
        assert_eq!(fetched.card_type, CardType::Prompt);
        assert!((fetched.x - 10.0).abs() < f64::EPSILON);
        assert!((fetched.y - 20.0).abs() < f64::EPSILON);
        assert_eq!(fetched.status, CardStatus::Active);
    }

    #[test]
    fn test_get_node_not_found() {
        let store = SqliteCanvasStore::in_memory().unwrap();
        let result = store.get_node("nonexistent").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_insert_and_get_edges() {
        let store = SqliteCanvasStore::in_memory().unwrap();
        store.insert_node(&make_node("n1", "c1", 0.0, 0.0)).unwrap();
        store.insert_node(&make_node("n2", "c1", 100.0, 0.0)).unwrap();
        store.insert_node(&make_node("n3", "c1", 200.0, 0.0)).unwrap();

        let e1 = make_edge("e1", "c1", "n1", "n2", EdgeType::ReplyTo);
        let e2 = make_edge("e2", "c1", "n1", "n3", EdgeType::References);
        store.insert_edge(&e1).unwrap();
        store.insert_edge(&e2).unwrap();

        let from_n1 = store.get_edges_from("n1").unwrap();
        assert_eq!(from_n1.len(), 2);

        let to_n2 = store.get_edges_to("n2").unwrap();
        assert_eq!(to_n2.len(), 1);
        assert_eq!(to_n2[0].source_id, "n1");
    }

    #[test]
    fn test_update_node_position() {
        let store = SqliteCanvasStore::in_memory().unwrap();
        store.insert_node(&make_node("n1", "c1", 10.0, 20.0)).unwrap();

        store.update_node_position("n1", 500.0, 600.0).unwrap();

        let node = store.get_node("n1").unwrap().unwrap();
        assert!((node.x - 500.0).abs() < f64::EPSILON);
        assert!((node.y - 600.0).abs() < f64::EPSILON);

        // Verify R-tree was updated: viewport query at new position should find it
        let found = store.query_viewport("c1", 490.0, 590.0, 610.0, 660.0).unwrap();
        assert_eq!(found.len(), 1);

        // Old position should not find it
        let not_found = store.query_viewport("c1", 0.0, 0.0, 50.0, 50.0).unwrap();
        assert_eq!(not_found.len(), 0);
    }

    #[test]
    fn test_update_node_position_not_found() {
        let store = SqliteCanvasStore::in_memory().unwrap();
        let result = store.update_node_position("nonexistent", 0.0, 0.0);
        assert!(matches!(result, Err(CanvasError::NodeNotFound(_))));
    }

    #[test]
    fn test_update_node_content() {
        let store = SqliteCanvasStore::in_memory().unwrap();
        store.insert_node(&make_node("n1", "c1", 0.0, 0.0)).unwrap();

        let new_content = serde_json::json!({"text": "updated"});
        store.update_node_content("n1", &new_content).unwrap();

        let node = store.get_node("n1").unwrap().unwrap();
        assert_eq!(node.content["text"], "updated");
    }

    #[test]
    fn test_update_node_status() {
        let store = SqliteCanvasStore::in_memory().unwrap();
        store.insert_node(&make_node("n1", "c1", 0.0, 0.0)).unwrap();

        store.update_node_status("n1", &CardStatus::DeadEnd).unwrap();

        let node = store.get_node("n1").unwrap().unwrap();
        assert_eq!(node.status, CardStatus::DeadEnd);
    }

    #[test]
    fn test_delete_node_cascades_edges() {
        let store = SqliteCanvasStore::in_memory().unwrap();
        store.insert_node(&make_node("n1", "c1", 0.0, 0.0)).unwrap();
        store.insert_node(&make_node("n2", "c1", 100.0, 0.0)).unwrap();
        store.insert_node(&make_node("n3", "c1", 200.0, 0.0)).unwrap();

        store.insert_edge(&make_edge("e1", "c1", "n1", "n2", EdgeType::ReplyTo)).unwrap();
        store.insert_edge(&make_edge("e2", "c1", "n2", "n3", EdgeType::ReplyTo)).unwrap();

        store.delete_node("n2").unwrap();

        // Node is gone
        assert!(store.get_node("n2").unwrap().is_none());

        // Edges involving n2 are gone
        let from_n1 = store.get_edges_from("n1").unwrap();
        assert_eq!(from_n1.len(), 0);
        let to_n3 = store.get_edges_to("n3").unwrap();
        assert_eq!(to_n3.len(), 0);

        // n2 gone from spatial index — query a viewport that would have included n2 but not n1
        let found = store.query_viewport("c1", 110.0, -50.0, 350.0, 100.0).unwrap();
        assert_eq!(found.len(), 1); // only n3
        assert_eq!(found[0].id, "n3");
    }

    #[test]
    fn test_delete_node_not_found() {
        let store = SqliteCanvasStore::in_memory().unwrap();
        let result = store.delete_node("nonexistent");
        assert!(matches!(result, Err(CanvasError::NodeNotFound(_))));
    }

    #[test]
    fn test_query_viewport() {
        let store = SqliteCanvasStore::in_memory().unwrap();
        // Nodes at various positions, all on canvas "c1"
        store.insert_node(&make_node("n1", "c1", 0.0, 0.0)).unwrap(); // 0..100, 0..50
        store.insert_node(&make_node("n2", "c1", 500.0, 500.0)).unwrap(); // 500..600, 500..550
        store.insert_node(&make_node("n3", "c1", 1000.0, 1000.0)).unwrap(); // 1000..1100, 1000..1050

        // Viewport that covers n1 and n2
        let nodes = store.query_viewport("c1", -10.0, -10.0, 610.0, 560.0).unwrap();
        assert_eq!(nodes.len(), 2);
        let ids: HashSet<String> = nodes.iter().map(|n| n.id.clone()).collect();
        assert!(ids.contains("n1"));
        assert!(ids.contains("n2"));
    }

    #[test]
    fn test_query_viewport_canvas_isolation() {
        let store = SqliteCanvasStore::in_memory().unwrap();
        store.insert_node(&make_node("n1", "c1", 0.0, 0.0)).unwrap();
        store.insert_node(&make_node("n2", "c2", 10.0, 10.0)).unwrap();

        // Query canvas c1 — should only see n1
        let nodes = store.query_viewport("c1", -100.0, -100.0, 200.0, 200.0).unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].id, "n1");
    }

    #[test]
    fn test_query_viewport_empty_canvas() {
        let store = SqliteCanvasStore::in_memory().unwrap();
        let nodes = store.query_viewport("empty", 0.0, 0.0, 100.0, 100.0).unwrap();
        assert!(nodes.is_empty());
    }

    #[test]
    fn test_query_radius() {
        let store = SqliteCanvasStore::in_memory().unwrap();
        // Node centers: n1 at (50,25), n2 at (550,525), n3 at (1050,1025)
        store.insert_node(&make_node("n1", "c1", 0.0, 0.0)).unwrap();
        store.insert_node(&make_node("n2", "c1", 500.0, 500.0)).unwrap();
        store.insert_node(&make_node("n3", "c1", 1000.0, 1000.0)).unwrap();

        // Radius query from origin, radius 100 — should find n1 (center at 50,25, dist ~56)
        let nodes = store.query_radius("c1", 0.0, 0.0, 100.0).unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].id, "n1");
    }

    #[test]
    fn test_query_nearest() {
        let store = SqliteCanvasStore::in_memory().unwrap();
        store.insert_node(&make_node("n1", "c1", 0.0, 0.0)).unwrap();
        store.insert_node(&make_node("n2", "c1", 200.0, 200.0)).unwrap();
        store.insert_node(&make_node("n3", "c1", 1000.0, 1000.0)).unwrap();

        // k=2 nearest to origin
        let nodes = store.query_nearest("c1", 0.0, 0.0, 2).unwrap();
        assert_eq!(nodes.len(), 2);
        // n1 should be closest, then n2
        assert_eq!(nodes[0].id, "n1");
        assert_eq!(nodes[1].id, "n2");
    }

    #[test]
    fn test_bfs_depth_1() {
        let store = SqliteCanvasStore::in_memory().unwrap();
        store.insert_node(&make_node("n1", "c1", 0.0, 0.0)).unwrap();
        store.insert_node(&make_node("n2", "c1", 100.0, 0.0)).unwrap();
        store.insert_node(&make_node("n3", "c1", 200.0, 0.0)).unwrap();

        store.insert_edge(&make_edge("e1", "c1", "n1", "n2", EdgeType::ReplyTo)).unwrap();
        store.insert_edge(&make_edge("e2", "c1", "n2", "n3", EdgeType::ReplyTo)).unwrap();

        let nodes = store.bfs("n1", 1).unwrap();
        let ids: HashSet<String> = nodes.iter().map(|n| n.id.clone()).collect();
        assert!(ids.contains("n1"));
        assert!(ids.contains("n2"));
        assert!(!ids.contains("n3")); // depth 2, not reached
    }

    #[test]
    fn test_bfs_depth_3() {
        let store = SqliteCanvasStore::in_memory().unwrap();
        store.insert_node(&make_node("n1", "c1", 0.0, 0.0)).unwrap();
        store.insert_node(&make_node("n2", "c1", 100.0, 0.0)).unwrap();
        store.insert_node(&make_node("n3", "c1", 200.0, 0.0)).unwrap();
        store.insert_node(&make_node("n4", "c1", 300.0, 0.0)).unwrap();

        store.insert_edge(&make_edge("e1", "c1", "n1", "n2", EdgeType::ReplyTo)).unwrap();
        store.insert_edge(&make_edge("e2", "c1", "n2", "n3", EdgeType::ReplyTo)).unwrap();
        store.insert_edge(&make_edge("e3", "c1", "n3", "n4", EdgeType::ReplyTo)).unwrap();

        let nodes = store.bfs("n1", 3).unwrap();
        assert_eq!(nodes.len(), 4);
    }

    #[test]
    fn test_bfs_cycle_safety() {
        let store = SqliteCanvasStore::in_memory().unwrap();
        store.insert_node(&make_node("n1", "c1", 0.0, 0.0)).unwrap();
        store.insert_node(&make_node("n2", "c1", 100.0, 0.0)).unwrap();
        store.insert_node(&make_node("n3", "c1", 200.0, 0.0)).unwrap();

        // Create a cycle: n1 -> n2 -> n3 -> n1
        store.insert_edge(&make_edge("e1", "c1", "n1", "n2", EdgeType::ReplyTo)).unwrap();
        store.insert_edge(&make_edge("e2", "c1", "n2", "n3", EdgeType::ReplyTo)).unwrap();
        store.insert_edge(&make_edge("e3", "c1", "n3", "n1", EdgeType::ReplyTo)).unwrap();

        // Should not loop forever; should return exactly 3 nodes
        let nodes = store.bfs("n1", 10).unwrap();
        assert_eq!(nodes.len(), 3);
    }

    #[test]
    fn test_bfs_not_found() {
        let store = SqliteCanvasStore::in_memory().unwrap();
        let result = store.bfs("nonexistent", 5);
        assert!(matches!(result, Err(CanvasError::NodeNotFound(_))));
    }

    #[test]
    fn test_connected_component_full() {
        let store = SqliteCanvasStore::in_memory().unwrap();
        // Component 1: n1 - n2 - n3
        store.insert_node(&make_node("n1", "c1", 0.0, 0.0)).unwrap();
        store.insert_node(&make_node("n2", "c1", 100.0, 0.0)).unwrap();
        store.insert_node(&make_node("n3", "c1", 200.0, 0.0)).unwrap();
        // Isolated node
        store.insert_node(&make_node("n4", "c1", 300.0, 0.0)).unwrap();

        store.insert_edge(&make_edge("e1", "c1", "n1", "n2", EdgeType::ReplyTo)).unwrap();
        store.insert_edge(&make_edge("e2", "c1", "n2", "n3", EdgeType::ReplyTo)).unwrap();

        let comp = store.connected_component("n1").unwrap();
        assert_eq!(comp.len(), 3);
        let ids: HashSet<String> = comp.iter().map(|n| n.id.clone()).collect();
        assert!(ids.contains("n1"));
        assert!(ids.contains("n2"));
        assert!(ids.contains("n3"));
        assert!(!ids.contains("n4"));
    }

    #[test]
    fn test_connected_component_isolated() {
        let store = SqliteCanvasStore::in_memory().unwrap();
        store.insert_node(&make_node("n1", "c1", 0.0, 0.0)).unwrap();

        let comp = store.connected_component("n1").unwrap();
        assert_eq!(comp.len(), 1);
        assert_eq!(comp[0].id, "n1");
    }

    #[test]
    fn test_get_all_nodes() {
        let store = SqliteCanvasStore::in_memory().unwrap();
        store.insert_node(&make_node("n1", "c1", 0.0, 0.0)).unwrap();
        store.insert_node(&make_node("n2", "c1", 100.0, 0.0)).unwrap();
        store.insert_node(&make_node("n3", "c2", 200.0, 0.0)).unwrap();

        let c1_nodes = store.get_all_nodes("c1").unwrap();
        assert_eq!(c1_nodes.len(), 2);

        let c2_nodes = store.get_all_nodes("c2").unwrap();
        assert_eq!(c2_nodes.len(), 1);
    }

    #[test]
    fn test_get_all_edges() {
        let store = SqliteCanvasStore::in_memory().unwrap();
        store.insert_node(&make_node("n1", "c1", 0.0, 0.0)).unwrap();
        store.insert_node(&make_node("n2", "c1", 100.0, 0.0)).unwrap();
        store.insert_node(&make_node("n3", "c2", 0.0, 0.0)).unwrap();
        store.insert_node(&make_node("n4", "c2", 100.0, 0.0)).unwrap();

        store.insert_edge(&make_edge("e1", "c1", "n1", "n2", EdgeType::ReplyTo)).unwrap();
        store.insert_edge(&make_edge("e2", "c2", "n3", "n4", EdgeType::Evolves)).unwrap();

        let c1_edges = store.get_all_edges("c1").unwrap();
        assert_eq!(c1_edges.len(), 1);
        assert_eq!(c1_edges[0].edge_type, EdgeType::ReplyTo);

        let c2_edges = store.get_all_edges("c2").unwrap();
        assert_eq!(c2_edges.len(), 1);
        assert_eq!(c2_edges[0].edge_type, EdgeType::Evolves);
    }

    #[test]
    fn test_node_with_no_edges() {
        let store = SqliteCanvasStore::in_memory().unwrap();
        store.insert_node(&make_node("n1", "c1", 0.0, 0.0)).unwrap();

        assert!(store.get_edges_from("n1").unwrap().is_empty());
        assert!(store.get_edges_to("n1").unwrap().is_empty());
    }

    #[test]
    fn test_scale_viewport_query_performance() {
        let store = SqliteCanvasStore::in_memory().unwrap();
        // Insert 10,000 nodes spread across a large canvas
        for i in 0..10_000 {
            let x = (i % 100) as f64 * 300.0;
            let y = (i / 100) as f64 * 200.0;
            let node = make_node(&format!("n{i}"), "c1", x, y);
            store.insert_node(&node).unwrap();
        }

        // Time viewport query
        let start = std::time::Instant::now();
        let nodes = store.query_viewport("c1", 0.0, 0.0, 3000.0, 2000.0).unwrap();
        let elapsed = start.elapsed();

        assert!(!nodes.is_empty());
        assert!(
            elapsed.as_millis() < 100,
            "Viewport query took {}ms, expected < 100ms",
            elapsed.as_millis()
        );
    }

    #[test]
    fn test_scale_radius_query_performance() {
        let store = SqliteCanvasStore::in_memory().unwrap();
        for i in 0..10_000 {
            let x = (i % 100) as f64 * 300.0;
            let y = (i / 100) as f64 * 200.0;
            let node = make_node(&format!("n{i}"), "c1", x, y);
            store.insert_node(&node).unwrap();
        }

        let start = std::time::Instant::now();
        let nodes = store.query_radius("c1", 5000.0, 5000.0, 2000.0).unwrap();
        let elapsed = start.elapsed();

        assert!(!nodes.is_empty());
        assert!(
            elapsed.as_millis() < 100,
            "Radius query took {}ms, expected < 100ms",
            elapsed.as_millis()
        );
    }

    #[test]
    fn test_persistence_with_file() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");

        // Write data
        {
            let store = SqliteCanvasStore::new(&db_path).unwrap();
            store.insert_node(&make_node("n1", "c1", 10.0, 20.0)).unwrap();
            store.insert_node(&make_node("n2", "c1", 30.0, 40.0)).unwrap();
            store.insert_edge(&make_edge("e1", "c1", "n1", "n2", EdgeType::ReplyTo)).unwrap();
        }

        // Read data in a new store instance
        {
            let store = SqliteCanvasStore::new(&db_path).unwrap();
            let node = store.get_node("n1").unwrap().unwrap();
            assert!((node.x - 10.0).abs() < f64::EPSILON);

            let edges = store.get_edges_from("n1").unwrap();
            assert_eq!(edges.len(), 1);

            // R-tree should have been rebuilt from SQLite
            let viewport = store.query_viewport("c1", 0.0, 0.0, 50.0, 50.0).unwrap();
            assert_eq!(viewport.len(), 2);
        }
    }
}
