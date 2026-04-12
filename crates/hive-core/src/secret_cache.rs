use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::LazyLock;

/// In-memory cache for OS keyring secrets, avoiding repeated keychain
/// prompts on macOS and credential manager lookups on Windows/Linux.
///
/// The cache lives in `hive-core` so both `hive-model` (which reads
/// secrets at request time) and `hivemind-desktop` (which saves/deletes
/// secrets via the UI) can access it without a circular dependency.
///
/// Uses `parking_lot::Mutex` which does not poison on panic.
static KEYRING_CACHE: LazyLock<Mutex<HashMap<String, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Returns a cached secret, or `None` if not in the cache.
pub fn get_cached_secret(key: &str) -> Option<String> {
    KEYRING_CACHE.lock().get(key).cloned()
}

/// Stores a secret in the cache after a successful keyring read.
pub fn cache_secret(key: &str, value: &str) {
    KEYRING_CACHE.lock().insert(key.to_string(), value.to_string());
}

/// Evicts a single key from the cache.
/// Call this when a secret is saved or deleted via the UI.
pub fn invalidate_cached_secret(key: &str) {
    KEYRING_CACHE.lock().remove(key);
}

/// Evicts all entries from the cache.
/// Call this on config reload or provider re-initialization.
pub fn invalidate_all_cached_secrets() {
    KEYRING_CACHE.lock().clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_miss_returns_none() {
        assert!(get_cached_secret("nonexistent-test-key-12345").is_none());
    }

    #[test]
    fn cache_stores_and_retrieves() {
        let key = "test-cache-roundtrip";
        cache_secret(key, "secret-value");
        assert_eq!(get_cached_secret(key), Some("secret-value".to_string()));
        // Clean up
        invalidate_cached_secret(key);
    }

    #[test]
    fn invalidate_single_key() {
        let key = "test-invalidate-single";
        cache_secret(key, "val");
        invalidate_cached_secret(key);
        assert!(get_cached_secret(key).is_none());
    }

    #[test]
    fn invalidate_all_clears_everything() {
        cache_secret("test-all-a", "a");
        cache_secret("test-all-b", "b");
        invalidate_all_cached_secrets();
        assert!(get_cached_secret("test-all-a").is_none());
        assert!(get_cached_secret("test-all-b").is_none());
    }
}
