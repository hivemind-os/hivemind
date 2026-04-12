//! Daemon authentication token management.
//!
//! On every daemon startup a fresh, cryptographically-random token is
//! generated and persisted to the OS keyring via the existing
//! [`crate::secret_store`].  Clients (CLI, Tauri desktop) read the
//! token from the same keyring and attach it as a `Bearer` header to
//! every API request.

use crate::secret_store;

const TOKEN_KEY: &str = "daemon:auth-token";

/// Generate a new random token, persist it to the OS keyring, and
/// return the token string.  Any previously stored token is
/// overwritten.
pub fn generate_and_store() -> String {
    let token = uuid::Uuid::new_v4().to_string();
    secret_store::save(TOKEN_KEY, &token);
    token
}

/// Load the current daemon auth token from the OS keyring.
/// Returns `None` when no daemon token has been stored (e.g. the
/// daemon has never started, or the token was already cleared).
pub fn load() -> Option<String> {
    secret_store::load(TOKEN_KEY)
}

/// Load the daemon auth token directly from the OS keyring, bypassing
/// the full secret-store cache.  This reads only the single
/// `daemon:auth-token` entry instead of enumerating all secrets.
///
/// Prefer this in the desktop app to avoid triggering keychain
/// permission prompts for every stored secret.
pub fn load_direct() -> Option<String> {
    secret_store::load_single(TOKEN_KEY)
}

/// Remove the daemon auth token from the OS keyring.
/// Called during daemon shutdown so stale tokens don't linger.
pub fn clear() {
    secret_store::delete(TOKEN_KEY);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_generate_load_clear() {
        let token = generate_and_store();
        assert!(!token.is_empty());
        assert_eq!(load(), Some(token));
        clear();
        assert_eq!(load(), None);
    }
}
