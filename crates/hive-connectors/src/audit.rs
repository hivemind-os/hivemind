use anyhow::{Context, Result};
use hive_classification::DataClass;
use hive_contracts::comms::MessageDirection;
use hive_contracts::connectors::{ConnectorProvider, ServiceAuditEntry, ServiceType};
use parking_lot::Mutex;
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Abstraction over the queryable connector audit log.
pub trait AuditStore: Send + Sync {
    /// Record a connector service event.
    fn record(&self, entry: &ServiceAuditEntry) -> Result<()>;

    /// Query connector audit entries with optional filters.
    fn query(&self, filter: &ConnectorAuditFilter) -> Result<Vec<ServiceAuditEntry>>;

    /// Return the path to the backing store (if any).
    fn path(&self) -> &Path;
}

// ---------------------------------------------------------------------------
// SQLite implementation
// ---------------------------------------------------------------------------

/// Queryable connector audit log backed by SQLite.
///
/// Complements the tamper-evident `AuditLogger` in hive-core with a
/// searchable, filterable store for the connector audit UI.
pub struct SqliteAuditStore {
    conn: Arc<Mutex<Connection>>,
    path: PathBuf,
}

/// Backward-compatible alias.
pub type ConnectorAuditLog = SqliteAuditStore;

impl SqliteAuditStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating connector audit dir {}", parent.display()))?;
        }

        let conn = Connection::open(&path)
            .with_context(|| format!("opening connector audit db {}", path.display()))?;

        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS connector_audit (
                id                TEXT PRIMARY KEY,
                connector_id      TEXT NOT NULL,
                provider          TEXT NOT NULL,
                service_type      TEXT NOT NULL,
                operation         TEXT NOT NULL,
                direction         TEXT,
                from_address      TEXT,
                to_address        TEXT,
                subject           TEXT,
                resource_id       TEXT,
                resource_name     TEXT,
                body_hash         TEXT NOT NULL,
                body_preview      TEXT,
                data_class        TEXT NOT NULL,
                approval_decision TEXT,
                agent_id          TEXT,
                session_id        TEXT,
                timestamp_ms      INTEGER NOT NULL,
                metadata          TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_connector_audit_connector
                ON connector_audit(connector_id, timestamp_ms);
            CREATE INDEX IF NOT EXISTS idx_connector_audit_service
                ON connector_audit(service_type, timestamp_ms);
            CREATE INDEX IF NOT EXISTS idx_connector_audit_agent
                ON connector_audit(agent_id, timestamp_ms);
            CREATE INDEX IF NOT EXISTS idx_connector_audit_direction
                ON connector_audit(direction, timestamp_ms);
            ",
        )
        .context("initializing connector audit schema")?;

        Ok(Self { conn: Arc::new(Mutex::new(conn)), path })
    }
}

impl AuditStore for SqliteAuditStore {
    fn record(&self, entry: &ServiceAuditEntry) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT OR REPLACE INTO connector_audit
             (id, connector_id, provider, service_type, operation, direction,
              from_address, to_address, subject, resource_id, resource_name,
              body_hash, body_preview, data_class, approval_decision,
              agent_id, session_id, timestamp_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
            params![
                entry.id,
                entry.connector_id,
                entry.provider.as_str(),
                entry.service_type.as_str(),
                entry.operation,
                entry.direction.map(|d| d.as_str().to_string()),
                entry.from_address,
                entry.to_address,
                entry.subject,
                entry.resource_id,
                entry.resource_name,
                entry.body_hash,
                entry.body_preview,
                format!("{}", entry.data_class),
                entry.approval_decision,
                entry.agent_id,
                entry.session_id,
                entry.timestamp_ms as i64,
            ],
        )
        .context("inserting connector audit entry")?;
        Ok(())
    }

    fn query(&self, filter: &ConnectorAuditFilter) -> Result<Vec<ServiceAuditEntry>> {
        let conn = self.conn.lock();
        let mut sql = "SELECT id, connector_id, provider, service_type, operation,
                        direction, from_address, to_address, subject,
                        resource_id, resource_name, body_hash, body_preview,
                        data_class, approval_decision, agent_id, session_id,
                        timestamp_ms
                 FROM connector_audit WHERE 1=1"
            .to_string();
        let mut bind_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(connector_id) = &filter.connector_id {
            sql.push_str(" AND connector_id = ?");
            bind_values.push(Box::new(connector_id.clone()));
        }
        if let Some(service_type) = &filter.service_type {
            sql.push_str(" AND service_type = ?");
            bind_values.push(Box::new(service_type.as_str().to_string()));
        }
        if let Some(direction) = &filter.direction {
            sql.push_str(" AND direction = ?");
            bind_values.push(Box::new(direction.as_str().to_string()));
        }
        if let Some(agent_id) = &filter.agent_id {
            sql.push_str(" AND agent_id = ?");
            bind_values.push(Box::new(agent_id.clone()));
        }
        if let Some(since_ms) = filter.since_ms {
            sql.push_str(" AND timestamp_ms >= ?");
            bind_values.push(Box::new(since_ms as i64));
        }
        if let Some(until_ms) = filter.until_ms {
            sql.push_str(" AND timestamp_ms <= ?");
            bind_values.push(Box::new(until_ms as i64));
        }

        sql.push_str(" ORDER BY timestamp_ms DESC");

        if let Some(limit) = filter.limit {
            sql.push_str(&format!(" LIMIT {limit}"));
        }

        let params_refs: Vec<&dyn rusqlite::types::ToSql> =
            bind_values.iter().map(|b| b.as_ref()).collect();

        let mut stmt = conn.prepare(&sql).context("preparing connector audit query")?;
        let rows = stmt
            .query_map(params_refs.as_slice(), |row| {
                Ok(ServiceAuditEntry {
                    id: row.get(0)?,
                    connector_id: row.get(1)?,
                    provider: parse_provider(&row.get::<_, String>(2)?),
                    service_type: parse_service_type(&row.get::<_, String>(3)?),
                    operation: row.get(4)?,
                    direction: row.get::<_, Option<String>>(5)?.as_deref().map(parse_direction),
                    from_address: row.get(6)?,
                    to_address: row.get(7)?,
                    subject: row.get(8)?,
                    resource_id: row.get(9)?,
                    resource_name: row.get(10)?,
                    body_hash: row.get(11)?,
                    body_preview: row.get(12)?,
                    data_class: parse_data_class(&row.get::<_, String>(13)?),
                    approval_decision: row.get(14)?,
                    agent_id: row.get(15)?,
                    session_id: row.get(16)?,
                    timestamp_ms: row.get::<_, i64>(17)? as u128,
                })
            })
            .context("executing connector audit query")?;

        let mut entries = Vec::new();
        for row in rows {
            entries.push(row.context("reading connector audit row")?);
        }
        Ok(entries)
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

// ---------------------------------------------------------------------------
// Filter
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone)]
pub struct ConnectorAuditFilter {
    pub connector_id: Option<String>,
    pub service_type: Option<ServiceType>,
    pub direction: Option<MessageDirection>,
    pub agent_id: Option<String>,
    pub since_ms: Option<u128>,
    pub until_ms: Option<u128>,
    pub limit: Option<usize>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

pub fn body_hash(body: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(body.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub fn body_preview(body: &str, max_len: usize) -> String {
    if body.len() <= max_len {
        body.to_string()
    } else {
        let mut s = body[..max_len].to_string();
        s.push('…');
        s
    }
}

pub fn now_ms() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis()
}

fn parse_provider(s: &str) -> ConnectorProvider {
    match s {
        "microsoft" => ConnectorProvider::Microsoft,
        "gmail" => ConnectorProvider::Gmail,
        "email" | "imap" => ConnectorProvider::Imap,
        "slack" => ConnectorProvider::Slack,
        "discord" => ConnectorProvider::Discord,
        "telegram" => ConnectorProvider::Telegram,
        "whatsapp" => ConnectorProvider::WhatsApp,
        _ => ConnectorProvider::Imap,
    }
}

fn parse_service_type(s: &str) -> ServiceType {
    match s {
        "communication" => ServiceType::Communication,
        "calendar" => ServiceType::Calendar,
        "drive" => ServiceType::Drive,
        "contacts" => ServiceType::Contacts,
        other => ServiceType::Other(other.to_string()),
    }
}

fn parse_direction(s: &str) -> MessageDirection {
    match s {
        "inbound" => MessageDirection::Inbound,
        _ => MessageDirection::Outbound,
    }
}

fn parse_data_class(s: &str) -> DataClass {
    match s {
        "PUBLIC" => DataClass::Public,
        "CONFIDENTIAL" => DataClass::Confidential,
        "RESTRICTED" => DataClass::Restricted,
        _ => DataClass::Internal,
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
    fn round_trip_audit_entry() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("audit.db");
        let log = ConnectorAuditLog::open(&db_path).unwrap();

        let entry = ServiceAuditEntry {
            id: "msg-1".into(),
            connector_id: "work-email".into(),
            provider: ConnectorProvider::Imap,
            service_type: ServiceType::Communication,
            operation: "send".into(),
            direction: Some(MessageDirection::Outbound),
            from_address: Some("me@example.com".into()),
            to_address: Some("alice@outlook.com".into()),
            subject: Some("Hello".into()),
            resource_id: None,
            resource_name: None,
            body_hash: body_hash("Hello world"),
            body_preview: Some("Hello world".into()),
            data_class: DataClass::Internal,
            approval_decision: Some("auto".into()),
            agent_id: Some("agent-1".into()),
            session_id: Some("sess-1".into()),
            timestamp_ms: now_ms(),
        };

        log.record(&entry).unwrap();

        let results = log.query(&ConnectorAuditFilter::default()).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "msg-1");
        assert_eq!(results[0].to_address.as_deref(), Some("alice@outlook.com"));
    }

    #[test]
    fn query_with_filters() {
        let dir = tempdir().unwrap();
        let log = ConnectorAuditLog::open(dir.path().join("audit.db")).unwrap();

        for i in 0..5 {
            log.record(&ServiceAuditEntry {
                id: format!("msg-{i}"),
                connector_id: if i % 2 == 0 { "cn-a" } else { "cn-b" }.into(),
                provider: ConnectorProvider::Imap,
                service_type: ServiceType::Communication,
                operation: "send".into(),
                direction: Some(MessageDirection::Outbound),
                from_address: Some("me@test.com".into()),
                to_address: Some(format!("user{i}@test.com")),
                subject: None,
                resource_id: None,
                resource_name: None,
                body_hash: body_hash(&format!("body {i}")),
                body_preview: None,
                data_class: DataClass::Internal,
                approval_decision: None,
                agent_id: None,
                session_id: None,
                timestamp_ms: (1000 + i) as u128,
            })
            .unwrap();
        }

        let filtered = log
            .query(&ConnectorAuditFilter {
                connector_id: Some("cn-a".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(filtered.len(), 3);

        let limited =
            log.query(&ConnectorAuditFilter { limit: Some(2), ..Default::default() }).unwrap();
        assert_eq!(limited.len(), 2);
    }

    #[test]
    fn query_by_service_type() {
        let dir = tempdir().unwrap();
        let log = ConnectorAuditLog::open(dir.path().join("audit.db")).unwrap();

        log.record(&ServiceAuditEntry {
            id: "cal-1".into(),
            connector_id: "ms-conn".into(),
            provider: ConnectorProvider::Microsoft,
            service_type: ServiceType::Calendar,
            operation: "list_events".into(),
            direction: None,
            from_address: None,
            to_address: None,
            subject: None,
            resource_id: Some("evt-123".into()),
            resource_name: Some("Team standup".into()),
            body_hash: body_hash(""),
            body_preview: None,
            data_class: DataClass::Internal,
            approval_decision: None,
            agent_id: None,
            session_id: None,
            timestamp_ms: 2000,
        })
        .unwrap();

        log.record(&ServiceAuditEntry {
            id: "msg-1".into(),
            connector_id: "ms-conn".into(),
            provider: ConnectorProvider::Microsoft,
            service_type: ServiceType::Communication,
            operation: "send".into(),
            direction: Some(MessageDirection::Outbound),
            from_address: Some("me@test.com".into()),
            to_address: Some("bob@test.com".into()),
            subject: Some("Hi".into()),
            resource_id: None,
            resource_name: None,
            body_hash: body_hash("hi"),
            body_preview: Some("hi".into()),
            data_class: DataClass::Internal,
            approval_decision: None,
            agent_id: None,
            session_id: None,
            timestamp_ms: 3000,
        })
        .unwrap();

        let cal_only = log
            .query(&ConnectorAuditFilter {
                service_type: Some(ServiceType::Calendar),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(cal_only.len(), 1);
        assert_eq!(cal_only[0].id, "cal-1");
    }
}
