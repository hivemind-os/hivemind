//! Persistent MCP tool catalog.
//!
//! Stores discovered tools, resources and prompts for each configured MCP
//! server so that new sessions can register bridge tools without connecting
//! first.  The catalog is persisted to `<hivemind_home>/mcp_catalog.json` and
//! refreshed whenever the MCP server configuration changes or the user
//! explicitly requests a refresh.
//!
//! Entries are keyed by **content-addressed cache key** (a SHA-256 hash of
//! transport type + command + args + url).  Two personas with identical MCP
//! server configs will share the same catalog entry.

use hive_classification::ChannelClass;
use hive_contracts::{McpCatalog, McpCatalogEntry, McpPromptInfo, McpResourceInfo, McpToolInfo};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use tracing;

const CATALOG_FILENAME: &str = "mcp_catalog.json";

/// In-memory + disk-backed MCP tool catalog.
#[derive(Clone)]
pub struct McpCatalogStore {
    /// Path to the catalog JSON file on disk.
    catalog_path: PathBuf,
    /// In-memory cache: cache_key → catalog entry.
    entries: Arc<RwLock<HashMap<String, McpCatalogEntry>>>,
    /// Serializes disk writes so concurrent persists cannot reorder.
    persist_lock: Arc<tokio::sync::Mutex<()>>,
}

impl McpCatalogStore {
    /// Create a new catalog store rooted at `hivemind_home`.
    pub fn new(hivemind_home: &Path) -> Self {
        let catalog_path = hivemind_home.join(CATALOG_FILENAME);
        let entries = match Self::load_from_disk(&catalog_path) {
            Ok(map) => map,
            Err(e) => {
                tracing::warn!(error = %e, "failed to load MCP catalog from disk; starting empty");
                HashMap::new()
            }
        };
        Self {
            catalog_path,
            entries: Arc::new(RwLock::new(entries)),
            persist_lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    /// Create a catalog store backed by an explicit path (useful for tests).
    pub fn with_path(catalog_path: PathBuf) -> Self {
        let entries = Self::load_from_disk(&catalog_path).unwrap_or_default();
        Self {
            catalog_path,
            entries: Arc::new(RwLock::new(entries)),
            persist_lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    /// Return the catalog entry for a specific server by its cache key.
    pub async fn get(&self, cache_key: &str) -> Option<McpCatalogEntry> {
        let entries = self.entries.read().await;
        entries.get(cache_key).cloned()
    }

    /// Return the catalog entry for a server by its human-readable server ID.
    /// Scans all entries (for backward compat & convenience).
    pub async fn get_by_server_id(&self, server_id: &str) -> Option<McpCatalogEntry> {
        let entries = self.entries.read().await;
        entries.values().find(|e| e.server_id == server_id).cloned()
    }

    /// Return all catalog entries.
    pub async fn all(&self) -> Vec<McpCatalogEntry> {
        let entries = self.entries.read().await;
        entries.values().cloned().collect()
    }

    /// Return the list of known tools for a specific server from the catalog
    /// (without needing a live connection).  Looks up by server_id for
    /// backward compatibility.
    pub async fn tools_for_server(&self, server_id: &str) -> Vec<McpToolInfo> {
        let entries = self.entries.read().await;
        entries
            .values()
            .find(|e| e.server_id == server_id)
            .map(|e| e.tools.clone())
            .unwrap_or_default()
    }

    /// Return the list of known resources for a specific server from the catalog.
    pub async fn resources_for_server(&self, server_id: &str) -> Vec<McpResourceInfo> {
        let entries = self.entries.read().await;
        entries
            .values()
            .find(|e| e.server_id == server_id)
            .map(|e| e.resources.clone())
            .unwrap_or_default()
    }

    /// Return the list of known prompts for a specific server from the catalog.
    pub async fn prompts_for_server(&self, server_id: &str) -> Vec<McpPromptInfo> {
        let entries = self.entries.read().await;
        entries
            .values()
            .find(|e| e.server_id == server_id)
            .map(|e| e.prompts.clone())
            .unwrap_or_default()
    }

    /// Return all cataloged tools across all servers, each tagged with server
    /// ID, cache key, and channel class (the same shape used by the bridge
    /// tool registration layer).
    pub async fn all_cataloged_tools(&self) -> Vec<CatalogedTool> {
        let entries = self.entries.read().await;
        let mut out = Vec::new();
        for entry in entries.values() {
            for tool in &entry.tools {
                out.push(CatalogedTool {
                    server_id: entry.server_id.clone(),
                    cache_key: entry.cache_key.clone(),
                    channel_class: entry.channel_class,
                    tool: tool.clone(),
                });
            }
        }
        out
    }

    /// Update (or insert) the catalog entry for a single server.
    /// Persists to disk.
    ///
    /// The `cache_key` is the content-addressed key for this server config
    /// (computed via `McpServerConfig::cache_key()`).  The `server_id` is
    /// the human-readable identifier kept for display purposes.
    pub async fn upsert(
        &self,
        server_id: &str,
        cache_key: &str,
        channel_class: ChannelClass,
        tools: Vec<McpToolInfo>,
        resources: Vec<McpResourceInfo>,
        prompts: Vec<McpPromptInfo>,
    ) {
        let entry = McpCatalogEntry {
            server_id: server_id.to_string(),
            cache_key: cache_key.to_string(),
            channel_class,
            tools,
            resources,
            prompts,
            last_updated_ms: now_ms(),
        };
        {
            let mut entries = self.entries.write().await;
            entries.insert(cache_key.to_string(), entry);
        }
        if let Err(e) = self.persist().await {
            tracing::warn!(error = %e, "failed to persist MCP catalog to disk");
        }
    }

    /// Remove a server from the catalog by cache key.  Persists to disk.
    pub async fn remove(&self, cache_key: &str) {
        {
            let mut entries = self.entries.write().await;
            entries.remove(cache_key);
        }
        if let Err(e) = self.persist().await {
            tracing::warn!(error = %e, "failed to persist MCP catalog to disk after removal");
        }
    }

    /// Remove a server from the catalog by server_id.  Persists to disk.
    pub async fn remove_by_server_id(&self, server_id: &str) {
        let changed = {
            let mut entries = self.entries.write().await;
            let before = entries.len();
            entries.retain(|_, e| e.server_id != server_id);
            entries.len() != before
        };
        if changed {
            if let Err(e) = self.persist().await {
                tracing::warn!(error = %e, "failed to persist MCP catalog after removal");
            }
        }
    }

    /// Remove entries from the catalog that are NOT in the provided list of
    /// cache keys.  This is used during config reconciliation.
    pub async fn retain_keys(&self, cache_keys: &[String]) {
        let changed = {
            let mut entries = self.entries.write().await;
            let before = entries.len();
            entries.retain(|key, _| cache_keys.contains(key));
            entries.len() != before
        };
        if changed {
            if let Err(e) = self.persist().await {
                tracing::warn!(error = %e, "failed to persist MCP catalog after cleanup");
            }
        }
    }

    /// Persist current in-memory catalog to disk.
    ///
    /// Holds `persist_lock` across the snapshot + write so concurrent calls
    /// cannot interleave and overwrite newer data with an older snapshot.
    async fn persist(&self) -> Result<(), std::io::Error> {
        let _guard = self.persist_lock.lock().await;
        let catalog = {
            let entries = self.entries.read().await;
            McpCatalog { entries: entries.values().cloned().collect() }
        };
        let json = serde_json::to_string_pretty(&catalog)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        tokio::fs::write(&self.catalog_path, json).await
    }

    /// Load the catalog from a JSON file on disk.
    fn load_from_disk(path: &Path) -> Result<HashMap<String, McpCatalogEntry>, anyhow::Error> {
        if !path.exists() {
            return Ok(HashMap::new());
        }
        let data = std::fs::read_to_string(path)?;
        let catalog: McpCatalog = serde_json::from_str(&data)?;
        let map = catalog
            .entries
            .into_iter()
            .map(|e| {
                // Use cache_key if present, fall back to server_id for legacy data.
                let key =
                    if e.cache_key.is_empty() { e.server_id.clone() } else { e.cache_key.clone() };
                (key, e)
            })
            .collect();
        Ok(map)
    }
}

/// A tool from the catalog tagged with its server ID, cache key, and channel
/// class.  Used by the bridge tool registration layer to create
/// `McpBridgeTool` instances without needing a live connection.
#[derive(Debug, Clone)]
pub struct CatalogedTool {
    pub server_id: String,
    pub cache_key: String,
    pub channel_class: ChannelClass,
    pub tool: McpToolInfo,
}

fn now_ms() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    fn make_tool(name: &str) -> McpToolInfo {
        McpToolInfo {
            name: name.to_string(),
            description: format!("{name} description"),
            input_schema: json!({"type": "object"}),
            ui_meta: None,
        }
    }

    fn make_resource(uri: &str) -> McpResourceInfo {
        McpResourceInfo {
            uri: uri.to_string(),
            name: uri.to_string(),
            description: None,
            mime_type: None,
            size: None,
        }
    }

    #[tokio::test]
    async fn upsert_and_get() {
        let dir = TempDir::new().unwrap();
        let store = McpCatalogStore::new(dir.path());

        store
            .upsert(
                "test-server",
                "ck-test",
                ChannelClass::Internal,
                vec![make_tool("tool1")],
                vec![make_resource("file:///a.txt")],
                vec![],
            )
            .await;

        let entry = store.get("ck-test").await.unwrap();
        assert_eq!(entry.tools.len(), 1);
        assert_eq!(entry.tools[0].name, "tool1");
        assert_eq!(entry.resources.len(), 1);

        // Also reachable by server_id
        let entry2 = store.get_by_server_id("test-server").await.unwrap();
        assert_eq!(entry2.cache_key, "ck-test");
    }

    #[tokio::test]
    async fn persist_and_reload() {
        let dir = TempDir::new().unwrap();

        // Upsert into store A and drop it.
        {
            let store = McpCatalogStore::new(dir.path());
            store
                .upsert(
                    "s1",
                    "ck-s1",
                    ChannelClass::Internal,
                    vec![make_tool("t1"), make_tool("t2")],
                    vec![],
                    vec![],
                )
                .await;
        }

        // Reload from disk.
        let store = McpCatalogStore::new(dir.path());
        let entry = store.get("ck-s1").await.unwrap();
        assert_eq!(entry.tools.len(), 2);
    }

    #[tokio::test]
    async fn remove_and_retain() {
        let dir = TempDir::new().unwrap();
        let store = McpCatalogStore::new(dir.path());

        store.upsert("a", "ck-a", ChannelClass::Internal, vec![], vec![], vec![]).await;
        store.upsert("b", "ck-b", ChannelClass::Internal, vec![], vec![], vec![]).await;
        store.upsert("c", "ck-c", ChannelClass::Internal, vec![], vec![], vec![]).await;

        // Remove one.
        store.remove("ck-b").await;
        assert!(store.get("ck-b").await.is_none());

        // Retain only "ck-a".
        store.retain_keys(&["ck-a".to_string()]).await;
        assert!(store.get("ck-a").await.is_some());
        assert!(store.get("ck-c").await.is_none());
    }

    #[tokio::test]
    async fn all_cataloged_tools() {
        let dir = TempDir::new().unwrap();
        let store = McpCatalogStore::new(dir.path());

        store
            .upsert("s1", "ck-s1", ChannelClass::Internal, vec![make_tool("t1")], vec![], vec![])
            .await;
        store
            .upsert(
                "s2",
                "ck-s2",
                ChannelClass::Public,
                vec![make_tool("t2"), make_tool("t3")],
                vec![],
                vec![],
            )
            .await;

        let tools = store.all_cataloged_tools().await;
        assert_eq!(tools.len(), 3);
        // Each tool has a cache_key
        assert!(tools.iter().all(|t| !t.cache_key.is_empty()));
    }
}
