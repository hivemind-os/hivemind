# hive-knowledge

SQLite-based property graph for long-term memory with full-text and vector search capabilities. Part of [HiveMind OS](../../README.md), a cross-platform, privacy-aware desktop AI agent.

## Overview

`hive-knowledge` stores entities and relationships as a directed, weighted graph backed by SQLite. It provides two complementary search strategies—FTS5 keyword search over node content and 384-dimensional vector similarity search via the [sqlite-vec](https://github.com/asg017/sqlite-vec) extension—so callers can combine lexical and semantic retrieval.

## Key Types

| Type | Purpose |
|------|---------|
| `Node` | Graph entity with `id`, `node_type`, `name`, `data_class`, and optional `content`. |
| `NewNode` | Input struct for creating a node (same fields, no `id`). |
| `Edge` | Directed relationship with `source_id`, `target_id`, `edge_type`, and `weight`. |
| `SearchResult` | Full-text search hit (same shape as `Node`). |
| `VectorSearchResult` | Nearest-neighbour hit carrying `id`, `distance`, and `data_class`. |
| `KnowledgeGraph` | Main entry point wrapping a SQLite connection. |

## API

### Initialization

```rust
// File-backed graph (creates or opens an existing database)
let kg = KnowledgeGraph::open("knowledge.db")?;

// In-memory graph (useful for tests)
let kg = KnowledgeGraph::open_in_memory()?;
```

Tables (`nodes`, `edges`, `nodes_fts`, `vec_nodes`) and indexes are created automatically on first use.

### Node Operations

```rust
let id = kg.insert_node(&NewNode { node_type: "person".into(), name: "Ada".into(), data_class: DataClass::Internal, content: Some("Mathematician".into()) })?;
let node = kg.get_node(id)?;
let node = kg.find_node_by_type_and_name("person", "Ada")?;
let nodes = kg.list_nodes(Some("person"), None, 100)?;
let removed = kg.remove_node(id)?;           // also deletes connected edges
let count = kg.node_count()?;
let by_type = kg.node_counts_by_type()?;
```

### Edge Operations

```rust
let eid = kg.insert_edge(child_id, parent_id, "child_of", 1.0)?;
let edges = kg.get_edges_for_node(child_id)?;
let targets = kg.list_outbound_nodes(source_id, "related_to", DataClass::Restricted, 50)?;
let removed = kg.remove_edge(eid)?;
```

### Search

#### Full-text search (FTS5)

Searches over `name` and `content` fields using SQLite FTS5 match syntax:

```rust
let hits = kg.search_text("mathematician", 10)?;
let hits = kg.search_text_filtered("mathematician", DataClass::Internal, 10)?;
```

#### Vector similarity search

384-dimensional embeddings are stored alongside nodes and queried via `sqlite-vec`:

```rust
kg.set_embedding(node_id, &embedding_vec)?;   // &[f32; 384]
let similar = kg.search_similar(&query_vec, DataClass::Restricted, 10)?;
// Returns VectorSearchResult { id, distance, data_class }
```

## Classification Inheritance

Every node carries a `DataClass` (from `hive-classification`):

```
Public (0) < Internal (1) < Confidential (2) < Restricted (3)
```

A node's **effective** classification is the maximum of its own class and all ancestors reachable via `child_of` edges. This is computed with a recursive CTE:

```rust
let effective = kg.effective_class(node_id)?;
```

All search methods accept a `max_class` parameter so results never exceed the caller's clearance level.

## Database Schema

| Table / Virtual Table | Engine | Purpose |
|-----------------------|--------|---------|
| `nodes` | Regular | Node storage with timestamps |
| `edges` | Regular | Weighted, typed relationships with foreign keys to `nodes` |
| `nodes_fts` | FTS5 | Full-text index on `name` and `content` (external-content mode) |
| `vec_nodes` | vec0 | 384-dim float embeddings for similarity search |

FTS5 stays in sync with `nodes` via `AFTER INSERT/UPDATE/DELETE` triggers—no manual reindexing required.

## Dependencies

| Crate | Role |
|-------|------|
| `hive-classification` | `DataClass` enum and ordering |
| `rusqlite` | SQLite driver |
| `sqlite-vec` | Vector search extension (`vec0` virtual tables) |
| `serde` | Serialization of graph types |
