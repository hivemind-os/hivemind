use crate::{RiskServiceError, RiskVerdict};
use hive_classification::DataClass;
use hive_contracts::{RiskScanRecord, ScanActionTaken, ScanDecision};
use rusqlite::{params, Connection};
use std::path::PathBuf;

/// Abstraction over the persistence layer used by [`crate::RiskService`].
///
/// All methods are **synchronous** — callers are expected to invoke them inside
/// `tokio::task::spawn_blocking` (which is exactly what `RiskService` already
/// does).
pub trait RiskStore: Send + Sync {
    /// Persist a single risk scan record.
    fn log_scan(&self, record: &RiskScanRecord) -> Result<(), RiskServiceError>;

    /// Return the most recent scans for a given session, newest first.
    fn query_scans(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<Vec<RiskScanRecord>, RiskServiceError>;

    /// Insert or replace a file-level scan cache entry.
    fn cache_file_verdict(
        &self,
        path: &str,
        mtime_ms: i64,
        size: i64,
        verdict: &str,
        scanned_at_ms: i64,
    ) -> Result<(), RiskServiceError>;

    /// Load cached file verdicts that were scanned after `since_ms`, newest
    /// first, up to `limit` rows.
    #[allow(clippy::type_complexity)]
    fn get_cached_verdicts(
        &self,
        since_ms: i64,
        limit: usize,
    ) -> Result<Vec<(String, i64, i64, String, i64)>, RiskServiceError>;
}

/// SQLite-backed implementation of [`RiskStore`].
///
/// Each method opens a **fresh** connection via [`open_ledger`], matching the
/// original behaviour in `RiskService`.
pub struct SqliteRiskStore {
    ledger_path: PathBuf,
}

impl SqliteRiskStore {
    pub fn new(ledger_path: PathBuf) -> Self {
        Self { ledger_path }
    }
}

impl RiskStore for SqliteRiskStore {
    fn log_scan(&self, record: &RiskScanRecord) -> Result<(), RiskServiceError> {
        let connection = open_ledger(&self.ledger_path)?;
        let flagged_spans = serde_json::to_string(&record.flagged_spans).map_err(|error| {
            RiskServiceError::LedgerFailed {
                operation: "serialize_flagged_spans",
                detail: error.to_string(),
            }
        })?;
        connection
            .execute(
                "
                INSERT INTO risk_scans (
                    id, scan_type, payload_hash, payload_preview, source, source_session,
                    verdict, confidence, threat_type, flagged_spans, action_taken,
                    user_decision, model_used, scan_duration_ms, data_class, scanned_at_ms
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
                ",
                params![
                    record.id,
                    record.scan_type,
                    record.payload_hash,
                    record.payload_preview,
                    record.source,
                    record.source_session,
                    verdict_to_str(record.verdict),
                    record.confidence,
                    record.threat_type,
                    flagged_spans,
                    action_to_str(record.action_taken),
                    record.user_decision.map(decision_to_str),
                    record.model_used,
                    record.scan_duration_ms as i64,
                    record.data_class.as_str(),
                    record.scanned_at_ms as i64,
                ],
            )
            .map_err(|error| RiskServiceError::LedgerFailed {
                operation: "insert_risk_scan",
                detail: error.to_string(),
            })?;
        Ok(())
    }

    fn query_scans(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<Vec<RiskScanRecord>, RiskServiceError> {
        let connection = open_ledger(&self.ledger_path)?;
        let mut statement = connection
            .prepare(
                "
                SELECT id, scan_type, payload_hash, payload_preview, source, source_session,
                       verdict, confidence, threat_type, flagged_spans, action_taken,
                       user_decision, model_used, scan_duration_ms, data_class, scanned_at_ms
                FROM risk_scans
                WHERE source_session = ?1
                ORDER BY scanned_at_ms DESC
                LIMIT ?2
                ",
            )
            .map_err(|error| RiskServiceError::LedgerFailed {
                operation: "prepare_list_scans",
                detail: error.to_string(),
            })?;
        let rows = statement
            .query_map(params![session_id, limit as i64], map_scan_record)
            .map_err(|error| RiskServiceError::LedgerFailed {
                operation: "query_list_scans",
                detail: error.to_string(),
            })?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(|error| RiskServiceError::LedgerFailed {
            operation: "collect_list_scans",
            detail: error.to_string(),
        })
    }

    fn cache_file_verdict(
        &self,
        path: &str,
        mtime_ms: i64,
        size: i64,
        verdict: &str,
        scanned_at_ms: i64,
    ) -> Result<(), RiskServiceError> {
        let connection = open_ledger(&self.ledger_path)?;
        connection
            .execute(
                "INSERT OR REPLACE INTO file_scan_cache \
                 (file_path, mtime_ms, size, verdict, scanned_at_ms) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![path, mtime_ms, size, verdict, scanned_at_ms],
            )
            .map_err(|error| RiskServiceError::LedgerFailed {
                operation: "cache_file_verdict",
                detail: error.to_string(),
            })?;
        Ok(())
    }

    fn get_cached_verdicts(
        &self,
        since_ms: i64,
        limit: usize,
    ) -> Result<Vec<(String, i64, i64, String, i64)>, RiskServiceError> {
        let connection = open_ledger(&self.ledger_path)?;
        let mut stmt = connection
            .prepare(
                "SELECT file_path, mtime_ms, size, verdict, scanned_at_ms \
                 FROM file_scan_cache WHERE scanned_at_ms > ?1 \
                 ORDER BY scanned_at_ms DESC LIMIT ?2",
            )
            .map_err(|error| RiskServiceError::LedgerFailed {
                operation: "prepare_cached_verdicts",
                detail: error.to_string(),
            })?;
        let rows = stmt
            .query_map(params![since_ms, limit as i64], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })
            .map_err(|error| RiskServiceError::LedgerFailed {
                operation: "query_cached_verdicts",
                detail: error.to_string(),
            })?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(|error| RiskServiceError::LedgerFailed {
            operation: "collect_cached_verdicts",
            detail: error.to_string(),
        })
    }
}

// ── helpers (moved from lib.rs) ──────────────────────────────────────────

pub(crate) fn open_ledger(path: &PathBuf) -> Result<Connection, RiskServiceError> {
    let connection = Connection::open(path).map_err(|error| RiskServiceError::LedgerFailed {
        operation: "open_risk_ledger",
        detail: error.to_string(),
    })?;
    connection.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS risk_scans (
            id TEXT PRIMARY KEY,
            scan_type TEXT NOT NULL,
            payload_hash TEXT NOT NULL,
            payload_preview TEXT,
            source TEXT NOT NULL,
            source_session TEXT,
            verdict TEXT NOT NULL,
            confidence REAL NOT NULL,
            threat_type TEXT,
            flagged_spans TEXT NOT NULL,
            action_taken TEXT NOT NULL,
            user_decision TEXT,
            model_used TEXT NOT NULL,
            scan_duration_ms INTEGER NOT NULL,
            data_class TEXT NOT NULL,
            scanned_at_ms INTEGER NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_risk_scans_session ON risk_scans(source_session, scanned_at_ms DESC);
        CREATE INDEX IF NOT EXISTS idx_risk_scans_verdict ON risk_scans(verdict, scanned_at_ms DESC);
        CREATE INDEX IF NOT EXISTS idx_risk_scans_hash ON risk_scans(payload_hash);

        CREATE TABLE IF NOT EXISTS file_scan_cache (
            file_path TEXT PRIMARY KEY,
            mtime_ms INTEGER NOT NULL,
            size INTEGER NOT NULL,
            verdict TEXT NOT NULL,
            scanned_at_ms INTEGER NOT NULL
        );
        ",
    )
    .map_err(|error| RiskServiceError::LedgerFailed {
        operation: "create_risk_schema",
        detail: error.to_string(),
    })?;
    Ok(connection)
}

pub(crate) fn verdict_to_str(verdict: RiskVerdict) -> &'static str {
    match verdict {
        RiskVerdict::Clean => "clean",
        RiskVerdict::Suspicious => "suspicious",
        RiskVerdict::Detected => "detected",
    }
}

pub(crate) fn verdict_from_str(verdict: &str) -> RiskVerdict {
    match verdict {
        "suspicious" => RiskVerdict::Suspicious,
        "detected" => RiskVerdict::Detected,
        _ => RiskVerdict::Clean,
    }
}

pub(crate) fn action_to_str(action: ScanActionTaken) -> &'static str {
    match action {
        ScanActionTaken::Passed => "passed",
        ScanActionTaken::Blocked => "blocked",
        ScanActionTaken::Redacted => "redacted",
        ScanActionTaken::UserAllowed => "user_allowed",
        ScanActionTaken::UserBlocked => "user_blocked",
        ScanActionTaken::Flagged => "flagged",
    }
}

fn action_from_str(action: &str) -> ScanActionTaken {
    match action {
        "blocked" => ScanActionTaken::Blocked,
        "redacted" => ScanActionTaken::Redacted,
        "user_allowed" => ScanActionTaken::UserAllowed,
        "user_blocked" => ScanActionTaken::UserBlocked,
        "flagged" => ScanActionTaken::Flagged,
        _ => ScanActionTaken::Passed,
    }
}

pub(crate) fn decision_to_str(decision: ScanDecision) -> &'static str {
    match decision {
        ScanDecision::Allow => "allow",
        ScanDecision::Block => "block",
        ScanDecision::Redact => "redact",
    }
}

fn decision_from_optional_str(decision: Option<String>) -> Option<ScanDecision> {
    match decision.as_deref() {
        Some("allow") => Some(ScanDecision::Allow),
        Some("block") => Some(ScanDecision::Block),
        Some("redact") => Some(ScanDecision::Redact),
        _ => None,
    }
}

fn map_scan_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<RiskScanRecord> {
    let verdict: String = row.get(6)?;
    let flagged_spans: String = row.get(9)?;
    let action_taken: String = row.get(10)?;
    let user_decision: Option<String> = row.get(11)?;
    let data_class: String = row.get(14)?;
    Ok(RiskScanRecord {
        id: row.get(0)?,
        scan_type: row.get(1)?,
        payload_hash: row.get(2)?,
        payload_preview: row.get(3)?,
        source: row.get(4)?,
        source_session: row.get(5)?,
        verdict: verdict_from_str(&verdict),
        confidence: row.get(7)?,
        threat_type: row.get(8)?,
        flagged_spans: serde_json::from_str(&flagged_spans).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                9,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        action_taken: action_from_str(&action_taken),
        user_decision: decision_from_optional_str(user_decision),
        model_used: row.get(12)?,
        scan_duration_ms: row.get::<_, i64>(13)? as u64,
        data_class: match data_class.as_str() {
            "PUBLIC" => DataClass::Public,
            "CONFIDENTIAL" => DataClass::Confidential,
            "RESTRICTED" => DataClass::Restricted,
            _ => DataClass::Internal,
        },
        scanned_at_ms: row.get::<_, i64>(15)? as u64,
    })
}
