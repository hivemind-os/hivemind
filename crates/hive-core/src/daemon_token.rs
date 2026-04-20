//! Daemon authentication token management.
//!
//! On every daemon startup a fresh, cryptographically-random token is
//! generated and persisted to **both** the OS keyring and a fallback
//! file (`<run_dir>/daemon.token`).  Clients (CLI, Tauri desktop) read
//! the token from the keyring first and fall back to the file when the
//! keyring is unavailable or returns an error.
//!
//! The dual-storage approach ensures authentication survives OS keyring
//! failures (permissions, corruption, credential-manager bugs) which
//! would otherwise cause permanent 401 errors with no user-visible
//! recovery path.

use crate::secret_store;
use std::path::PathBuf;
use tracing::warn;

const TOKEN_KEY: &str = "daemon:auth-token";
const TOKEN_FILE_NAME: &str = "daemon.token";

/// Resolve the token file path: `<run_dir>/daemon.token`.
fn token_file_path() -> Option<PathBuf> {
    crate::config::discover_paths()
        .ok()
        .map(|p| p.run_dir.join(TOKEN_FILE_NAME))
}

/// Generate a new random token, persist it to both the OS keyring and
/// the fallback token file, and return the token string.  Any
/// previously stored token is overwritten.
pub fn generate_and_store() -> String {
    let token = uuid::Uuid::new_v4().to_string();
    let keyring_ok = secret_store::save(TOKEN_KEY, &token);
    if !keyring_ok {
        warn!("failed to persist daemon auth token to OS keyring — using file fallback only");
    }

    // Always write the file fallback so clients can recover even when
    // the keyring is broken.
    if let Some(path) = token_file_path() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::write(&path, &token) {
            warn!(path = %path.display(), error = %e, "failed to write daemon token file");
        }
    }

    token
}

/// Load the current daemon auth token from the OS keyring (cached).
/// Returns `None` when no daemon token has been stored (e.g. the
/// daemon has never started, or the token was already cleared).
pub fn load() -> Option<String> {
    secret_store::load(TOKEN_KEY)
}

/// Load the daemon auth token, trying the OS keyring first and falling
/// back to the token file.  Reads the keyring entry directly without
/// loading the full secret-store cache.
///
/// Prefer this in the desktop app to avoid triggering keychain
/// permission prompts for every stored secret.
pub fn load_direct() -> Option<String> {
    // Primary: OS keyring.
    if let Some(token) = secret_store::load_single(TOKEN_KEY) {
        return Some(token);
    }

    // Fallback: token file written by the daemon.
    if let Some(path) = token_file_path() {
        match std::fs::read_to_string(&path) {
            Ok(contents) => {
                let token = contents.trim().to_string();
                if !token.is_empty() {
                    return Some(token);
                }
            }
            Err(_) => {}
        }
    }

    None
}

/// Remove the daemon auth token from both the OS keyring and the
/// fallback file.  Called during daemon shutdown so stale tokens
/// don't linger.
pub fn clear() {
    secret_store::delete(TOKEN_KEY);
    if let Some(path) = token_file_path() {
        let _ = std::fs::remove_file(&path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Single test to avoid parallel conflicts on shared keyring/file state.
    #[test]
    fn roundtrip_and_file_fallback() {
        // ── roundtrip: generate → load → load_direct → clear ──
        let token = generate_and_store();
        assert!(!token.is_empty());
        assert_eq!(load(), Some(token.clone()));
        assert_eq!(load_direct(), Some(token.clone()));
        clear();
        assert_eq!(load(), None);

        // ── file fallback: survives keyring absence ──
        let token2 = generate_and_store();

        // Simulate keyring failure by deleting only the keyring entry.
        secret_store::delete(TOKEN_KEY);

        // load_direct should still find the token via the file fallback.
        assert_eq!(load_direct(), Some(token2));

        clear();
    }
}
