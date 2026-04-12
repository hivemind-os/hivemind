use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::Path;

// ── Public types ────────────────────────────────────────────────────

/// A typed entity reference in the ownership graph.
/// Format: `"session/<id>"`, `"agent/<id>"`, or `"workflow/<id>"`.
pub type EntityRef = String;

/// The kind of entity in the graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityType {
    Session,
    Agent,
    Workflow,
}

impl EntityType {
    pub fn as_str(&self) -> &'static str {
        match self {
            EntityType::Session => "session",
            EntityType::Agent => "agent",
            EntityType::Workflow => "workflow",
        }
    }
}

impl std::fmt::Display for EntityType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for EntityType {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "session" => Ok(EntityType::Session),
            "agent" => Ok(EntityType::Agent),
            "workflow" => Ok(EntityType::Workflow),
            other => Err(format!("unknown entity type: {other}")),
        }
    }
}

/// A node in the entity ownership graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityNode {
    pub entity_id: EntityRef,
    pub entity_type: EntityType,
    pub parent_ref: Option<EntityRef>,
    pub label: String,
    pub created_at: i64,
}

// ── Helpers for building entity refs ─────────────────────────────────

pub fn session_ref(id: &str) -> EntityRef {
    format!("session/{id}")
}

pub fn agent_ref(id: &str) -> EntityRef {
    format!("agent/{id}")
}

pub fn workflow_ref(id: &str) -> EntityRef {
    format!("workflow/{id}")
}

/// Parse an entity ref into (type, id). Returns None if malformed.
pub fn parse_entity_ref(entity_ref: &str) -> Option<(EntityType, &str)> {
    let (prefix, id) = entity_ref.split_once('/')?;
    let entity_type: EntityType = prefix.parse().ok()?;
    Some((entity_type, id))
}

// ── Entity Graph Store ──────────────────────────────────────────────

/// SQLite-backed entity ownership graph.
///
/// Thread-safe via internal mutex. Designed to be wrapped in `Arc` and
/// shared across services.
pub struct EntityGraph {
    db: Mutex<Connection>,
}

impl EntityGraph {
    /// Open (or create) a file-backed entity graph database.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, rusqlite::Error> {
        let conn = Connection::open(path)?;
        Self::init(&conn)?;
        Ok(Self { db: Mutex::new(conn) })
    }

    /// Create an in-memory entity graph (for tests).
    pub fn in_memory() -> Result<Self, rusqlite::Error> {
        let conn = Connection::open_in_memory()?;
        Self::init(&conn)?;
        Ok(Self { db: Mutex::new(conn) })
    }

    fn init(conn: &Connection) -> Result<(), rusqlite::Error> {
        conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            PRAGMA foreign_keys = ON;
            PRAGMA busy_timeout = 5000;
            PRAGMA synchronous = NORMAL;

            CREATE TABLE IF NOT EXISTS entity_graph (
                entity_id   TEXT PRIMARY KEY,
                entity_type TEXT NOT NULL,
                parent_ref  TEXT,
                label       TEXT NOT NULL DEFAULT '',
                created_at  INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_entity_parent
                ON entity_graph(parent_ref);

            CREATE INDEX IF NOT EXISTS idx_entity_type
                ON entity_graph(entity_type);
        ",
        )?;
        Ok(())
    }

    // ── Mutations ───────────────────────────────────────────────────

    /// Register an entity in the graph. If it already exists, update
    /// its parent_ref and label (idempotent upsert).
    pub fn register(
        &self,
        entity_id: &str,
        entity_type: EntityType,
        parent_ref: Option<&str>,
        label: &str,
    ) {
        let now = now_ms();
        let db = self.db.lock();
        let _ = db.execute(
            "INSERT INTO entity_graph (entity_id, entity_type, parent_ref, label, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(entity_id) DO UPDATE SET parent_ref = ?3, label = ?4",
            params![entity_id, entity_type.as_str(), parent_ref, label, now],
        );
    }

    /// Remove an entity and all its descendants from the graph.
    pub fn remove(&self, entity_id: &str) {
        let descendants = self.descendants(entity_id);
        let db = self.db.lock();
        for node in &descendants {
            let _ = db
                .execute("DELETE FROM entity_graph WHERE entity_id = ?1", params![node.entity_id]);
        }
        let _ = db.execute("DELETE FROM entity_graph WHERE entity_id = ?1", params![entity_id]);
    }

    // ── Queries ─────────────────────────────────────────────────────

    /// Get a single entity node.
    pub fn get(&self, entity_id: &str) -> Option<EntityNode> {
        let db = self.db.lock();
        db.query_row(
            "SELECT entity_id, entity_type, parent_ref, label, created_at
             FROM entity_graph WHERE entity_id = ?1",
            params![entity_id],
            |row| Ok(row_to_node(row)),
        )
        .optional()
        .ok()
        .flatten()
    }

    /// Get direct children of an entity.
    pub fn children(&self, entity_id: &str) -> Vec<EntityNode> {
        let db = self.db.lock();
        let mut stmt = db
            .prepare(
                "SELECT entity_id, entity_type, parent_ref, label, created_at
                 FROM entity_graph WHERE parent_ref = ?1
                 ORDER BY created_at",
            )
            .unwrap();
        stmt.query_map(params![entity_id], |row| Ok(row_to_node(row)))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect()
    }

    /// Walk up the ancestor chain from an entity to the root.
    /// Returns ancestors in bottom-up order (immediate parent first).
    /// Does NOT include the entity itself.
    pub fn ancestors(&self, entity_id: &str) -> Vec<EntityNode> {
        let mut result = Vec::new();
        let mut current = entity_id.to_string();
        // Guard against cycles (max depth 50)
        for _ in 0..50 {
            let node = match self.get(&current) {
                Some(n) => n,
                None => break,
            };
            match &node.parent_ref {
                Some(parent) => {
                    current = parent.clone();
                    if let Some(parent_node) = self.get(&current) {
                        result.push(parent_node);
                    } else {
                        break;
                    }
                }
                None => break,
            }
        }
        result
    }

    /// Get all descendants of an entity (recursive).
    /// Returns in breadth-first order.
    pub fn descendants(&self, entity_id: &str) -> Vec<EntityNode> {
        let mut result = Vec::new();
        let mut queue = vec![entity_id.to_string()];
        // Guard against cycles
        let mut visited = std::collections::HashSet::new();
        visited.insert(entity_id.to_string());

        while let Some(current) = queue.pop() {
            let kids = self.children(&current);
            for kid in kids {
                if visited.insert(kid.entity_id.clone()) {
                    queue.push(kid.entity_id.clone());
                    result.push(kid);
                }
            }
        }
        result
    }

    /// Get all descendant entity_ids of an entity (just the IDs, more efficient).
    pub fn descendant_ids(&self, entity_id: &str) -> Vec<EntityRef> {
        self.descendants(entity_id).into_iter().map(|n| n.entity_id).collect()
    }

    /// Get all entities of a given type.
    pub fn list_by_type(&self, entity_type: EntityType) -> Vec<EntityNode> {
        let db = self.db.lock();
        let mut stmt = db
            .prepare(
                "SELECT entity_id, entity_type, parent_ref, label, created_at
                 FROM entity_graph WHERE entity_type = ?1
                 ORDER BY created_at",
            )
            .unwrap();
        stmt.query_map(params![entity_type.as_str()], |row| Ok(row_to_node(row)))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect()
    }

    /// Get all root entities (no parent).
    pub fn roots(&self) -> Vec<EntityNode> {
        let db = self.db.lock();
        let mut stmt = db
            .prepare(
                "SELECT entity_id, entity_type, parent_ref, label, created_at
                 FROM entity_graph WHERE parent_ref IS NULL
                 ORDER BY created_at",
            )
            .unwrap();
        stmt.query_map([], |row| Ok(row_to_node(row))).unwrap().filter_map(|r| r.ok()).collect()
    }

    /// Count total entities in the graph.
    pub fn count(&self) -> usize {
        let db = self.db.lock();
        db.query_row("SELECT COUNT(*) FROM entity_graph", [], |row| row.get::<_, i64>(0))
            .unwrap_or(0) as usize
    }
}

// ── Internal helpers ────────────────────────────────────────────────

fn row_to_node(row: &rusqlite::Row) -> EntityNode {
    EntityNode {
        entity_id: row.get(0).unwrap(),
        entity_type: row.get::<_, String>(1).unwrap().parse().unwrap_or(EntityType::Session),
        parent_ref: row.get(2).unwrap(),
        label: row.get(3).unwrap(),
        created_at: row.get(4).unwrap(),
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_graph() -> EntityGraph {
        EntityGraph::in_memory().unwrap()
    }

    #[test]
    fn register_and_get() {
        let g = test_graph();
        g.register("session/abc", EntityType::Session, None, "My Chat");
        let node = g.get("session/abc").unwrap();
        assert_eq!(node.entity_type, EntityType::Session);
        assert_eq!(node.label, "My Chat");
        assert!(node.parent_ref.is_none());
    }

    #[test]
    fn upsert_updates_parent_and_label() {
        let g = test_graph();
        g.register("agent/x", EntityType::Agent, None, "Old");
        g.register("agent/x", EntityType::Agent, Some("session/s1"), "New");
        let node = g.get("agent/x").unwrap();
        assert_eq!(node.parent_ref.as_deref(), Some("session/s1"));
        assert_eq!(node.label, "New");
    }

    #[test]
    fn children_query() {
        let g = test_graph();
        g.register("session/s1", EntityType::Session, None, "Session");
        g.register("agent/a1", EntityType::Agent, Some("session/s1"), "Agent 1");
        g.register("agent/a2", EntityType::Agent, Some("session/s1"), "Agent 2");
        g.register("agent/a3", EntityType::Agent, Some("agent/a1"), "Sub-agent");

        let kids = g.children("session/s1");
        assert_eq!(kids.len(), 2);
        assert!(kids.iter().any(|n| n.entity_id == "agent/a1"));
        assert!(kids.iter().any(|n| n.entity_id == "agent/a2"));
    }

    #[test]
    fn ancestors_walk_up() {
        let g = test_graph();
        g.register("session/s1", EntityType::Session, None, "Session");
        g.register("workflow/w1", EntityType::Workflow, Some("session/s1"), "WF");
        g.register("agent/a1", EntityType::Agent, Some("workflow/w1"), "Agent");

        let ancestors = g.ancestors("agent/a1");
        assert_eq!(ancestors.len(), 2);
        assert_eq!(ancestors[0].entity_id, "workflow/w1");
        assert_eq!(ancestors[1].entity_id, "session/s1");
    }

    #[test]
    fn descendants_recursive() {
        let g = test_graph();
        g.register("session/s1", EntityType::Session, None, "Session");
        g.register("workflow/w1", EntityType::Workflow, Some("session/s1"), "WF");
        g.register("agent/a1", EntityType::Agent, Some("workflow/w1"), "Agent");
        g.register("agent/a2", EntityType::Agent, Some("agent/a1"), "Sub-agent");

        let desc = g.descendants("session/s1");
        assert_eq!(desc.len(), 3);
        let ids: Vec<&str> = desc.iter().map(|n| n.entity_id.as_str()).collect();
        assert!(ids.contains(&"workflow/w1"));
        assert!(ids.contains(&"agent/a1"));
        assert!(ids.contains(&"agent/a2"));
    }

    #[test]
    fn remove_cascades_to_descendants() {
        let g = test_graph();
        g.register("session/s1", EntityType::Session, None, "Session");
        g.register("workflow/w1", EntityType::Workflow, Some("session/s1"), "WF");
        g.register("agent/a1", EntityType::Agent, Some("workflow/w1"), "Agent");

        g.remove("workflow/w1");
        assert!(g.get("workflow/w1").is_none());
        assert!(g.get("agent/a1").is_none());
        assert!(g.get("session/s1").is_some()); // parent not removed
    }

    #[test]
    fn parse_entity_ref_works() {
        let (ty, id) = parse_entity_ref("session/abc-123").unwrap();
        assert_eq!(ty, EntityType::Session);
        assert_eq!(id, "abc-123");

        let (ty, id) = parse_entity_ref("agent/bot-deadbeef").unwrap();
        assert_eq!(ty, EntityType::Agent);
        assert_eq!(id, "bot-deadbeef");

        let (ty, id) = parse_entity_ref("workflow/wf-456").unwrap();
        assert_eq!(ty, EntityType::Workflow);
        assert_eq!(id, "wf-456");

        assert!(parse_entity_ref("invalid").is_none());
        assert!(parse_entity_ref("unknown/x").is_none());
    }

    #[test]
    fn descendant_ids() {
        let g = test_graph();
        g.register("session/s1", EntityType::Session, None, "Session");
        g.register("agent/a1", EntityType::Agent, Some("session/s1"), "A1");
        g.register("agent/a2", EntityType::Agent, Some("session/s1"), "A2");

        let ids = g.descendant_ids("session/s1");
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"agent/a1".to_string()));
        assert!(ids.contains(&"agent/a2".to_string()));
    }

    #[test]
    fn roots_returns_top_level_only() {
        let g = test_graph();
        g.register("session/s1", EntityType::Session, None, "Session");
        g.register("workflow/w1", EntityType::Workflow, None, "Top WF");
        g.register("agent/a1", EntityType::Agent, Some("session/s1"), "Child");

        let roots = g.roots();
        assert_eq!(roots.len(), 2);
        let ids: Vec<&str> = roots.iter().map(|n| n.entity_id.as_str()).collect();
        assert!(ids.contains(&"session/s1"));
        assert!(ids.contains(&"workflow/w1"));
    }

    #[test]
    fn empty_graph() {
        let g = test_graph();
        assert_eq!(g.count(), 0);
        assert!(g.get("session/nope").is_none());
        assert!(g.children("session/nope").is_empty());
        assert!(g.ancestors("session/nope").is_empty());
        assert!(g.descendants("session/nope").is_empty());
    }
}
