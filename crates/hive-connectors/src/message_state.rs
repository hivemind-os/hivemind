use anyhow::{Context, Result};
use parking_lot::Mutex;
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Abstraction over message-tracking state for connector folders/channels.
pub trait MessageStateStore: Send + Sync {
    /// Get the stored UID validity for a folder. Returns `None` if no state exists.
    fn uid_validity(&self, folder: &str) -> Result<Option<u32>>;

    /// Get the last seen UID for a folder.
    fn last_seen_uid(&self, folder: &str) -> Result<u32>;

    /// Update the UID validity and reset state if it changed.
    ///
    /// Returns `true` if the validity changed (requiring a full re-sync).
    fn update_uid_validity(&self, folder: &str, new_validity: u32) -> Result<bool>;

    /// Mark a UID / message identifier as seen and update `last_seen_uid` if higher.
    fn mark_seen(&self, folder: &str, uid: u32) -> Result<()>;

    /// Check whether a UID / message identifier has already been processed.
    fn is_seen(&self, folder: &str, uid: u32) -> Result<bool>;

    /// Return the last-seen UID for catch-up fetching.
    fn unseen_uids_since(&self, folder: &str) -> Result<u32>;

    /// Remove all tracked state (all folders, all seen UIDs).
    fn reset(&self) -> Result<()>;
}

// ---------------------------------------------------------------------------
// SQLite implementation
// ---------------------------------------------------------------------------

/// Tracks which messages have been seen/processed per folder (or channel).
///
/// This is a general-purpose, SQLite-backed message-tracking module used by
/// Email (IMAP), Gmail, Microsoft Graph, and any other connector that needs
/// to avoid re-processing messages.
///
/// Persists:
/// - `uid_validity`: a validity token for the selected folder.
///   If this changes (e.g. IMAP UID validity reset), the server has
///   restructured identifiers and we must re-sync.
/// - Seen UIDs / message IDs: set of identifiers already fetched/processed.
pub struct SqliteMessageStateStore {
    conn: Mutex<Connection>,
    #[allow(dead_code)]
    path: PathBuf,
}

/// Backward-compatible alias.
pub type MessageState = SqliteMessageStateStore;

impl SqliteMessageStateStore {
    /// Open (or create) the SQLite database at `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating message state dir {}", parent.display()))?;
        }

        let conn = Connection::open(&path)
            .with_context(|| format!("opening message state db {}", path.display()))?;

        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS imap_state (
                folder          TEXT PRIMARY KEY,
                uid_validity    INTEGER NOT NULL,
                last_seen_uid   INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS seen_uids (
                folder  TEXT NOT NULL,
                uid     INTEGER NOT NULL,
                PRIMARY KEY (folder, uid)
            );
            ",
        )
        .context("initializing message state schema")?;

        Ok(Self { conn: Mutex::new(conn), path })
    }
}

impl MessageStateStore for SqliteMessageStateStore {
    fn uid_validity(&self, folder: &str) -> Result<Option<u32>> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT uid_validity FROM imap_state WHERE folder = ?1")
            .context("preparing uid_validity query")?;
        let result =
            stmt.query_row(params![folder], |row| row.get::<_, i64>(0)).ok().map(|v| v as u32);
        Ok(result)
    }

    fn last_seen_uid(&self, folder: &str) -> Result<u32> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT last_seen_uid FROM imap_state WHERE folder = ?1")
            .context("preparing last_seen_uid query")?;
        let uid = stmt.query_row(params![folder], |row| row.get::<_, i64>(0)).unwrap_or(0);
        Ok(uid as u32)
    }

    fn update_uid_validity(&self, folder: &str, new_validity: u32) -> Result<bool> {
        let conn = self.conn.lock();

        // Inline uid_validity lookup to avoid double-lock
        let mut stmt = conn
            .prepare("SELECT uid_validity FROM imap_state WHERE folder = ?1")
            .context("preparing uid_validity query")?;
        let old: Option<u32> =
            stmt.query_row(params![folder], |row| row.get::<_, i64>(0)).ok().map(|v| v as u32);
        drop(stmt);

        if old == Some(new_validity) {
            return Ok(false); // No change
        }

        if old.is_some() {
            tracing::warn!(
                folder,
                old = old.unwrap_or(0),
                new = new_validity,
                "UID validity changed, resetting message state"
            );
            conn.execute("DELETE FROM seen_uids WHERE folder = ?1", params![folder])
                .context("purging seen UIDs after validity change")?;
        }

        conn.execute(
            "INSERT OR REPLACE INTO imap_state (folder, uid_validity, last_seen_uid)
                 VALUES (?1, ?2, 0)",
            params![folder, new_validity as i64],
        )
        .context("updating uid_validity")?;

        Ok(old.is_some()) // true if this was a reset
    }

    fn mark_seen(&self, folder: &str, uid: u32) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT OR IGNORE INTO seen_uids (folder, uid) VALUES (?1, ?2)",
            params![folder, uid as i64],
        )
        .context("inserting seen uid")?;

        conn.execute(
            "UPDATE imap_state SET last_seen_uid = MAX(last_seen_uid, ?2) WHERE folder = ?1",
            params![folder, uid as i64],
        )
        .context("updating last_seen_uid")?;

        Ok(())
    }

    fn is_seen(&self, folder: &str, uid: u32) -> Result<bool> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT 1 FROM seen_uids WHERE folder = ?1 AND uid = ?2")
            .context("preparing is_seen query")?;
        Ok(stmt.exists(params![folder, uid as i64]).unwrap_or(false))
    }

    fn unseen_uids_since(&self, folder: &str) -> Result<u32> {
        self.last_seen_uid(folder)
    }

    fn reset(&self) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute_batch("DELETE FROM seen_uids; DELETE FROM imap_state;")
            .context("resetting message state")?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn fresh_state_has_no_validity() {
        let dir = tempdir().unwrap();
        let state = MessageState::open(dir.path().join("msg.db")).unwrap();
        assert_eq!(state.uid_validity("INBOX").unwrap(), None);
        assert_eq!(state.last_seen_uid("INBOX").unwrap(), 0);
    }

    #[test]
    fn set_and_read_validity() {
        let dir = tempdir().unwrap();
        let state = MessageState::open(dir.path().join("msg.db")).unwrap();

        let changed = state.update_uid_validity("INBOX", 12345).unwrap();
        assert!(!changed);

        assert_eq!(state.uid_validity("INBOX").unwrap(), Some(12345));
    }

    #[test]
    fn validity_change_resets_state() {
        let dir = tempdir().unwrap();
        let state = MessageState::open(dir.path().join("msg.db")).unwrap();

        state.update_uid_validity("INBOX", 100).unwrap();
        state.mark_seen("INBOX", 1).unwrap();
        state.mark_seen("INBOX", 2).unwrap();
        assert!(state.is_seen("INBOX", 1).unwrap());

        let changed = state.update_uid_validity("INBOX", 200).unwrap();
        assert!(changed);

        assert!(!state.is_seen("INBOX", 1).unwrap());
        assert_eq!(state.last_seen_uid("INBOX").unwrap(), 0);
    }

    #[test]
    fn mark_seen_updates_last_uid() {
        let dir = tempdir().unwrap();
        let state = MessageState::open(dir.path().join("msg.db")).unwrap();

        state.update_uid_validity("INBOX", 100).unwrap();
        state.mark_seen("INBOX", 5).unwrap();
        state.mark_seen("INBOX", 3).unwrap();
        state.mark_seen("INBOX", 10).unwrap();

        assert_eq!(state.last_seen_uid("INBOX").unwrap(), 10);
        assert!(state.is_seen("INBOX", 5).unwrap());
        assert!(state.is_seen("INBOX", 10).unwrap());
        assert!(!state.is_seen("INBOX", 7).unwrap());
    }

    #[test]
    fn reset_clears_everything() {
        let dir = tempdir().unwrap();
        let state = MessageState::open(dir.path().join("msg.db")).unwrap();

        state.update_uid_validity("INBOX", 100).unwrap();
        state.mark_seen("INBOX", 1).unwrap();

        state.reset().unwrap();

        assert_eq!(state.uid_validity("INBOX").unwrap(), None);
        assert!(!state.is_seen("INBOX", 1).unwrap());
    }
}
