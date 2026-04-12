//! Global secret store backed by per-key OS keyring entries.
//!
//! Each secret gets its own keyring entry (service `"hivemind"`,
//! key = the secret key).  A small manifest entry (`__keys__`) tracks
//! all known keys so the full set can be enumerated on startup.
//!
//! This avoids the Windows Credential Manager 2560-byte blob limit
//! that caused silent data loss with the previous single-blob design.
//!
//! **Only the daemon process should write to this store.**  The Tauri
//! desktop app delegates all secret writes to the daemon via its HTTP
//! API (`PUT /api/v1/secrets/:key`).
//!
//! An in-memory cache ensures subsequent reads within the same process
//! never hit the keyring again.

use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::LazyLock;
use tracing::{debug, error, warn};

const SERVICE: &str = "hivemind";
const MANIFEST_KEY: &str = "__keys__";
/// Legacy blob key from the previous single-blob design.
const LEGACY_BLOB_KEY: &str = "secrets";

/// In-memory mirror of the keyring contents.
/// `None` means we haven't loaded from keyring yet this process lifetime.
static STORE: LazyLock<Mutex<Option<HashMap<String, String>>>> = LazyLock::new(|| Mutex::new(None));

// ── Internal helpers ────────────────────────────────────────────────

/// Ensure all secrets are loaded from keyring into memory.  No-op after
/// the first call.  Returns a clone of the current map.
fn ensure_loaded() -> HashMap<String, String> {
    let mut guard = STORE.lock();
    if let Some(map) = guard.as_ref() {
        return map.clone();
    }
    let map = load_all_from_keyring();
    *guard = Some(map.clone());
    map
}

/// Read a single keyring entry by key.
fn read_entry(key: &str) -> Option<String> {
    match keyring::Entry::new(SERVICE, key) {
        Ok(entry) => match entry.get_password() {
            Ok(val) if !val.is_empty() => Some(val),
            Ok(_) => None,
            Err(keyring::Error::NoEntry) => None,
            Err(e) => {
                error!(key = %key, error = %e, "failed to read keyring entry");
                None
            }
        },
        Err(e) => {
            error!(key = %key, error = %e, "failed to create keyring entry handle");
            None
        }
    }
}

/// Write a single keyring entry.  Returns `true` on success.
fn write_entry(key: &str, value: &str) -> bool {
    match keyring::Entry::new(SERVICE, key) {
        Ok(entry) => {
            if let Err(e) = entry.set_password(value) {
                error!(key = %key, error = %e, "failed to write keyring entry");
                false
            } else {
                true
            }
        }
        Err(e) => {
            error!(key = %key, error = %e, "failed to create keyring entry handle for writing");
            false
        }
    }
}

/// Delete a single keyring entry.
fn delete_entry(key: &str) {
    if let Ok(entry) = keyring::Entry::new(SERVICE, key) {
        let _ = entry.delete_credential();
    }
}

/// Read the manifest (JSON array of known secret keys).
fn read_manifest() -> Vec<String> {
    read_entry(MANIFEST_KEY).and_then(|json| serde_json::from_str(&json).ok()).unwrap_or_default()
}

/// Persist the manifest.
fn write_manifest(keys: &[String]) {
    let json = serde_json::to_string(keys).unwrap_or_default();
    if !write_entry(MANIFEST_KEY, &json) {
        error!("failed to persist secret key manifest");
    }
}

/// Sync the manifest from the current in-memory map.
fn sync_manifest(map: &HashMap<String, String>) {
    let keys: Vec<String> = map.keys().cloned().collect();
    write_manifest(&keys);
}

/// Load every secret from the keyring (manifest → per-key reads).
/// Also migrates the legacy single-blob format if present.
fn load_all_from_keyring() -> HashMap<String, String> {
    migrate_from_blob();

    let keys = read_manifest();
    let mut map = HashMap::new();
    for key in &keys {
        if let Some(val) = read_entry(key) {
            map.insert(key.clone(), val);
        }
    }
    debug!("loaded {} secrets from keyring ({} manifest keys)", map.len(), keys.len());
    map
}

/// One-time migration from the legacy single-blob format to per-key
/// entries.  Reads the old blob, writes each entry individually,
/// updates the manifest, and deletes the blob.
fn migrate_from_blob() {
    let blob_map: HashMap<String, String> = match keyring::Entry::new(SERVICE, LEGACY_BLOB_KEY) {
        Ok(entry) => match entry.get_password() {
            Ok(json) => serde_json::from_str(&json).unwrap_or_default(),
            Err(keyring::Error::NoEntry) => return,
            Err(_) => return,
        },
        Err(_) => return,
    };

    if blob_map.is_empty() {
        return;
    }

    debug!(count = blob_map.len(), "migrating secrets from legacy blob to per-key entries");

    let mut migrated_keys = read_manifest();
    let mut failures = 0;
    for (key, value) in &blob_map {
        if write_entry(key, value) {
            if !migrated_keys.contains(key) {
                migrated_keys.push(key.clone());
            }
        } else {
            failures += 1;
        }
    }

    write_manifest(&migrated_keys);

    if failures == 0 {
        delete_entry(LEGACY_BLOB_KEY);
        debug!("legacy blob migration complete");
    } else {
        warn!(
            failures,
            total = blob_map.len(),
            "blob migration had failures; keeping legacy blob as backup"
        );
    }
}

// ── Public API ──────────────────────────────────────────────────────

/// Load a secret by key.  Returns `None` if not present.
pub fn load(key: &str) -> Option<String> {
    let map = ensure_loaded();
    map.get(key).cloned()
}

/// Load a single secret directly from the OS keyring, bypassing the
/// in-memory cache and without loading the full secret store.
///
/// Use this when only one specific key is needed and you want to avoid
/// the cost of enumerating all secrets (e.g. the desktop app reading
/// the daemon auth token).
pub fn load_single(key: &str) -> Option<String> {
    read_entry(key)
}

/// Store (upsert) a secret.  Updates the in-memory cache and writes
/// the entry to the OS keyring.
///
/// Returns `true` if the secret was persisted to the OS keyring,
/// `false` if it was only saved in memory (keyring write failed).
pub fn save(key: &str, value: &str) -> bool {
    let mut guard = STORE.lock();
    let map = guard.get_or_insert_with(load_all_from_keyring);
    let is_new = !map.contains_key(key);
    map.insert(key.to_string(), value.to_string());
    let ok = write_entry(key, value);
    if ok && is_new {
        sync_manifest(map);
    }
    ok
}

/// Remove a secret.  Updates the in-memory cache and removes the
/// keyring entry.
pub fn delete(key: &str) {
    let mut guard = STORE.lock();
    let map = guard.get_or_insert_with(load_all_from_keyring);
    if map.remove(key).is_some() {
        delete_entry(key);
        sync_manifest(map);
    }
}

/// Return a snapshot of all stored secrets.
pub fn load_all() -> HashMap<String, String> {
    ensure_loaded()
}

/// Delete all secrets whose key starts with the given prefix.
/// Returns the number of entries removed.
pub fn delete_by_prefix(prefix: &str) -> usize {
    let mut guard = STORE.lock();
    let map = guard.get_or_insert_with(load_all_from_keyring);
    let keys_to_remove: Vec<String> =
        map.keys().filter(|k| k.starts_with(prefix)).cloned().collect();
    let count = keys_to_remove.len();
    if count > 0 {
        for k in &keys_to_remove {
            map.remove(k);
            delete_entry(k);
        }
        sync_manifest(map);
    }
    count
}

/// Bulk-insert secrets (used during migration).
pub fn save_bulk(entries: &HashMap<String, String>) {
    if entries.is_empty() {
        return;
    }
    let mut guard = STORE.lock();
    let map = guard.get_or_insert_with(load_all_from_keyring);
    for (k, v) in entries {
        map.insert(k.clone(), v.clone());
        write_entry(k, v);
    }
    sync_manifest(map);
}

/// Drop the in-memory cache so the next access re-reads from the
/// OS keyring.
pub fn invalidate_cache() {
    let mut guard = STORE.lock();
    *guard = None;
}

/// Check whether the store has at least one entry.
pub fn is_initialized() -> bool {
    let map = ensure_loaded();
    !map.is_empty()
}

// ── Legacy migration ────────────────────────────────────────────────

/// Sentinel key indicating migration has already run.
const MIGRATION_DONE_KEY: &str = "__migration_v1_done";

/// Migrate secrets from legacy keyring entries into the global blob.
///
/// This reads:
/// - Per-provider individual keyring entries (`provider:{id}:api-key`)
/// - Per-connector JSON blob entries (`connector:{id}`)
/// - Legacy channel-based entries (`channel:{id}` and `channel:{id}:{field}`)
/// - HF token from config (`hf_token` field, if present)
///
/// After migration, old entries are deleted and a sentinel is written so
/// migration never runs again.
pub fn migrate_legacy(
    provider_ids: &[String],
    connector_ids: &[String],
    hf_token_from_config: Option<&str>,
) {
    // Skip if already migrated.
    if load(MIGRATION_DONE_KEY).is_some() {
        return;
    }

    let mut migrated = HashMap::new();

    // 1. Provider API keys (individual keyring entries)
    for pid in provider_ids {
        let key = format!("provider:{pid}:api-key");
        if let Some(val) = read_legacy_entry(&key) {
            migrated.insert(key.clone(), val);
            delete_legacy_entry(&key);
        }
    }

    // 2. Connector secrets (per-connector JSON blobs + per-field legacy entries)
    let connector_fields = [
        "password",
        "client_secret",
        "access_token",
        "refresh_token",
        "bot_token",
        "app_token",
        "client_id_override",
    ];
    for cid in connector_ids {
        // Read consolidated blob format: connector:{id}
        let blob_key = format!("connector:{cid}");
        if let Some(json) = read_legacy_entry(&blob_key) {
            if let Ok(map) = serde_json::from_str::<HashMap<String, String>>(&json) {
                for (field, val) in map {
                    migrated.insert(format!("connector:{cid}:{field}"), val);
                }
            }
            delete_legacy_entry(&blob_key);
        }

        // Read legacy channel blob format: channel:{id}
        let channel_blob_key = format!("channel:{cid}");
        if let Some(json) = read_legacy_entry(&channel_blob_key) {
            if let Ok(map) = serde_json::from_str::<HashMap<String, String>>(&json) {
                for (field, val) in map {
                    migrated.entry(format!("connector:{cid}:{field}")).or_insert(val);
                }
            }
            delete_legacy_entry(&channel_blob_key);
        }

        // Read per-field legacy entries
        for field in &connector_fields {
            for prefix in &["connector", "channel"] {
                let legacy_key = format!("{prefix}:{cid}:{field}");
                if let Some(val) = read_legacy_entry(&legacy_key) {
                    migrated.entry(format!("connector:{cid}:{field}")).or_insert(val);
                    delete_legacy_entry(&legacy_key);
                }
            }
        }
    }

    // 3. HF token from config
    if let Some(token) = hf_token_from_config {
        if !token.is_empty() {
            migrated.entry("hf_token".to_string()).or_insert_with(|| token.to_string());
        }
    }

    if !migrated.is_empty() {
        debug!("migrating {} legacy secrets to global store", migrated.len());
        save_bulk(&migrated);
    }

    // Mark migration as done.
    save(MIGRATION_DONE_KEY, "1");
}

/// Read a single legacy keyring entry (old format: individual entries).
fn read_legacy_entry(key: &str) -> Option<String> {
    match keyring::Entry::new(SERVICE, key) {
        Ok(entry) => match entry.get_password() {
            Ok(val) if !val.is_empty() => Some(val),
            _ => None,
        },
        Err(_) => None,
    }
}

/// Delete a single legacy keyring entry.
fn delete_legacy_entry(key: &str) {
    if let Ok(entry) = keyring::Entry::new(SERVICE, key) {
        let _ = entry.delete_credential();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tests use unique key prefixes and clean up after themselves to
    // avoid races when running in parallel (they share the global STORE).

    #[test]
    fn load_missing_returns_none() {
        assert!(load("test:ss:missing-key-xyz").is_none());
    }

    #[test]
    fn save_and_load_roundtrip() {
        save("test:ss:roundtrip", "hello");
        assert_eq!(load("test:ss:roundtrip"), Some("hello".to_string()));
        delete("test:ss:roundtrip");
    }

    #[test]
    fn delete_removes_key() {
        save("test:ss:delete-me", "val");
        delete("test:ss:delete-me");
        assert!(load("test:ss:delete-me").is_none());
    }

    #[test]
    fn delete_nonexistent_is_noop() {
        delete("test:ss:never-existed"); // should not panic
    }

    #[test]
    fn delete_by_prefix_removes_matching() {
        save("test:ss:pfx:email:password", "pw");
        save("test:ss:pfx:email:token", "tk");
        save("test:ss:pfx:keep", "sk");
        let removed = delete_by_prefix("test:ss:pfx:email:");
        assert_eq!(removed, 2);
        assert!(load("test:ss:pfx:email:password").is_none());
        assert!(load("test:ss:pfx:email:token").is_none());
        assert_eq!(load("test:ss:pfx:keep"), Some("sk".to_string()));
        delete("test:ss:pfx:keep");
    }

    #[test]
    fn save_bulk_inserts_all() {
        let mut entries = HashMap::new();
        entries.insert("test:ss:bulk:a".to_string(), "1".to_string());
        entries.insert("test:ss:bulk:b".to_string(), "2".to_string());
        save_bulk(&entries);
        assert_eq!(load("test:ss:bulk:a"), Some("1".to_string()));
        assert_eq!(load("test:ss:bulk:b"), Some("2".to_string()));
        delete("test:ss:bulk:a");
        delete("test:ss:bulk:b");
    }

    #[test]
    fn load_all_contains_saved_entries() {
        save("test:ss:snap:x", "1");
        save("test:ss:snap:y", "2");
        let all = load_all();
        assert_eq!(all.get("test:ss:snap:x"), Some(&"1".to_string()));
        assert_eq!(all.get("test:ss:snap:y"), Some(&"2".to_string()));
        delete("test:ss:snap:x");
        delete("test:ss:snap:y");
    }
}
