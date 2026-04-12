use anyhow::{Context, Result};
use hive_classification::DataClass;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuditEntry {
    pub sequence: u64,
    pub timestamp_ms: u128,
    pub actor: String,
    pub action: String,
    pub subject: String,
    pub data_class: String,
    pub detail_hash: String,
    pub outcome: String,
    pub prev_hash: String,
    pub entry_hash: String,
}

#[derive(Debug, Clone)]
pub struct NewAuditEntry {
    pub actor: String,
    pub action: String,
    pub subject: String,
    pub data_class: DataClass,
    pub detail: String,
    pub outcome: String,
}

impl NewAuditEntry {
    pub fn new(
        actor: impl Into<String>,
        action: impl Into<String>,
        subject: impl Into<String>,
        data_class: DataClass,
        detail: impl Into<String>,
        outcome: impl Into<String>,
    ) -> Self {
        Self {
            actor: actor.into(),
            action: action.into(),
            subject: subject.into(),
            data_class,
            detail: detail.into(),
            outcome: outcome.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AuditLogger {
    path: PathBuf,
    state: Arc<Mutex<AuditState>>,
}

#[derive(Debug)]
struct AuditState {
    next_sequence: u64,
    last_hash: String,
}

impl AuditLogger {
    pub fn new(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("failed to create audit directory {}", parent.display())
            })?;
        }

        let (next_sequence, last_hash) =
            if path.exists() { read_existing_state(&path)? } else { (1, "GENESIS".to_string()) };

        Ok(Self { path, state: Arc::new(Mutex::new(AuditState { next_sequence, last_hash })) })
    }

    pub fn append(&self, entry: NewAuditEntry) -> Result<AuditEntry> {
        let mut state = self.state.lock();
        let timestamp_ms = unix_timestamp_millis()?;
        let detail_hash = sha256_hex(entry.detail.as_bytes());
        let prev_hash = state.last_hash.clone();
        let sequence = state.next_sequence;
        let data_class_str = entry.data_class.as_str().to_string();

        let entry_hash = sha256_hex(
            format!(
                "{}|{}|{}|{}|{}|{}|{}|{}|{}",
                sequence,
                timestamp_ms,
                entry.actor,
                entry.action,
                entry.subject,
                data_class_str,
                detail_hash,
                entry.outcome,
                prev_hash
            )
            .as_bytes(),
        );

        let record = AuditEntry {
            sequence,
            timestamp_ms,
            actor: entry.actor,
            action: entry.action,
            subject: entry.subject,
            data_class: data_class_str,
            detail_hash,
            outcome: entry.outcome,
            prev_hash,
            entry_hash: entry_hash.clone(),
        };

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .with_context(|| format!("failed to open audit log {}", self.path.display()))?;

        // Restrict audit log permissions on Unix to owner-only (0600).
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));
        }

        serde_json::to_writer(&mut file, &record).context("failed to write audit entry")?;
        writeln!(&mut file).context("failed to terminate audit entry with newline")?;

        state.next_sequence += 1;
        state.last_hash = entry_hash;

        Ok(record)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn read_existing_state(path: &Path) -> Result<(u64, String)> {
    let file =
        fs::File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let reader = BufReader::new(file);

    let mut last_entry: Option<AuditEntry> = None;
    for line in reader.lines() {
        let line = line.with_context(|| format!("failed reading {}", path.display()))?;
        if line.trim().is_empty() {
            continue;
        }

        match serde_json::from_str::<AuditEntry>(&line) {
            Ok(entry) => last_entry = Some(entry),
            Err(e) => {
                tracing::warn!("skipping corrupted audit entry in {}: {e}", path.display());
                continue;
            }
        }
    }

    Ok(match last_entry {
        Some(entry) => (entry.sequence + 1, entry.entry_hash),
        None => (1, "GENESIS".to_string()),
    })
}

fn unix_timestamp_millis() -> Result<u128> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before unix epoch")?
        .as_millis())
}

fn sha256_hex(input: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input);
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn appends_tamper_evident_chain() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("audit.log");
        let logger = AuditLogger::new(&path).expect("logger");

        let first = logger
            .append(NewAuditEntry::new(
                "user",
                "config.show",
                "config",
                DataClass::Internal,
                "showing config",
                "allowed",
            ))
            .expect("first append");
        let second = logger
            .append(NewAuditEntry::new(
                "daemon",
                "daemon.started",
                "daemon",
                DataClass::Internal,
                "daemon online",
                "success",
            ))
            .expect("second append");

        assert_eq!(first.sequence, 1);
        assert_eq!(second.sequence, 2);
        assert_eq!(second.prev_hash, first.entry_hash);

        let contents = fs::read_to_string(path).expect("read audit log");
        assert_eq!(contents.lines().count(), 2);
    }
}
